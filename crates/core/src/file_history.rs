//! File history and line attribution pinned to one revision and literal path.
use crate::{filesystem, git, history::validate_oid, text};
use anyhow::{Context, Result, ensure};
use similar::{ChangeTag, TextDiff};
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Duration,
};

const HISTORY_LIMIT: usize = 500;
const BLAME_LIMIT: usize = 50_000;

#[derive(Debug, Clone)]
pub struct FileRevision {
    pub oid: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    /// Path in this commit (in its parent for a deletion).
    pub path: PathBuf,
    pub deleted: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FileHistory {
    pub entries: Vec<FileRevision>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct BlameLine {
    /// One-based line number in the selected version.
    pub line: usize,
    /// None for lines introduced or modified in the worktree.
    pub oid: Option<String>,
    pub author: String,
    pub date: String,
    pub text: String,
}

/// With no explicit revision, start at this worktree's HEAD and account for a
/// staged rename. Historical requests never consult the working index or files.
pub fn file_history(root: &Path, path: &Path, revision: Option<&str>) -> Result<FileHistory> {
    let root = git::repository_root(root)?;
    let path = filesystem::relative(&root, path)?;
    let Some(tip) = tip(&root, revision)? else {
        return Ok(FileHistory::default());
    };
    let start = if revision.is_none() {
        staged_source(&root, &tip, &path)?
    } else {
        path
    };
    let limit = format!("--max-count={}", HISTORY_LIMIT + 1);
    let args = [
        "log",
        "--no-decorate",
        "--no-show-signature",
        "--no-notes",
        "--no-ext-diff",
        "--no-textconv",
        "--color=never",
        "--follow",
        "--diff-merges=first-parent",
        "--root",
        "--find-renames",
        "--name-status",
        "-z",
        "--format=%H%x00%an%x00%aI%x00%s",
        &limit,
        &tip,
        "--",
    ];
    let bytes = with_path(&root, &args, &start)?;
    let mut fields = nul_fields(&bytes);
    let mut entries = Vec::new();
    while let Some(oid) = fields.next() {
        let oid = std::str::from_utf8(oid)?.to_owned();
        validate_oid(&oid)?;
        let author = field_text(&mut fields)?;
        let date = field_text(&mut fields)?;
        let subject = field_text(&mut fields)?;
        let status = fields.next().context("Missing file history status")?;
        let status = status.strip_prefix(b"\n").unwrap_or(status);
        let (_, path) = status_paths(&mut fields, status)?;
        entries.push(FileRevision {
            oid,
            author,
            date,
            subject,
            path,
            deleted: status == b"D",
        });
    }
    let truncated = entries.len() > HISTORY_LIMIT;
    entries.truncate(HISTORY_LIMIT);
    Ok(FileHistory { entries, truncated })
}

fn field_text<'a>(fields: &mut impl Iterator<Item = &'a [u8]>) -> Result<String> {
    Ok(
        String::from_utf8_lossy(fields.next().context("Malformed file history metadata")?)
            .into_owned(),
    )
}

fn status_paths<'a>(
    fields: &mut impl Iterator<Item = &'a [u8]>,
    status: &[u8],
) -> Result<(Option<PathBuf>, PathBuf)> {
    let first = fields.next().context("Missing Git status path")?;
    ensure!(!first.is_empty(), "Empty Git status path");
    if matches!(status.first(), Some(b'R' | b'C')) {
        let second = fields.next().context("Missing renamed file destination")?;
        Ok((Some(git::path(first)), git::path(second)))
    } else {
        ensure!(
            matches!(status, b"A" | b"D" | b"M" | b"T" | b"U"),
            "Unexpected file status"
        );
        Ok((None, git::path(first)))
    }
}

fn with_path(root: &Path, args: &[&str], path: &Path) -> Result<Vec<u8>> {
    let mut args = args.iter().map(OsStr::new).collect::<Vec<_>>();
    args.push(path.as_os_str());
    git::output(root, &args)
}

fn tip(root: &Path, revision: Option<&str>) -> Result<Option<String>> {
    if let Some(oid) = revision {
        validate_oid(oid)?;
        ensure!(
            git::line(&git::words(root, &["cat-file", "-t", oid])?)? == "commit",
            "Select an immutable commit to inspect"
        );
        return Ok(Some(oid.to_owned()));
    }
    git::head(root)
}

// NUL records keep non-UTF8 filenames, tabs, and embedded newlines literal.
fn nul_fields(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split_inclusive(|b| *b == 0)
        .map(|field| field.strip_suffix(&[0]).unwrap_or(field))
}

fn staged_source(root: &Path, tip: &str, path: &Path) -> Result<PathBuf> {
    let data = git::words(
        root,
        &[
            "diff-index",
            "--cached",
            "--name-status",
            "-z",
            "--find-renames",
            "--diff-filter=R",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=all",
            tip,
            "--",
        ],
    )?;
    let mut fields = nul_fields(&data);
    while let Some(status) = fields.next() {
        let (source, destination) = status_paths(&mut fields, status)?;
        if destination == path {
            return source.context("Missing source of staged rename");
        }
    }
    Ok(path.to_owned())
}

