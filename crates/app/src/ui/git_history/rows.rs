use super::*;

pub(super) struct Columns {
    labels: f32,
    graph: f32,
    step: f32,
    show_author: bool,
}

impl Columns {
    pub fn new(entries: &[HistoryEntry], window: &Window) -> Self {
        let narrow = window.viewport_size().width < px(1100.);
        let lanes = entries.iter().map(|e| e.graph.columns).max().unwrap_or(1);
        let graph = (lanes as f32 * 13. + 16.).clamp(50., if narrow { 104. } else { 150. });
        Self {
            labels: if narrow { 128. } else { 166. },
            graph,
            step: ((graph - 16.) / lanes.saturating_sub(1).max(1) as f32).min(13.),
            show_author: !narrow,
        }
    }

    pub fn header(&self, palette: Palette) -> Div {
        div()
            .h(px(26.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(palette.rgb(SIDE))
            .text_size(px(11.))
            .text_color(palette.rgb(MUTED))
            .child(div().w(px(self.labels)).flex_none().child("브랜치 / 태그"))
            .child(div().w(px(self.graph)).flex_none().child("그래프"))
            .child(div().flex_1().child("커밋 메시지"))
            .when(self.show_author, |r| {
                r.child(div().w(px(90.)).flex_none().child("작성자"))
            })
            .child(div().w(px(76.)).flex_none().child("날짜"))
            .child(div().w(px(82.)).flex_none().child("커밋"))
    }
}

impl Pokarak {
    pub(in crate::ui) fn hover_graph_branch(
        &mut self,
        oid: &str,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let current = self.git_history.hovered_tip.as_deref();
        if hovered && current != Some(oid) {
            self.git_history.hovered_tip = Some(oid.to_owned());
        } else if !hovered && current == Some(oid) {
            self.git_history.hovered_tip = None;
        } else {
            return;
        }
        cx.notify();
    }

    pub(in crate::ui) fn reveal_history_commit(
        &mut self,
        oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_graph_detail_collapsed(false, cx);
        self.load_history_detail(oid, 0, window, cx);
    }

    pub(super) fn commit_rows(
        &self,
        columns: Columns,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = Palette::new(cx);
        let ui = &self.git_history;
        let busy = history_disabled(self);
        let entries = ui.entries.clone();
        let selected = ui.detail.as_ref().map(|detail| detail.commit.oid.clone());
        let trace = ui
            .hovered_tip
            .as_deref()
            .and_then(|tip| BranchTrace::from_tip(&entries, tip));
        let weak = cx.entity().downgrade();
        uniform_list("history-commits", entries.len(), move |range, _, _| {
            range
                .map(|index| {
                    let entry = &entries[index];
                    let commit = &entry.commit;
                    let oid = commit.oid.clone();
                    let color = track_color(entry.graph.track, palette);
                    let refs = &commit.refs;
                    let all_refs = refs
                        .iter()
                        .map(|r| reference_name(r))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let label = format!(
                        "{}  {}  {}  {}  {}",
                        short(&oid),
                        visible(&commit.subject),
                        visible(&commit.author),
                        commit.date,
                        all_refs
                    );
                    let badge = refs
                        .iter()
                        .find(|r| r.starts_with("refs/heads/"))
                        .or_else(|| refs.first());
                    let refs_cell = div()
                        .w(px(columns.labels))
                        .min_w_0()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .when_some(badge, |cell, reference| {
                            let target = weak.clone();
                            let tip = oid.clone();
                            let titles = all_refs.clone();
                            cell.child(
                                div()
                                    .id(("graph-ref", index))
                                    .min_w_0()
                                    .px_1()
                                    .rounded(px(3.))
                                    .bg(color.opacity(0.15))
                                    .text_color(color)
                                    .text_size(px(11.))
                                    .text_ellipsis()
                                    .child(visible(reference_name(reference)))
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(titles.clone())
                                            .build(window, cx)
                                    })
                                    .on_hover(move |hovered, _, cx| {
                                        let _ = target.update(cx, |app, cx| {
                                            app.hover_graph_branch(&tip, *hovered, cx)
                                        });
                                    }),
                            )
                            .when(refs.len() > 1, |cell| {
                                cell.child(
                                    div()
                                        .flex_none()
                                        .text_color(palette.rgb(MUTED))
                                        .text_size(px(10.))
                                        .child(format!("+{}", refs.len() - 1)),
                                )
                            })
                        });
                    let target = weak.clone();
                    let related = trace.as_ref().is_none_or(|path| path.row(index).node);
                    Button::new(("history-row", index))
                        .ghost()
                        .small()
                        .w_full()
                        .h(px(30.))
                        .px_0()
                        .selected(selected.as_ref() == Some(&oid))
                        .disabled(busy)
                        .tooltip(label.clone())
                        .accessibility_label(label)
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .min_w_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(refs_cell)
                                .child(graph_view(
                                    entry.graph.clone(),
                                    columns.graph,
                                    columns.step,
                                    index,
                                    trace.as_ref(),
                                    palette,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_ellipsis()
                                        .text_color(palette.rgb(if related { TEXT } else { MUTED }))
                                        .child(visible(&commit.subject)),
                                )
                                .when(columns.show_author, |r| {
                                    r.child(metadata_cell(90., visible(&commit.author), palette))
                                })
                                .child(metadata_cell(
                                    76.,
                                    commit.date.get(..10).unwrap_or(&commit.date).to_owned(),
                                    palette,
                                ))
                                .child(
                                    metadata_cell(82., short(&oid).to_owned(), palette)
                                        .font_family("JetBrains Mono"),
                                ),
                        )
                        .on_click(move |_, w, cx| {
                            let _ = target.update(cx, |app, cx| {
                                app.reveal_history_commit(oid.clone(), w, cx)
                            });
                        })
                })
                .collect()
        })
        .track_scroll(&ui.scroll)
        .flex_1()
        .min_h_0()
        .w_full()
    }
}

fn metadata_cell(width: f32, text: String, palette: Palette) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .text_size(px(11.))
        .text_color(palette.rgb(MUTED))
        .text_ellipsis()
        .child(text)
}
