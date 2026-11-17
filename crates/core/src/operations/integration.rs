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
        let ignored: Vec<_> = ignored
            .split(|&b| b == 0)
            .filter(|path| !path.is_empty())
            .map(crate::git::path)
            .collect();
        let scopes: BTreeSet<OsString> = ignored
            .iter()
            .filter_map(|path| {
                path.components()
                    .next()
                    .map(|first| first.as_os_str().to_owned())
            })
            .collect();
        if scopes.is_empty() {
            return Ok(());
        }
        self.guard_directory_renames(expected, &ignored)?;
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
    // ort can infer a new filename absent from both input trees when one
    // side moves a directory and the other adds a file under its old name.
    // Preserve ignored files within these directories as well.
    fn guard_directory_renames(
        &self,
        expected: &BranchIntegration,
        ignored: &[PathBuf],
    ) -> Result<()> {
        let range = format!("{}...{}", expected.current.head, expected.oid);
        let log = self.metadata(&[
            "log",
            "--format=",
            "--name-status",
            "-z",
            "--diff-merges=first-parent",
            "--find-renames",
            "--diff-filter=R",
            &range,
            "--",
        ])?;
        guard_renamed_dirs(&log, ignored)?;
        // Net renames can span several commits (e.g. copying and then
        // deleting in a later commit), so check each merge input as well.
        let bases = self.run(&["merge-base", "--all", &expected.current.head, &expected.oid])?;
        ensure!(
            matches!(bases.code, Some(0 | 1)) && !bases.truncated,
            "Could not inspect common ancestors before integration"
        );
        for base in std::str::from_utf8(&bases.stdout)?.lines() {
            for tip in [&expected.current.head, &expected.oid] {
                if tip != base {
                    let names = self.metadata(&[
                        "diff",
                        "--name-status",
                        "-z",
                        "--find-renames",
                        "--diff-filter=R",
                        base,
                        tip,
                        "--",
                    ])?;
                    guard_renamed_dirs(&names, ignored)?;
                }
            }
        }
        Ok(())
    }
}

fn guard_renamed_dirs(names: &[u8], ignored: &[PathBuf]) -> Result<()> {
    let mut fields = names.split(|&b| b == 0).filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        ensure!(
            status.starts_with(b"R"),
            "Could not inspect directory renames"
        );
        let from = crate::git::path(fields.next().context("Missing old rename path")?);
        let to = crate::git::path(fields.next().context("Missing new rename path")?);
        let mut old = from.parent().unwrap_or(Path::new(""));
        let mut new = to.parent().unwrap_or(Path::new(""));
        if old == new {
            continue;
        }
        // A move of old/sub/f → new/sub/f can imply a move of old/ to new/.
        while old.file_name().is_some() && old.file_name() == new.file_name() {
            old = old.parent().unwrap_or(Path::new(""));
            new = new.parent().unwrap_or(Path::new(""));
        }
        for dir in [old, new]
            .into_iter()
            .filter(|dir| !dir.as_os_str().is_empty())
        {
            if let Some(path) = ignored
                .iter()
                .find(|path| path.starts_with(dir) || dir.starts_with(path))
            {
                bail!(
                    "Ignored path {:?} is in a directory renamed across these branches. Move it before integrating because Git may infer new filenames there.",
                    path
                );
            }
        }
    }
    Ok(())
}