/// Read raw committed lines and their attribution. For the worktree, carry
/// attribution only onto equal lines. Reading working bytes through the same
/// no-follow path as the editor also avoids executing Git clean/textconv filters.
pub fn file_blame(root: &Path, path: &Path, revision: Option<&str>) -> Result<Vec<BlameLine>> {
    let root = git::repository_root(root)?;
    let path = filesystem::relative(&root, path)?;
    let tip = tip(&root, revision)?;
    let working = if revision.is_none() {
        let working = crate::read_file(&root, &path)?;
        check_line_limit(&working)?;
        Some(working)
    } else {
        None
    };
    let mut old = String::new();
    let mut lines = Vec::new();
    if let Some(oid) = tip {
        let source = if working.is_some() {
            staged_source(&root, &oid, &path)?
        } else {
            path
        };
        let record = with_path(
            &root,
            &["ls-tree", "-z", "--full-tree", &oid, "--"],
            &source,
        )?;
        if !record.is_empty() {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .context("Missing tree path")?;
            let header = std::str::from_utf8(&record[..tab])?
                .split_whitespace()
                .collect::<Vec<_>>();
            ensure!(
                header.len() == 3 && header[1] == "blob",
                "Blame requires a text file"
            );
            let entry = git::Entry {
                mode: header[0].into(),
                oid: header[2].into(),
            };
            let (bytes, truncated) = git::blob(&root, &entry)?;
            ensure!(!truncated, "Blame exceeds the 2 MiB text preview limit");
            old = text::decode(bytes, false).context("Blame is unavailable for binary files")?;
            check_line_limit(&old)?;
            let data = with_path(
                &root,
                &["blame", "--no-textconv", "--porcelain", &oid, "--"],
                &source,
            )?;
            lines = parse_blame(&data)?;
            ensure!(
                lines.len() == old.lines().count(),
                "Blame does not match the selected blob"
            );
        } else {
            ensure!(
                working.is_some(),
                "This path does not exist at the selected commit"
            );
        }
    }
    let Some(working) = working else {
        return Ok(lines);
    };
    if working == old {
        return Ok(lines);
    }
    // Git considers only LF a line terminator; keep lone CR bytes within a line.
    let old_lines: Vec<_> = old.split_inclusive('\n').collect();
    let working_lines: Vec<_> = working.split_inclusive('\n').collect();
    let diff = TextDiff::configure()
        .timeout(Duration::from_secs(1))
        .diff_slices(&old_lines, &working_lines);
    Ok(diff
        .iter_all_changes()
        .filter_map(|change| {
            let line = change.new_index()? + 1;
            if change.tag() == ChangeTag::Equal {
                let mut record = lines[change.old_index()?].clone();
                record.line = line;
                Some(record)
            } else {
                let text = change.value().strip_suffix('\n').unwrap_or(change.value());
                Some(BlameLine {
                    line,
                    oid: None,
                    author: String::new(),
                    date: String::new(),
                    text: text.strip_suffix('\r').unwrap_or(text).to_owned(),
                })
            }
        })
        .collect())
}

fn check_line_limit(contents: &str) -> Result<()> {
    ensure!(
        contents.lines().take(BLAME_LIMIT + 1).count() <= BLAME_LIMIT,
        "Blame exceeds the 50,000 line preview limit"
    );
    Ok(())
}

fn parse_blame(data: &[u8]) -> Result<Vec<BlameLine>> {
    let mut metadata = HashMap::<&str, (String, String)>::new();
    let mut records = Vec::new();
    let mut fields = data.split(|b| *b == b'\n').peekable();
    while fields.peek().is_some_and(|line| !line.is_empty()) {
        let mut header = std::str::from_utf8(fields.next().unwrap())?.split_whitespace();
        let oid = header.next().context("Missing blame commit")?;
        validate_oid(oid)?;
        let line: usize = header.nth(1).context("Missing blame line")?.parse()?;
        ensure!(
            line == records.len() + 1 && line <= BLAME_LIMIT,
            "Unexpected blame line"
        );
        let meta = metadata.entry(oid).or_default();
        let mut timestamp = None;
        loop {
            let field = fields.next().context("Missing blame source line")?;
            if let Some(value) = field.strip_prefix(b"\t") {
                records.push(BlameLine {
                    line,
                    oid: Some(oid.to_owned()),
                    author: meta.0.clone(),
                    date: meta.1.clone(),
                    text: std::str::from_utf8(value.strip_suffix(b"\r").unwrap_or(value))?.into(),
                });
                break;
            } else if let Some(author) = field.strip_prefix(b"author ") {
                meta.0 = String::from_utf8_lossy(author).chars().take(256).collect();
            } else if let Some(time) = field.strip_prefix(b"author-time ") {
                timestamp = Some(std::str::from_utf8(time)?.parse::<i64>()?);
            } else if let Some(zone) = field.strip_prefix(b"author-tz ")
                && let Some(time) = timestamp
            {
                meta.1 = author_date(time, zone)?;
            }
        }
    }
    Ok(records)
}

/// Blame porcelain reports Unix seconds plus the author's numeric timezone.
fn author_date(seconds: i64, zone: &[u8]) -> Result<String> {
    ensure!(
        zone.len() == 5 && matches!(zone[0], b'+' | b'-'),
        "Invalid blame timezone"
    );
    let hours = std::str::from_utf8(&zone[1..3])?.parse::<i64>()?;
    let minutes = std::str::from_utf8(&zone[3..5])?.parse::<i64>()?;
    let offset = (hours * 60 + minutes) * 60 * if zone[0] == b'-' { -1 } else { 1 };
    let local = seconds.checked_add(offset).context("Invalid blame date")?;
    let mut date = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: local is a valid time_t, date points to writable tm storage.
    ensure!(
        !unsafe { libc::gmtime_r(&local, date.as_mut_ptr()) }.is_null(),
        "Invalid blame date"
    );
    // SAFETY: gmtime_r initialized date on success.
    let date = unsafe { date.assume_init() };
    Ok(format!(
        "{:04}-{:02}-{:02}",
        i64::from(date.tm_year) + 1900,
        date.tm_mon + 1,
        date.tm_mday
    ))
}
