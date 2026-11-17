//! Selectable unified patches. Source-line coordinates are separate from UTF-8 offsets in the patch.
use super::*;
use super::{split_diff::DiffSyntax, syntax::CodeSyntax};
use gpui_kit::component::{
    ActiveTheme, Theme,
    input::{GutterDecoration, TextDecoration},
};
use std::ops::Range;

pub(super) const ADD_BG: u32 = 0x234535;
pub(super) const DELETE_BG: u32 = 0x492d35;

fn coordinate(field: &str, sign: char) -> Option<(usize, usize)> {
    let value = field.strip_prefix(sign)?;
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    Some((
        start.parse::<usize>().ok()?.saturating_sub(1),
        count.parse().ok()?,
    ))
}

fn hunk_header(line: &str) -> Option<[(usize, usize); 2]> {
    let mut fields = line.strip_prefix("@@ ")?.split_ascii_whitespace();
    let coords = [
        coordinate(fields.next()?, '-')?,
        coordinate(fields.next()?, '+')?,
    ];
    (fields.next()? == "@@").then_some(coords)
}

// Git quotes control characters, quotes, backslashes and (by default) UTF-8 bytes.
// Decode only the --- / +++ paths; they are unambiguous even when filenames contain spaces.
fn header_path(field: &str) -> Option<PathBuf> {
    let field = field.trim_end_matches(['\r', '\n']);
    let path = if let Some(quoted) = field.strip_prefix('"') {
        let mut bytes = quoted.bytes();
        let mut decoded = Vec::new();
        loop {
            match bytes.next()? {
                b'"' => break,
                b'\\' => {
                    let escaped = bytes.next()?;
                    let decoded_byte = match escaped {
                        b'0'..=b'3' => {
                            let b = bytes.next()?.checked_sub(b'0')?;
                            let c = bytes.next()?.checked_sub(b'0')?;
                            if b > 7 || c > 7 {
                                return None;
                            }
                            (escaped - b'0') * 64 + b * 8 + c
                        }
                        b'a' => 7,
                        b'b' => 8,
                        b't' => 9,
                        b'n' => 10,
                        b'v' => 11,
                        b'f' => 12,
                        b'r' => 13,
                        b'"' | b'\\' => escaped,
                        _ => return None,
                    };
                    decoded.push(decoded_byte);
                }
                byte => decoded.push(byte),
            }
        }
        String::from_utf8_lossy(&decoded).into_owned()
    } else {
        field.split('\t').next()?.to_owned()
    };
    (path != "/dev/null").then(|| {
        PathBuf::from(
            path.strip_prefix("a/")
                .or_else(|| path.strip_prefix("b/"))
                .unwrap_or(&path),
        )
    })
}

enum Sources {
    Document(Arc<DiffSyntax>),
    // Two entries per hunk: deletions and context on the old side, additions and context on the new.
    Hunks(Vec<CodeSyntax>),
}
impl Sources {
    fn get(&self, id: usize) -> &CodeSyntax {
        match self {
            Self::Document(pair) if id == 0 => &pair.old,
            Self::Document(pair) => &pair.new,
            Self::Hunks(hunks) => &hunks[id],
        }
    }
}

struct PatchLine {
    range: Range<usize>,
    kind: DiffKind,
    source: Option<(usize, usize)>,
    content_end: usize,
}

