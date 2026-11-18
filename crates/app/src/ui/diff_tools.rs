//! Bounded display-only diff analysis. Source patches and staging coordinates stay intact.
use super::*;
use gpui_kit::component::ActiveTheme;
use pokarak_core::DiffLine;
use std::ops::Range;

actions!(pokarak, [NextDiffHunk, PreviousDiffHunk]);

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-down", NextDiffHunk, Some("DiffReview")),
        KeyBinding::new("alt-up", PreviousDiffHunk, Some("DiffReview")),
    ]);
}

const TOKEN_LIMIT: usize = 128;
const WORD_BUDGET: usize = 500_000;

fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut result: Vec<Range<usize>> = Vec::new();
    let mut previous = None;
    for (offset, ch) in text.char_indices() {
        let group = if ch.is_whitespace() {
            0
        } else if ch.is_alphanumeric() || ch == '_' {
            1
        } else {
            2
        };
        if previous == Some(group) && group != 2 {
            result.last_mut().unwrap().end = offset + ch.len_utf8();
        } else {
            result.push(offset..offset + ch.len_utf8());
            if result.len() > TOKEN_LIMIT {
                break;
            }
        }
        previous = Some(group);
    }
    result
}

fn fallback_ranges(old: &str, new: &str) -> [Vec<Range<usize>>; 2] {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    [old, new].map(|text| {
        let end = text.len() - suffix;
        if prefix < end {
            std::iter::once(prefix..end).collect()
        } else {
            vec![]
        }
    })
}

fn append_range(ranges: &mut Vec<Range<usize>>, next: Range<usize>) {
    if let Some(previous) = ranges.last_mut()
        && previous.end == next.start
    {
        previous.end = next.end;
    } else {
        ranges.push(next);
    }
}

/// Token LCS is bounded per pair and per document; long lines use a Unicode-safe
/// common-prefix/suffix fallback instead of quadratic work.
fn word_ranges(old: &str, new: &str, budget: &mut usize) -> [Vec<Range<usize>>; 2] {
    if old == new {
        return [vec![], vec![]];
    }
    if old.len() > 8192 || new.len() > 8192 {
        return fallback_ranges(old, new);
    }
    let a = tokens(old);
    let b = tokens(new);
    let cost = (a.len() + 1) * (b.len() + 1);
    if a.len() > TOKEN_LIMIT || b.len() > TOKEN_LIMIT || cost > *budget {
        return fallback_ranges(old, new);
    }
    *budget -= cost;
    let stride = b.len() + 1;
    let mut table = vec![0u16; cost];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * stride + j] = if old[a[i].clone()] == new[b[j].clone()] {
                table[(i + 1) * stride + j + 1] + 1
            } else {
                table[(i + 1) * stride + j].max(table[i * stride + j + 1])
            };
        }
    }
    let mut result = [Vec::new(), Vec::new()];
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && old[a[i].clone()] == new[b[j].clone()] {
            i += 1;
            j += 1;
        } else if i < a.len()
            && (j == b.len() || table[(i + 1) * stride + j] >= table[i * stride + j + 1])
        {
            append_range(&mut result[0], a[i].clone());
            i += 1;
        } else {
            append_range(&mut result[1], b[j].clone());
            j += 1;
        }
    }
    result
}

fn without_space_eq(old: &str, new: &str) -> bool {
    old.chars()
        .filter(|ch| !ch.is_whitespace())
        .eq(new.chars().filter(|ch| !ch.is_whitespace()))
}

pub(super) struct DiffAnalysis {
    pub(super) inline: Vec<Vec<Range<usize>>>,
    whitespace_only: Vec<bool>,
}

