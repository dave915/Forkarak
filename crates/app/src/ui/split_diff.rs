//! Align both halves of a change on a single virtualized vertical list.
use super::*;
use pokarak_core::DiffLine;
use std::{cell::OnceCell, ops::Deref};

pub(super) struct ReviewDiff {
    document: Arc<DiffDocument>,
    split: OnceCell<Entity<SplitDiff>>,
}

impl From<DiffDocument> for ReviewDiff {
    fn from(document: DiffDocument) -> Self {
        Self {
            document: Arc::new(document),
            split: OnceCell::new(),
        }
    }
}

impl Deref for ReviewDiff {
    type Target = DiffDocument;
    fn deref(&self) -> &DiffDocument {
        &self.document
    }
}

impl ReviewDiff {
    pub(super) fn split(&self, cx: &mut Context<Pokarak>) -> AnyElement {
        if self.document.lines.is_empty() {
            let text = if self.document.patch.is_empty() {
                "텍스트 변경이 없습니다.".into()
            } else {
                self.document.patch.clone()
            };
            return div()
                .p_3()
                .text_size(px(12.))
                .child(text)
                .into_any_element();
        }
        self.split
            .get_or_init(|| cx.new(|_| SplitDiff::new(self.document.clone())))
            .clone()
            .into_any_element()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Header(usize),
    Sides(Option<usize>, Option<usize>),
}

fn align(lines: &[DiffLine]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(lines.len());
    let mut pos = 0;
    while pos < lines.len() {
        match lines[pos].kind {
            DiffKind::Context => {
                rows.push(Row::Sides(Some(pos), Some(pos)));
                pos += 1;
            }
            DiffKind::Header => {
                rows.push(Row::Header(pos));
                pos += 1;
            }
            _ => {
                let old = pos;
                while pos < lines.len() && lines[pos].kind == DiffKind::Deletion {
                    pos += 1;
                }
                let new = pos;
                while pos < lines.len() && lines[pos].kind == DiffKind::Addition {
                    pos += 1;
                }
                let len = (new - old).max(pos - new);
                rows.extend((0..len).map(|i| {
                    Row::Sides(
                        (old + i < new).then_some(old + i),
                        (new + i < pos).then_some(new + i),
                    )
                }));
            }
        }
    }
    rows
}

struct SplitDiff {
    doc: Arc<DiffDocument>,
    rows: Arc<[Row]>,
    scroll: UniformListScrollHandle,
    horizontal: f32,
    max_columns: usize,
}

impl SplitDiff {
    fn new(doc: Arc<DiffDocument>) -> Self {
        let rows = align(&doc.lines).into();
        let max_columns = doc
            .lines
            .iter()
            .filter(|l| l.kind != DiffKind::Header)
            .map(|l| {
                l.text
                    .chars()
                    .map(|c| {
                        if c == '\t' {
                            4
                        } else if c.is_ascii() {
                            1
                        } else {
                            2
                        }
                    })
                    .sum()
            })
            .max()
            .unwrap_or(0);
        Self {
            doc,
            rows,
            scroll: UniformListScrollHandle::new(),
            horizontal: 0.,
            max_columns,
        }
    }
}

fn cell(line: Option<&DiffLine>, left: bool, offset: f32, palette: Palette) -> Div {
    let (bg, ink) = match line.map(|l| &l.kind) {
        Some(DiffKind::Deletion) => (0x3c2a30, RED),
        Some(DiffKind::Addition) => (0x24382f, GREEN),
        None => (SIDE, MUTED),
        _ => (BG, TEXT),
    };
    let number = line.and_then(|l| if left { l.old_line } else { l.new_line });
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .bg(palette.rgb(bg))
        .when(left, |d| d.border_r_1().border_color(palette.rgb(LINE)))
        .child(
            div()
                .w(px(42.))
                .flex_none()
                .pr_2()
                .text_right()
                .text_color(palette.rgb(MUTED))
                .child(number.map(|n| n.to_string()).unwrap_or_default()),
        )
        .child(
            div().flex_1().min_w_0().overflow_hidden().child(
                div()
                    .ml(px(-offset))
                    .whitespace_nowrap()
                    .text_color(palette.rgb(ink))
                    .child(
                        line.map(|l| l.text.replace('\t', "    "))
                            .unwrap_or_default(),
                    ),
            ),
        )
}

impl Render for SplitDiff {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let rows = self.rows.clone();
        let doc = self.doc.clone();
        let offset = self.horizontal;
        let header = |title| div().flex_1().min_w_0().px_2().child(title);
        div()
            .size_full()
            .v_flex()
            .overflow_hidden()
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .bg(palette.rgb(SIDE))
                    .text_color(palette.rgb(MUTED))
                    .child(header("변경 전"))
                    .child(header("변경 후")),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(20.));
                let dx = if event.modifiers.shift && delta.x == px(0.) {
                    delta.y
                } else {
                    delta.x
                };
                if dx != px(0.) {
                    let width = f32::from(this.scroll.0.borrow().base_handle.bounds().size.width);
                    let max = (this.max_columns as f32 * 7.3 - (width / 2. - 42.)).max(0.);
                    this.horizontal = (this.horizontal - f32::from(dx)).clamp(0., max);
                    cx.notify();
                }
            }))
            .child(
                uniform_list("split-diff-lines", rows.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let row = div()
                                .w_full()
                                .h(px(21.))
                                .flex()
                                .items_center()
                                .font_family("JetBrains Mono")
                                .text_size(px(12.));
                            match rows[i] {
                                Row::Header(ix) => row
                                    .bg(palette.rgb(PANEL))
                                    .text_color(palette.rgb(BLUE))
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(doc.lines[ix].text.clone()),
                                Row::Sides(l, r) => row
                                    .child(cell(l.map(|i| &doc.lines[i]), true, offset, palette))
                                    .child(cell(r.map(|i| &doc.lines[i]), false, offset, palette)),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0()
                .w_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Row, align};
    use pokarak_core::{DiffKind::*, DiffLine};
    #[test]
    fn aligns_replacements_without_crossing_hunks_or_context() {
        let input = [
            Header, Deletion, Deletion, Addition, Context, Addition, Header, Deletion,
        ];
        let lines = input
            .into_iter()
            .map(|kind| DiffLine {
                kind,
                old_line: None,
                new_line: None,
                text: String::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            align(&lines),
            vec![
                Row::Header(0),
                Row::Sides(Some(1), Some(3)),
                Row::Sides(Some(2), None),
                Row::Sides(Some(4), Some(4)),
                Row::Sides(None, Some(5)),
                Row::Header(6),
                Row::Sides(Some(7), None)
            ]
        );
    }
}
