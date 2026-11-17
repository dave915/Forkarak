//! Explicit Git line colors in selectable, read-only patch editors.
use super::*;
use gpui_kit::component::{
    ActiveTheme, Theme,
    input::{GutterDecoration, TextDecoration},
};
use std::ops::Range;

pub(super) const ADD_BG: u32 = 0x234535;
pub(super) const DELETE_BG: u32 = 0x492d35;

fn line_count(field: &str, sign: char) -> Option<usize> {
    let field = field.strip_prefix(sign)?;
    let (start, count) = field.split_once(',').unwrap_or((field, "1"));
    start.parse::<usize>().ok()?;
    count.parse().ok()
}

fn counts(header: &str) -> Option<(usize, usize)> {
    let mut fields = header.strip_prefix("@@ ")?.split_ascii_whitespace();
    let old = line_count(fields.next()?, '-')?;
    let new = line_count(fields.next()?, '+')?;
    (fields.next()? == "@@").then_some((old, new))
}

// Count the hunk's source lines so even literal "---" and "+++" content is
// distinguished from the next file's metadata. Decorations use UTF-8 bytes.
fn patch_ranges(text: &str) -> Vec<(Range<usize>, DiffKind)> {
    let mut result: Vec<(Range<usize>, DiffKind)> = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let kind = if let Some(remaining) = counts(line) {
            (old, new) = remaining;
            DiffKind::Header
        } else {
            match line.as_bytes()[0] {
                b'-' if old > 0 => {
                    old -= 1;
                    DiffKind::Deletion
                }
                b'+' if new > 0 => {
                    new -= 1;
                    DiffKind::Addition
                }
                b' ' if old > 0 && new > 0 => {
                    old -= 1;
                    new -= 1;
                    continue;
                }
                b'\\' => continue,
                _ => {
                    (old, new) = (0, 0);
                    continue;
                }
            }
        };
        if let Some((range, previous)) = result.last_mut()
            && *previous == kind
            && range.end == start
        {
            range.end = offset;
        } else {
            result.push((start..offset, kind));
        }
    }
    result
}

fn decorations(ranges: &[(Range<usize>, DiffKind)], palette: Palette) -> Vec<TextDecoration> {
    ranges
        .iter()
        .map(|(range, kind)| {
            let (ink, background) = match kind {
                DiffKind::Addition => (GREEN, ADD_BG),
                DiffKind::Deletion => (RED, DELETE_BG),
                _ => (BLUE, 0x283744),
            };
            TextDecoration::new(
                range.clone(),
                HighlightStyle {
                    color: Some(palette.rgb(ink).into()),
                    background_color: Some(palette.rgb(background).into()),
                    ..Default::default()
                },
            )
        })
        .collect()
}

pub(super) fn editor(
    text: String,
    numbers: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    cx.new(|cx| {
        let mut editor = EditorState::new(window, cx)
            .language("plaintext")
            .line_number(numbers)
            .soft_wrap(false)
            .default_value(text);
        editor.set_readonly(true, cx);
        let ranges = patch_ranges(&editor.value());
        if !ranges.is_empty() {
            let gutter = ranges
                .iter()
                .filter_map(|(range, kind)| {
                    let color = match kind {
                        DiffKind::Addition => 0x469666,
                        DiffKind::Deletion => 0xc26470,
                        _ => return None,
                    };
                    Some(GutterDecoration::new(range.clone()).with_color(rgb(color).into()))
                })
                .collect();
            editor.create_gutter_decorations_collection(gutter, cx);
            let collection =
                editor.create_decorations_collection(decorations(&ranges, Palette::new(cx)), cx);
            let mut dark = cx.theme().is_dark();
            cx.observe_global::<Theme>(move |editor, cx| {
                let next = cx.theme().is_dark();
                if dark != next {
                    dark = next;
                    let ranges = patch_ranges(&editor.value());
                    let styles = decorations(&ranges, Palette::new(cx));
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
    use super::{DiffKind, patch_ranges};
    #[test]
    fn colors_only_hunk_content_in_multifile_patches() {
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@ 이름\r\n---literal\n-한글\n\\ No newline at end of file\n+++literal\n+🦀\n--- a/b\n+++ b/b\n@@ -0,0 +1 @@\n+\n";
        let colored = patch_ranges(patch)
            .into_iter()
            .map(|(range, kind)| (&patch[range], kind))
            .collect::<Vec<_>>();
        assert_eq!(
            colored,
            vec![
                ("@@ -1,2 +1,2 @@ 이름\r\n", DiffKind::Header),
                ("---literal\n-한글\n", DiffKind::Deletion),
                ("+++literal\n+🦀\n", DiffKind::Addition),
                ("@@ -0,0 +1 @@\n", DiffKind::Header),
                ("+\n", DiffKind::Addition),
            ]
        );
        assert!(patch_ranges("Binary files differ\n+not a hunk").is_empty());
    }
}