struct Layout {
    rows: Vec<PatchLine>,
    sources: Sources,
}
impl Layout {
    fn new(text: &str, document: Option<Arc<DiffSyntax>>) -> Self {
        let mut sources = document
            .map(Sources::Document)
            .unwrap_or_else(|| Sources::Hunks(Vec::new()));
        let mut rows = Vec::new();
        let mut paths = [None, None];
        let mut offset = 0;
        let mut lines = text
            .split_inclusive('\n')
            .map(|line| {
                let start = offset;
                offset += line.len();
                (start, line)
            })
            .peekable();
        while let Some((start, line)) = lines.next() {
            let Some(coords) = hunk_header(line) else {
                if line.starts_with("diff --git ") {
                    paths = [None, None];
                } else if let Some(path) = line.strip_prefix("--- ") {
                    paths[0] = header_path(path);
                } else if let Some(path) = line.strip_prefix("+++ ") {
                    paths[1] = header_path(path);
                }
                continue;
            };
            let end = start + line.len();
            rows.push(PatchLine {
                range: start..end,
                kind: DiffKind::Header,
                source: None,
                content_end: end,
            });
            let base = match &sources {
                Sources::Hunks(hunks) => hunks.len(),
                Sources::Document(_) => 0,
            };
            let mut remaining = [coords[0].1, coords[1].1];
            let mut row = if matches!(sources, Sources::Document(_)) {
                [coords[0].0, coords[1].0]
            } else {
                [0, 0]
            };
            let mut reconstructed = [String::new(), String::new()];
            while remaining != [0, 0]
                && let Some(&(start, line)) = lines.peek()
            {
                let (kind, side) = match line.as_bytes()[0] {
                    b'-' if remaining[0] > 0 => (DiffKind::Deletion, 0),
                    b'+' if remaining[1] > 0 => (DiffKind::Addition, 1),
                    b' ' if remaining[0] > 0 && remaining[1] > 0 => (DiffKind::Context, 1),
                    b'\\' => {
                        lines.next();
                        continue;
                    }
                    _ => break,
                };
                lines.next();
                let end = start + line.len();
                rows.push(PatchLine {
                    range: start..end,
                    kind: kind.clone(),
                    source: Some((base + side, row[side])),
                    content_end: start + line.trim_end_matches('\n').trim_end_matches('\r').len(),
                });
                for side_ix in 0..2 {
                    if side_ix == side || kind == DiffKind::Context {
                        if matches!(sources, Sources::Hunks(_)) {
                            reconstructed[side_ix].push_str(&line[1..]);
                        }
                        row[side_ix] = row[side_ix].saturating_add(1);
                        remaining[side_ix] -= 1;
                    }
                }
            }
            if let Sources::Hunks(hunks) = &mut sources {
                for (path, text) in paths.iter().zip(reconstructed) {
                    hunks.push(CodeSyntax::new(
                        path.as_deref().unwrap_or(Path::new("")),
                        &text,
                    ));
                }
            }
        }
        Self { rows, sources }
    }

    fn decorations(&self, text: &str, cx: &App) -> Vec<TextDecoration> {
        let palette = Palette::new(cx);
        let mut out = Vec::new();
        for row in &self.rows {
            let (prefix, bg) = match row.kind {
                DiffKind::Header => (Some(BLUE), Some(0x283744)),
                DiffKind::Addition => (Some(GREEN), Some(ADD_BG)),
                DiffKind::Deletion => (Some(RED), Some(DELETE_BG)),
                DiffKind::Context => (None, None),
            };
            let bg = bg.map(|c| palette.rgb(c).into());
            let base = HighlightStyle {
                background_color: bg,
                ..Default::default()
            };
            let prefix_style = HighlightStyle {
                color: prefix.map(|c| palette.rgb(c).into()),
                ..base
            };
            let Some((source, line)) = row.source else {
                out.push(TextDecoration::new(row.range.clone(), prefix_style));
                continue;
            };
            let content = row.range.start + 1;
            if prefix.is_some() {
                out.push(TextDecoration::new(row.range.start..content, prefix_style));
            }
            let mut cursor = content;
            for (range, mut style) in self.sources.get(source).spans(line, cx) {
                let start = content + range.start;
                let end = content + range.end;
                // Out-of-range spans are omitted rather than slicing a neighboring line.
                if start < cursor
                    || end > row.content_end
                    || start >= end
                    || !text.is_char_boundary(start)
                    || !text.is_char_boundary(end)
                {
                    continue;
                }
                if bg.is_some() && cursor < start {
                    out.push(TextDecoration::new(cursor..start, base));
                }
                if bg.is_some() {
                    style.background_color = bg;
                }
                out.push(TextDecoration::new(start..end, style));
                cursor = end;
            }
            if bg.is_some() && cursor < row.range.end {
                out.push(TextDecoration::new(cursor..row.range.end, base));
            }
        }
        out
    }
}

pub(super) struct Prepared {
    text: String,
    layout: Layout,
}
impl Prepared {
    pub(super) fn new(text: String, document: Option<Arc<DiffSyntax>>) -> Self {
        let layout = Layout::new(&text, document);
        Self { text, layout }
    }
}

