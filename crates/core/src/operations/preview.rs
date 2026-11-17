//! Local-only, immutable previews retained through explicit execution. No fetch,
//! push --dry-run, hooks, filters or working-file writes occur while previewing.
use super::{
    integration::{BranchIntegration, IntegrationKind},
    *,
};
use crate::{
    ChangedFile,
    comparison::{self, RefComparison},
    history::Commit,
};

#[derive(Clone, Debug)]
pub struct OperationPreview {
    pub root: PathBuf,
    pub base_oid: Option<String>,
    pub head_oid: String,
    pub commits: Vec<Commit>,
    pub files: Vec<ChangedFile>,
    pub commits_truncated: bool,
    /// True only when a unique locally known tracking tip is available. This
    /// never means that the server was contacted or is currently up to date.
    pub remote_state_known: bool,
    pub baseline_description: String,
    comparison: RefComparison,
    binding: Binding,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct PushBinding {
    root: PathBuf,
    remote: String,
    local: String,
    destination: String,
    head: String,
    base: Option<String>,
    signature: Vec<u8>,
}
#[derive(Clone, Debug)]
enum Binding {
    Push(PushBinding),
    Integration(IntegrationKind, BranchIntegration),
}

impl OperationPreview {
    pub fn diff(&self, root: &Path, path: &Path) -> Result<crate::DiffDocument> {
        comparison::diff(root, &self.comparison, path)
    }
}
impl GitOperations {
    pub fn preview_push(
        &self,
        remote: &str,
        local_branch: &str,
        remote_branch: &str,
    ) -> Result<OperationPreview> {
        let binding = self.push_binding(remote, local_branch, remote_branch)?;
        let comparison =
            comparison::compare_oids(&self.root, binding.base.as_deref(), &binding.head)?;
        let (commits, commits_truncated) =
            self.preview_commits(binding.base.as_deref(), &binding.head)?;
        let known = binding.base.is_some();
        Ok(OperationPreview {
            root: self.root.clone(), base_oid: binding.base.clone(), head_oid: binding.head.clone(),
            files: comparison.files.clone(), commits, commits_truncated, remote_state_known: known,
            baseline_description: if known { "Locally known remote-tracking tip; fetch explicitly for newer server state" } else { "Remote tip unknown locally; showing all reachable source commits and source files, not an exact server delta" }.into(),
            comparison, binding: Binding::Push(binding),
        })
    }
    pub fn execute_push_preview(
        &self,
        preview: &OperationPreview,
        set_upstream: bool,
    ) -> Result<OperationOutput> {
        let Binding::Push(expected) = &preview.binding else {
            bail!("Select a Push preview");
        };
        ensure!(
            self.push_binding(&expected.remote, &expected.local, &expected.destination)?
                == *expected,
            "Push source, remote configuration or locally known destination changed; preview and confirm again"
        );
        // Push the retained OID so a concurrent branch movement cannot change
        // the reviewed source after validation. Never add a force/lease option.
        let mirror = format!("remote.{}.mirror=false", expected.remote);
        let spec = format!("{}:refs/heads/{}", expected.head, expected.destination);
        let result = self.checked(&[
            "-c",
            &mirror,
            "-c",
            "push.autoSetupRemote=false",
            "push",
            "--no-force",
            "--no-follow-tags",
            "--no-prune",
            "--recurse-submodules=no",
            "--",
            &expected.remote,
            &spec,
        ])?;
        if set_upstream {
            ensure!(
                self.branch_commit(&format!("refs/heads/{}", expected.local))?
                    .as_deref()
                    == Some(&expected.head),
                "Push succeeded, but the local branch changed. Tracking was not updated; inspect before retrying"
            );
            let remote_key = format!("branch.{}.remote", expected.local);
            let merge_key = format!("branch.{}.merge", expected.local);
            let remote_ref = format!("refs/heads/{}", expected.destination);
            self.checked(&["config", "--local", "--replace-all", "--", &remote_key, &expected.remote]).context("Push succeeded, but saving the upstream remote failed; inspect tracking configuration")?;
            self.checked(&["config", "--local", "--replace-all", "--", &merge_key, &remote_ref]).context("Push succeeded and upstream remote was saved, but upstream branch save failed; inspect tracking configuration")?;
        }
        self.workflow_output(result)
    }
    pub fn preview_integration(
        &self,
        kind: IntegrationKind,
        expected: &BranchIntegration,
    ) -> Result<OperationPreview> {
        ensure!(
            expected.current.root == self.root && self.integration_choices()?.contains(expected),
            "Integration target changed; select it again"
        );
        let (base, head) = match kind {
            IntegrationKind::Merge => (expected.current.head.as_str(), expected.oid.as_str()),
            // Rebase previews the local commits to replay, and the endpoint
            // file delta from the destination to this branch; conflicts may
            // change the final result, which is not invented here.
            IntegrationKind::Rebase => (expected.oid.as_str(), expected.current.head.as_str()),
        };
        let comparison = comparison::compare_oids(&self.root, Some(base), head)?;
        let (commits, commits_truncated) = self.preview_commits(Some(base), head)?;
        Ok(OperationPreview {
            root: self.root.clone(), base_oid: Some(base.into()), head_oid: head.into(),
            commits, commits_truncated, files: comparison.files.clone(), remote_state_known: false,
            baseline_description: match kind {
                IntegrationKind::Merge => "Incoming commits and exact current-to-source file delta; the merged result may require conflict resolution",
                IntegrationKind::Rebase => "Local commits to replay and exact destination-to-current file delta; the rebased result may require conflict resolution",
            }.into(), comparison, binding: Binding::Integration(kind, expected.clone()),
        })
    }
    pub fn execute_integration_preview(
        &self,
        preview: &OperationPreview,
    ) -> Result<OperationOutput> {
        let Binding::Integration(kind, expected) = &preview.binding else {
            bail!("Select a Merge/Rebase preview");
        };
        self.integrate_branch(*kind, expected)
    }
    fn preview_commits(&self, base: Option<&str>, head: &str) -> Result<(Vec<Commit>, bool)> {
        let range = base
            .map(|base| format!("{base}..{head}"))
            .unwrap_or_else(|| head.into());
        let bytes = self.metadata(&[
            "log",
            "--max-count=201",
            "--no-decorate",
            "--no-show-signature",
            "--no-notes",
            "-z",
            "--format=%H%x00%P%x00%an%x00%aI%x00%s",
            &range,
            "--",
        ])?;
        let mut commits = crate::history::parse_commits(&bytes)?;
        let truncated = commits.len() > 200;
        commits.truncate(200);
        Ok((commits, truncated))
    }
    fn push_binding(&self, remote: &str, local: &str, destination: &str) -> Result<PushBinding> {
        self.require_local_branch(local)?;
        self.validate_branch(destination)?;
        self.require_remote(remote, true)?;
        let selected = self
            .remotes()?
            .into_iter()
            .find(|r| r.name == remote)
            .context("Remote disappeared")?;
        let key = format!("remote.{remote}.fetch");
        let fetch = self.run(&["config", "--null", "--get-all", "--", &key])?;
        ensure!(
            matches!(fetch.code, Some(0 | 1)) && !fetch.truncated,
            "Cannot inspect remote fetch mappings"
        );
        let mut signature = format!("{:?}", selected).into_bytes();
        signature.extend_from_slice(&fetch.stdout);
        let source = format!("refs/heads/{destination}");
        let specs: Vec<_> = std::str::from_utf8(&fetch.stdout)?
            .split('\0')
            .filter(|s| !s.is_empty())
            .collect();
        let mut tracking = std::collections::BTreeSet::new();
        let excluded = specs
            .iter()
            .filter_map(|s| s.strip_prefix('^'))
            .any(|s| map_ref(s, s, &source).is_some());
        if !excluded && selected.fetch_urls == selected.push_urls {
            for spec in specs {
                if let Some((from, to)) = spec.trim_start_matches('+').split_once(':')
                    && let Some(reference) = map_ref(from, to, &source)
                {
                    tracking.insert(reference);
                }
            }
        }
        let base = if tracking.len() == 1 {
            self.branch_commit(tracking.first().unwrap())?
        } else {
            None
        };
        let head = self
            .branch_commit(&format!("refs/heads/{local}"))?
            .context("Source branch has no commit")?;
        Ok(PushBinding {
            root: self.root.clone(),
            remote: remote.into(),
            local: local.into(),
            destination: destination.into(),
            head,
            base,
            signature,
        })
    }
}
fn map_ref(from: &str, to: &str, reference: &str) -> Option<String> {
    if let Some((prefix, suffix)) = from.split_once('*') {
        let middle = reference.strip_prefix(prefix)?.strip_suffix(suffix)?;
        let (before, after) = to.split_once('*')?;
        Some(format!("{before}{middle}{after}"))
    } else {
        (from == reference).then(|| to.into())
    }
}
