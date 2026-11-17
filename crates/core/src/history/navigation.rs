//! References, repository-wide search and bounded neighborhoods on a pinned graph.
use super::*;
use std::collections::HashSet;

const MATCH_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefGroup {
    Local,
    Remote,
    Tag,
    Stash,
}

#[derive(Clone, Debug)]
pub struct GraphRef {
    pub group: RefGroup,
    /// Full refname, or Git's stash reflog selector.
    pub name: String,
    pub label: String,
    pub oid: String,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub refs: Vec<GraphRef>,
    cursor: HistoryCursor,
}

#[derive(Debug, Default)]
pub struct SearchResults {
    pub commits: Vec<Commit>,
    pub truncated: bool,
}

pub(super) fn stashes(root: &Path) -> Result<Vec<GraphRef>> {
    let data = git::words(
        root,
        &[
            "stash",
            "list",
            "-z",
            "--no-color",
            "--format=%H%x00%gd%x00%gs",
        ],
    )?;
    if data.is_empty() {
        return Ok(vec![]);
    }
    let fields: Vec<_> = data
        .strip_suffix(&[0])
        .unwrap_or(&data)
        .split(|b| *b == 0)
        .collect();
    let (records, rest) = fields.as_chunks::<3>();
    ensure!(rest.is_empty(), "Malformed stash list");
    records
        .iter()
        .map(|[oid, name, subject]| {
            let oid = std::str::from_utf8(oid)?.to_owned();
            validate_oid(&oid)?;
            let name = String::from_utf8_lossy(name).into_owned();
            Ok(GraphRef {
                group: RefGroup::Stash,
                label: format!("{name}  {}", String::from_utf8_lossy(subject)),
                name,
                oid,
            })
        })
        .collect()
}

pub fn open(root: &Path, size: usize) -> Result<(HistoryPage, Catalog)> {
    let (cursor, stashes) = initial_cursor(root)?;
    let mut refs = Vec::new();
    for (oid, names) in &cursor.refs {
        for name in names {
            if let Some((group, label)) = [
                ("refs/heads/", RefGroup::Local),
                ("refs/remotes/", RefGroup::Remote),
                ("refs/tags/", RefGroup::Tag),
            ]
            .into_iter()
            .find_map(|(prefix, group)| name.strip_prefix(prefix).map(|s| (group, s)))
            {
                refs.push(GraphRef {
                    group,
                    name: name.clone(),
                    label: label.to_owned(),
                    oid: oid.clone(),
                });
            }
        }
    }
    refs.sort_by(|a, b| a.name.cmp(&b.name));
    refs.extend(stashes);
    Ok((
        next_page(&cursor.root, &cursor, size)?,
        Catalog { refs, cursor },
    ))
}

impl Catalog {
    /// The catalog is ordered by group, then refname (stash in reflog order).
    pub fn group(&self, group: RefGroup) -> &[GraphRef] {
        let start = self.refs.partition_point(|entry| entry.group < group);
        let end = self.refs.partition_point(|entry| entry.group <= group);
        &self.refs[start..end]
    }

    fn visit(&self, visit: impl FnMut(&str) -> bool) -> Result<()> {
        let mut args = vec!["rev-list", "--topo-order"];
        args.extend(self.cursor.tips.iter().map(String::as_str));
        args.push("--");
        git::visit_oids(&self.cursor.root, &args, visit)
    }

    pub fn search(&self, root: &Path, query: &str) -> Result<SearchResults> {
        ensure!(
            git::repository_root(root)? == self.cursor.root,
            "Search belongs to another worktree"
        );
        let query = query.trim();
        if query.is_empty() || self.cursor.tips.is_empty() {
            return Ok(SearchResults::default());
        }
        ensure!(
            query.len() <= 512 && !query.contains(['\0', '\n', '\r']),
            "Enter a single search term up to 512 bytes"
        );
        let mut commits = Vec::new();
        let max = format!("--max-count={}", MATCH_LIMIT + 1);
        if (4..=64).contains(&query.len()) && query.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut matches = Vec::new();
            let prefix = query.to_ascii_lowercase();
            self.visit(|oid| {
                if oid.starts_with(&prefix) {
                    matches.push(oid.to_owned());
                }
                matches.len() <= MATCH_LIMIT
            })?;
            if !matches.is_empty() {
                let mut args = vec![
                    "log",
                    "--no-walk=unsorted",
                    "--no-notes",
                    "--no-decorate",
                    "--no-show-signature",
                    "-z",
                    FORMAT,
                ];
                args.extend(matches.iter().map(String::as_str));
                args.push("--");
                commits = parse_commits(&git::words(&self.cursor.root, &args)?)?;
            }
        }
        // Separate searches give message OR author semantics. Git also searches
        // message bodies and email addresses without loading histories into the UI.
        for filter in [format!("--grep={query}"), format!("--author={query}")] {
            let mut args = vec![
                "log",
                "--topo-order",
                "--no-decorate",
                "--no-show-signature",
                "--no-notes",
                "--fixed-strings",
                "--regexp-ignore-case",
                "-z",
                FORMAT,
                &max,
                &filter,
            ];
            args.extend(self.cursor.tips.iter().map(String::as_str));
            args.push("--");
            commits.extend(parse_commits(&git::words(&self.cursor.root, &args)?)?);
        }
        let mut seen = HashSet::new();
        commits.retain(|c| seen.insert(c.oid.clone()));
        let truncated = commits.len() > MATCH_LIMIT;
        commits.truncate(MATCH_LIMIT);
        Ok(SearchResults { commits, truncated })
    }

    /// Find the target in the same topological order as ordinary graph pages.
    /// Start up to 24 rows earlier and retain standard forward pagination.
    pub fn around(&self, root: &Path, oid: &str) -> Result<(HistoryPage, usize)> {
        ensure!(
            git::repository_root(root)? == self.cursor.root,
            "Graph target belongs to another worktree"
        );
        validate_oid(oid)?;
        let mut index = 0usize;
        let mut found = false;
        self.visit(|candidate| {
            if candidate == oid {
                found = true;
            } else {
                index += 1;
            }
            !found
        })?;
        ensure!(
            found,
            "The commit is outside this graph. Refresh the worktree to load its current refs."
        );
        let mut cursor = self.cursor.clone();
        cursor.start = index.saturating_sub(24usize);
        let selected = index - cursor.start;
        let page = next_page(root, &cursor, 100)?;
        ensure!(
            page.entries
                .get(selected)
                .is_some_and(|entry| entry.commit.oid == oid),
            "Graph changed while locating commit"
        );
        Ok((page, selected))
    }
}
