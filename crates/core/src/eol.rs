//! Git's built-in worktree-to-blob CRLF conversion. Attribute lookup and index
//! EOL metadata are batched. Working contents never pass through Git drivers.
use crate::git;
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Git's auto-text heuristic is based on the whole byte stream, including
/// lone CR and control characters. CRLF pairs may straddle read buffers.
#[derive(Default)]
pub(crate) struct Stats {
    length: u64,
    cr: u64,
    pairs: u64,
    printable: u64,
    nonprintable: u64,
    nul: bool,
    last: u8,
}

impl Stats {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.length += bytes.len() as u64;
        for &byte in bytes {
            match byte {
                b'\r' => self.cr += 1,
                b'\n' => self.pairs += u64::from(self.last == b'\r'),
                8 | 9 | 12 | 27 => self.printable += 1,
                0..=31 | 127 => {
                    self.nonprintable += 1;
                    self.nul |= byte == 0;
                }
                _ => self.printable += 1,
            }
            self.last = byte;
        }
    }

    pub(crate) fn has_crlf(&self) -> bool {
        self.pairs != 0
    }

    pub(crate) fn normalized_len(&self) -> u64 {
        self.length - self.pairs
    }

    fn binary(&self) -> bool {
        self.nul
            || self.cr != self.pairs
            || self.nonprintable - u64::from(self.last == 0x1a) > self.printable / 128
    }
}

/// Remove just the CR in each CRLF pair, preserving standalone CR and EOF.
pub(crate) fn normalize(bytes: &mut [u8]) -> usize {
    let mut written = 0;
    for i in 0..bytes.len() {
        if bytes[i] != b'\r' || bytes.get(i + 1) != Some(&b'\n') {
            bytes[written] = bytes[i];
            written += 1;
        }
    }
    written
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Raw,
    Text,
    Auto,
}

fn text_rule(value: &[u8]) -> Option<Rule> {
    match value {
        b"unset" => Some(Rule::Raw),
        b"set" | b"input" => Some(Rule::Text),
        b"auto" => Some(Rule::Auto),
        _ => None,
    }
}

fn default_rule(root: &Path) -> Result<Rule> {
    let args = ["config", "--default=false", "--get", "core.autocrlf"];
    let value = git::words(root, &args)?;
    let enabled = match git::line(&value)? {
        "false" => false,
        "true" => true,
        v if v.eq_ignore_ascii_case("input") => true,
        // Let Git parse its remaining boolean spellings, including an implicit true.
        _ => {
            git::words(
                root,
                &[
                    "config",
                    "--type=bool",
                    "--default=false",
                    "--get",
                    "core.autocrlf",
                ],
            )? == b"true\n"
        }
    };
    Ok(if enabled { Rule::Auto } else { Rule::Raw })
}

// Bound argv by bytes (including pointer slots), even with long native paths.
fn batches<'a>(
    root: &Path,
    command: &[&str],
    paths: impl IntoIterator<Item = &'a Path>,
    mut consume: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut args: Vec<&OsStr> = command.iter().map(OsStr::new).collect();
    let fixed = args.len();
    let mut size = 0;
    for path in paths {
        let cost = path.as_os_str().len() + 1 + size_of::<&OsStr>();
        if size + cost > 48 * 1024 && args.len() > fixed {
            consume(&git::output(root, &args)?)?;
            args.truncate(fixed);
            size = 0;
        }
        args.push(path.as_os_str());
        size += cost;
    }
    if args.len() > fixed {
        consume(&git::output(root, &args)?)?;
    }
    Ok(())
}

pub(crate) fn should_normalize(root: &Path, path: &Path, bytes: &[u8]) -> Result<bool> {
    if !bytes.contains(&b'\r') {
        return Ok(false);
    }
    let mut stats = Stats::default();
    stats.update(bytes);
    Ok(stats.has_crlf()
        && normalized_paths(root, &BTreeMap::from([(path.into(), stats)]))?.contains(path))
}

pub(crate) fn normalized_paths(
    root: &Path,
    candidates: &BTreeMap<PathBuf, Stats>,
) -> Result<BTreeSet<PathBuf>> {
    let mut normalized = BTreeSet::new();
    if candidates.is_empty() {
        return Ok(normalized);
    }
    let default = default_rule(root)?;
    let mut auto = Vec::new();
    batches(
        root,
        &["check-attr", "-z", "text", "crlf", "eol", "--"],
        candidates.keys().map(PathBuf::as_path),
        |bytes| {
            let fields: Vec<_> = bytes
                .strip_suffix(&[0])
                .context("Incomplete Git EOL attributes")?
                .split(|b| *b == 0)
                .collect();
            let (records, remainder) = fields.as_chunks::<9>();
            ensure!(remainder.is_empty(), "Incomplete Git EOL attribute record");
            for &[name, a, text, name2, b, crlf, name3, c, eol] in records {
                ensure!(
                    name == name2 && name == name3 && a == b"text" && b == b"crlf" && c == b"eol",
                    "Malformed Git EOL attribute record"
                );
                let path = git::path(name);
                let stats = candidates
                    .get(&path)
                    .context("Unknown Git EOL attribute path")?;
                let rule = text_rule(text).or_else(|| text_rule(crlf)).unwrap_or(
                    if matches!(eol, b"lf" | b"crlf") {
                        Rule::Text
                    } else {
                        default
                    },
                );
                match rule {
                    Rule::Text => {
                        normalized.insert(path);
                    }
                    Rule::Auto if !stats.binary() => {
                        auto.push(path.clone());
                        normalized.insert(path);
                    }
                    _ => (),
                }
            }
            Ok(())
        },
    )?;
    // Auto preserves text blobs that already contain CRLF in the current index.
    // Request only the index field: Git must never open working files for us.
    batches(
        root,
        &[
            "ls-files",
            "-z",
            "--format=%(eolinfo:index)%x09%(path)",
            "--",
        ],
        auto.iter().map(PathBuf::as_path),
        |bytes| {
            for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
                let tab = record
                    .iter()
                    .position(|b| *b == b'\t')
                    .context("Malformed index EOL record")?;
                if matches!(&record[..tab], b"crlf" | b"mixed") {
                    normalized.remove(&git::path(&record[tab + 1..]));
                }
            }
            Ok(())
        },
    )?;
    Ok(normalized)
}
