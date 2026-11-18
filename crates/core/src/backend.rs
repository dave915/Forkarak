use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use crate::{
    ChangedFile, CompareMode, DiffDocument, FileStatus, SearchReport, Snapshot, SourceLocation,
    WorktreeChangeSummary, eol,
    filesystem::{self, Node, UnreadableFile, feed},
    git::{self, Entry, Index, Tree},
    search_exclusions::SearchExclusions,
    text,
};

struct Capture {
    head: Option<String>,
    head_tree: Tree,
    index: Index,
    current: Tree,
    normalized: BTreeSet<PathBuf>,
    files: Vec<PathBuf>,
    skipped_paths: BTreeSet<PathBuf>,
    fingerprint: String,
}

fn trusts_filemode(root: &Path) -> Result<bool> {
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
    Ok(!filemode.success || git::line(&filemode.bytes)? != "false")
}

pub(super) fn require_complete<'a>(
    mut paths: impl ExactSizeIterator<Item = &'a PathBuf>,
) -> Result<()> {
    let count = paths.len();
    if let Some(first) = paths.next() {
        bail!(
            "Incomplete worktree inspection: permission denied reading {} ({count} skipped)",
            first.display()
        );
    }
    Ok(())
}

fn record_denial(error: &anyhow::Error, skipped: &mut BTreeSet<PathBuf>) -> bool {
    error
        .downcast_ref::<UnreadableFile>()
        .is_some_and(|file| skipped.insert(file.0.clone()))
}