pub(super) fn editor(
    patch: Prepared,
    numbers: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let Prepared { text, layout } = patch;
    let text: SharedString = text.into();
    cx.new(|cx| {
        let mut editor = EditorState::new(window, cx)
            .language("plaintext")
            .line_number(numbers)
            .soft_wrap(false)
            .default_value(text.clone());
        editor.set_readonly(true, cx);
        if !layout.rows.is_empty() {
            let mut gutter: Vec<(Range<usize>, DiffKind)> = Vec::new();
            for row in &layout.rows {
                if matches!(row.kind, DiffKind::Addition | DiffKind::Deletion) {
                    if let Some((range, kind)) = gutter.last_mut()
                        && row.kind == *kind
                        && range.end == row.range.start
                    {
                        range.end = row.range.end;
                    } else {
                        gutter.push((row.range.clone(), row.kind.clone()));
                    }
                }
            }
            editor.create_gutter_decorations_collection(
                gutter
                    .into_iter()
                    .map(|(range, kind)| {
                        GutterDecoration::new(range).with_color(
                            rgb(if kind == DiffKind::Addition {
                                0x469666
                            } else {
                                0xc26470
                            })
                            .into(),
                        )
                    })
                    .collect(),
                cx,
            );
            let collection =
                editor.create_decorations_collection(layout.decorations(&text, cx), cx);
            let mut dark = cx.theme().is_dark();
            let mut theme = cx.theme().highlight_theme.clone();
            cx.observe_global::<Theme>(move |_, cx| {
                let next = cx.theme();
                if next.is_dark() != dark || !Arc::ptr_eq(&theme, &next.highlight_theme) {
                    dark = next.is_dark();
                    theme = next.highlight_theme.clone();
                    let styles = layout.decorations(&text, cx);
                    let collection = collection.clone();
                    cx.defer(move |cx| collection.set(styles, cx));
                }
            })
            .detach();
        }
        editor
    })
}

#[cfg(test)]
mod tests {
    use super::{DiffKind, Layout, Sources, header_path};
    use std::path::PathBuf;

    #[test]
    fn quoted_git_paths_spaces_dates_and_absent_sides() {
        assert_eq!(
            header_path("a/my project/hello.rs\t1980-01-01\r\n"),
            Some(PathBuf::from("my project/hello.rs"))
        );
        assert_eq!(
            header_path(r#""b/\355\225\234 lab/a\t\"x\".kt""#),
            Some(PathBuf::from("한 lab/a\t\"x\".kt"))
        );
        assert_eq!(header_path("/dev/null\n"), None);
        assert_eq!(header_path(r#""a/broken\8.txt""#), None);
    }

    #[test]
    fn reconstructs_each_file_and_hunk_with_independent_source_lines() {
        let patch = "diff --git a/old.rs b/new.ts\n--- a/old.rs\n+++ b/new.ts\n@@ -30,2 +40,2 @@ 이름\r\n---old 한\r\n\\ No newline at end of file\n+++new 🦀\r\n context\r\n@@ -89 +90 @@\n-old\n+new\ndiff --git a/b b/b\n--- /dev/null\n+++ b/a b.py\n@@ -0,0 +1 @@\n+print('한')\n";
        let layout = Layout::new(patch, None);
        let Sources::Hunks(hunks) = &layout.sources else {
            panic!("raw patch expected")
        };
        assert_eq!(hunks.len(), 6);
        let rows = layout
            .rows
            .iter()
            .filter_map(|row| {
                row.source.map(|index| {
                    (
                        &patch[row.range.start + 1..row.content_end],
                        row.kind.clone(),
                        index,
                    )
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            vec![
                ("--old 한", DiffKind::Deletion, (0, 0)),
                ("++new 🦀", DiffKind::Addition, (1, 0)),
                ("context", DiffKind::Context, (1, 1)),
                ("old", DiffKind::Deletion, (2, 0)),
                ("new", DiffKind::Addition, (3, 0)),
                ("print('한')", DiffKind::Addition, (5, 0))
            ]
        );
        assert!(
            Layout::new("Binary files differ\n+not a hunk", None)
                .rows
                .is_empty()
        );
    }
}
