use crate::{ChangedFile, DiffDocument, DiffKind, DiffLine};
use similar::{ChangeTag, TextDiff};
use std::time::Duration;

const MAX_LINES: usize = 50_000;

pub(crate) fn decode(bytes: Vec<u8>, truncated: bool) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(s) => Some(s),
        Err(e) if truncated && e.utf8_error().error_len().is_none() => {
            let end = e.utf8_error().valid_up_to();
            String::from_utf8(e.into_bytes()[..end].to_vec()).ok()
        }
        Err(_) => None,
    }
}

struct Preview {
    text: Option<(String, String)>,
    truncated: bool,
}

impl Preview {
    fn new(old: (Vec<u8>, bool), new: (Vec<u8>, bool)) -> Self {
        let mut truncated = old.1 || new.1;
        let text = decode(old.0, old.1).zip(decode(new.0, new.1));
        if let Some((old, new)) = &text {
            truncated |= [old, new]
                .iter()
                .any(|s| s.bytes().filter(|b| *b == b'\n').count() > MAX_LINES);
        }
        Self { text, truncated }
    }
}

fn line_diff<'a>(old: &'a str, new: &'a str) -> TextDiff<'a, 'a, 'a, str> {
    TextDiff::configure()
        .timeout(Duration::from_millis(200))
        .diff_lines(old, new)
}

/// Count the rows the selected preview would expose, without allocating its
/// patch string or its per-line strings and line numbers.
pub(crate) fn set_statistics(file: &mut ChangedFile, old: (Vec<u8>, bool), new: (Vec<u8>, bool)) {
    let preview = Preview::new(old, new);
    file.binary = preview.text.is_none();
    file.additions = 0;
    file.deletions = 0;
    if let Some((old, new)) = preview.text
        && !preview.truncated
    {
        let diff = line_diff(&old, &new);
        for hunk in diff.unified_diff().iter_hunks() {
            let mut starts_patch_line = true;
            for change in hunk.iter_changes() {
                if starts_patch_line {
                    match change.tag() {
                        ChangeTag::Insert => file.additions += 1,
                        ChangeTag::Delete => file.deletions += 1,
                        ChangeTag::Equal => (),
                    }
                }
                // similar tokenizes a bare CR as a line ending. The formatted
                // patch retains it, so parse_patch sees the next tag on the
                // same LF-delimited row. A missing final newline gets a hint.
                starts_patch_line = !change.value().ends_with('\r');
            }
        }
    }
}

pub(crate) fn render(document: &mut DiffDocument, old: (Vec<u8>, bool), new: (Vec<u8>, bool)) {
    let preview = Preview::new(old, new);
    document.truncated = preview.truncated;
    document.binary = preview.text.is_none();
    if let Some((old, new)) = preview.text {
        document.old_text = old;
        document.new_text = new;
        if document.truncated {
            document.patch =
                "Diff omitted: file exceeds the 2 MiB / 50,000 line preview limit.\n".into();
        } else {
            let diff = line_diff(&document.old_text, &document.new_text);
            let old_name = format!(
                "{:?}",
                format!(
                    "a/{}",
                    document
                        .old_path
                        .as_ref()
                        .unwrap_or(&document.path)
                        .to_string_lossy()
                )
            );
            let new_name = format!("{:?}", format!("b/{}", document.path.to_string_lossy()));
            document.patch = diff
                .unified_diff()
                .context_radius(3)
                .header(&old_name, &new_name)
                .to_string();
            if document.patch.is_empty()
                && let Some(old_path) = &document.old_path
            {
                document.patch = format!(
                    "rename from {:?}\nrename to {:?}\n",
                    old_path, document.path
                );
            }
        }
    } else {
        document.patch = "Binary contents differ; text preview is unavailable.\n".into();
    }
    document.lines = parse_patch(&document.patch);
}

pub(crate) fn parse_patch(patch: &str) -> Vec<DiffLine> {
    let mut old = 0;
    let mut new = 0;
    let mut in_hunk = false;
    patch
        .lines()
        .map(|line| {
            if line.starts_with("@@ ") {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                old = fields
                    .get(1)
                    .and_then(|s| s.strip_prefix('-'))
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                new = fields
                    .get(2)
                    .and_then(|s| s.strip_prefix('+'))
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                in_hunk = true;
            } else if in_hunk {
                let (old_line, new_line, kind) = match line.as_bytes().first() {
                    Some(b' ') => {
                        let pair = (Some(old), Some(new), DiffKind::Context);
                        old += 1;
                        new += 1;
                        pair
                    }
                    Some(b'-') => {
                        let pair = (Some(old), None, DiffKind::Deletion);
                        old += 1;
                        pair
                    }
                    Some(b'+') => {
                        let pair = (None, Some(new), DiffKind::Addition);
                        new += 1;
                        pair
                    }
                    _ => (None, None, DiffKind::Header),
                };
                let text = if kind == DiffKind::Header {
                    line
                } else {
                    &line[1..]
                };
                return DiffLine {
                    old_line,
                    new_line,
                    kind,
                    text: text.into(),
                };
            }
            DiffLine {
                old_line: None,
                new_line: None,
                kind: DiffKind::Header,
                text: line.into(),
            }
        })
        .collect()
}
