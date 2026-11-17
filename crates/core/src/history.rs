//! Immutable commit inspection. Never checks out files, invokes filters, starts
//! analysis, refreshes the index, or fetches missing objects.
pub mod navigation;

use crate::{ChangedFile, DiffDocument, FileStatus, git, text};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const PAGE_LIMIT: usize = 200;
pub const HISTORY_LIMIT: usize = 5_000;
const FORMAT: &str = "--format=%H%x00%P%x00%an%x00%aI%x00%s";

#[derive(Debug, Clone)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    /// ISO 8601 author date, including its recorded timezone.
    pub date: String,
    pub subject: String,
    pub refs: Vec<String>,
}

/// Each edge connects a lane at this row's midpoint to a lane at its bottom.
/// `incoming` lanes continue from the previous row's bottom to this midpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub incoming: usize,
    /// Track identity is independent of its column as lanes shift at forks and joins.
    pub track: usize,
    pub incoming_tracks: Vec<usize>,
    pub outgoing_tracks: Vec<usize>,
    pub columns: usize,
    pub edges: Vec<(usize, usize)>,
}
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub commit: Commit,
    pub graph: GraphRow,
}
/// Bound to canonical worktree and immutable tips. Refs are a snapshot, not
/// newly re-resolved on each page; moving a branch cannot skip/duplicate rows.
#[derive(Debug, Clone)]
pub struct HistoryCursor {
    root: PathBuf,
    tips: Vec<String>,
    refs: BTreeMap<String, Vec<String>>,
    lanes: Vec<GraphLane>,
    next_track: usize,
    start: usize,
    offset: usize,
    shallow: bool,
}
#[derive(Debug, Clone)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub next: Option<HistoryCursor>,
    pub loaded: usize,
    pub limit_reached: bool,
    pub shallow: bool,
}
#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub commit: Commit,
    /// None for a root commit, otherwise the explicitly selected parent OID.
    pub parent: Option<String>,
    pub files: Vec<ChangedFile>,
    root: PathBuf,
    old_tree: git::Tree,
    new_tree: git::Tree,
}

pub fn first_page(root: &Path, size: usize) -> Result<HistoryPage> {
    Ok(navigation::open(root, size)?.0)
}

fn initial_cursor(root: &Path) -> Result<(HistoryCursor, Vec<navigation::GraphRef>)> {
    let root = git::repository_root(root)?;
    let mut refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let data = git::words(
        &root,
        &[
            "for-each-ref",
            "--format=%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(refname)",
            "--",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
        ],
    )?;
    for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let fields = line.split(|b| *b == 0).collect::<Vec<_>>();
        ensure!(fields.len() == 5, "Malformed history ref record");
        let oid = if fields[1] == b"commit" {
            fields[0]
        } else if fields[3] == b"commit" {
            fields[2]
        } else {
            continue;
        };
        let oid = std::str::from_utf8(oid)?.to_owned();
        validate_oid(&oid)?;
        refs.entry(oid)
            .or_default()
            .push(String::from_utf8_lossy(fields[4]).into_owned());
    }
    if let Some(head) = git::head(&root)? {
        refs.entry(head).or_default().push("HEAD".into());
    }
    let shallow = git::line(&git::words(
        &root,
        &["rev-parse", "--is-shallow-repository"],
    )?)? == "true";
    let stashes = navigation::stashes(&root)?;
    for stash in &stashes {
        refs.entry(stash.oid.clone())
            .or_default()
            .push(stash.name.clone());
    }
    let mut tips: BTreeSet<_> = refs.keys().cloned().collect();
    for tree in git::worktrees(&root)? {
        if tree.is_detached && !tree.head.is_empty() {
            tips.insert(tree.head);
        }
    }
    let cursor = HistoryCursor {
        root,
        tips: tips.into_iter().collect(),
        refs,
        lanes: vec![],
        next_track: 0,
        start: 0,
        offset: 0,
        shallow,
    };
    Ok((cursor, stashes))
}

