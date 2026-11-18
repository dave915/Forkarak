use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use crate::{
    ChangedFile, CompareMode, DiffDocument, FileStatus, Snapshot, SourceLocation,
    filesystem::{self, Node, feed},
    git::{self, Entry, Index, Tree},
    search_exclusions::SearchExclusions,
    text,
};

struct Capture {
    head: Option<String>,
    head_tree: Tree,
    index: Index,
    current: Tree,
    files: Vec<PathBuf>,
    fingerprint: String,
}

impl Capture {
    fn read(root: &Path, extra: &Tree) -> Result<Self> {
        let head = git::head(root)?;
        let head_tree = git::tree(root, head.as_deref())?;
        let index = git::index(root)?;
        let untracked = git::untracked(root)?;
        let sha256 =
            git::line(&git::words(root, &["rev-parse", "--show-object-format"])?)? == "sha256";
        let filemode = git::run(
            root,
            &[
                "config".as_ref(),
                "--bool".as_ref(),
                "--get".as_ref(),
                "core.filemode".as_ref(),
            ],
            256,
        )?;
        let filemode = !filemode.success || git::line(&filemode.bytes)? != "false";
        let listed = index
            .keys()
            .cloned()
            .chain(untracked)
            .collect::<BTreeSet<_>>();
        let paths = listed
            .iter()
            .chain(head_tree.keys())
            .chain(extra.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut current = Tree::new();
        let mut files = Vec::new();
        let mut fingerprint = Sha256::new();
        feed(&mut fingerprint, root.as_os_str().as_bytes());
        feed(&mut fingerprint, head.as_deref().unwrap_or("").as_bytes());
        for path in paths {
            let stages = index.get(&path);
            let indexed = stages.and_then(|s| s.get(&0));
            let submodule = indexed.filter(|e| e.mode == "160000");
            // Submodules are opaque; never recurse into a separately owned repo.
            let state = if let Some(entry) = submodule {
                Some(entry.clone())
            } else {
                filesystem::inspect(root, &path, sha256, filemode)?
            };
            feed(&mut fingerprint, path.as_os_str().as_bytes());
            if let Some(stages) = stages {
                for (stage, entry) in stages {
                    feed(&mut fingerprint, &[*stage]);
                    feed(&mut fingerprint, entry.mode.as_bytes());
                    feed(&mut fingerprint, entry.oid.as_bytes());
                }
            }
            if let Some(mut entry) = state {
                if !filemode
                    && entry.mode == "100644"
                    && let Some(original) = indexed
                        .or_else(|| head_tree.get(&path))
                        .filter(|e| e.mode == "100755")
                {
                    entry.mode.clone_from(&original.mode);
                }
                feed(&mut fingerprint, entry.mode.as_bytes());
                feed(&mut fingerprint, entry.oid.as_bytes());
                if listed.contains(&path) && submodule.is_none() {
                    files.push(path.clone());
                }
                current.insert(path, entry);
            } else {
                feed(&mut fingerprint, b"missing");
            }
        }
        Ok(Self {
            head,
            head_tree,
            index,
            current,
            files,
            fingerprint: format!("{:x}", fingerprint.finalize()),
        })
    }

    fn changes(&self, root: &Path, baseline: &Tree, uncommitted: bool) -> Result<Vec<ChangedFile>> {
        let paths = baseline
            .keys()
            .chain(self.current.keys())
            .chain(self.index.keys())
            .chain(self.head_tree.keys())
            .collect::<BTreeSet<_>>();
        let mut changes = Vec::new();
        for path in paths {
            let old = baseline.get(path);
            let new = self.current.get(path);
            let stages = self.index.get(path);
            let indexed = stages.and_then(|s| s.get(&0));
            let conflict = stages.is_some_and(|s| s.keys().any(|stage| *stage != 0));
            let staged = indexed != self.head_tree.get(path) || conflict;
            let unstaged = indexed != new || conflict;
            if old == new && !conflict && !(uncommitted && (staged || unstaged)) {
                continue;
            }
            let status = if conflict {
                FileStatus::Conflicted
            } else if old.is_none() && new.is_some() {
                if stages.is_none() && !self.head_tree.contains_key(path) {
                    FileStatus::Untracked
                } else {
                    FileStatus::Added
                }
            } else if new.is_none() {
                FileStatus::Deleted
            } else {
                FileStatus::Modified
            };
            changes.push(ChangedFile {
                path: path.clone(),
                status,
                staged,
                unstaged,
                ..ChangedFile::default()
            });
        }
        detect_renames(root, baseline, &self.index, &self.current, &mut changes)?;
        Ok(changes)
    }
}

/// Count working changes against HEAD without reading sibling worktrees or
/// generating line-by-line diffs.
pub(crate) fn worktree_change_count(root: &Path) -> Result<usize> {
    let capture = Capture::read(root, &Tree::new())?;
    Ok(capture.changes(root, &capture.head_tree, true)?.len())
}

/// Graph-only refresh checks include refs and worktree registration, but never
/// read sibling working files or generate per-file diff statistics.
pub(crate) fn worktree_change_summary(root: &Path) -> Result<(usize, String)> {
    let capture = Capture::read(root, &Tree::new())?;
    let count = capture.changes(root, &capture.head_tree, true)?.len();
    let mut fingerprint = Sha256::new();
    feed(&mut fingerprint, capture.fingerprint.as_bytes());
    let refs = git::words(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)",
            "--",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
            "refs/stash",
        ],
    )?;
    feed(&mut fingerprint, &refs);
    feed(
        &mut fingerprint,
        &git::words(root, &["worktree", "list", "--porcelain", "-z"])?,
    );
    Ok((count, format!("{:x}", fingerprint.finalize())))
}

