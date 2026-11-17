//! Align both halves of a change on a single virtualized vertical list.
use super::{syntax::CodeSyntax, *};
use gpui_kit::{
    base::ElementExt,
    component::{ActiveTheme, resizable::resize_handle},
};
use pokarak_core::DiffLine;
use std::{cell::OnceCell, ops::Deref};

/// The two revisions may use different languages (rename) and lexical state.
#[derive(Default)]
pub(super) struct DiffSyntax {
    pub(super) old: CodeSyntax,
    pub(super) new: CodeSyntax,
}

impl DiffSyntax {
    pub(super) fn new(doc: &DiffDocument) -> Self {
        if doc.binary || doc.truncated {
            return Self::default();
        }
        Self {
            old: CodeSyntax::new(doc.old_path.as_deref().unwrap_or(&doc.path), &doc.old_text),
            new: CodeSyntax::new(&doc.path, &doc.new_text),
        }
    }

    fn source_line(&self, line: &DiffLine, before: bool) -> Option<(&CodeSyntax, usize)> {
        let (syntax, number) = if before {
            (&self.old, line.old_line)
        } else {
            (&self.new, line.new_line)
        };
        Some((syntax, number?.checked_sub(1)?))
    }

    pub(super) fn unified_line(&self, line: &DiffLine, cx: &App) -> StyledText {
        if let Some((syntax, index)) = self.source_line(line, line.new_line.is_none()) {
            syntax.styled_line(index, &line.text, false, cx)
        } else {
            StyledText::new(line.text.clone())
        }
    }
}

pub(super) struct PreparedDiff {
    document: Arc<DiffDocument>,
    syntax: Arc<DiffSyntax>,
}

impl PreparedDiff {
    pub(super) fn new(document: DiffDocument) -> Self {
        Self {
            syntax: Arc::new(DiffSyntax::new(&document)),
            document: Arc::new(document),
        }
    }
}

pub(super) struct ReviewDiff {
    pub(super) document: Arc<DiffDocument>,
    pub(super) syntax: Arc<DiffSyntax>,
    split: OnceCell<Entity<SplitDiff>>,
}

