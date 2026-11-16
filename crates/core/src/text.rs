use crate::{DiffDocument, DiffKind, DiffLine};
use similar::TextDiff;
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

pub(crate) fn render(document: &mut DiffDocument, old: (Vec<u8>, bool), new: (Vec<u8>, bool)) {
    document.truncated = old.1 || new.1;
    let old_text = decode(old.0, old.1);
    let new_text = decode(new.0, new.1);
    document.binary = old_text.is_none() || new_text.is_none();
    if document.binary {
        document.patch = "Binary contents differ; text preview is unavailable.\n".into();
    } else {
        document.old_text = old_text.unwrap_or_default();
        document.new_text = new_text.unwrap_or_default();
        document.truncated |= document.old_text.bytes().filter(|b| *b == b'\n').count() > MAX_LINES
            || document.new_text.bytes().filter(|b| *b == b'\n').count() > MAX_LINES;
        if document.truncated {
            document.patch =
                "Diff omitted: file exceeds the 2 MiB / 50,000 line preview limit.\n".into();
        } else {
            let diff = TextDiff::configure()
                .timeout(Duration::from_millis(200))
                .diff_lines(&document.old_text, &document.new_text);
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
    }
    document.lines = parse_patch(&document.patch);
}

fn parse_patch(patch: &str) -> Vec<DiffLine> {
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
