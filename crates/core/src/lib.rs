//! Read-only Git and filesystem inspection, plus explicit writes in `operations`.
//! No shell evaluation; all paths are argv.
//!
//! Paths in snapshots are relative to the canonical worktree root. Diff line
//! numbers are one-based; search/navigation locations are zero-based UTF-16.
//! Working files are compared as stored on disk: repository clean/textconv
//! filters are deliberately never executed. Gitlinks are opaque index entries.
mod backend;
mod filesystem;
mod git;
pub mod history;
pub mod index;
pub mod operations;
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
    pub changed_files: usize,
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
    pub branches: Vec<String>,
    pub compare: CompareMode,
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

/// Find the canonical root of this worktree (including linked worktrees).
pub fn repository_root(path: &Path) -> Result<PathBuf> {
    git::repository_root(path)
}
/// Capture index, HEAD, working bytes, and worktree inventory without refreshing
/// the index. Inventory counts are uncommitted counts, independent of comparison.
pub fn snapshot(path: &Path, compare: &CompareMode) -> Result<Snapshot> {
    backend::snapshot(path, compare)
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
    backend::search(root, query, limit)
}

/// Maximum bytes retained from either side of a diff or a whole-file read.
pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