/// Working change inventory for the independent commit dialog. No index refresh,
/// filters, worktree inventory traversal, or disk-backed staged preview.
pub(crate) fn commit_working_changes(root: &Path) -> Result<Vec<ChangedFile>> {
    let capture = Capture::read(root, &Tree::new())?;
    Ok(capture
        .changes(root, &capture.head_tree, true)?
        .into_iter()
        .filter(|file| file.unstaged)
        .collect())
}

pub(crate) fn snapshot(path: &Path, compare: &CompareMode) -> Result<Snapshot> {
    snapshot_inner(path, compare, SnapshotScope::AllWorktrees)
}

pub(crate) fn snapshot_current(path: &Path, compare: &CompareMode) -> Result<Snapshot> {
    snapshot_inner(path, compare, SnapshotScope::Current)
}

pub(crate) fn snapshot_current_inventory(path: &Path, compare: &CompareMode) -> Result<Snapshot> {
    snapshot_inner(path, compare, SnapshotScope::CurrentInventory)
}

#[derive(PartialEq)]
enum SnapshotScope {
    AllWorktrees,
    Current,
    CurrentInventory,
}

fn snapshot_inner(path: &Path, compare: &CompareMode, scope: SnapshotScope) -> Result<Snapshot> {
    let root = git::repository_root(path)?;
    let head = git::head(&root)?;
    let baseline = git::baseline(&root, compare, head.as_deref())?;
    let base_tree = git::tree(&root, baseline.as_deref())?;
    let capture = Capture::read(&root, &base_tree)?;
    ensure!(
        head == capture.head,
        "HEAD changed during refresh; refresh again"
    );
    let mut changes = capture.changes(
        &root,
        &base_tree,
        matches!(compare, CompareMode::Uncommitted),
    )?;
    if scope != SnapshotScope::CurrentInventory {
        let mut batch = git::BlobBatch::new(&root);
        for change in &mut changes {
            let (old, new) =
                comparison_bytes(&root, change, &base_tree, Some(&capture.index), |entry| {
                    batch.read(entry)
                })?;
            text::set_statistics(change, old, new);
        }
    }
    let mut worktrees = git::worktrees(&root)?;
    let mut fingerprint = Sha256::new();
    feed(&mut fingerprint, capture.fingerprint.as_bytes());
    feed(
        &mut fingerprint,
        baseline.as_deref().unwrap_or("").as_bytes(),
    );
    feed(&mut fingerprint, format!("{compare:?}").as_bytes());
    for worktree in &mut worktrees {
        if worktree.path == root {
            worktree.changed_files = if matches!(compare, CompareMode::Uncommitted) {
                changes.len()
            } else {
                capture.changes(&root, &capture.head_tree, true)?.len()
            };
            worktree.changed_files_available = true;
        } else if scope == SnapshotScope::AllWorktrees
            && !worktree.is_prunable
            && worktree.branch != "(bare)"
        {
            let inspected = (|| -> Result<(usize, String)> {
                let other = Capture::read(&worktree.path, &Tree::new())?;
                let count = other.changes(&worktree.path, &other.head_tree, true)?.len();
                Ok((count, other.fingerprint))
            })()
            .with_context(|| format!("Cannot inspect worktree {}", worktree.path.display()));
            match inspected {
                Ok((count, other_fingerprint)) => {
                    worktree.changed_files = count;
                    worktree.changed_files_available = true;
                    feed(&mut fingerprint, other_fingerprint.as_bytes());
                }
                Err(error) => worktree.inspection_error = Some(format!("{error:#}")),
            }
        }
        feed(&mut fingerprint, worktree.path.as_os_str().as_bytes());
        feed(&mut fingerprint, worktree.branch.as_bytes());
        feed(&mut fingerprint, worktree.head.as_bytes());
        feed(
            &mut fingerprint,
            &[
                worktree.is_main as u8,
                worktree.is_locked as u8,
                worktree.is_prunable as u8,
                worktree.is_detached as u8,
            ],
        );
        feed(&mut fingerprint, &worktree.changed_files.to_le_bytes());
        feed(&mut fingerprint, &[worktree.changed_files_available as u8]);
        if let Some(error) = &worktree.inspection_error {
            feed(&mut fingerprint, error.as_bytes());
        }
    }
    let worktree = worktrees
        .iter()
        .find(|w| w.path == root)
        .cloned()
        .context("Selected worktree disappeared; reopen the repository")?;
    let branches = git::branches(&root)?;
    for branch in &branches {
        feed(&mut fingerprint, branch.as_bytes());
    }
    Ok(Snapshot {
        worktree,
        worktrees,
        changes,
        files: capture.files,
        branches,
        compare: compare.clone(),
        fingerprint: format!("{:x}", fingerprint.finalize()),
    })
}

