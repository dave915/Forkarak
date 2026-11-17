//! Exact immutable endpoint comparisons, deliberately distinct from the
//! merge-base semantics of CompareMode::Branch. No working file reads.
use crate::{ChangedFile, DiffDocument, FileStatus, git, history};
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeSet, path::{Path, PathBuf}};

#[derive(Clone, Debug)]
pub struct RefComparison {
    pub root: PathBuf,
    bound_root: PathBuf,
    pub left_oid: String,
    pub right_oid: String,
    pub files: Vec<ChangedFile>,
    pub(crate) left_tree: git::Tree,
    pub(crate) right_tree: git::Tree,
}

pub fn compare_refs(root: &Path, left: &str, right: &str) -> Result<RefComparison> {
    let root = git::repository_root(root)?;
    let left_oid = resolve(&root, left)?;
    let right_oid = resolve(&root, right)?;
    compare_oids(&root, Some(&left_oid), &right_oid)
}

pub(crate) fn resolve(root: &Path, reference: &str) -> Result<String> {
    ensure!(!reference.is_empty() && reference.len() <= 4096 && !reference.contains('\0'), "Select a commit, tag or branch");
    let expression = format!("{reference}^{{commit}}");
    let bytes = git::words(root, &["rev-parse", "--verify", "--end-of-options", &expression])
        .with_context(|| format!("Cannot resolve {reference:?}; fetch missing objects explicitly"))?;
    let oid = git::line(&bytes)?.to_owned();
    history::validate_oid(&oid)?;
    Ok(oid)
}

pub(crate) fn compare_oids(root: &Path, left: Option<&str>, right: &str) -> Result<RefComparison> {
    if let Some(oid) = left { history::validate_oid(oid)?; }
    history::validate_oid(right)?;
    let left_tree = git::tree(root, left)?;
    let right_tree = git::tree(root, Some(right))?;
    let files = left_tree.keys().chain(right_tree.keys()).collect::<BTreeSet<_>>()
        .into_iter().filter(|path| left_tree.get(*path) != right_tree.get(*path))
        .map(|path| ChangedFile {
            path: path.clone(),
            status: match (left_tree.get(path), right_tree.get(path)) {
                (None, _) => FileStatus::Added,
                (_, None) => FileStatus::Deleted,
                _ => FileStatus::Modified,
            },
            ..Default::default()
        }).collect();
    Ok(RefComparison { bound_root: root.into(), root: root.into(), left_oid: left.unwrap_or("").into(), right_oid: right.into(), files, left_tree, right_tree })
}

pub fn diff(root: &Path, comparison: &RefComparison, path: &Path) -> Result<DiffDocument> {
    ensure!(git::repository_root(root)? == comparison.bound_root && comparison.root == comparison.bound_root, "Comparison belongs to a different worktree");
    ensure!(comparison.files.iter().any(|f| f.path == path), "Select a changed path in this comparison");
    // Public display fields cannot redirect the immutable tree snapshots.
    history::diff_entries(&comparison.bound_root, path, None, comparison.left_tree.get(path), comparison.right_tree.get(path))
}
