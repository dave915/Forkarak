//! Independent, read-only index view. Preview bytes are captured blob identities,
//! never working files, textconv/clean filters, or semantic navigation sources.
use crate::{ChangedFile, DiffDocument, FileStatus, backend, filesystem, git, text};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexGuard {
    pub(crate) root: PathBuf,
    git_dir: PathBuf,
    pub(crate) head: Option<String>,
    pub(crate) branch: Option<String>,
    digest: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PreviousCommit {
    pub oid: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct IndexSnapshot {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub staged: Vec<ChangedFile>,
    pub unstaged: Vec<ChangedFile>,
    pub previous: Option<PreviousCommit>,
    pub has_conflicts: bool,
    pub guard: IndexGuard,
    entries: git::Index,
    baseline: git::Tree,
}

fn git_path(root: &Path, name: &str) -> Result<PathBuf> {
    let bytes = git::words(root, &["rev-parse", "--git-path", name])?;
    let path = git::path(bytes.strip_suffix(b"\n").unwrap_or(&bytes));
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

/// Also binds the snapshot to the branch and Git directory: same HEAD/index in
/// another worktree, or a checkout to an equal commit, is not the same target.
pub(crate) fn guard(root: &Path) -> Result<IndexGuard> {
    let root = git::repository_root(root)?;
    let head = git::head(&root)?;
    let branch = git::run(
        &root,
        &["symbolic-ref".as_ref(), "--quiet".as_ref(), "HEAD".as_ref()],
        4096,
    )?;
    ensure!(
        !branch.truncated && (branch.success || branch.error.is_empty()),
        "Cannot read current branch"
    );
    let branch = branch
        .success
        .then(|| git::line(&branch.bytes).map(str::to_owned))
        .transpose()?;
    let git_dir = git_path(&root, "")?.canonicalize()?;
    let mut digest = Sha256::new();
    // Raw index detects flags such as intent-to-add; staged entries include the
    // shared index contents when split-index is enabled. Never write-tree here.
    match fs::File::open(git_path(&root, "index")?) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 32 * 1024 * 1024,
                "Index exceeds the 32 MiB preview limit"
            );
            filesystem::feed(&mut digest, &bytes);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            filesystem::feed(&mut digest, b"no index")
        }
        Err(error) => return Err(error.into()),
    }
    filesystem::feed(
        &mut digest,
        &git::words(&root, &["ls-files", "--stage", "-z", "--"])?,
    );
    Ok(IndexGuard {
        root,
        git_dir,
        head,
        branch,
        digest: digest.finalize().to_vec(),
    })
}

pub fn snapshot(path: &Path) -> Result<IndexSnapshot> {
    let expected = guard(path)?;
    let root = &expected.root;
    let entries = git::index(root)?;
    let baseline = git::tree(root, expected.head.as_deref())?;
    let mut args = vec![
        "diff",
        "--cached",
        "--name-status",
        "-z",
        "--find-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--ita-invisible-in-index",
    ];
    if let Some(head) = &expected.head {
        args.push(head);
    }
    args.push("--");
    let raw = git::words(root, &args)?;
    let mut records = raw
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut staged = Vec::new();
    while let Some(status) = records.next() {
        let first = git::path(records.next().context("Incomplete staged path")?);
        let (path, old_path) = if status.starts_with(b"R") {
            (
                git::path(records.next().context("Incomplete staged rename")?),
                Some(first),
            )
        } else {
            (first, None)
        };
        let status = match status[0] {
            b'A' => FileStatus::Added,
            b'D' => FileStatus::Deleted,
            b'R' => FileStatus::Renamed,
            b'U' => FileStatus::Conflicted,
            _ => FileStatus::Modified,
        };
        staged.push(ChangedFile {
            path,
            old_path,
            status,
            staged: true,
            ..Default::default()
        });
    }
    let previous = expected
        .head
        .as_ref()
        .map(|head| -> Result<PreviousCommit> {
            let bytes = git::words(
                root,
                &[
                    "show",
                    "--no-patch",
                    "--no-show-signature",
                    "--format=%an%x00%ae%x00%aI%x00%B",
                    head,
                    "--",
                ],
            )?;
            let fields = bytes.splitn(4, |byte| *byte == 0).collect::<Vec<_>>();
            ensure!(fields.len() == 4, "Cannot read previous commit metadata");
            Ok(PreviousCommit {
                oid: head.clone(),
                author: String::from_utf8_lossy(fields[0]).into_owned(),
                email: String::from_utf8_lossy(fields[1]).into_owned(),
                date: String::from_utf8_lossy(fields[2]).into_owned(),
                message: String::from_utf8_lossy(fields[3])
                    .trim_end_matches('\n')
                    .into(),
            })
        })
        .transpose()?;
    let unstaged = backend::commit_working_changes(root)?;
    ensure!(
        guard(root)? == expected,
        "HEAD or index changed while reading; refresh the commit view"
    );
    Ok(IndexSnapshot {
        root: root.clone(),
        branch: expected.branch.clone(),
        staged,
        unstaged,
        previous,
        has_conflicts: entries
            .values()
            .any(|stages| stages.keys().any(|stage| *stage != 0)),
        guard: expected,
        entries,
        baseline,
    })
}

/// Render the selected snapshot's index blobs, even if the worktree changed.
/// A later write must independently validate `guard` before executing.
pub fn diff(snapshot: &IndexSnapshot, path: &Path) -> Result<DiffDocument> {
    let file = snapshot
        .staged
        .iter()
        .find(|file| file.path == path)
        .context("Select a staged file from this snapshot")?;
    ensure!(
        file.status != FileStatus::Conflicted,
        "Unmerged index stages: use Stash / conflict resolution"
    );
    let old_entry = snapshot
        .baseline
        .get(file.old_path.as_deref().unwrap_or(path));
    let new_entry = snapshot.entries.get(path).and_then(|stages| stages.get(&0));
    let old = old_entry
        .map(|entry| git::blob(&snapshot.root, entry))
        .transpose()?
        .unwrap_or_default();
    let new = new_entry
        .map(|entry| git::blob(&snapshot.root, entry))
        .transpose()?
        .unwrap_or_default();
    let mut document = DiffDocument {
        path: path.into(),
        old_path: file.old_path.clone(),
        ..Default::default()
    };
    text::render(&mut document, old, new);
    if old_entry.map(|entry| &entry.mode) != new_entry.map(|entry| &entry.mode) {
        document.patch = format!(
            "mode {} → {}\n{}",
            old_entry
                .map(|entry| entry.mode.as_str())
                .unwrap_or("absent"),
            new_entry
                .map(|entry| entry.mode.as_str())
                .unwrap_or("absent"),
            document.patch
        );
    }
    if document.patch.is_empty() {
        document.patch = "Index metadata changed; file content is unchanged.\n".into();
    }
    Ok(document)
}
