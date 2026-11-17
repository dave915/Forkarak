//! Only read-only Git plumbing lives here. Never run status/diff filters,
//! refresh an index, materialize objects, or lazily fetch a missing object.
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::Read,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

use crate::{CompareMode, Worktree};

const MAX_METADATA: usize = 32 * 1024 * 1024;

pub(crate) struct Output {
    pub bytes: Vec<u8>,
    pub success: bool,
    pub error: String,
    pub truncated: bool,
}

pub(crate) fn run(root: &Path, args: &[&OsStr], limit: usize) -> Result<Output> {
    let mut command = Command::new("git");
    // An inherited GIT_DIR/INDEX_FILE/WORK_TREE must not redirect a worktree read.
    for (key, _) in std::env::vars_os() {
        if key.as_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .args([
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.quotePath=false",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-C",
        ])
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context("Could not launch Git; install Git and ensure it is on PATH")?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    // Drain diagnostics concurrently, retaining at most 16 KiB.
    let errors = thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            let retain = n.min((16 * 1024usize).saturating_sub(kept.len()));
            kept.extend_from_slice(&buf[..retain]);
        }
        kept
    });
    let mut bytes = Vec::new();
    let result = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes);
    let truncated = bytes.len() > limit;
    if truncated || result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait();
    let error = String::from_utf8_lossy(&errors.join().unwrap_or_default())
        .trim()
        .to_owned();
    result.context("Could not read Git output")?;
    let status = status.context("Could not wait for Git")?;
    bytes.truncate(limit);
    Ok(Output {
        bytes,
        success: status.success(),
        error,
        truncated,
    })
}

pub(crate) fn output(root: &Path, args: &[&OsStr]) -> Result<Vec<u8>> {
    let result = run(root, args, MAX_METADATA)?;
    ensure!(
        !result.truncated,
        "Git metadata exceeds the 32 MiB safety limit; narrow the repository before opening it"
    );
    ensure!(
        result.success,
        "Git {:?} failed in {}: {}",
        args.first(),
        root.display(),
        result.error
    );
    Ok(result.bytes)
}

pub(crate) fn words(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    output(root, &args.iter().map(OsStr::new).collect::<Vec<_>>())
}

pub(crate) fn path(bytes: &[u8]) -> PathBuf {
    OsString::from_vec(bytes.to_vec()).into()
}

pub(crate) fn line(bytes: &[u8]) -> Result<&str> {
    Ok(std::str::from_utf8(
        bytes.strip_suffix(b"\n").unwrap_or(bytes),
    )?)
}

pub(crate) fn repository_root(path: &Path) -> Result<PathBuf> {
    let location = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let bytes = words(location, &["rev-parse", "--show-toplevel"]).with_context(|| {
        format!(
            "{} is not an accessible Git worktree; select a repository directory",
            path.display()
        )
    })?;
    // Remove exactly Git's delimiter: a root may itself end in a newline.
    let root = self::path(bytes.strip_suffix(b"\n").unwrap_or(&bytes));
    root.canonicalize()
        .context("The Git worktree root no longer exists")
}

pub(crate) fn head(root: &Path) -> Result<Option<String>> {
    let result = run(
        root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("--end-of-options"),
            OsStr::new("HEAD^{commit}"),
        ],
        256,
    )?;
    if result.success {
        return Ok(Some(line(&result.bytes)?.to_owned()));
    }
    let symbolic = run(
        root,
        &[
            OsStr::new("symbolic-ref"),
            OsStr::new("--quiet"),
            OsStr::new("HEAD"),
        ],
        4096,
    )?;
    if symbolic.success {
        let reference = line(&symbolic.bytes)?;
        let exists = run(
            root,
            &[
                OsStr::new("show-ref"),
                OsStr::new("--verify"),
                OsStr::new("--quiet"),
                OsStr::new("--"),
                OsStr::new(reference),
            ],
            256,
        )?;
        if !exists.success && exists.error.is_empty() {
            return Ok(None);
        }
    }
    bail!(
        "Cannot read HEAD in {}; repair the repository or select another worktree: {}",
        root.display(),
        result.error
    )
}

