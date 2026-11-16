//! Read-only Git and filesystem operations. No shell evaluation; all paths are argv.
use std::path::{Path, PathBuf};
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompareMode { #[default] Uncommitted, Branch(String) }

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
pub enum FileStatus { Added, Deleted, Renamed, Untracked, Conflicted, #[default] Modified }

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
pub enum DiffKind { Context, Addition, Deletion, Header }

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

pub fn repository_root(_path: &Path) -> Result<PathBuf> { anyhow::bail!("Git backend is not initialized") }
pub fn snapshot(_path: &Path, _compare: &CompareMode) -> Result<Snapshot> { anyhow::bail!("Git backend is not initialized") }
pub fn diff(_root: &Path, _file: &ChangedFile, _compare: &CompareMode) -> Result<DiffDocument> { anyhow::bail!("Git backend is not initialized") }
pub fn read_file(_root: &Path, _path: &Path) -> Result<String> { anyhow::bail!("Git backend is not initialized") }
pub fn search(_root: &Path, _query: &str, _limit: usize) -> Result<Vec<SourceLocation>> { anyhow::bail!("Search backend is not initialized") }

