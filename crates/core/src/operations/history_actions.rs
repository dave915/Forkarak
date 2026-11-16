//! Single-commit actions. All subprocesses use the shared cancellable runner.
use super::*;
use crate::history::validate_oid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAction {
    CherryPick,
    Revert,
}
impl HistoryAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }
}
/// Immutable confirmation target. Capture immediately before showing the user
/// the branch, worktree and HEAD, and revalidate immediately before the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTarget {
    pub root: PathBuf,
    pub branch: String,
    pub head: String,
}
impl GitOperations {
    pub fn history_target(&self) -> Result<HistoryTarget> {
        let branch = self.current_branch()?.context(
            "History actions require a checked-out branch; detached HEAD is read-only here",
        )?;
        let head = self.metadata(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let head = std::str::from_utf8(&head)?
            .trim_end_matches('\n')
            .to_owned();
        validate_oid(&head)?;
        Ok(HistoryTarget {
            root: self.root.clone(),
            branch,
            head,
        })
    }
    pub fn history_action(
        &self,
        action: HistoryAction,
        source: &str,
        target: &HistoryTarget,
    ) -> Result<OperationOutput> {
        validate_oid(source)?;
        ensure!(
            self.history_target()? == *target,
            "The target branch, HEAD or worktree changed; reopen history and confirm the destination again"
        );
        ensure!(
            self.in_progress()?.is_none(),
            "A Git operation is already in progress. Open Stash / conflicts to continue or abort it first"
        );
        let kind = self.metadata(&["cat-file", "-t", source])?;
        ensure!(kind == b"commit\n", "Select a commit object");
        let parents = self.metadata(&["rev-list", "--parents", "-n", "1", source, "--"])?;
        ensure!(
            std::str::from_utf8(&parents)?.split_whitespace().count() <= 2,
            "Merge commits require an explicit mainline parent. Cherry-pick/revert of merge commits is not supported here; use Git externally with a chosen -m parent"
        );
        ensure!(
            self.metadata(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--"
            ])?
            .is_empty(),
            "History actions require a clean worktree and index. Commit or move local changes first; no stash or discard is performed"
        );
        // Ignore unrelated build/editor output, but never let Git replace an
        // ignored file (including a file/directory ancestor collision). Explicit
        // --no-renames includes both sides of a rename regardless of config.
        let affected = self.metadata(&[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--no-renames",
            "--name-only",
            "-r",
            "-z",
            source,
            "--",
        ])?;
        let affected = affected
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(crate::git::path)
            .collect::<Vec<_>>();
        // Scope ignored-file enumeration to affected paths. A large unrelated
        // target/ or node_modules/ must not consume the metadata output budget.
        // Also include non-directory ancestors (including symlinks), which can
        // collide with an incoming directory even if its child path is absent.
        let mut ignored_paths = std::collections::BTreeSet::new();
        for changed in &affected {
            ignored_paths.insert(changed.clone());
            for ancestor in changed
                .ancestors()
                .skip(1)
                .filter(|p| !p.as_os_str().is_empty())
            {
                match std::fs::symlink_metadata(self.root.join(ancestor)) {
                    Ok(metadata) if !metadata.is_dir() => {
                        ignored_paths.insert(ancestor.to_owned());
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if !ignored_paths.is_empty() {
            let mut command = self.command(&[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
                "-z",
                "--",
            ]);
            command.args(ignored_paths);
            let ignored = self.run_command(command)?;
            ensure!(
                ignored.code == Some(0) && !ignored.truncated,
                "Could not safely inspect ignored paths for this commit; no history action was started"
            );
            if let Some(path) = ignored.stdout.split(|b| *b == 0).find(|b| !b.is_empty()) {
                bail!(
                    "Ignored path {:?} overlaps a path changed by this commit. Move it explicitly before continuing; no file was discarded",
                    crate::git::path(path)
                );
            }
        }
        // Read the confirmation target again after all preflight work. The shared
        // UI job guard serializes Pokarak writes; Git owns the repository locks.
        ensure!(
            self.history_target()? == *target,
            "The target branch or HEAD changed during validation; confirm again"
        );
        let mut command = self.command(&[
            "-c",
            "rerere.enabled=false",
            "-c",
            "rerere.autoupdate=false",
            action.label(),
            "--no-edit",
            source,
            "--",
        ]);
        command
            .env("GIT_EDITOR", "/usr/bin/true")
            .env("GIT_SEQUENCE_EDITOR", "/usr/bin/true")
            .env("GIT_MERGE_AUTOEDIT", "no");
        let result = self.run_command(command)?;
        ensure!(
            result.code == Some(0),
            "{} did not complete. Any conflict/index state is retained. Open Stash / conflicts to resolve, continue or explicitly abort. No automatic cleanup was performed.\n{}\n{}",
            action.label(),
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout)
        );
        Ok(OperationOutput {
            root: self.root.clone(),
            stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            truncated: result.truncated,
        })
    }
}
#[cfg(test)]
mod tests;
