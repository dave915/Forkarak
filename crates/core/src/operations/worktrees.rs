//! User-selected linked worktree operations. Removing a worktree keeps its branch.
use super::*;
use crate::{Worktree, git};
use std::{fs, os::unix::ffi::OsStrExt, path::Component};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeBranch {
    Existing(String),
    /// Create from the displayed HEAD.
    New(String),
}

impl GitOperations {
    pub fn create_worktree(
        &self,
        destination: &Path,
        branch: &WorktreeBranch,
        expected_head: &str,
    ) -> Result<OperationOutput> {
        let destination = worktree_destination(destination)?;
        ensure!(
            !destination.starts_with(self.worktree_common_dir()?),
            "Choose a folder outside the Git administrative directory"
        );
        let mut command = self.command(&["worktree", "add", "--no-guess-remote"]);
        match branch {
            WorktreeBranch::Existing(name) => {
                self.require_local_branch(name)?;
                self.require_unoccupied(name)?;
                command.arg("--").arg(&destination).arg(name);
            }
            WorktreeBranch::New(name) => {
                self.validate_branch(name)?;
                command
                    .args(["--no-track", "-b", name, "--"])
                    .arg(&destination)
                    .arg(expected_head);
            }
        }
        crate::history::validate_oid(expected_head)?;
        let head = self.metadata(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        ensure!(
            git::line(&head)? == expected_head,
            "HEAD changed. Refresh the worktree list and confirm the starting point again."
        );
        self.run_worktree_change(command)
    }

    /// Accepts the worktree shown in the confirmation, including its full HEAD.
    /// A missing worktree can be unregistered individually without pruning others.
    pub fn remove_worktree(&self, selected: &Worktree) -> Result<OperationOutput> {
        let validate = || -> Result<()> {
            let records = self.metadata(&["worktree", "list", "--porcelain", "-z"])?;
            let current = git::parse_worktrees(&records)
                .into_iter()
                .find(|w| w.path == selected.path)
                .context("The selected worktree disappeared; refresh the worktree list")?;
            ensure!(
                !current.is_main && current.path != self.root,
                "Open a different worktree before removing a linked worktree. The primary worktree is permanent."
            );
            ensure!(!current.is_locked, "This worktree is locked");
            ensure!(
                current.head == selected.head
                    && current.branch == selected.branch
                    && current.is_detached == selected.is_detached
                    && current.is_prunable == selected.is_prunable,
                "The selected worktree changed. Refresh and confirm its path, branch and HEAD again."
            );
            Ok(())
        };
        validate()?;
        if selected.is_prunable {
            ensure!(
                matches!(selected.path.symlink_metadata(), Err(e) if e.kind() == io::ErrorKind::NotFound),
                "A directory or file still exists at this stale worktree path. Inspect it before cleanup."
            );
        } else {
            ensure!(
                selected.path.symlink_metadata()?.is_dir(),
                "The selected worktree directory changed"
            );
            let target = Self {
                root: selected.path.clone(),
                ..self.clone()
            };
            let raw = target.metadata(&["rev-parse", "--absolute-git-dir"])?;
            let admin = git::path(raw.strip_suffix(b"\n").unwrap_or(&raw)).canonicalize()?;
            let common = self.worktree_common_dir()?;
            ensure!(
                admin.parent() == Some(common.join("worktrees").as_path()),
                "The folder now points to another Git repository"
            );
            let link = fs::read(admin.join("gitdir"))?;
            let link = git::path(link.strip_suffix(b"\n").unwrap_or(&link));
            ensure!(
                admin.join(link).canonicalize()? == selected.path.join(".git"),
                "The worktree registration points to a different folder"
            );
            ensure!(
                target.in_progress()?.is_none(),
                "Finish the in-progress Git operation in this worktree before removing it"
            );
            let status = target.metadata(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignored=matching",
                "--ignore-submodules=none",
                "--",
            ])?;
            ensure!(
                status.is_empty(),
                "The selected worktree has staged, modified, untracked or ignored files. Move or commit them before removing it."
            );
            // Git status trusts assume-unchanged and sparse index entries. A
            // materialized file with either flag may contain private modifications.
            let flags = capture_with_limit(
                target.command(&["ls-files", "-v", "-z", "--"]),
                target.timeout,
                &target.cancellation,
                32 * 1024 * 1024,
            )?;
            ensure!(
                flags.code == Some(0) && !flags.truncated,
                "Could not inspect all index flags before removal"
            );
            let flags = flags.stdout;
            for row in flags.split(|b| *b == 0).filter(|row| row.len() >= 3) {
                if row[0].is_ascii_lowercase() || row[0] == b'S' {
                    let path = target.root.join(git::path(&row[2..]));
                    ensure!(
                        matches!(path.symlink_metadata(), Err(e) if e.kind() == io::ErrorKind::NotFound),
                        "Git is skipping change checks for {}. Verify this file and clear its index flag before removal.",
                        path.display()
                    );
                }
            }
        }
        validate()?;
        let mut command = self.command(&["worktree", "remove", "--"]);
        command.arg(&selected.path);
        self.run_worktree_change(command)
    }

    fn worktree_common_dir(&self) -> Result<PathBuf> {
        let path = self.metadata(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        Ok(git::path(path.strip_suffix(b"\n").unwrap_or(&path)).canonicalize()?)
    }

    fn run_worktree_change(&self, command: Command) -> Result<OperationOutput> {
        let result = self.run_command(command)?;
        let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        ensure!(
            result.code == Some(0),
            "Git worktree operation failed. Refresh the worktrees and branches before retrying.\n{stderr}\n{stdout}"
        );
        Ok(OperationOutput {
            root: self.root.clone(),
            stdout,
            stderr,
            truncated: result.truncated,
        })
    }
}

// Resolve symlinked parents and reject a symlink at the selected destination.
// Missing parent directories are supported by git worktree add.
fn worktree_destination(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "Enter an absolute worktree folder path");
    ensure!(
        !path.components().any(|c| matches!(c, Component::ParentDir)),
        "Choose the destination folder without '..'"
    );
    match fs::symlink_metadata(path) {
        Ok(meta) => ensure!(
            meta.is_dir() && fs::read_dir(path)?.next().is_none(),
            "Choose a new or empty folder for the worktree"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let existing = path
        .ancestors()
        .find(|dir| dir.exists())
        .context("No accessible parent directory")?;
    let canonical = existing.canonicalize()?.join(path.strip_prefix(existing)?);
    ensure!(
        !canonical
            .components()
            .any(|c| c.as_os_str().as_bytes().eq_ignore_ascii_case(b".git")),
        "Choose a folder outside the Git administrative directory"
    );
    Ok(canonical)
}
