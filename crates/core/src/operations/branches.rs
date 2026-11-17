//! Explicit branch actions with confirmed source and current-worktree guards.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchStart {
    /// HEAD, a full refs/heads or refs/remotes ref, or the exact confirmed OID.
    pub reference: String,
    pub oid: String,
}

impl GitOperations {
    pub fn branch_starts(&self) -> Result<Vec<BranchStart>> {
        let mut starts = Vec::new();
        if let Some(oid) = self.branch_commit("HEAD")? {
            starts.push(BranchStart {
                reference: "HEAD".into(),
                oid,
            });
        }
        let bytes = self.metadata(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(objecttype)%00%(symref)",
            "--",
            "refs/heads/",
            "refs/remotes/",
        ])?;
        for line in std::str::from_utf8(&bytes)?.lines() {
            let fields: Vec<_> = line.split('\0').collect();
            ensure!(fields.len() == 4, "Incomplete branch metadata");
            if fields[2] != "commit" || !fields[3].is_empty() {
                continue;
            }
            crate::history::validate_oid(fields[1])?;
            starts.push(BranchStart {
                reference: fields[0].into(),
                oid: fields[1].into(),
            });
        }
        Ok(starts)
    }

    /// An empty expected_head represents an unborn HEAD. Explicit commit starts
    /// use their full OID in both BranchStart fields. Tracking requires a remote ref.
    pub fn create_branch_from(
        &self,
        name: &str,
        start: &BranchStart,
        checkout: bool,
        track: bool,
        expected_head: &str,
        expected_branch: Option<&str>,
    ) -> Result<OperationOutput> {
        self.validate_branch(name)?;
        self.require_new_branch(name)?;
        crate::history::validate_oid(&start.oid)?;
        if !expected_head.is_empty() {
            crate::history::validate_oid(expected_head)?;
        }
        let is_ref = start.reference == "HEAD"
            || start.reference.starts_with("refs/heads/")
            || start.reference.starts_with("refs/remotes/");
        ensure!(
            is_ref || start.reference == start.oid,
            "Choose HEAD, a full branch reference, or an exact commit OID"
        );
        if is_ref {
            ensure!(
                self.branch_starts()?.contains(start),
                "The selected starting branch changed; refresh and confirm it again"
            );
        } else {
            ensure!(
                self.branch_commit(&start.oid)?.as_deref() == Some(&start.oid),
                "The selected commit is unavailable"
            );
        }
        if track {
            ensure!(
                start.reference.starts_with("refs/remotes/"),
                "Only a remote-tracking branch can be tracked"
            );
            self.require_tracking_mapping(&start.reference)?;
        }
        if checkout {
            ensure!(
                self.in_progress()?.is_none(),
                "Finish or abort the current Git operation before switching"
            );
        }
        // Recheck immediately before the explicit action. Non-tracking creation
        // uses the confirmed immutable OID, never a short or ambiguous ref.
        ensure!(
            self.current_branch()?.as_deref() == expected_branch
                && self.branch_commit("HEAD")?.as_deref()
                    == (!expected_head.is_empty()).then_some(expected_head),
            "The checked-out branch or HEAD changed; refresh and confirm again"
        );
        ensure!(
            self.branch_commit(&start.reference)?.as_deref() == Some(&start.oid),
            "The selected starting commit changed; refresh and confirm again"
        );
        self.require_new_branch(name)?;
        let source = if track { &start.reference } else { &start.oid };
        let tracking = if track {
            "--track=direct"
        } else {
            "--no-track"
        };
        let result = if checkout {
            self.checked(&[
                "switch",
                "--no-guess",
                "--no-overwrite-ignore",
                "--no-recurse-submodules",
                tracking,
                "-c",
                name,
                "--",
                source,
            ])?
        } else {
            self.checked(&["branch", tracking, "--", name, source])?
        };
        Ok(branch_output(self, result))
    }

    pub fn rename_branch(
        &self,
        old: &str,
        new: &str,
        expected_oid: &str,
    ) -> Result<OperationOutput> {
        self.require_local_branch(old)?;
        self.validate_branch(new)?;
        crate::history::validate_oid(expected_oid)?;
        self.require_new_branch(new)?;
        let occupied = crate::git::parse_worktrees(&self.metadata(&[
            "worktree",
            "list",
            "--porcelain",
            "-z",
        ])?);
        ensure!(
            !occupied
                .iter()
                .any(|w| w.branch == old && w.path != self.root),
            "The branch is checked out in another worktree; rename it there"
        );
        ensure!(
            self.in_progress()?.is_none(),
            "Finish or abort the current Git operation before renaming"
        );
        ensure!(
            self.branch_commit(&format!("refs/heads/{old}"))?.as_deref() == Some(expected_oid),
            "The branch changed; refresh and confirm its commit again"
        );
        let result = self.checked(&["branch", "-m", "--", old, new])?;
        Ok(branch_output(self, result))
    }

    fn require_new_branch(&self, name: &str) -> Result<()> {
        self.require_unoccupied(name)?;
        let result = self.run(&[
            "show-ref",
            "--verify",
            "--quiet",
            "--",
            &format!("refs/heads/{name}"),
        ])?;
        ensure!(
            result.code == Some(1),
            "Branch {name:?} already exists or cannot be inspected"
        );
        Ok(())
    }

    // Reverse exact/single-wildcard fetch mappings, including slash-containing
    // branch names. Require one unambiguous remote and honor negative refspecs.
    fn require_tracking_mapping(&self, reference: &str) -> Result<()> {
        let mut candidates = BTreeSet::new();
        for remote in lines(&self.metadata(&["remote"])?)? {
            validate_remote_name(&remote)?;
            let key = format!("remote.{remote}.fetch");
            let config = self.run(&["config", "--null", "--get-all", "--", &key])?;
            ensure!(
                !config.truncated && matches!(config.code, Some(0 | 1)),
                "Cannot inspect remote fetch mappings"
            );
            let specs: Vec<_> = std::str::from_utf8(&config.stdout)?
                .split('\0')
                .filter(|s| !s.is_empty())
                .collect();
            for spec in &specs {
                let spec = spec.strip_prefix('+').unwrap_or(spec);
                let Some((source, destination)) = spec.split_once(':') else {
                    continue;
                };
                let Some(branch) = reverse_mapping(source, destination, reference) else {
                    continue;
                };
                if !branch.starts_with("refs/heads/") {
                    continue;
                }
                self.validate_branch(&branch["refs/heads/".len()..])?;
                if specs
                    .iter()
                    .filter_map(|s| s.strip_prefix('^'))
                    .any(|negative| ref_matches(negative, &branch))
                {
                    continue;
                }
                candidates.insert((remote.clone(), branch));
            }
        }
        ensure!(
            candidates.len() == 1,
            "The selected remote branch has no unique supported fetch mapping; configure its tracking with Git externally"
        );
        Ok(())
    }
}

fn reverse_mapping(source: &str, destination: &str, reference: &str) -> Option<String> {
    match (source.split_once('*'), destination.split_once('*')) {
        (None, None) => (destination == reference).then(|| source.to_owned()),
        (Some((before, after)), Some((prefix, suffix)))
            if !after.contains('*') && !suffix.contains('*') =>
        {
            let middle = reference.strip_prefix(prefix)?.strip_suffix(suffix)?;
            Some(format!("{before}{middle}{after}"))
        }
        _ => None,
    }
}

fn ref_matches(pattern: &str, reference: &str) -> bool {
    match pattern.split_once('*') {
        Some((before, after)) => reference
            .strip_prefix(before)
            .and_then(|s| s.strip_suffix(after))
            .is_some(),
        None => pattern == reference,
    }
}

fn branch_output(op: &GitOperations, result: Captured) -> OperationOutput {
    OperationOutput {
        root: op.root.clone(),
        stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
        truncated: result.truncated,
    }
}