pub fn next_page(root: &Path, cursor: &HistoryCursor, size: usize) -> Result<HistoryPage> {
    ensure!(
        git::repository_root(root)? == cursor.root,
        "History cursor belongs to a different worktree; reload history"
    );
    let mut cursor = cursor.clone();
    let count = size
        .clamp(1, PAGE_LIMIT)
        .min(HISTORY_LIMIT.saturating_sub(cursor.offset));
    let mut commits = if cursor.tips.is_empty() || count == 0 {
        vec![]
    } else {
        let skip = format!("--skip={}", cursor.start + cursor.offset);
        let max = format!("--max-count={}", count + 1);
        let mut args = vec![
            "log",
            "--topo-order",
            "--no-decorate",
            "--no-show-signature",
            "--no-notes",
            "-z",
            FORMAT,
            &skip,
            &max,
        ];
        args.extend(cursor.tips.iter().map(String::as_str));
        args.push("--");
        parse_commits(&git::words(&cursor.root, &args)?)?
    };
    let more = commits.len() > count;
    commits.truncate(count);
    let entries = commits
        .into_iter()
        .map(|mut commit| {
            commit.refs = cursor.refs.get(&commit.oid).cloned().unwrap_or_default();
            let graph = graph_row(&mut cursor.lanes, &mut cursor.next_track, &commit);
            HistoryEntry { commit, graph }
        })
        .collect::<Vec<_>>();
    cursor.offset += entries.len();
    let limit_reached = more && cursor.offset >= HISTORY_LIMIT;
    Ok(HistoryPage {
        entries,
        loaded: cursor.offset,
        shallow: cursor.shallow,
        limit_reached,
        next: (more && !limit_reached).then_some(cursor),
    })
}

