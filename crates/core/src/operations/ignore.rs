//! Git's own ignore matcher against an isolated shadow tree. Preview never
//! edits the inspected repository, its index, configuration, or ignore files.
use super::{file_save::Target, *};
use crate::git;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs, io,
};
const PREVIEW_PATH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct IgnorePreview {
    pub root: PathBuf,
    pub path: PathBuf,
    pub replacement: String,
    pub newly_ignored: Vec<PathBuf>,
    pub newly_unignored: Vec<PathBuf>,
    root_bound: PathBuf,
    path_bound: PathBuf,
    expected: Option<String>,
    replacement_bound: String,
    fingerprint: [u8; 32],
}

pub fn preview_gitignore(
    root: &Path,
    path: &Path,
    expected: Option<&str>,
    replacement: &str,
) -> Result<IgnorePreview> {
    ensure!(
        path.file_name() == Some(OsStr::new(".gitignore")),
        "Choose a worktree .gitignore file"
    );
    ensure!(
        replacement.len() <= crate::MAX_TEXT_BYTES && !replacement.contains('\0'),
        "Ignore rules must be UTF-8 text up to 2 MiB without NUL"
    );
    let target = super::files::guarded(root, path)?;
    let root = target.root.clone();
    ensure!(
        read_ignore(&root, path)?.as_deref() == expected,
        "Ignore file changed externally; preview the current rules again"
    );
    // Without --exclude-standard Git includes ignored untracked paths too, so
    // removals/negations can preview paths that will become visible again.
    let data = git::words(
        &root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--full-name",
            "-z",
            "--",
        ],
    )?;
    ensure!(
        data.len() <= PREVIEW_PATH_BYTES,
        "Ignore preview exceeds the 8 MiB path limit"
    );
    let tracked = git::words(&root, &["ls-files", "--cached", "--full-name", "-z", "--"])?;
    let tracked_data = tracked.clone();
    let tracked: BTreeSet<_> = tracked
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(git::path)
        .collect();
    let mut paths: BTreeSet<_> = data
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(git::path)
        .collect();
    paths.insert(path.into());
    ensure!(
        paths.len() <= 100_000,
        "Ignore preview exceeds the 100000-path limit"
    );
    let shadow = tempfile::tempdir()?;
    let working = shadow.path().join("tree");
    fs::create_dir(&working)?;
    let runner = GitOperations {
        root: working.clone(),
        timeout: Duration::from_secs(30),
        cancellation: OperationCancellation::default(),
        progress: None,
        #[cfg(test)]
        global_config: Some(PathBuf::from("/dev/null")),
    };
    runner.checked(&[
        "-c",
        "core.hooksPath=/dev/null",
        "init",
        "--template=",
        "--quiet",
        "--",
        ".",
    ])?;
    let mut digest = Sha256::new();
    crate::filesystem::feed(&mut digest, &data);
    crate::filesystem::feed(&mut digest, &tracked_data);
    for key in ["core.ignoreCase", "core.precomposeUnicode"] {
        let setting = git::run(
            &root,
            &[
                OsStr::new("config"),
                OsStr::new("--bool"),
                OsStr::new("--get"),
                OsStr::new(key),
            ],
            64,
        )?;
        ensure!(
            !setting.truncated && (setting.success || setting.error.is_empty()),
            "Cannot inspect ignore matching setting {key}"
        );
        if setting.success {
            let value = git::line(&setting.bytes)?;
            runner.checked(&["config", "--local", "--", key, value])?;
            crate::filesystem::feed(&mut digest, key.as_bytes());
            crate::filesystem::feed(&mut digest, value.as_bytes());
        }
    }
    let mut input = Vec::new();
    for candidate in &paths {
        let is_directory = candidate.as_os_str().as_encoded_bytes().ends_with(b"/");
        let candidate = crate::filesystem::relative(&root, candidate)?;
        let destination = working.join(&candidate);
        fs::create_dir_all(
            destination
                .parent()
                .context("Missing ignore preview parent")?,
        )?;
        if is_directory {
            fs::create_dir_all(&destination)?;
        } else if !destination.exists() {
            fs::write(&destination, [])?;
        }
        if candidate.file_name() == Some(OsStr::new(".gitignore")) {
            if let Some(contents) = read_ignore(&root, &candidate)? {
                crate::filesystem::feed(&mut digest, candidate.as_os_str().as_encoded_bytes());
                crate::filesystem::feed(&mut digest, contents.as_bytes());
                fs::write(&destination, contents)?;
            } else {
                fs::remove_file(&destination)?;
            }
        }
        if !tracked.contains(&candidate) {
            input.extend_from_slice(b"./");
            input.extend_from_slice(candidate.as_os_str().as_encoded_bytes());
            input.push(0);
        }
    }
    // Preserve both lower-priority exclude sources independently, retaining
    // Git's precedence: global excludes < info/exclude < per-directory rules.
    let info_path = git::words(
        &root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    )?;
    let info = read_optional(&git::path(
        info_path.strip_suffix(b"\n").unwrap_or(&info_path),
    ))?;
    crate::filesystem::feed(&mut digest, &info);
    fs::create_dir_all(working.join(".git/info"))?;
    fs::write(working.join(".git/info/exclude"), info)?;
    let excludes = git::run(
        &root,
        &[
            OsStr::new("config"),
            OsStr::new("--path"),
            OsStr::new("--get"),
            OsStr::new("core.excludesFile"),
        ],
        8192,
    )?;
    ensure!(
        !excludes.truncated,
        "Global ignore path exceeds the safety limit"
    );
    ensure!(
        excludes.success || excludes.error.is_empty(),
        "Cannot inspect global ignore configuration: {}",
        excludes.error
    );
    let global_path = if excludes.success && excludes.bytes == b"\n" {
        None
    } else if excludes.success {
        let path = git::path(
            excludes
                .bytes
                .strip_suffix(b"\n")
                .unwrap_or(&excludes.bytes),
        );
        Some(if path.is_absolute() {
            path
        } else {
            root.join(path)
        })
    } else {
        // Git's default core.excludesFile is $XDG_CONFIG_HOME/git/ignore or
        // $HOME/.config/git/ignore. We copy bytes; never change that file.
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .map(|p| p.join("git/ignore"))
    };
    let global = global_path
        .as_deref()
        .map(read_optional)
        .transpose()?
        .unwrap_or_default();
    crate::filesystem::feed(&mut digest, &global);
    let global_copy = shadow.path().join("global-excludes");
    fs::write(&global_copy, global)?;
    let input_file = shadow.path().join("ignore-input");
    fs::write(&input_file, &input)?;
    let before = ignored_paths(&runner, &input_file, &global_copy)?;
    let destination = working.join(path);
    fs::create_dir_all(destination.parent().context("Missing .gitignore parent")?)?;
    fs::write(&destination, replacement)?;
    let after = ignored_paths(&runner, &input_file, &global_copy)?;
    // Revalidate the selected worktree/parent and expected bytes after reading
    // rules; callers also revalidate this fingerprint at explicit apply time.
    target.revalidate()?;
    ensure!(
        read_ignore(&root, path)?.as_deref() == expected,
        "Ignore file changed during preview; try again"
    );
    Ok(IgnorePreview {
        root: root.clone(),
        path: path.into(),
        replacement: replacement.into(),
        newly_ignored: after.difference(&before).cloned().collect(),
        newly_unignored: before.difference(&after).cloned().collect(),
        root_bound: root,
        path_bound: path.into(),
        expected: expected.map(str::to_owned),
        replacement_bound: replacement.into(),
        fingerprint: digest.finalize().into(),
    })
}