pub(crate) fn diff(root: &Path, file: &ChangedFile, compare: &CompareMode) -> Result<DiffDocument> {
    let root = git::repository_root(root)?;
    let mut file = file.clone();
    file.path = filesystem::relative(&root, &file.path)?;
    file.old_path = file
        .old_path
        .map(|path| filesystem::relative(&root, &path))
        .transpose()?;
    let head = git::head(&root)?;
    let baseline = git::baseline(&root, compare, head.as_deref())?;
    let baseline = git::tree(&root, baseline.as_deref())?;
    let (old, new) = comparison_bytes(&root, &file, &baseline, None, |entry| {
        git::blob(&root, entry)
    })?;
    let mut document = DiffDocument {
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        ..DiffDocument::default()
    };
    text::render(&mut document, old, new);
    Ok(document)
}

type PreviewBytes = (Vec<u8>, bool);

fn comparison_bytes(
    root: &Path,
    file: &ChangedFile,
    baseline: &Tree,
    index: Option<&Index>,
    mut read_blob: impl FnMut(&Entry) -> Result<PreviewBytes>,
) -> Result<(PreviewBytes, PreviewBytes)> {
    let old = baseline
        .get(file.old_path.as_ref().unwrap_or(&file.path))
        .map(&mut read_blob)
        .transpose()?;
    let new = match filesystem::open(root, &file.path)? {
        Node::Missing if old.is_some() => None,
        node @ (Node::Missing | Node::Directory) => {
            let loaded;
            let index = match index {
                Some(index) => index,
                None => {
                    loaded = git::index(root)?;
                    &loaded
                }
            };
            let stages = index.get(&file.path);
            ensure!(
                old.is_some() || stages.is_some(),
                "{} is absent from the comparison and working tree, or is beneath a symlink; refresh and select an accessible file",
                file.path.display()
            );
            // Only submodule directories have preview content in the index.
            stages
                .and_then(|s| s.get(&0))
                .filter(|e| matches!(node, Node::Directory) && e.mode == "160000")
                .map(read_blob)
                .transpose()?
        }
        node => filesystem::read_node(node)?,
    };
    Ok((old.unwrap_or_default(), new.unwrap_or_default()))
}

pub(crate) fn read_file(root: &Path, path: &Path) -> Result<String> {
    let root = git::repository_root(root)?;
    let path = filesystem::relative(&root, path)?;
    let (bytes, truncated) = filesystem::read(&root, &path)?.with_context(|| {
        format!(
            "{} does not exist, is a directory, or is beneath a symlink; refresh the file list",
            path.display()
        )
    })?;
    ensure!(
        !truncated,
        "{} exceeds the 2 MiB preview limit; open it in an external editor",
        path.display()
    );
    text::decode(bytes, false)
        .with_context(|| format!("{} is binary or is not valid UTF-8 text", path.display()))
}