pub(crate) fn parse_commits(data: &[u8]) -> Result<Vec<Commit>> {
    if data.is_empty() {
        return Ok(vec![]);
    }
    let fields = data
        .strip_suffix(&[0])
        .unwrap_or(data)
        .split(|b| *b == 0)
        .collect::<Vec<_>>();
    let (records, remainder) = fields.as_chunks::<5>();
    ensure!(
        remainder.is_empty(),
        "Malformed history metadata (possibly an embedded NUL in a commit)"
    );
    records
        .iter()
        .map(|f| {
            let oid = std::str::from_utf8(f[0])?.to_owned();
            validate_oid(&oid)?;
            let parents = std::str::from_utf8(f[1])?
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for parent in &parents {
                validate_oid(parent)?;
            }
            Ok(Commit {
                oid,
                parents,
                author: String::from_utf8_lossy(f[2]).into_owned(),
                date: String::from_utf8_lossy(f[3]).into_owned(),
                subject: String::from_utf8_lossy(f[4]).into_owned(),
                refs: vec![],
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
struct GraphLane {
    oid: String,
    track: usize,
}

fn graph_row(lanes: &mut Vec<GraphLane>, next_track: &mut usize, commit: &Commit) -> GraphRow {
    let incoming = lanes.len();
    let mut new_track = || {
        let track = *next_track;
        *next_track += 1;
        track
    };
    let lane = lanes
        .iter()
        .position(|l| l.oid == commit.oid)
        .unwrap_or_else(|| {
            lanes.push(GraphLane {
                oid: commit.oid.clone(),
                track: new_track(),
            });
            lanes.len() - 1
        });
    let before = lanes.clone();
    let track = lanes.remove(lane).track;
    let mut insertion = lane.min(lanes.len());
    for (index, parent) in commit.parents.iter().enumerate() {
        if let Some(existing) = lanes.iter_mut().find(|l| &l.oid == parent) {
            // Two first-parent tracks meet here. Keep the older track (the
            // trunk at a merge) regardless of which side topo-order showed first.
            if index == 0 {
                existing.track = existing.track.min(track);
            }
        } else {
            lanes.insert(
                insertion,
                GraphLane {
                    oid: parent.clone(),
                    track: if index == 0 { track } else { new_track() },
                },
            );
            insertion += 1;
        }
    }
    let mut edges = vec![];
    for (from, previous) in before.iter().enumerate() {
        if from != lane
            && let Some(to) = lanes.iter().position(|l| l.oid == previous.oid)
        {
            edges.push((from, to));
        }
    }
    for parent in &commit.parents {
        if let Some(to) = lanes.iter().position(|l| &l.oid == parent) {
            edges.push((lane, to));
        }
    }
    GraphRow {
        lane,
        incoming,
        track,
        incoming_tracks: before[..incoming].iter().map(|l| l.track).collect(),
        outgoing_tracks: lanes.iter().map(|l| l.track).collect(),
        columns: before.len().max(lanes.len()),
        edges,
    }
}

pub(crate) fn validate_oid(oid: &str) -> Result<()> {
    ensure!(
        matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()),
        "Select a full, immutable commit hash"
    );
    Ok(())
}

/// `parent_index` is zero-based. Root commits use zero and compare to empty.
/// Merge callers must expose the parent choice instead of implying a merge diff.
pub fn commit_detail(root: &Path, oid: &str, parent_index: usize) -> Result<CommitDetail> {
    validate_oid(oid)?;
    let root = git::repository_root(root)?;
    let data = git::words(
        &root,
        &[
            "log",
            "-1",
            "--no-decorate",
            "--no-show-signature",
            "--no-notes",
            "-z",
            FORMAT,
            oid,
            "--",
        ],
    )?;
    let commit = parse_commits(&data)?
        .pop()
        .context("Commit is missing; fetch missing objects explicitly")?;
    ensure!(
        commit.oid.eq_ignore_ascii_case(oid),
        "Object is not the selected commit"
    );
    ensure!(
        parent_index < commit.parents.len().max(1),
        "Select an existing parent"
    );
    let parent = commit.parents.get(parent_index).cloned();
    let old_tree = git::tree(&root, parent.as_deref())?;
    let new_tree = git::tree(&root, Some(oid))?;
    let mut files = vec![];
    for path in old_tree
        .keys()
        .chain(new_tree.keys())
        .collect::<BTreeSet<_>>()
    {
        let old = old_tree.get(path);
        let new = new_tree.get(path);
        if old == new {
            continue;
        }
        files.push(ChangedFile {
            path: path.clone(),
            status: match (old, new) {
                (None, _) => FileStatus::Added,
                (_, None) => FileStatus::Deleted,
                _ => FileStatus::Modified,
            },
            ..Default::default()
        });
    }
    Ok(CommitDetail {
        commit,
        parent,
        files,
        root,
        old_tree,
        new_tree,
    })
}

/// Paths are literal tree keys, including non-UTF8, tabs, newlines and ':('.
/// Never reads corresponding working files. Rename detection is intentionally
/// absent: a renamed path is represented by its deletion and addition.
pub fn historical_diff(root: &Path, detail: &CommitDetail, path: &Path) -> Result<DiffDocument> {
    ensure!(
        git::repository_root(root)? == detail.root,
        "Historical detail belongs to a different worktree"
    );
    ensure!(
        detail.files.iter().any(|f| f.path == path),
        "Select a changed path in this commit"
    );
    diff_entries(
        root,
        path,
        None,
        detail.old_tree.get(path),
        detail.new_tree.get(path),
    )
}

pub(crate) fn diff_entries(
    root: &Path,
    path: &Path,
    path_change: Option<(&Path, &str)>,
    old_entry: Option<&git::Entry>,
    new_entry: Option<&git::Entry>,
) -> Result<DiffDocument> {
    let bytes = |entry: Option<&git::Entry>| -> Result<_> {
        entry
            .map(|e| git::blob(root, e))
            .unwrap_or_else(|| Ok((vec![], false)))
    };
    let mut document = DiffDocument {
        path: path.into(),
        old_path: path_change.map(|(from, _)| from.to_owned()),
        ..Default::default()
    };
    text::render(&mut document, bytes(old_entry)?, bytes(new_entry)?);
    let same_blob = old_entry
        .zip(new_entry)
        .is_some_and(|(a, b)| a.oid == b.oid);
    if same_blob {
        // Report metadata alone even for binary or truncated contents.
        document.patch.clear();
        document.lines.clear();
    }
    if let Some((from, action)) = path_change {
        document.patch = format!(
            "{action} from {from:?}\n{action} to {path:?}\n{}",
            document.patch
        );
        document.lines = text::parse_patch(&document.patch);
    }
    let old_mode = old_entry.map(|entry| entry.mode.as_str());
    let new_mode = new_entry.map(|entry| entry.mode.as_str());
    if old_mode != new_mode
        && (document.patch.is_empty() || old_mode.is_some() && new_mode.is_some())
    {
        document.patch = format!(
            "File mode: {} → {}\n{}",
            old_mode.unwrap_or("absent"),
            new_mode.unwrap_or("absent"),
            document.patch
        );
        document.lines = text::parse_patch(&document.patch);
    }
    Ok(document)
}

#[cfg(test)]
mod tests;
