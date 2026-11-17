//! Read reflog history and perform explicit non-destructive recovery actions.
use super::{history_actions::HistoryTarget, *};

#[derive(Clone, Debug)]
pub struct ReflogEntry {
    pub root: PathBuf,
    pub oid: String,
    pub selector: String,
    pub subject: String,
    /// Reflog entry date, rather than the referenced commit's author date.
    pub timestamp: String,
    bound_root: PathBuf,
    bound_oid: String,
}
#[derive(Clone, Debug)]
pub struct UndoLastCommit {
    pub current: HistoryTarget,
    pub parent_oid: String,
    bound_current: HistoryTarget,
    bound_parent: String,
}
impl GitOperations {
    /// This worktree's HEAD reflog. Code/diffs can be inspected read-only with
    /// history::commit_detail and history::historical_diff using entry.oid.
    pub fn reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>> {
        ensure!(
            (1..=1000).contains(&limit),
            "Choose a reflog limit between 1 and 1000"
        );
        let count = format!("--max-count={limit}");
        let bytes = self.metadata(&[
            "log",
            "-g",
            &count,
            "--date=iso-strict",
            "--no-show-signature",
            "--no-decorate",
            "--no-notes",
            "-z",
            "--format=%H%x00%gD%x00%gs",
            "HEAD",
            "--",
        ])?;
        let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
        let mut entries = vec![];
        for fields in fields.chunks(3) {
            if fields.len() == 1 && fields[0].is_empty() {
                break;
            }
            ensure!(fields.len() == 3, "Incomplete reflog entry");
            let oid = std::str::from_utf8(fields[0])?.to_owned();
            crate::history::validate_oid(&oid)?;
            let selector = std::str::from_utf8(fields[1])?.to_owned();
            let timestamp = selector
                .split_once("@{")
                .and_then(|(_, s)| s.strip_suffix('}'))
                .unwrap_or("")
                .to_owned();
            entries.push(ReflogEntry {
                root: self.root.clone(),
                bound_root: self.root.clone(),
                bound_oid: oid.clone(),
                oid,
                selector,
                timestamp,
                subject: String::from_utf8_lossy(fields[2]).into_owned(),
            });
        }
        Ok(entries)
    }
    /// Create without checkout, overwrite, staging, or changes to HEAD/index/files.
    pub fn create_recovery_branch(
        &self,
        name: &str,
        entry: &ReflogEntry,
    ) -> Result<OperationOutput> {
        ensure!(
            entry.bound_root == self.root
                && entry.root == entry.bound_root
                && entry.oid == entry.bound_oid,
            "Reflog entry belongs to another worktree or changed"
        );
        self.validate_branch(name)?;
        crate::history::validate_oid(&entry.bound_oid)?;
        ensure!(
            self.branch_commit(&entry.bound_oid)?.as_deref() == Some(&entry.bound_oid),
            "Recovery commit is unavailable; it may have expired"
        );
        self.workflow_output(self.checked(&[
            "branch",
            "--no-track",
            "--",
            name,
            &entry.bound_oid,
        ])?)
    }
    pub fn undo_last_commit_target(&self) -> Result<UndoLastCommit> {
        ensure!(
            self.in_progress()?.is_none(),
            "Finish or abort the current Git operation before undoing a commit"
        );
        let current = self.history_target()?;
        let bytes = self.metadata(&["rev-list", "--parents", "-n", "1", &current.head, "--"])?;
        let parent = std::str::from_utf8(&bytes)?
            .split_whitespace()
            .nth(1)
            .context("The first commit cannot be undone with this action")?
            .to_owned();
        crate::history::validate_oid(&parent)?;
        Ok(UndoLastCommit {
            bound_current: current.clone(),
            current,
            parent_oid: parent.clone(),
            bound_parent: parent,
        })
    }
    /// Move the confirmed current branch to its first parent, atomically comparing
    /// the old OID. Index and working bytes remain exactly as they are (soft undo),
    /// including pre-existing staged and unstaged edits. Detached/unborn/root HEAD
    /// and ongoing integration operations are rejected. Reflog retains recovery.
    pub fn undo_last_commit(&self, expected: &UndoLastCommit) -> Result<OperationOutput> {
        ensure!(
            expected.current == expected.bound_current
                && expected.parent_oid == expected.bound_parent,
            "Undo confirmation changed"
        );
        let current = self.undo_last_commit_target()?;
        ensure!(
            current.bound_current == expected.bound_current
                && current.bound_parent == expected.bound_parent,
            "Branch, HEAD or operation state changed; confirm undo again"
        );
        let reference = format!("refs/heads/{}", expected.bound_current.branch);
        self.workflow_output(self.checked(&[
            "update-ref",
            "--no-deref",
            "-m",
            "Forkarak: undo last commit (keep index and worktree)",
            &reference,
            &expected.bound_parent,
            &expected.bound_current.head,
        ])?)
    }
}