pub(crate) fn baseline(
    root: &Path,
    compare: &CompareMode,
    head: Option<&str>,
) -> Result<Option<String>> {
    match compare {
        CompareMode::Uncommitted => Ok(head.map(str::to_owned)),
        CompareMode::Branch(base) => {
            ensure!(
                !base.is_empty(),
                "Choose a nonempty comparison branch or commit"
            );
            let expression = format!("{base}^{{commit}}");
            let resolved = run(
                root,
                &[
                    OsStr::new("rev-parse"),
                    OsStr::new("--verify"),
                    OsStr::new("--end-of-options"),
                    OsStr::new(&expression),
                ],
                1024,
            )?;
            ensure!(
                resolved.success,
                "Invalid comparison base {base:?}; choose an existing local branch, remote-tracking branch, or commit (fetch it explicitly if absent): {}",
                resolved.error
            );
            let head = head.context("This repository has no commits yet; choose Uncommitted until the first commit exists")?;
            let merge = run(
                root,
                &[
                    OsStr::new("merge-base"),
                    OsStr::new("--"),
                    OsStr::new(line(&resolved.bytes)?),
                    OsStr::new(head),
                ],
                1024,
            )?;
            ensure!(
                merge.success,
                "No merge base between {base:?} and HEAD; choose a related branch or fetch missing history explicitly: {}",
                merge.error
            );
            Ok(Some(line(&merge.bytes)?.to_owned()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub mode: String,
    pub oid: String,
}

pub(crate) type Tree = BTreeMap<PathBuf, Entry>;
pub(crate) type Index = BTreeMap<PathBuf, BTreeMap<u8, Entry>>;

pub(crate) fn tree(root: &Path, revision: Option<&str>) -> Result<Tree> {
    let Some(revision) = revision else {
        return Ok(Tree::new());
    };
    let bytes = words(
        root,
        &["ls-tree", "-r", "-z", "--full-tree", revision, "--"],
    )?;
    let mut entries = Tree::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .context("Malformed Git tree record")?;
        let fields = std::str::from_utf8(&record[..tab])?
            .split(' ')
            .collect::<Vec<_>>();
        ensure!(fields.len() == 3, "Malformed Git tree entry");
        entries.insert(
            path(&record[tab + 1..]),
            Entry {
                mode: fields[0].into(),
                oid: fields[2].into(),
            },
        );
    }
    Ok(entries)
}

pub(crate) fn index(root: &Path) -> Result<Index> {
    let bytes = words(root, &["ls-files", "--stage", "--full-name", "-z", "--"])?;
    let mut entries = Index::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .context("Malformed Git index record")?;
        let fields = std::str::from_utf8(&record[..tab])?
            .split(' ')
            .collect::<Vec<_>>();
        ensure!(fields.len() == 3, "Malformed Git index entry");
        entries.entry(path(&record[tab + 1..])).or_default().insert(
            fields[2].parse()?,
            Entry {
                mode: fields[0].into(),
                oid: fields[1].into(),
            },
        );
    }
    Ok(entries)
}

pub(crate) fn untracked(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(words(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
            "--",
        ],
    )?
    .split(|b| *b == 0)
    .filter(|r| !r.is_empty())
    .map(path)
    .collect())
}

pub(crate) fn blob(root: &Path, entry: &Entry) -> Result<(Vec<u8>, bool)> {
    if entry.mode == "160000" {
        return Ok((
            format!("Subproject commit {}\n", entry.oid).into_bytes(),
            false,
        ));
    }
    let result = run(
        root,
        &[
            OsStr::new("cat-file"),
            OsStr::new("blob"),
            OsStr::new(&entry.oid),
        ],
        crate::MAX_TEXT_BYTES,
    )?;
    ensure!(
        result.success || result.truncated,
        "Cannot read Git blob {} (fetch missing objects explicitly): {}",
        entry.oid,
        result.error
    );
    Ok((result.bytes, result.truncated))
}

pub(crate) fn worktrees(root: &Path) -> Result<Vec<Worktree>> {
    let bytes = words(root, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(parse_worktrees(&bytes))
}

pub(crate) fn parse_worktrees(bytes: &[u8]) -> Vec<Worktree> {
    let mut result = Vec::new();
    let mut current = Worktree::default();
    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            if !current.path.as_os_str().is_empty() {
                current.is_main = result.is_empty();
                if current.branch.is_empty() {
                    current.branch = if current.is_detached {
                        "(detached HEAD)"
                    } else {
                        "(bare)"
                    }
                    .into();
                }
                result.push(std::mem::take(&mut current));
            }
        } else if let Some(p) = field.strip_prefix(b"worktree ") {
            current.path = path(p);
            if let Ok(canonical) = current.path.canonicalize() {
                current.path = canonical;
            }
        } else if let Some(head) = field.strip_prefix(b"HEAD ") {
            current.head = String::from_utf8_lossy(head).into_owned();
            if current.head.bytes().all(|b| b == b'0') {
                current.head.clear();
            }
        } else if let Some(branch) = field.strip_prefix(b"branch ") {
            current.branch =
                String::from_utf8_lossy(branch.strip_prefix(b"refs/heads/").unwrap_or(branch))
                    .into_owned();
        } else if field == b"detached" {
            current.is_detached = true;
        } else if field == b"locked" || field.starts_with(b"locked ") {
            current.is_locked = true;
        } else if field == b"prunable" || field.starts_with(b"prunable ") {
            current.is_prunable = true;
        }
    }
    result
}

pub(crate) fn branches(root: &Path) -> Result<Vec<String>> {
    let bytes = words(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/",
            "refs/remotes/",
        ],
    )?;
    Ok(std::str::from_utf8(&bytes)?
        .lines()
        .filter(|s| !s.ends_with("/HEAD"))
        .map(str::to_owned)
        .collect())
}
