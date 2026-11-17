//! Local synchronization metadata and explicitly selected push destinations.
use super::*;
use std::{fs, time::SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// Full local tracking reference, including custom fetch mappings.
    pub reference: String,
    pub remote: String,
    /// Full source reference on the remote (usually refs/heads/...).
    pub branch: String,
    pub missing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchSync {
    pub branch: Option<String>,
    pub upstream: Option<Upstream>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    /// This worktree's FETCH_HEAD modification time, not another worktree's.
    pub last_fetch: Option<SystemTime>,
}

impl GitOperations {
    /// Reads only local refs/objects and FETCH_HEAD; never fetches or runs filters.
    pub fn branch_sync(&self) -> Result<BranchSync> {
        let fetch_path = self.metadata(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "FETCH_HEAD",
        ])?;
        let fetch_path = crate::git::path(fetch_path.strip_suffix(b"\n").unwrap_or(&fetch_path));
        let last_fetch = match fs::metadata(fetch_path) {
            Ok(metadata) => Some(metadata.modified()?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("Cannot read this worktree's fetch time"),
        };
        let mut sync = BranchSync {
            branch: self.current_branch()?,
            upstream: None,
            ahead: None,
            behind: None,
            last_fetch,
        };
        let Some(branch) = &sync.branch else {
            return Ok(sync);
        };
        let reference = format!("refs/heads/{branch}");
        let bytes = self.metadata(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)",
            "--", &reference,
        ])?;
        for line in std::str::from_utf8(&bytes)?.lines() {
            let fields: Vec<_> = line.split('\0').collect();
            ensure!(fields.len() == 5, "Incomplete upstream metadata");
            // for-each-ref patterns can include descendants; only accept this ref.
            if fields[0] != reference || fields[2].is_empty() {
                continue;
            }
            let oid = self.branch_commit(fields[2])?;
            sync.upstream = Some(Upstream {
                reference: fields[2].into(),
                remote: fields[3].into(),
                branch: fields[4].into(),
                missing: oid.is_none(),
            });
            if let Some(oid) = oid {
                crate::history::validate_oid(fields[1])?;
                let range = format!("{}...{oid}", fields[1]);
                let counts = self.run(&["rev-list", "--left-right", "--count", &range, "--"])?;
                if counts.code == Some(0) && !counts.truncated {
                    let values: Vec<_> = std::str::from_utf8(&counts.stdout)?
                        .split_whitespace()
                        .collect();
                    ensure!(values.len() == 2, "Invalid ahead/behind counts");
                    sync.ahead = Some(values[0].parse()?);
                    sync.behind = Some(values[1].parse()?);
                }
            }
            break;
        }
        Ok(sync)
    }

    /// Push one local branch to one explicitly named remote branch, without force.
    pub fn push_branch(
        &self,
        remote: &str,
        local_branch: &str,
        remote_branch: &str,
        set_upstream: bool,
    ) -> Result<OperationOutput> {
        self.require_local_branch(local_branch)?;
        self.validate_branch(remote_branch)?;
        self.require_remote(remote, true)?;
        let mirror = format!("remote.{remote}.mirror=false");
        let reference = format!("refs/heads/{local_branch}:refs/heads/{remote_branch}");
        let mut args = vec![
            "-c",
            &mirror,
            "-c",
            "push.autoSetupRemote=false",
            "push",
            "--progress",
            "--no-force",
            "--no-follow-tags",
            "--no-prune",
            "--recurse-submodules=no",
        ];
        if set_upstream {
            args.push("--set-upstream");
        }
        args.extend(["--", remote, &reference]);
        let result = self.checked(&args)?;
        Ok(OperationOutput {
            root: self.root.clone(),
            stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            truncated: result.truncated,
        })
    }

    // Resolve a full, validated reference or OID. Missing/unborn refs are not zero.
    pub(super) fn branch_commit(&self, reference: &str) -> Result<Option<String>> {
        let commit = format!("{reference}^{{commit}}");
        let result = self.run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &commit,
        ])?;
        match result.code {
            Some(0) => {
                ensure!(
                    !result.truncated,
                    "Commit metadata exceeds the safety limit"
                );
                let oid = crate::git::line(&result.stdout)?.to_owned();
                crate::history::validate_oid(&oid)?;
                Ok(Some(oid))
            }
            Some(1) => Ok(None),
            _ => bail!(
                "Cannot resolve the selected commit: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
        }
    }
}