impl From<PreparedDiff> for ReviewDiff {
    fn from(prepared: PreparedDiff) -> Self {
        Self {
            document: prepared.document,
            syntax: prepared.syntax,
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
    pub(super) fn split(&self, app: &Pokarak, cx: &mut Context<Pokarak>) -> AnyElement {
        if self.document.lines.is_empty() {
            let text = if self.document.patch.is_empty() {
                "텍스트 변경이 없습니다.".into()
            } else {
                self.document.patch.clone()
            };
            return div()
                .p_3()
                .when(!self.document.patch.is_empty(), |d| {
                    d.font_family(cx.theme().mono_font_family.clone())
                        .text_size(cx.theme().mono_font_size)
                })
                .child(text)
                .into_any_element();
        }
        self.split
            .get_or_init(|| app.new_split_diff(self.document.clone(), self.syntax.clone(), cx))
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
                // Both paths already appear in the file controls; keep the first changed
                // lines visible even when the graph detail is short.
                let file_header = pos < 2
                    && (lines[pos].text.starts_with("--- ") || lines[pos].text.starts_with("+++ "));
                if !file_header {
                    rows.push(Row::Header(pos));
                }
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

pub(super) struct SplitDiff {
    doc: Arc<DiffDocument>,
    syntax: Arc<DiffSyntax>,
    rows: Arc<[Row]>,
    scroll: UniformListScrollHandle,
    horizontal: f32,
    max_columns: [usize; 2],
    number_digits: usize,
    panel_width: f32,
    owner: WeakEntity<Pokarak>,
    worktree: Option<PathBuf>,
    resizing: bool,
}

impl Pokarak {
    pub(super) fn new_split_diff(
        &self,
        doc: Arc<DiffDocument>,
        syntax: Arc<DiffSyntax>,
        cx: &mut Context<Self>,
    ) -> Entity<SplitDiff> {
        let owner = cx.weak_entity();
        let worktree = self.current.clone();
        cx.new(|_| SplitDiff::new(doc, syntax, owner, worktree))
    }
}

#[derive(Clone)]
struct DividerDrag(EntityId);

impl Render for DividerDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

fn split_ratio(preferred: f32, width: f32, min_pane: f32) -> f32 {
    if width <= 0. || !preferred.is_finite() {
        return 0.5;
    }
    let minimum = (min_pane / width).min(0.5);
    preferred.clamp(minimum, 1. - minimum)
}

fn max_offset(columns: [usize; 2], advance: f32, gutter: f32, width: f32, ratio: f32) -> f32 {
    let widths = [width * ratio, width * (1. - ratio)];
    columns
        .into_iter()
        .zip(widths)
        .map(|(count, pane)| count as f32 * advance - (pane - gutter).max(0.))
        .fold(0., f32::max)
}

impl SplitDiff {
    fn new(
        doc: Arc<DiffDocument>,
        syntax: Arc<DiffSyntax>,
        owner: WeakEntity<Pokarak>,
        worktree: Option<PathBuf>,
    ) -> Self {
        let rows = align(&doc.lines).into();
        let mut max_columns = [0; 2];
        let mut last_number = 1;
        for line in &doc.lines {
            if line.kind == DiffKind::Header {
                continue;
            }
            let width = line
                .text
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
                .sum();
            for (side, number) in [line.old_line, line.new_line].into_iter().enumerate() {
                if let Some(number) = number {
                    max_columns[side] = max_columns[side].max(width);
                    last_number = last_number.max(number);
                }
            }
        }
        Self {
            doc,
            syntax,
            rows,
            scroll: UniformListScrollHandle::new(),
            horizontal: 0.,
            max_columns,
            number_digits: last_number.ilog10() as usize + 1,
            panel_width: 0.,
            owner,
            worktree,
            resizing: false,
        }
    }

    fn preferred_ratio(&self, cx: &App) -> f32 {
        self.owner
            .read_with(cx, |app, _| {
                self.worktree
                    .as_ref()
                    .and_then(|root| app.sessions.get(root))
                    .and_then(|session| session.saved.diff_split_ratio)
            })
            .ok()
            .flatten()
            .unwrap_or(0.5)
    }

    fn resize(
        &mut self,
        event: &DragMoveEvent<DividerDrag>,
        min_pane: f32,
        cx: &mut Context<Self>,
    ) {
        if event.drag(cx).0 != cx.entity_id() {
            return;
        }
        let width = f32::from(event.bounds.size.width);
        let Some(root) = &self.worktree else { return };
        if width <= 2. * min_pane {
            return;
        }
        let x = f32::from(event.event.position.x - event.bounds.left());
        let ratio = split_ratio(x / width, width, min_pane);
        let _ = self.owner.update(cx, |app, _| {
            if let Some(session) = app.sessions.get_mut(root) {
                session.saved.diff_split_ratio = Some(ratio);
            }
        });
        self.resizing = true;
        cx.notify();
    }

    fn finish_resize(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resizing) {
            let _ = self.owner.update(cx, |app, cx| app.persist(cx));
        }
    }
}

fn cell(
    line: Option<&DiffLine>,
    before: bool,
    syntax: &DiffSyntax,
    offset: f32,
    gutter: Pixels,
    palette: Palette,
    cx: &App,
) -> Div {
    let (bg, ink) = match line.map(|l| &l.kind) {
        Some(DiffKind::Deletion) => (patch::DELETE_BG, RED),
        Some(DiffKind::Addition) => (patch::ADD_BG, GREEN),
        None => (SIDE, MUTED),
        _ => (BG, TEXT),
    };
    let source = line.and_then(|line| {
        syntax
            .source_line(line, before)
            .map(|(syntax, index)| (line, syntax, index))
    });
    let sign = match line.map(|l| &l.kind) {
        Some(DiffKind::Deletion) => " −",
        Some(DiffKind::Addition) => " +",
        _ => "  ",
    };
    div()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .bg(palette.rgb(bg))
        .child(
            div()
                .w(gutter)
                .flex_none()
                .pr_2()
                .flex()
                .justify_end()
                .text_color(palette.rgb(MUTED))
                .whitespace_nowrap()
                .when_some(source, |gutter, (_, _, index)| {
                    gutter
                        .child((index + 1).to_string())
                        .child(div().text_color(palette.rgb(ink)).child(sign))
                }),
        )
        .child(
            div().flex_1().min_w_0().overflow_hidden().child(
                div()
                    .ml(px(-offset))
                    .whitespace_nowrap()
                    .text_color(palette.rgb(TEXT))
                    .when_some(source, |body, (line, syntax, index)| {
                        body.child(syntax.styled_line(index, &line.text, true, cx))
                    }),
            ),
        )
}

impl Render for SplitDiff {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let rows = self.rows.clone();
        let doc = self.doc.clone();
        let syntax = self.syntax.clone();
        let font_size = cx.theme().mono_font_size;
        let font_family = cx.theme().mono_font_family.clone();
        let text = cx.text_system();
        let advance = text
            .advance(
                text.resolve_font(&font(font_family.clone())),
                font_size,
                '0',
            )
            .map(|size| f32::from(size.width))
            .ok()
            .filter(|w| *w > 0.)
            .unwrap_or(f32::from(font_size) * 0.61);
        let gutter = (self.number_digits + 2).max(4) as f32 * advance + f32::from(ui_px(10.));
        let min_pane = gutter + 7. * advance;
        let row_height = theme::code_row_height(font_size);
        let ratio = split_ratio(self.preferred_ratio(cx), self.panel_width, min_pane);
        let limit = max_offset(self.max_columns, advance, gutter, self.panel_width, ratio);
        self.horizontal = self.horizontal.min(limit);
        let offset = self.horizontal;
        let header = |title| {
            div()
                .min_w_0()
                .px_2()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(title)
        };
        let measured = cx.weak_entity();
        div()
            .id("split-diff")
            .relative()
            .size_full()
            .v_flex()
            .overflow_hidden()
            .on_prepaint(move |bounds, _, cx| {
                let width = f32::from(bounds.size.width);
                let _ = measured.update(cx, |this, cx| {
                    if this.panel_width != width {
                        this.panel_width = width;
                        cx.notify();
                    }
                });
            })
            .on_drag_move(cx.listener(move |this, event, _, cx| this.resize(event, min_pane, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_resize))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::finish_resize))
            .child(
                div()
                    .h(ui_px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .text_size(ui_text(11.))
                    .bg(palette.rgb(SIDE))
                    .text_color(palette.rgb(MUTED))
                    .child(
                        header("변경 전")
                            .id("split-before-header")
                            .flex_none()
                            .w(relative(ratio)),
                    )
                    .child(header("변경 후").id("split-after-header").flex_1()),
            )
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                let delta = event.delta.pixel_delta(ui_px(20.));
                let dx = if event.modifiers.shift && delta.x == px(0.) {
                    delta.y
                } else {
                    delta.x
                };
                if dx != px(0.) {
                    this.horizontal = (this.horizontal - f32::from(dx)).clamp(0., limit);
                    cx.notify();
                }
            }))
            .child(
                uniform_list("split-diff-lines", rows.len(), move |range, _, cx| {
                    range
                        .map(|i| {
                            let row = div()
                                .w_full()
                                .h(row_height)
                                .flex()
                                .items_center()
                                .font_family(font_family.clone())
                                .text_size(font_size);
                            match rows[i] {
                                Row::Header(ix) => row
                                    .bg(palette.rgb(PANEL))
                                    .text_color(palette.rgb(BLUE))
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(doc.lines[ix].text.clone()),
                                Row::Sides(l, r) => row
                                    .child(
                                        cell(
                                            l.map(|i| &doc.lines[i]),
                                            true,
                                            &syntax,
                                            offset,
                                            px(gutter),
                                            palette,
                                            cx,
                                        )
                                        .flex_none()
                                        .w(relative(ratio)),
                                    )
                                    .child(
                                        cell(
                                            r.map(|i| &doc.lines[i]),
                                            false,
                                            &syntax,
                                            offset,
                                            px(gutter),
                                            palette,
                                            cx,
                                        )
                                        .flex_1(),
                                    ),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .map(|mut list| {
                    // The shared code offset handles horizontal gestures. Keep the list
                    // from interpreting the same gesture as vertical scrolling.
                    list.style().restrict_scroll_to_axis = Some(true);
                    list
                })
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0()
                .w_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(relative(ratio))
                    .h_full()
                    .w_0()
                    .child(
                        resize_handle("split-diff-divider", Axis::Horizontal)
                            .on_drag(DividerDrag(cx.entity_id()), |drag, _, _, cx| {
                                cx.new(|_| drag.clone())
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Row, align, max_offset, split_ratio};

    #[test]
    fn rows_use_their_own_revision_and_original_file_line() {
        use super::{PreparedDiff, ReviewDiff};
        use pokarak_core::{CompareMode, diff, snapshot};
        use std::{fs, process::Command, ptr};

        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().join("release.ts");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(repo.path())
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgSign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet"]);
        let old = "/* résumé\n1\n2\n3\n4\n5\n*/\nconst n = 10;\nuse(n);\n";
        let new = old.replace("const n = 10;", "const s = \"새 버전\";\nconst n = 20;");
        fs::write(&path, old).unwrap();
        git(&["add", "--", "release.ts"]);
        git(&["commit", "-qm", "original"]);
        fs::write(path, &new).unwrap();

        let mode = CompareMode::Uncommitted;
        let state = snapshot(repo.path(), &mode).unwrap();
        let review: ReviewDiff =
            PreparedDiff::new(diff(repo.path(), &state.changes[0], &mode).unwrap()).into();
        let row = |text: &str| review.lines.iter().find(|line| line.text == text).unwrap();
        let (syntax, index) = review
            .syntax
            .source_line(row("const n = 10;"), true)
            .unwrap();
        assert!(ptr::eq(syntax, &review.syntax.old));
        assert_eq!(old.lines().nth(index), Some("const n = 10;"));
        assert!(
            review
                .syntax
                .source_line(row("const n = 10;"), false)
                .is_none()
        );
        let (syntax, index) = review
            .syntax
            .source_line(row("const n = 20;"), false)
            .unwrap();
        assert!(ptr::eq(syntax, &review.syntax.new));
        assert_eq!(new.lines().nth(index), Some("const n = 20;"));
        assert!(
            review
                .syntax
                .source_line(row("const n = 20;"), true)
                .is_none()
        );
        let context = row("use(n);");
        assert_eq!(review.syntax.source_line(context, true).unwrap().1, 8);
        assert_eq!(review.syntax.source_line(context, false).unwrap().1, 9);
    }

    #[test]
    fn divider_respects_each_pane_at_all_widths() {
        assert_eq!(split_ratio(0.7, 1000., 100.), 0.7);
        assert_eq!(split_ratio(1.2, 400., 100.), 0.75);
        assert_eq!(split_ratio(-1., 400., 100.), 0.25);
        assert_eq!(split_ratio(0.7, 175., 100.), 0.5);
        assert_eq!(split_ratio(f32::NAN, 1000., 100.), 0.5);
        assert_eq!(split_ratio(0.7, 0., 100.), 0.5);
    }

    #[test]
    fn horizontal_scroll_reaches_long_lines_on_either_side_after_resizing() {
        assert_eq!(max_offset([100, 40], 8., 40., 800., 0.75), 240.);
        assert_eq!(max_offset([40, 100], 8., 40., 800., 0.75), 640.);
        assert_eq!(max_offset([8, 8], 8., 40., 800., 0.25), 0.);
    }
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
