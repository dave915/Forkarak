//! Explicit merge or normal rebase of the checked-out branch.
use super::{history_actions::HistoryTarget, *};
use std::{collections::BTreeSet, ffi::OsString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationKind {
    Merge,
    Rebase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchIntegration {
    pub current: HistoryTarget,
    /// Fully qualified local or remote-tracking branch ref.
    pub reference: String,
    pub oid: String,
}

impl BranchIntegration {
    /// Keep the namespace visible even when local and remote names overlap.
    pub fn label(&self) -> &str {
        self.reference
            .strip_prefix("refs/")
            .unwrap_or(&self.reference)
    }
}

impl GitOperations {
    pub fn integration_choices(&self) -> Result<Vec<BranchIntegration>> {
        let current = self.history_target()?;
        let current_ref = format!("refs/heads/{}", current.branch);
        let bytes = self.metadata(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(objecttype)%00%(symref)",
            "--",
            "refs/heads/",
            "refs/remotes/",
        ])?;
        let mut choices = Vec::new();
        for line in std::str::from_utf8(&bytes)?.lines() {
            let mut fields = line.splitn(4, '\0');
            let reference = fields.next().context("Missing branch name")?;
            let oid = fields.next().context("Missing branch commit")?;
            if fields.next() != Some("commit")
                || fields.next() != Some("")
                || reference == current_ref
            {
                continue;
            }
            crate::history::validate_oid(oid)?;
            choices.push(BranchIntegration {
                current: current.clone(),
                reference: reference.into(),
                oid: oid.into(),
            });
        }
        Ok(choices)
    }

    pub fn integrate_branch(
        &self,
        kind: IntegrationKind,
        expected: &BranchIntegration,
    ) -> Result<OperationOutput> {
        ensure!(
            self.in_progress()?.is_none(),
            "Finish or abort the current Git operation first"
        );
        ensure!(
            self.metadata(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignore-submodules=none",
                "--"
            ])?
            .is_empty(),
            "Merge and rebase require a clean index and worktree. Commit or move local changes first."
        );
        crate::history::validate_oid(&expected.oid)?;
        crate::history::validate_oid(&expected.current.head)?;
        ensure!(
            expected.current.root == self.root,
            "The confirmed worktree changed"
        );
        self.guard_integration_ignored(expected, kind)?;
        ensure!(
            self.integration_choices()?.contains(expected),
            "The checked-out branch, HEAD or selected branch changed. Reopen the branch action and confirm both commits again."
        );
        let mut command = self.command(&[
            "-c",
            "rerere.enabled=false",
            "-c",
            "rerere.autoupdate=false",
        ]);
        match kind {
            IntegrationKind::Merge => {
                let message = format!(
                    "Merge {} into {}",
                    expected.label(),
                    expected.current.branch
                );
                command
                    .arg("-c")
                    .arg(format!("branch.{}.mergeOptions=", expected.current.branch))
                    .args([
                        "merge",
                        "--ff",
                        "--commit",
                        "--no-squash",
                        "--no-edit",
                        "--no-autostash",
                        "--no-overwrite-ignore",
                        "-m",
                        &message,
                        "--",
                        &expected.oid,
                    ]);
            }
            IntegrationKind::Rebase => {
                command.args([
                    "rebase",
                    "--merge",
                    "--no-autostash",
                    "--no-update-refs",
                    "--no-autosquash",
                    "--no-fork-point",
                    "--no-rebase-merges",
                    "--",
                    &expected.oid,
                ]);
            }
        }
        command
            .env("GIT_EDITOR", "/usr/bin/true")
            .env("GIT_SEQUENCE_EDITOR", "/usr/bin/true")
            .env("GIT_MERGE_AUTOEDIT", "no");
        let result = self.run_command(command)?;
        let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        ensure!(
            result.code == Some(0),
            "{kind:?} did not complete. For conflicts, open Stash / conflicts to resolve and continue or abort.\n{stderr}\n{stdout}"
        );
        Ok(OperationOutput {
            root: self.root.clone(),
            stdout,
            stderr,
            truncated: result.truncated,
        })
    }

    // The ort merge strategy can replace ignored paths even with
    // --no-overwrite-ignore. Rebase can also materialize intermediate versions.
    fn guard_integration_ignored(
        &self,
        expected: &BranchIntegration,
        kind: IntegrationKind,
    ) -> Result<()> {
        let ignored = self.metadata(&[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
            "-z",
            "--",
        ])?;
        let mut scopes = BTreeSet::<OsString>::new();
        for path in ignored.split(|&b| b == 0).filter(|path| !path.is_empty()) {
            if let Some(first) = crate::git::path(path).components().next() {
                scopes.insert(first.as_os_str().to_owned());
            }
        }
        if scopes.is_empty() {
            return Ok(());
        }
        let replay = format!("{}..{}", expected.oid, expected.current.head);
        let checkout = &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "-z",
            &expected.current.head,
            &expected.oid,
            "--",
        ][..];
        let commits = &[
            "log",
            "--no-merges",
            "--full-history",
            "--root",
            "--format=",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            &replay,
            "--",
        ][..];
        let ranges = match kind {
            IntegrationKind::Merge => vec![checkout],
            IntegrationKind::Rebase => vec![checkout, commits],
        };
        for args in ranges {
            let mut command = self.command(args);
            command.args(&scopes);
            let changes = self.run_command(command)?;
            ensure!(
                changes.code == Some(0) && !changes.truncated,
                "Could not inspect ignored paths before {kind:?}: {}",
                String::from_utf8_lossy(&changes.stderr)
            );
            self.guard_ignored_paths(&changes.stdout)?;
        }
        Ok(())
    }
}
