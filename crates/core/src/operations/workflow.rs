//! Stashes are repository-wide; conflicts and operation state belong to one worktree.
//! Call writes only from the application's shared, explicit Git write queue.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs,
    io::Write,
    os::{
        fd::FromRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
    },
    path::Component,
};

pub const MAX_RESOLUTION_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    pub oid: String,
    pub selector: String,
    pub message: String,
    /// Recorded by Git in the stash subject, not an ownership claim.
    pub source_branch: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictStage {
    pub stage: u8,
    pub mode: String,
    pub oid: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictFile {
    pub path: PathBuf,
    pub stages: Vec<ConflictStage>,
}
#[derive(Debug, Clone)]
pub enum ConflictContent {
    Text(String),
    Binary(Vec<u8>),
    Unsupported(String),
}
impl ConflictContent {
    pub fn text(&self) -> Option<&str> {
        if let Self::Text(s) = self {
            Some(s)
        } else {
            None
        }
    }
    fn bytes(&self) -> Result<&[u8]> {
        match self {
            Self::Text(s) => Ok(s.as_bytes()),
            Self::Binary(b) => Ok(b),
            Self::Unsupported(s) => bail!("{s}"),
        }
    }
}
#[derive(Debug, Clone)]
pub struct StageContent {
    pub entry: ConflictStage,
    pub content: ConflictContent,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct DiskVersion {
    bytes: Vec<u8>,
    dev: u64,
    ino: u64,
    modified: (i64, i64),
    mode: u32,
}
#[derive(Debug, Clone)]
pub struct ConflictSnapshot {
    pub file: ConflictFile,
    pub stages: Vec<StageContent>,
    pub working: Option<ConflictContent>,
    root: PathBuf,
    disk: Option<DiskVersion>,
}
#[derive(Debug, Clone)]
pub enum Resolution {
    Ours,
    Theirs,
    Edited(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressKind {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}
impl ProgressKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }
    pub fn side_labels(self) -> (&'static str, &'static str) {
        if self == Self::Rebase {
            (
                "Ours: rebased-onto history (stage 2)",
                "Theirs: commit being replayed (stage 3)",
            )
        } else {
            (
                "Ours: current index side (stage 2)",
                "Theirs: incoming side (stage 3)",
            )
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InProgress {
    pub kind: ProgressKind,
    pub root: PathBuf,
    fingerprint: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct WorkflowState {
    pub root: PathBuf,
    pub stashes: Vec<StashEntry>,
    pub conflicts: Vec<ConflictFile>,
    pub progress: Option<InProgress>,
}

impl GitOperations {
    pub fn workflow_state(&self) -> Result<WorkflowState> {
        Ok(WorkflowState {
            root: self.root.clone(),
            stashes: self.stashes()?,
            conflicts: self.conflicts()?,
            progress: self.in_progress()?,
        })
    }
    pub fn stashes(&self) -> Result<Vec<StashEntry>> {
        let data = self.metadata(&["stash", "list", "--format=%H%x00%gd%x00%gs%x00"])?;
        let mut fields = data.split(|b| *b == 0);
        let mut entries = vec![];
        while let Some(oid) = fields.next() {
            let oid = std::str::from_utf8(oid)?.trim_start_matches('\n');
            if oid.is_empty() {
                break;
            }
            validate_oid(oid)?;
            let selector =
                std::str::from_utf8(fields.next().context("Missing stash selector")?)?.to_owned();
            let message =
                std::str::from_utf8(fields.next().context("Missing stash subject")?)?.to_owned();
            let source_branch = message
                .strip_prefix("On ")
                .or_else(|| message.strip_prefix("WIP on "))
                .and_then(|s| s.split_once(": "))
                .map(|(branch, _)| branch.to_owned());
            entries.push(StashEntry {
                oid: oid.into(),
                selector,
                message,
                source_branch,
            });
        }
        Ok(entries)
    }
    pub fn stash_patch(&self, entry: &StashEntry) -> Result<OperationOutput> {
        self.require_stash_object(&entry.oid)?;
        self.workflow_output(self.checked(&[
            "stash",
            "show",
            "--patch",
            "--include-untracked",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            &entry.oid,
            "--",
        ])?)
    }
    /// Read a tracked or untracked stash file by immutable commit OID.
    pub fn stash_content(
        &self,
        entry: &StashEntry,
        path: &Path,
        untracked: bool,
    ) -> Result<ConflictContent> {
        validate_path(path)?;
        self.require_stash_object(&entry.oid)?;
        let tree = if untracked {
            format!("{}^3", entry.oid)
        } else {
            entry.oid.clone()
        };
        let data = self.metadata(&["ls-tree", "-z", &tree, "--", path_text(path)?])?;
        let record = data
            .split(|b| *b == 0)
            .find(|r| !r.is_empty())
            .context("File is absent from this stash tree")?;
        let (meta, _) = record
            .split_once_byte(b'\t')
            .context("Invalid stash tree entry")?;
        let parts: Vec<_> = std::str::from_utf8(meta)?.split_whitespace().collect();
        ensure!(
            parts.len() == 3 && parts[1] == "blob",
            "This entry is not a file"
        );
        self.workflow_blob(parts[2], parts[0])
    }
    pub fn stash_save(&self, message: &str, include_untracked: bool) -> Result<OperationOutput> {
        ensure!(
            !message.trim().is_empty() && message.len() <= 4096 && !message.contains('\0'),
            "Enter a stash message (up to 4096 bytes)"
        );
        ensure!(
            self.conflicts()?.is_empty(),
            "Resolve unmerged files before saving a stash"
        );
        // No user paths are accepted here. Git stash internally uses a magic
        // pathspec for untracked cleanup, which --literal-pathspecs breaks.
        let mut args = vec![
            "--no-literal-pathspecs",
            "stash",
            "push",
            "--message",
            message,
        ];
        if include_untracked {
            args.push("--include-untracked");
        }
        args.push("--");
        self.workflow_output(self.checked(&args)?)
    }
    pub fn stash_apply(&self, entry: &StashEntry) -> Result<OperationOutput> {
        self.require_stash_object(&entry.oid)?;
        self.workflow_output(self.checked(&["stash", "apply", &entry.oid]).context("Stash apply failed or has conflicts. The stash is retained; inspect the working tree before retrying")?)
    }
    /// Apply by OID, then remove only the still-matching reflog entry on success.
    pub fn stash_pop(&self, entry: &StashEntry) -> Result<OperationOutput> {
        self.require_stash_identity(entry)?;
        let applied = self.stash_apply(entry)?;
        self.stash_drop(entry).context("Stash was applied, but was NOT dropped. The stash list changed or removal failed; refresh before any further action")?;
        Ok(applied)
    }
    /// Caller must present a concrete confirmation naming selector, OID and message.
    pub fn stash_drop(&self, entry: &StashEntry) -> Result<OperationOutput> {
        self.require_stash_identity(entry)?;
        self.workflow_output(self.checked(&["stash", "drop", &entry.selector])?)
    }
    fn require_stash_object(&self, oid: &str) -> Result<()> {
        validate_oid(oid)?;
        self.metadata(&[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{oid}^2^{{commit}}"),
        ])?;
        Ok(())
    }
    fn require_stash_identity(&self, entry: &StashEntry) -> Result<()> {
        ensure!(
            self.stashes()?.iter().any(|current| current == entry),
            "The selected stash moved or changed. Refresh the stash list and select it again; nothing was dropped"
        );
        Ok(())
    }
    pub fn conflicts(&self) -> Result<Vec<ConflictFile>> {
        let data = self.metadata(&["ls-files", "--unmerged", "-z", "--"])?;
        let mut paths: BTreeMap<PathBuf, Vec<ConflictStage>> = BTreeMap::new();
        for record in data.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let (meta, path) = record
                .split_once_byte(b'\t')
                .context("Invalid unmerged entry")?;
            let parts: Vec<_> = std::str::from_utf8(meta)?.split_whitespace().collect();
            ensure!(parts.len() == 3, "Invalid unmerged stages");
            validate_oid(parts[1])?;
            let stage = parts[2].parse::<u8>()?;
            ensure!((1..=3).contains(&stage), "Invalid conflict stage");
            let path = crate::git::path(path);
            validate_path(&path)?;
            paths.entry(path).or_default().push(ConflictStage {
                stage,
                mode: parts[0].into(),
                oid: parts[1].into(),
            });
        }
        Ok(paths
            .into_iter()
            .map(|(path, stages)| ConflictFile { path, stages })
            .collect())
    }
    pub fn inspect_conflict(&self, file: &ConflictFile) -> Result<ConflictSnapshot> {
        self.require_stages(file)?;
        let path = self.safe_working_path(&file.path)?;
        let disk = read_disk(&path)?;
        let working = disk.as_ref().map(|d| content(d.bytes.clone()));
        let stages = file
            .stages
            .iter()
            .map(|entry| {
                Ok(StageContent {
                    entry: entry.clone(),
                    content: self.workflow_blob(&entry.oid, &entry.mode)?,
                })
            })
            .collect::<Result<_>>()?;
        self.require_stages(file)?;
        Ok(ConflictSnapshot {
            file: file.clone(),
            stages,
            working,
            root: self.root.clone(),
            disk,
        })
    }
    fn workflow_blob(&self, oid: &str, mode: &str) -> Result<ConflictContent> {
        validate_oid(oid)?;
        if !matches!(mode, "100644" | "100755") {
            return Ok(ConflictContent::Unsupported(format!(
                "Mode {mode} requires an external Git tool (symlink/submodule/type conflict)"
            )));
        }
        let size = self.metadata(&["cat-file", "-s", oid])?;
        let size = std::str::from_utf8(&size)?.trim().parse::<usize>()?;
        if size > MAX_RESOLUTION_BYTES {
            return Ok(ConflictContent::Unsupported(format!(
                "File exceeds the {} KiB resolution limit; resolve externally",
                MAX_RESOLUTION_BYTES / 1024
            )));
        }
        Ok(content(self.metadata(&["cat-file", "blob", oid])?))
    }
    /// Explicitly save one result and stage it. Never commits or drops a stash.
    /// On stage failure the saved result remains on disk and can be inspected/retried.
    pub fn resolve_conflict(
        &self,
        snapshot: &ConflictSnapshot,
        resolution: &Resolution,
    ) -> Result<OperationOutput> {
        ensure!(
            snapshot.root == self.root,
            "Conflict belongs to a different worktree"
        );
        let desired = match resolution {
            Resolution::Edited(text) => {
                ensure!(
                    text.len() <= MAX_RESOLUTION_BYTES && !text.contains('\0'),
                    "Result must be UTF-8 text within the resolution limit"
                );
                ensure!(
                    snapshot
                        .stages
                        .iter()
                        .all(|s| matches!(s.content, ConflictContent::Text(_))),
                    "Binary/type/large conflicts require an explicit side selection or an external tool"
                );
                let mode = snapshot
                    .stages
                    .iter()
                    .find(|s| s.entry.stage == 2)
                    .or_else(|| snapshot.stages.iter().find(|s| s.entry.stage == 3))
                    .context("No result mode available")?
                    .entry
                    .mode
                    .clone();
                Some((text.as_bytes().to_vec(), mode))
            }
            side => {
                let stage = if matches!(side, Resolution::Ours) {
                    2
                } else {
                    3
                };
                snapshot
                    .stages
                    .iter()
                    .find(|s| s.entry.stage == stage)
                    .map(|s| -> Result<_> {
                        Ok((s.content.bytes()?.to_vec(), s.entry.mode.clone()))
                    })
                    .transpose()?
            }
        };
        let path = self.safe_working_path(&snapshot.file.path)?;
        self.require_stages(&snapshot.file)?;
        ensure!(
            read_disk(&path)? == snapshot.disk,
            "Working file changed externally. Nothing saved; reopen the conflict to compare the new content"
        );
        // Write a new inode: never follow a symlink or modify other hard links.
        if let Some((bytes, mode)) = desired {
            let name = format!(
                ".pokarak-resolution-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_nanos()
            );
            let temp = CString::new(name)?;
            let result = (|| -> Result<()> {
                let mut file = path
                    .open_name(&temp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?
                    .context("Temporary result not created")?;
                file.write_all(&bytes)?;
                file.set_permissions(fs::Permissions::from_mode(if mode == "100755" {
                    0o755
                } else {
                    0o644
                }))?;
                file.sync_all()?;
                path.require_same_parent(&self.safe_working_path(&snapshot.file.path)?)?;
                self.require_stages(&snapshot.file)?;
                ensure!(
                    read_disk(&path)? == snapshot.disk,
                    "Working file changed while preparing the result; nothing saved"
                );
                path.rename(&temp, &path.name)?;
                Ok(())
            })();
            let _ = path.unlink(&temp);
            result?;
        } else {
            // A missing stage represents deletion, including delete/modify conflicts.
            if snapshot.disk.is_some() {
                path.require_same_parent(&self.safe_working_path(&snapshot.file.path)?)?;
                ensure!(
                    read_disk(&path)? == snapshot.disk,
                    "Working file changed before deletion; nothing saved"
                );
                path.unlink(&path.name)?;
            }
        }
        self.require_stages(&snapshot.file).context("Result saved, but index changed before staging. The result remains on disk; inspect it and resolve/stage explicitly")?;
        self.workflow_output(self.checked(&["add", "--", path_text(&snapshot.file.path)?]).context("Result saved, but staging failed. The saved result remains on disk; reopen the conflict or stage it with Git. No commit was made")?)
    }
    fn require_stages(&self, file: &ConflictFile) -> Result<()> {
        ensure!(
            self.conflicts()?.iter().any(|current| current == file),
            "Index conflict stages changed. Reopen this conflict before saving"
        );
        Ok(())
    }
    fn safe_working_path(&self, relative: &Path) -> Result<WorkingPath> {
        validate_path(relative)?;
        let mut parent = fs::File::open(&self.root)?;
        let components: Vec<_> = relative.components().collect();
        for component in &components[..components.len() - 1] {
            let name = CString::new(component.as_os_str().as_bytes())?;
            // SAFETY: owned directory FD and NUL-terminated component stay live.
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            ensure!(
                fd >= 0,
                "Unsafe or missing conflict parent: {}",
                io::Error::last_os_error()
            );
            // SAFETY: openat returned a new owned descriptor.
            parent = unsafe { fs::File::from_raw_fd(fd) };
        }
        let path = WorkingPath {
            parent,
            name: CString::new(components.last().unwrap().as_os_str().as_bytes())?,
        };
        if let Some(file) = path.open_name(&path.name, libc::O_RDONLY)? {
            ensure!(
                file.metadata()?.is_file(),
                "Conflict path is not a regular file"
            );
        }
        Ok(path)
    }
    pub fn in_progress(&self) -> Result<Option<InProgress>> {
        let mut digest = Sha256::new();
        let mut present = BTreeMap::new();
        for name in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge/head-name",
            "rebase-merge/onto",
            "rebase-merge/orig-head",
            "rebase-merge/msgnum",
            "rebase-apply/head-name",
            "rebase-apply/onto",
            "rebase-apply/orig-head",
            "rebase-apply/next",
            "sequencer/todo",
            "sequencer/head",
        ] {
            let raw = self.metadata(&["rev-parse", "--git-path", name])?;
            let p = crate::git::path(raw.strip_suffix(b"\n").unwrap_or(&raw));
            let p = if p.is_absolute() {
                p
            } else {
                self.root.join(p)
            };
            match fs::File::open(p) {
                Ok(file) => {
                    let mut bytes = vec![];
                    file.take((OUTPUT_LIMIT + 1) as u64)
                        .read_to_end(&mut bytes)?;
                    ensure!(
                        bytes.len() <= OUTPUT_LIMIT,
                        "Operation metadata is too large"
                    );
                    digest.update(name);
                    digest.update(&bytes);
                    present.insert(name, bytes);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let kind = if present.contains_key("rebase-merge/head-name")
            || present.contains_key("rebase-apply/head-name")
        {
            Some(ProgressKind::Rebase)
        } else if present.contains_key("MERGE_HEAD") {
            Some(ProgressKind::Merge)
        } else if present.contains_key("CHERRY_PICK_HEAD") {
            Some(ProgressKind::CherryPick)
        } else if present.contains_key("REVERT_HEAD") {
            Some(ProgressKind::Revert)
        } else {
            present.get("sequencer/todo").and_then(|todo| {
                if todo.starts_with(b"pick ") {
                    Some(ProgressKind::CherryPick)
                } else if todo.starts_with(b"revert ") {
                    Some(ProgressKind::Revert)
                } else {
                    None
                }
            })
        };
        if let Some(kind) = kind {
            digest.update(self.metadata(&["rev-parse", "--verify", "HEAD"])?);
            Ok(Some(InProgress {
                kind,
                root: self.root.clone(),
                fingerprint: digest.finalize().to_vec(),
            }))
        } else {
            Ok(None)
        }
    }
    /// Abort requires UI confirmation naming the operation and worktree.
    pub fn finish_operation(&self, expected: &InProgress, abort: bool) -> Result<OperationOutput> {
        ensure!(
            self.in_progress()?.as_ref() == Some(expected),
            "In-progress operation changed; refresh before continuing or aborting"
        );
        if !abort {
            ensure!(
                self.conflicts()?.is_empty(),
                "Resolve and stage all unmerged files before continuing"
            );
        }
        let mut command = self.command(&[
            expected.kind.label(),
            if abort { "--abort" } else { "--continue" },
        ]);
        command
            .env("GIT_EDITOR", "/usr/bin/true")
            .env("GIT_SEQUENCE_EDITOR", "/usr/bin/true")
            .env("GIT_MERGE_AUTOEDIT", "no");
        let result = capture(command, self.timeout)?;
        ensure!(
            result.code == Some(0),
            "{} {} failed. Inspect the operation state before retrying:\n{}\n{}",
            expected.kind.label(),
            if abort { "abort" } else { "continue" },
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout)
        );
        self.workflow_output(result)
    }
    fn workflow_output(&self, output: Captured) -> Result<OperationOutput> {
        Ok(OperationOutput {
            root: self.root.clone(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            truncated: output.truncated,
        })
    }
}
use std::time::SystemTime;
fn validate_oid(oid: &str) -> Result<()> {
    ensure!(
        matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid object identity"
    );
    Ok(())
}
fn validate_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(s) if !s.as_bytes().eq_ignore_ascii_case(b".git") && !s.as_bytes().contains(&0))),
        "Path must be a relative worktree file without traversal or .git components"
    );
    Ok(())
}
fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .context("Non-UTF-8 paths require an external Git tool")
}
fn content(bytes: Vec<u8>) -> ConflictContent {
    if !bytes.contains(&0) {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            return ConflictContent::Text(text.into());
        }
    }
    ConflictContent::Binary(bytes)
}
// Directory-relative file access never follows a replaced parent symlink.
struct WorkingPath {
    parent: fs::File,
    name: CString,
}
impl WorkingPath {
    fn open_name(&self, name: &CString, flags: i32) -> Result<Option<fs::File>> {
        // SAFETY: live parent FD, terminated component; mode used only with O_CREAT.
        let fd = unsafe {
            libc::openat(
                self.parent.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error)
                .context("Cannot safely open conflict file; symlinks are unsupported");
        }
        // SAFETY: newly owned descriptor.
        Ok(Some(unsafe { fs::File::from_raw_fd(fd) }))
    }
    fn require_same_parent(&self, other: &Self) -> Result<()> {
        let a = self.parent.metadata()?;
        let b = other.parent.metadata()?;
        ensure!(
            (a.dev(), a.ino()) == (b.dev(), b.ino()),
            "Conflict parent directory changed; reopen the conflict"
        );
        Ok(())
    }
    fn rename(&self, from: &CString, to: &CString) -> Result<()> {
        // SAFETY: descriptor and both terminated names stay live; neither name follows links.
        ensure!(
            unsafe {
                libc::renameat(
                    self.parent.as_raw_fd(),
                    from.as_ptr(),
                    self.parent.as_raw_fd(),
                    to.as_ptr(),
                )
            } == 0,
            "Cannot install result: {}",
            io::Error::last_os_error()
        );
        Ok(())
    }
    fn unlink(&self, name: &CString) -> Result<()> {
        // SAFETY: directory-relative unlink never follows a symlink.
        ensure!(
            unsafe { libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), 0) } == 0,
            "Cannot remove result: {}",
            io::Error::last_os_error()
        );
        Ok(())
    }
}
fn read_disk(path: &WorkingPath) -> Result<Option<DiskVersion>> {
    let Some(mut file) = path.open_name(&path.name, libc::O_RDONLY)? else {
        return Ok(None);
    };
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.len() <= MAX_RESOLUTION_BYTES as u64,
        "Working file is not a regular file within the 128 KiB resolution limit; resolve externally"
    );
    let mut bytes = vec![];
    (&mut file)
        .take((MAX_RESOLUTION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() <= MAX_RESOLUTION_BYTES
            && (meta.len(), meta.mtime(), meta.mtime_nsec())
                == (after.len(), after.mtime(), after.mtime_nsec()),
        "File changed while reading; reopen it"
    );
    Ok(Some(DiskVersion {
        bytes,
        dev: meta.dev(),
        ino: meta.ino(),
        modified: (meta.mtime(), meta.mtime_nsec()),
        mode: meta.mode(),
    }))
}
trait SplitByte {
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])>;
}
impl SplitByte for [u8] {
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])> {
        self.iter()
            .position(|b| *b == byte)
            .map(|i| (&self[..i], &self[i + 1..]))
    }
}

#[cfg(test)]
mod tests;