pub(crate) fn search_with_exclusions(
    root: &Path,
    query: &str,
    limit: usize,
    exclude_patterns: &[String],
) -> Result<Vec<SourceLocation>> {
    let exclusions = SearchExclusions::compile(exclude_patterns)?;
    if query.is_empty() || limit == 0 || query.contains(['\n', '\r']) {
        return Ok(Vec::new());
    }
    let root = git::repository_root(root)?;
    let paths = git::index(&root)?
        .into_keys()
        .chain(git::untracked(&root)?)
        .collect::<BTreeSet<_>>();
    let mut hits = Vec::new();
    for path in paths {
        let path = filesystem::relative(&root, &path)?;
        if exclusions.excludes(&path) {
            continue;
        }
        if !matches!(filesystem::open(&root, &path)?, Node::File(_)) {
            continue;
        }
        let Some((bytes, truncated)) = filesystem::read(&root, &path)? else {
            continue;
        };
        if truncated {
            continue;
        }
        let Some(text) = text::decode(bytes, false) else {
            continue;
        };
        for (line_number, line) in text.lines().enumerate() {
            for (offset, _) in line.match_indices(query) {
                let start = line[..offset]
                    .char_indices()
                    .rev()
                    .nth(60)
                    .map_or(0, |(i, _)| i);
                hits.push(SourceLocation {
                    path: root.join(&path),
                    line: line_number as u32,
                    column: line[..offset].encode_utf16().count() as u32,
                    preview: line[start..].chars().take(240).collect(),
                });
                if hits.len() >= limit.min(10_000) {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

// Rename inference operates only on bounded contents. Exact moves (including
// moves subsequently edited in the working tree) are matched before similarity.
fn detect_renames(
    root: &Path,
    baseline: &Tree,
    index: &Index,
    current: &Tree,
    changes: &mut Vec<ChangedFile>,
) -> Result<()> {
    let deletions = changes
        .iter()
        .enumerate()
        .filter(|(_, f)| f.status == FileStatus::Deleted && baseline.contains_key(&f.path))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let additions = changes
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.status, FileStatus::Added | FileStatus::Untracked))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let mut matched = BTreeSet::new();
    let mut pairs = Vec::new();
    for &added in &additions {
        let destination = &changes[added].path;
        let new = current.get(destination);
        let staged = index.get(destination).and_then(|s| s.get(&0));
        if let Some(&deleted) = deletions.iter().find(|&&i| {
            !matched.contains(&i) && {
                let old = baseline.get(&changes[i].path);
                old == new || (staged.is_some() && old == staged)
            }
        }) {
            matched.insert(deleted);
            matched.insert(added);
            pairs.push((deleted, added));
        }
    }
    // Similarity detection has a fixed budget, independent of repository size.
    // Large/ambiguous sets remain explicit adds/deletes instead of guessing.
    if deletions.len() <= 16
        && additions.len() <= 16
        && deletions.iter().any(|i| !matched.contains(i))
        && additions.iter().any(|i| !matched.contains(i))
    {
        let mut old_profiles = Vec::new();
        let mut new_profiles = Vec::new();
        for &i in &deletions {
            if !matched.contains(&i) {
                let entry = &baseline[&changes[i].path];
                if let Some(profile) = profile(git::blob(root, entry)?) {
                    old_profiles.push((i, entry.mode.clone(), profile));
                }
            }
        }
        for &i in &additions {
            if !matched.contains(&i)
                && let Some(bytes) = filesystem::read(root, &changes[i].path)?
                && let Some(profile) = profile(bytes)
            {
                new_profiles.push((i, current[&changes[i].path].mode.clone(), profile));
            }
        }
        let mut scores = Vec::new();
        for (deleted, old_mode, old) in &old_profiles {
            for (added, new_mode, new) in &new_profiles {
                if old_mode != new_mode || old.total == 0 || new.total == 0 {
                    continue;
                }
                let common: usize = old
                    .lines
                    .iter()
                    .map(|(line, n)| n.min(new.lines.get(line).unwrap_or(&0)) * line.len())
                    .sum();
                let score = common * 100 / old.total.max(new.total);
                if score >= 50 {
                    scores.push((score, *deleted, *added));
                }
            }
        }
        scores.sort_by_key(|&(score, deleted, added)| (std::cmp::Reverse(score), deleted, added));
        for (_, deleted, added) in scores {
            if !matched.contains(&deleted) && !matched.contains(&added) {
                matched.insert(deleted);
                matched.insert(added);
                pairs.push((deleted, added));
            }
        }
    }
    let mut removed = BTreeSet::new();
    for (deleted, added) in pairs {
        changes[added].old_path = Some(changes[deleted].path.clone());
        changes[added].status = FileStatus::Renamed;
        changes[added].staged |= changes[deleted].staged;
        changes[added].unstaged |= changes[deleted].unstaged;
        removed.insert(deleted);
    }
    let mut i = 0;
    changes.retain(|_| {
        let retain = !removed.contains(&i);
        i += 1;
        retain
    });
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(())
}

struct Profile {
    total: usize,
    lines: BTreeMap<Vec<u8>, usize>,
}
fn profile((bytes, truncated): (Vec<u8>, bool)) -> Option<Profile> {
    if truncated || bytes.len() > 256 * 1024 || bytes.contains(&0) {
        return None;
    }
    let mut result = Profile {
        total: bytes.len(),
        lines: BTreeMap::new(),
    };
    for (i, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        if i >= 10_000 {
            return None;
        }
        *result.lines.entry(line.to_vec()).or_default() += 1;
    }
    Some(result)
}