impl DiffAnalysis {
    pub(super) fn new(lines: &[DiffLine]) -> Self {
        let mut result = Self {
            inline: vec![vec![]; lines.len()],
            whitespace_only: vec![false; lines.len()],
        };
        let mut budget = WORD_BUDGET;
        let mut at = 0;
        while at < lines.len() {
            if !matches!(lines[at].kind, DiffKind::Addition | DiffKind::Deletion) {
                at += 1;
                continue;
            }
            let old = at;
            while at < lines.len() && lines[at].kind == DiffKind::Deletion {
                at += 1;
            }
            let new = at;
            while at < lines.len() && lines[at].kind == DiffKind::Addition {
                at += 1;
            }
            for offset in 0..(new - old).max(at - new) {
                let before = (old + offset < new).then_some(old + offset);
                let after = (new + offset < at).then_some(new + offset);
                match (before, after) {
                    (Some(a), Some(b)) => {
                        let [deleted, added] =
                            word_ranges(&lines[a].text, &lines[b].text, &mut budget);
                        result.inline[a] = deleted;
                        result.inline[b] = added;
                        if without_space_eq(&lines[a].text, &lines[b].text) {
                            result.whitespace_only[a] = true;
                            result.whitespace_only[b] = true;
                        }
                    }
                    (Some(i), None) | (None, Some(i)) => {
                        if !lines[i].text.is_empty() {
                            result.inline[i] = std::iter::once(0..lines[i].text.len()).collect();
                        }
                        result.whitespace_only[i] = lines[i].text.chars().all(char::is_whitespace);
                    }
                    _ => {}
                }
            }
        }
        result
    }

    pub(super) fn view(&self, lines: &[DiffLine], hide_whitespace: bool) -> DiffView {
        let mut keep = vec![true; lines.len()];
        if hide_whitespace {
            for (index, hidden) in self.whitespace_only.iter().enumerate() {
                keep[index] = !hidden;
            }
            let headers: Vec<_> = lines
                .iter()
                .enumerate()
                .filter_map(|(i, line)| {
                    (line.kind == DiffKind::Header && line.text.starts_with("@@")).then_some(i)
                })
                .collect();
            for (position, &start) in headers.iter().enumerate() {
                let end = headers.get(position + 1).copied().unwrap_or(lines.len());
                if !(start..end).any(|i| {
                    keep[i] && matches!(lines[i].kind, DiffKind::Addition | DiffKind::Deletion)
                }) {
                    keep[start..end].fill(false);
                }
            }
            if !(0..lines.len()).any(|i| {
                keep[i] && matches!(lines[i].kind, DiffKind::Addition | DiffKind::Deletion)
            }) {
                keep.fill(false);
            }
        }
        let visible: Vec<_> = keep
            .iter()
            .enumerate()
            .filter_map(|(i, &keep)| keep.then_some(i))
            .collect();
        let mut hunks: Vec<_> = visible
            .iter()
            .enumerate()
            .filter_map(|(row, &i)| {
                (lines[i].kind == DiffKind::Header && lines[i].text.starts_with("@@"))
                    .then_some(row)
            })
            .collect();
        if hunks.is_empty() {
            for (row, &i) in visible.iter().enumerate() {
                if matches!(lines[i].kind, DiffKind::Addition | DiffKind::Deletion)
                    && (row == 0
                        || !matches!(
                            lines[visible[row - 1]].kind,
                            DiffKind::Addition | DiffKind::Deletion
                        ))
                {
                    hunks.push(row);
                }
            }
        }
        DiffView {
            visible: visible.into(),
            keep,
            hunks,
        }
    }
}

pub(super) struct DiffView {
    pub(super) visible: Arc<[usize]>,
    pub(super) keep: Vec<bool>,
    pub(super) hunks: Vec<usize>,
}

pub(super) fn hunk_target(hunks: &[usize], current: usize, forward: bool) -> Option<usize> {
    if forward {
        hunks
            .iter()
            .copied()
            .find(|&row| row > current)
            .or_else(|| hunks.last().copied())
    } else {
        hunks
            .iter()
            .rev()
            .copied()
            .find(|&row| row < current)
            .or_else(|| hunks.first().copied())
    }
}

