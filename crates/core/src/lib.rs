//! Read-only Git and filesystem inspection, plus explicit writes in `operations`.
//! No shell evaluation; all paths are argv.
//!
//! Paths in snapshots are relative to the canonical worktree root. Diff line
//! numbers are one-based; search/navigation locations are zero-based UTF-16.
//! Comparisons apply Git's intrinsic text EOL rules to regular working files.
//! File reads return disk bytes. Gitlinks are opaque index entries.
mod backend;
pub mod comparison;
mod eol;
pub mod file_history;
mod filesystem;
mod git;
pub mod history;
pub mod index;
pub mod operations;
mod search_exclusions;
mod text;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompareMode {
    #[default]
    Uncommitted,
    Branch(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub head: String,
    pub is_main: bool,
    pub is_locked: bool,
    pub is_prunable: bool,
    pub is_detached: bool,
    /// Exact when `changed_files_available` is true. A partial inspection
    /// reports only the changes it could verify.
    pub changed_files: usize,
    /// False for partial, failed and inventory-only inspections.
    #[serde(default)]
    pub changed_files_available: bool,
    /// A sibling inspection failure never prevents viewing a healthy worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FileStatus {
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
    #[default]
    Modified,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub status: FileStatus,
    pub staged: bool,
    pub unstaged: bool,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub worktree: Worktree,
    pub worktrees: Vec<Worktree>,
    pub changes: Vec<ChangedFile>,
    pub files: Vec<PathBuf>,
    /// Selected-worktree-relative regular files omitted from change comparison
    /// because reading their contents was denied. Listed files remain in `files`.
    pub skipped_paths: Vec<PathBuf>,
    pub branches: Vec<String>,
    pub compare: CompareMode,
    pub fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct WorktreeChangeSummary {
    /// Number of verified changed files against HEAD.
    pub changed_files: usize,
    pub skipped_paths: Vec<PathBuf>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Addition,
    Deletion,
    Header,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct DiffDocument {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub old_text: String,
    pub new_text: String,
    pub patch: String,
    pub lines: Vec<DiffLine>,
    pub binary: bool,
    pub truncated: bool,
}

/// Zero-based line and UTF-16 column. Absolute path for navigation/search hits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub preview: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchReport {
    pub matches: Vec<SourceLocation>,
    /// Worktree-relative regular files denied before the match limit was reached.
    pub skipped_paths: Vec<PathBuf>,
}

/// Find the canonical root of this worktree (including linked worktrees).
pub fn repository_root(path: &Path) -> Result<PathBuf> {
    git::repository_root(path)
}
/// Canonical filesystem roots to watch for this worktree's files and Git state.
/// Includes the real Git and common directories for linked worktrees; discovery
/// never writes metadata or reads sibling working files.
pub fn repository_watch_paths(root: &Path) -> Result<Vec<PathBuf>> {
    git::repository_watch_paths(root)
}
/// List registered worktrees without reading their working files. Change counts
/// are not populated; use a snapshot when counts are needed.
pub fn worktree_inventory(root: &Path) -> Result<Vec<Worktree>> {
    git::worktrees(root)
}
/// Count changes against HEAD in the selected worktree, including staged changes.
/// Requires a complete inspection.
pub fn worktree_change_count(root: &Path) -> Result<usize> {
    backend::worktree_change_count(root)
}
/// Return the selected worktree's dirty count and a graph refresh fingerprint.
/// Ignored, untracked build output does not contribute to this fingerprint.
/// Requires a complete inspection. For partial refreshes use [`worktree_change_summary_report`].
pub fn worktree_change_summary(root: &Path) -> Result<(usize, String)> {
    let report = worktree_change_summary_report(root)?;
    backend::require_complete(report.skipped_paths.iter())?;
    Ok((report.changed_files, report.fingerprint))
}
/// Inspect changes against HEAD, returning the verified count, relative skipped
/// paths and a refresh fingerprint that incorporates those omissions.
pub fn worktree_change_summary_report(root: &Path) -> Result<WorktreeChangeSummary> {
    backend::worktree_change_summary_report(root)
}
/// Capture index, HEAD, working bytes, and worktree inventory without refreshing
/// the index. Inventory counts are uncommitted counts, independent of comparison.
pub fn snapshot(path: &Path, compare: &CompareMode) -> Result<Snapshot> {
    backend::snapshot(path, compare)
}
/// Capture the selected worktree's contents and change statistics, plus the registered
/// worktree inventory. Sibling dirty counts are unavailable, not assumed zero.
pub fn snapshot_current(path: &Path, compare: &CompareMode) -> Result<Snapshot> {
    backend::snapshot_current(path, compare)
}
/// List files, changed paths (including rename sources), status and staging flags
/// for the selected worktree. Shares the comparison, ordering, content fingerprint
/// and worktree counts of [`snapshot_current`]. Rename matching may read bounded
/// candidate contents. Ordinary listing does not load baseline blobs or compute patches.
///
/// The `additions`, `deletions` and `binary` fields on these [`ChangedFile`] rows
/// are unmeasured and retain their defaults. Use [`diff`] for the selected row's
/// preview or [`snapshot_current`] for measured statistics on every row.
pub fn snapshot_current_inventory(path: &Path, compare: &CompareMode) -> Result<Snapshot> {
    backend::snapshot_current_inventory(path, compare)
}
/// Compare against HEAD, or merge-base(base, HEAD), including working changes.
/// Large text is returned as a bounded preview with `truncated = true`; its
/// omitted diff has zero line counts. Binary contents are never decoded as text.
pub fn diff(root: &Path, file: &ChangedFile, compare: &CompareMode) -> Result<DiffDocument> {
    backend::diff(root, file, compare)
}
/// Read UTF-8 text, or a symlink's target spelling, without following symlinks.
/// Returns an actionable error for binary, oversized, or unsafe paths.
pub fn read_file(root: &Path, path: &Path) -> Result<String> {
    backend::read_file(root, path)
}
/// Literal, case-sensitive text search over tracked and nonignored untracked
/// files. Skips binary files, symlinks, and files above `MAX_TEXT_BYTES`.
/// At most 10,000 matches are returned; empty or multiline queries return none.
pub fn search(root: &Path, query: &str, limit: usize) -> Result<Vec<SourceLocation>> {
    search_with_exclusions(root, query, limit, &[])
}

/// Literal text search with ordered, case-sensitive gitignore-style exclusions.
/// Rules match normalized worktree-relative paths before files are opened or
/// hits count toward the limit. Basenames match at any depth; a slash anchors a
/// path at the worktree root, `**` spans directories, and a trailing `/` matches
/// directories and their descendants. The last matching rule wins; `!` can
/// reinclude a file even beneath an excluded directory. This does not reinclude
/// files omitted by Git's existing ignored-untracked-file discovery.
///
/// Blank rules and `#` comments are ignored; backslash escapes literal glob
/// characters and leading `!`/`#`. Invalid patterns (including unclosed classes,
/// NUL and embedded line breaks) return an error identifying the rule before
/// any repository inspection, even with an empty query or zero limit.
/// Otherwise, ordering, safety checks and limits are the same as [`search`].
pub fn search_with_exclusions(
    root: &Path,
    query: &str,
    limit: usize,
    exclude_patterns: &[String],
) -> Result<Vec<SourceLocation>> {
    Ok(search_with_exclusions_report(root, query, limit, exclude_patterns)?.matches)
}

/// Like [`search_with_exclusions`], also reporting regular files that could
/// not be read due to permissions. The scan stops when it reaches the hit limit.
pub fn search_with_exclusions_report(
    root: &Path,
    query: &str,
    limit: usize,
    exclude_patterns: &[String],
) -> Result<SearchReport> {
    backend::search_with_exclusions_report(root, query, limit, exclude_patterns)
}

/// Maximum bytes retained from either side of a diff or a whole-file read.
pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