pub fn apply_gitignore(preview: &IgnorePreview) -> Result<()> {
    ensure!(
        preview.root == preview.root_bound
            && preview.path == preview.path_bound
            && preview.replacement == preview.replacement_bound,
        "Ignore confirmation changed"
    );
    let current = preview_gitignore(
        &preview.root_bound,
        &preview.path_bound,
        preview.expected.as_deref(),
        &preview.replacement_bound,
    )?;
    ensure!(
        current.fingerprint == preview.fingerprint,
        "Ignore rules or affected paths changed; preview again before applying"
    );
    match &preview.expected {
        Some(expected) => super::save_file(
            &preview.root_bound,
            &preview.path_bound,
            expected,
            &preview.replacement_bound,
        ),
        None => super::files::create_file(
            &preview.root_bound,
            &preview.path_bound,
            preview.replacement_bound.as_bytes(),
        ),
    }
}
fn read_ignore(root: &Path, path: &Path) -> Result<Option<String>> {
    let target = Target::open(root, path)?;
    match target.read() {
        Ok(file) => Ok(Some(String::from_utf8(file.bytes)?)),
        Err(e)
            if e.chain().any(|e| {
                e.downcast_ref::<io::Error>()
                    .is_some_and(|e| e.kind() == io::ErrorKind::NotFound)
            }) =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
fn read_optional(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = vec![];
            file.take(crate::MAX_TEXT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= crate::MAX_TEXT_BYTES,
                "Ignore file exceeds the 2 MiB limit"
            );
            Ok(bytes)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}
fn ignored_paths(
    runner: &GitOperations,
    input: &Path,
    excludes: &Path,
) -> Result<BTreeSet<PathBuf>> {
    // Input has at most 8 MiB of paths; built-in check-ignore emits each path
    // at most once. No subprocess can request user input or run Git filters.
    let mut setting = OsString::from("core.excludesFile=");
    setting.push(excludes);
    let output = runner
        .command(&["--no-literal-pathspecs"])
        .arg("-c")
        .arg(setting)
        .args(["check-ignore", "--no-index", "-z", "--stdin"])
        .stdin(fs::File::open(input)?)
        .output()?;
    ensure!(
        matches!(output.status.code(), Some(0 | 1)),
        "Cannot preview ignore rules: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        output.stdout.len() <= PREVIEW_PATH_BYTES,
        "Ignore match output exceeds the safety limit"
    );
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| git::path(p.strip_prefix(b"./").unwrap_or(p)))
        .collect())
}