pub(super) fn line_highlights(
    text: &str,
    syntax: Vec<(Range<usize>, HighlightStyle)>,
    changed: &[Range<usize>],
    kind: &DiffKind,
    base: Option<Hsla>,
    cx: &App,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let syntax: Vec<_> = syntax
        .into_iter()
        .filter(|(range, _)| text.get(range.clone()).is_some())
        .collect();
    let mut boundaries = vec![0, text.len()];
    for range in syntax.iter().map(|(range, _)| range).chain(changed) {
        if text.get(range.clone()).is_some() {
            boundaries.extend([range.start, range.end]);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
        .windows(2)
        .filter_map(|pair| {
            let range = pair[0]..pair[1];
            if range.is_empty() {
                return None;
            }
            let mut style = syntax
                .iter()
                .find(|(token, _)| token.start <= range.start && token.end >= range.end)
                .map(|(_, style)| *style)
                .unwrap_or_default();
            if base.is_some() {
                style.background_color = base;
            }
            if changed
                .iter()
                .any(|word| word.start <= range.start && word.end >= range.end)
            {
                style.background_color = Some(
                    rgb(match (cx.theme().is_dark(), kind) {
                        (true, DiffKind::Addition) => 0x3b6c4c,
                        (false, DiffKind::Addition) => 0xa6d5b6,
                        (true, _) => 0x7f4650,
                        (false, _) => 0xf0adb8,
                    })
                    .into(),
                );
            }
            Some((range, style))
        })
        .collect()
}

pub(super) fn styled_line(
    text: &str,
    syntax: Vec<(Range<usize>, HighlightStyle)>,
    changed: &[Range<usize>],
    kind: &DiffKind,
    expand_tabs: bool,
    cx: &App,
) -> StyledText {
    let mut spans = line_highlights(text, syntax, changed, kind, None, cx);
    if expand_tabs && text.contains('\t') {
        let tabs: Vec<_> = text.match_indices('\t').map(|(at, _)| at).collect();
        let expanded = |position| position + 3 * tabs.partition_point(|&at| at < position);
        for (range, _) in &mut spans {
            *range = expanded(range.start)..expanded(range.end);
        }
        StyledText::new(text.replace('\t', "    ")).with_highlights(spans)
    } else {
        StyledText::new(text.to_owned()).with_highlights(spans)
    }
}

impl Pokarak {
    pub(super) fn hides_diff_whitespace(&self) -> bool {
        self.session()
            .is_some_and(|session| session.saved.diff_hide_whitespace)
    }

    pub(super) fn diff_reading_controls(
        &self,
        diff: &ReviewDiff,
        history: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if diff.binary || diff.truncated {
            return None;
        }
        let hidden = self.hides_diff_whitespace();
        let empty = diff.view(hidden).hunks.is_empty();
        Some(
            div()
                .id(if history {
                    "history-diff-tools"
                } else {
                    "working-diff-tools"
                })
                .flex_none()
                .flex()
                .items_center()
                .child(
                    toolbar::toolbar_icon(
                        "previous-diff-hunk",
                        IconName::ChevronUp,
                        "이전 변경 블록",
                    )
                    .disabled(empty)
                    .on_click(
                        cx.listener(|app, _, window, cx| app.jump_diff_hunk(false, window, cx)),
                    )
                    .keycap_tooltip("이전 변경 블록", "alt-up"),
                )
                .child(
                    toolbar::toolbar_icon(
                        "next-diff-hunk",
                        IconName::ChevronDown,
                        "다음 변경 블록",
                    )
                    .disabled(empty)
                    .on_click(
                        cx.listener(|app, _, window, cx| app.jump_diff_hunk(true, window, cx)),
                    )
                    .keycap_tooltip("다음 변경 블록", "alt-down"),
                )
                .when(hidden, |bar| {
                    bar.child(
                        toolbar::toolbar_icon(
                            "hide-whitespace-diff",
                            IconName::EyeOff,
                            "공백 변경 숨김 적용 중",
                        )
                        .selected(true)
                        .tooltip("공백 변경 숨김 적용 중 · 클릭하여 모든 변경 표시")
                        .on_click(cx.listener(|app, _, _, cx| app.toggle_diff_whitespace(cx))),
                    )
                })
                .into_any_element(),
        )
    }

    pub(super) fn toggle_diff_whitespace(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self
            .current
            .as_ref()
            .and_then(|root| self.sessions.get_mut(root))
        {
            session.saved.diff_hide_whitespace = !session.saved.diff_hide_whitespace;
            self.persist(cx);
            cx.notify();
        }
    }

    pub(super) fn jump_diff_hunk(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        if self.git_history.active {
            self.jump_history_diff_hunk(forward, window, cx);
            return;
        }
        let Some(session) = self.session().filter(|session| !session.saved.code) else {
            return;
        };
        let Some(diff) = session
            .saved
            .selected
            .as_ref()
            .and_then(|path| session.diffs.get(path))
        else {
            return;
        };
        diff.jump_hunk(self, Some(&session.scroll), forward, cx);
    }

    pub(super) fn diff_key_context(&self) -> &'static str {
        if self.git_history.active {
            if self.history_diff_active() {
                "Pokarak GitGraph DiffReview"
            } else {
                "Pokarak GitGraph"
            }
        } else if self.session().is_some_and(|session| {
            !session.saved.code
                && session
                    .saved
                    .selected
                    .as_ref()
                    .is_some_and(|path| session.diffs.contains_key(path))
        }) {
            "Pokarak DiffReview"
        } else {
            "Pokarak"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DiffAnalysis, WORD_BUDGET, hunk_target, word_ranges};
    use pokarak_core::{DiffDocument, DiffKind, DiffLine};

    fn line(kind: DiffKind, text: &str, old: Option<usize>, new: Option<usize>) -> DiffLine {
        DiffLine {
            kind,
            text: text.into(),
            old_line: old,
            new_line: new,
        }
    }

    #[test]
    fn unicode_words_are_independent_exact_utf8_ranges() {
        let old = "val 이름 = \"😀 오래된 값\"; count = 10;";
        let new = "val 이름 = \"😀 새로운 값\"; count = 20;";
        let mut budget = WORD_BUDGET;
        let ranges = word_ranges(old, new, &mut budget);
        let old_words = ranges[0]
            .iter()
            .map(|r| &old[r.clone()])
            .collect::<Vec<_>>();
        let new_words = ranges[1]
            .iter()
            .map(|r| &new[r.clone()])
            .collect::<Vec<_>>();
        assert_eq!(old_words, vec!["오래된", "10"]);
        assert_eq!(new_words, vec!["새로운", "20"]);
        assert!(
            ranges[0]
                .iter()
                .all(|range| !old[range.clone()].contains('😀'))
        );
        let fallback = word_ranges("😀abc", "😀ab😀", &mut 0);
        assert_eq!(&"😀abc"[fallback[0][0].clone()], "c");
        assert_eq!(&"😀ab😀"[fallback[1][0].clone()], "😀");
    }

    #[test]
    fn whitespace_filter_preserves_real_changes_and_original_coordinates() {
        let document = DiffDocument {
            patch: "unchanged original patch".into(),
            lines: vec![
                line(DiffKind::Header, "@@ -1,2 +1,2 @@", None, None),
                line(DiffKind::Deletion, "  keep();", Some(1), None),
                line(DiffKind::Deletion, "old(😀);", Some(2), None),
                line(DiffKind::Addition, "keep();", None, Some(1)),
                line(DiffKind::Addition, "new(😀);", None, Some(2)),
                line(DiffKind::Header, "@@ -20 +20 @@", None, None),
                line(DiffKind::Deletion, "x = 1;", Some(20), None),
                line(DiffKind::Addition, "x=1;", None, Some(20)),
            ],
            ..Default::default()
        };
        let analysis = DiffAnalysis::new(&document.lines);
        assert_eq!(
            analysis.view(&document.lines, false).visible.as_ref(),
            &[0, 1, 2, 3, 4, 5, 6, 7]
        );
        let filtered = analysis.view(&document.lines, true);
        assert_eq!(filtered.visible.as_ref(), &[0, 2, 4]);
        assert_eq!(document.lines[filtered.visible[1]].old_line, Some(2));
        assert_eq!(document.lines[filtered.visible[2]].new_line, Some(2));
        assert_eq!(filtered.hunks, vec![0]);
        assert_eq!(document.patch, "unchanged original patch");
        assert_eq!(document.lines.len(), 8);
    }

    #[test]
    fn pure_whitespace_empty_and_hunk_boundaries_are_safe() {
        let lines = vec![
            line(DiffKind::Header, "@@ -1 +1 @@", None, None),
            line(DiffKind::Deletion, "x = 1;", Some(1), None),
            line(DiffKind::Addition, "x=1;", None, Some(1)),
        ];
        assert!(
            DiffAnalysis::new(&lines)
                .view(&lines, true)
                .visible
                .is_empty()
        );
        assert!(DiffAnalysis::new(&[]).view(&[], false).hunks.is_empty());
        assert_eq!(hunk_target(&[], 0, true), None);
        assert_eq!(hunk_target(&[2, 10, 20], 0, true), Some(2));
        assert_eq!(hunk_target(&[2, 10, 20], 10, true), Some(20));
        assert_eq!(hunk_target(&[2, 10, 20], 20, true), Some(20));
        assert_eq!(hunk_target(&[2, 10, 20], 10, false), Some(2));
        assert_eq!(hunk_target(&[2, 10, 20], 0, false), Some(2));
    }
}