impl Capture {
    fn read(root: &Path, extra: &Tree) -> Result<Self> {
        let head = git::head(root)?;
        let head_tree = git::tree(root, head.as_deref())?;
        let index = git::index(root)?;
        let untracked = git::untracked(root)?;
        let sha256 =
            git::line(&git::words(root, &["rev-parse", "--show-object-format"])?)? == "sha256";
        let filemode = trusts_filemode(root)?;
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
        let mut crlf = BTreeMap::new();
        let mut files = Vec::new();
        let mut skipped_paths = BTreeSet::new();
        let mut fingerprint = Sha256::new();
        feed(&mut fingerprint, root.as_os_str().as_bytes());
        feed(&mut fingerprint, head.as_deref().unwrap_or("").as_bytes());
        for path in paths {
            let stages = index.get(&path);
            let indexed = stages.and_then(|s| s.get(&0));
            let submodule = indexed.filter(|e| e.mode == "160000");
            feed(&mut fingerprint, path.as_os_str().as_bytes());
            if let Some(stages) = stages {
                for (stage, entry) in stages {
                    feed(&mut fingerprint, &[*stage]);
                    feed(&mut fingerprint, entry.mode.as_bytes());
                    feed(&mut fingerprint, entry.oid.as_bytes());
                }
            }
            // Submodules are opaque; never recurse into a separately owned repo.
            let state = if let Some(entry) = submodule {
                Some((entry.clone(), None))
            } else {
                match filesystem::inspect(root, &path, sha256, filemode) {
                    Ok(state) => state,
                    Err(error) if record_denial(&error, &mut skipped_paths) => {
                        feed(&mut fingerprint, b"unreadable");
                        if listed.contains(&path) {
                            files.push(path);
                        }
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            };
            if let Some((mut entry, stats)) = state {
                if let Some(stats) = stats {
                    crlf.insert(path.clone(), stats);
                }
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
        let normalized = eol::normalized_paths(root, &crlf)?;
        for path in &normalized {
            let entry = current.get_mut(path).expect("inspected regular file");
            entry.oid = match filesystem::normalized_oid(
                root,
                path,
                sha256,
                crlf[path].normalized_len(),
                &entry.oid,
            ) {
                Ok(oid) => oid,
                Err(error) if record_denial(&error, &mut skipped_paths) => continue,
                Err(error) => return Err(error),
            };
            feed(&mut fingerprint, path.as_os_str().as_bytes());
            feed(&mut fingerprint, entry.oid.as_bytes());
        }
        Ok(Self {
            head,
            head_tree,
            index,
            current,
            normalized,
            files,
            skipped_paths,
            fingerprint: format!("{:x}", fingerprint.finalize()),
        })
    }

    /// Rename matching and statistics reopen working files. If access changes
    /// between passes, recompute rows without the denied path (including rename
    /// pairings and both comparison counts). Each retry excludes a new path.
    fn inspect_readable<T>(&mut self, inspect: impl Fn(&Self) -> Result<T>) -> Result<T> {
        loop {
            match inspect(self) {
                Err(error) if record_denial(&error, &mut self.skipped_paths) => continue,
                result => return result,
            }
        }
    }

    fn feed_fingerprint(&self, fingerprint: &mut Sha256) {
        feed(fingerprint, self.fingerprint.as_bytes());
        for path in &self.skipped_paths {
            feed(fingerprint, b"unreadable");
            feed(fingerprint, path.as_os_str().as_bytes());
        }
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
            if self.skipped_paths.contains(path) {
                continue;
            }
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
        detect_renames(
            root,
            baseline,
            &self.index,
            &self.current,
            &self.normalized,
            &mut changes,
        )?;
        Ok(changes)
    }
}

/// Count working changes against HEAD without reading sibling worktrees or
/// generating line-by-line diffs.
pub(crate) fn worktree_change_count(root: &Path) -> Result<usize> {
    let capture = Capture::read(root, &Tree::new())?;
    require_complete(capture.skipped_paths.iter())?;
    Ok(capture.changes(root, &capture.head_tree, true)?.len())
}

/// Graph-only refresh checks include refs and worktree registration, but never
/// read sibling working files or generate per-file diff statistics.
pub(crate) fn worktree_change_summary_report(root: &Path) -> Result<WorktreeChangeSummary> {
    let mut capture = Capture::read(root, &Tree::new())?;
    let count = capture.inspect_readable(|c| Ok(c.changes(root, &c.head_tree, true)?.len()))?;
    let mut fingerprint = Sha256::new();
    capture.feed_fingerprint(&mut fingerprint);
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
    Ok(WorktreeChangeSummary {
        changed_files: count,
        skipped_paths: capture.skipped_paths.into_iter().collect(),
        fingerprint: format!("{:x}", fingerprint.finalize()),
    })
}

/// Working change inventory for the independent commit dialog. No index refresh,
/// filters, worktree inventory traversal, or disk-backed staged preview.
pub(crate) fn commit_working_changes(root: &Path) -> Result<Vec<ChangedFile>> {
    let capture = Capture::read(root, &Tree::new())?;
    require_complete(capture.skipped_paths.iter())?;
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
    let mut capture = Capture::read(&root, &base_tree)?;
    ensure!(
        head == capture.head,
        "HEAD changed during refresh; refresh again"
    );
    let (changes, count) = capture.inspect_readable(|capture| {
        let uncommitted = matches!(compare, CompareMode::Uncommitted);
        let mut changes = capture.changes(&root, &base_tree, uncommitted)?;
        if scope != SnapshotScope::CurrentInventory {
            let mut batch = git::BlobBatch::new(&root);
            for change in &mut changes {
                let (old, new) = comparison_bytes(
                    &root,
                    change,
                    &base_tree,
                    Some(capture),
                    filesystem::open(&root, &change.path)?,
                    |entry| batch.read(entry),
                )?;
                text::set_statistics(change, old, new);
            }
        }
        let count = if uncommitted {
            changes.len()
        } else {
            capture.changes(&root, &capture.head_tree, true)?.len()
        };
        Ok((changes, count))
    })?;
    let mut worktrees = git::worktrees(&root)?;
    let mut fingerprint = Sha256::new();
    capture.feed_fingerprint(&mut fingerprint);
    feed(
        &mut fingerprint,
        baseline.as_deref().unwrap_or("").as_bytes(),
    );
    feed(&mut fingerprint, format!("{compare:?}").as_bytes());
    for worktree in &mut worktrees {
        if worktree.path == root {
            worktree.changed_files = count;
            worktree.changed_files_available = capture.skipped_paths.is_empty();
        } else if scope == SnapshotScope::AllWorktrees
            && !worktree.is_prunable
            && worktree.branch != "(bare)"
        {
            let inspected = (|| -> Result<(usize, String)> {
                let other = Capture::read(&worktree.path, &Tree::new())?;
                require_complete(other.skipped_paths.iter())?;
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
        skipped_paths: capture.skipped_paths.into_iter().collect(),
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
    let node = filesystem::open(&root, &file.path)?;
    let new_mode = working_mode(
        &node,
        &root,
        &file.path,
        &baseline,
        head.as_deref(),
        compare,
    )?;
    let (old, new) = comparison_bytes(&root, &file, &baseline, None, node, |entry| {
        git::blob(&root, entry)
    })?;
    let mut document = DiffDocument {
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        ..DiffDocument::default()
    };
    let old_mode = baseline
        .get(file.old_path.as_ref().unwrap_or(&file.path))
        .map(|e| e.mode.as_str());

    let same_bytes = !old.1 && !new.1 && old.0 == new.0;
    text::render(&mut document, old, new);
    if old_mode != new_mode.as_deref() {
        if same_bytes && document.binary {
            document.patch.clear();
        }
        text::prepend_mode_change(&mut document, old_mode, new_mode.as_deref());
    }
    Ok(document)
}

fn working_mode(
    node: &Node,
    root: &Path,
    path: &Path,
    baseline: &Tree,
    head: Option<&str>,
    compare: &CompareMode,
) -> Result<Option<String>> {
    match node {
        Node::Missing => Ok(None),
        Node::Link(_) => Ok(Some("120000".into())),
        Node::Directory => Ok(git::index(root)?
            .get(path)
            .and_then(|s| s.get(&0))
            .filter(|e| e.mode == "160000")
            .map(|e| e.mode.clone())),
        Node::File(file) => {
            let mode = if trusts_filemode(root)? {
                filesystem::regular_mode(&file.metadata()?)
            } else {
                let index = git::index(root)?;
                let head_tree;
                let tracked = if let Some(entry) = index.get(path).and_then(|s| s.get(&0)) {
                    Some(entry)
                } else if matches!(compare, CompareMode::Uncommitted) {
                    baseline.get(path)
                } else {
                    head_tree = git::tree(root, head)?;
                    head_tree.get(path)
                };
                if tracked.is_some_and(|entry| entry.mode == "100755") {
                    "100755"
                } else {
                    "100644"
                }
            };
            Ok(Some(mode.into()))
        }
    }
}

type PreviewBytes = (Vec<u8>, bool);

fn comparison_bytes(
    root: &Path,
    file: &ChangedFile,
    baseline: &Tree,
    capture: Option<&Capture>,
    node: Node,
    mut read_blob: impl FnMut(&Entry) -> Result<PreviewBytes>,
) -> Result<(PreviewBytes, PreviewBytes)> {
    let old = baseline
        .get(file.old_path.as_ref().unwrap_or(&file.path))
        .map(&mut read_blob)
        .transpose()?;
    let new = working_preview(
        root,
        &file.path,
        node,
        capture.map(|c| &c.index),
        capture.map(|c| c.normalized.contains(&file.path)),
    )?;
    if old.is_none() && new.is_none() {
        let indexed = match capture {
            Some(capture) => capture.index.contains_key(&file.path),
            None => git::index(root)?.contains_key(&file.path),
        };
        ensure!(
            indexed,
            "{} is absent from the comparison and working tree, or is beneath a symlink; refresh and select an accessible file",
            file.path.display()
        );
    }
    Ok((old.unwrap_or_default(), new.unwrap_or_default()))
}

fn working_preview(
    root: &Path,
    path: &Path,
    node: Node,
    index: Option<&Index>,
    normalized: Option<bool>,
) -> Result<Option<(Vec<u8>, bool)>> {
    match node {
        Node::Missing => Ok(None),
        Node::Link(target) => Ok(Some((target, false))),
        Node::Directory => {
            let loaded;
            let index = match index {
                Some(index) => index,
                None => {
                    loaded = git::index(root)?;
                    &loaded
                }
            };
            index
                .get(path)
                .and_then(|stages| stages.get(&0))
                .filter(|entry| entry.mode == "160000")
                .map(|entry| git::blob(root, entry))
                .transpose()
        }
        Node::File(file) => {
            let (mut bytes, truncated) = filesystem::preview(path, file)?;
            // A truncated side has no patch; keep its bounded disk preview.
            if !truncated && bytes.contains(&b'\r') {
                let convert = match normalized {
                    Some(convert) => convert,
                    None => eol::should_normalize(root, path, &bytes)?,
                };
                if convert {
                    let length = eol::normalize(&mut bytes);
                    bytes.truncate(length);
                }
            }
            Ok(Some((bytes, truncated)))
        }
    }
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

pub(crate) fn search_with_exclusions_report(
    root: &Path,
    query: &str,
    limit: usize,
    exclude_patterns: &[String],
) -> Result<SearchReport> {
    let exclusions = SearchExclusions::compile(exclude_patterns)?;
    if query.is_empty() || limit == 0 || query.contains(['\n', '\r']) {
        return Ok(SearchReport::default());
    }
    let root = git::repository_root(root)?;
    let paths = git::index(&root)?
        .into_keys()
        .chain(git::untracked(&root)?)
        .collect::<BTreeSet<_>>();
    let mut report = SearchReport::default();
    for path in paths {
        let path = filesystem::relative(&root, &path)?;
        if exclusions.excludes(&path) {
            continue;
        }
        let preview = filesystem::open(&root, &path).and_then(|node| match node {
            Node::File(file) => filesystem::preview(&path, file).map(Some),
            _ => Ok(None),
        });
        let (bytes, truncated) = match preview {
            Ok(Some(preview)) => preview,
            Ok(None) => continue,
            Err(error) if error.is::<UnreadableFile>() => {
                report.skipped_paths.push(path);
                continue;
            }
            Err(error) => return Err(error),
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
                report.matches.push(SourceLocation {
                    path: root.join(&path),
                    line: line_number as u32,
                    column: line[..offset].encode_utf16().count() as u32,
                    preview: line[start..].chars().take(240).collect(),
                });
                if report.matches.len() >= limit.min(10_000) {
                    return Ok(report);
                }
            }
        }
    }
    Ok(report)
}

// Rename inference operates only on bounded contents. Exact moves (including
// moves subsequently edited in the working tree) are matched before similarity.
fn detect_renames(
    root: &Path,
    baseline: &Tree,
    index: &Index,
    current: &Tree,
    normalized: &BTreeSet<PathBuf>,
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
                && let Some(bytes) = working_preview(
                    root,
                    &changes[i].path,
                    filesystem::open(root, &changes[i].path)?,
                    Some(index),
                    Some(normalized.contains(&changes[i].path)),
                )?
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
