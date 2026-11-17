use super::*;

use super::columns::Columns;
use crate::prefs::GraphColumn;

impl Pokarak {
    pub(super) fn clear_graph_hover(&mut self) {
        self.git_history.hovered_tip = None;
        self.git_history.hover_anchor = None;
        self.git_history.hover_lookup = None;
    }

    pub(super) fn refresh_graph_hover(&mut self, cx: &mut Context<Self>) {
        let ui = &mut self.git_history;
        ui.hover_lookup = None;
        ui.hover_anchor = None;
        let Some(tip) = ui.hovered_tip.clone() else {
            return;
        };
        if ui.entries.iter().any(|row| row.commit.oid == tip) {
            ui.hover_anchor = Some(tip);
            return;
        }
        if !ui.in_neighborhood {
            return;
        }
        let Some(root) = ui.root.clone() else { return };
        let entries = ui.entries.clone();
        let executor = cx.background_executor().clone();
        ui.hover_lookup = Some(cx.spawn(async move |view, cx| {
            executor.timer(Duration::from_millis(150)).await;
            let expected_tip = tip.clone();
            let expected_root = root.clone();
            let anchor =
                executor
                    .spawn(async move {
                        history::navigation::first_visible_ancestor(&root, &tip, &entries)
                    })
                    .await;
            let _ = view.update(cx, |app, cx| {
                let ui = &mut app.git_history;
                if ui.active
                    && ui.root.as_ref() == Some(&expected_root)
                    && ui.hovered_tip.as_ref() == Some(&expected_tip)
                {
                    ui.hover_anchor = anchor.ok().flatten();
                    cx.notify();
                }
            });
        }));
    }

    pub(in crate::ui) fn hover_graph_branch(
        &mut self,
        oid: &str,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let current = self.git_history.hovered_tip.as_deref();
        if hovered && current != Some(oid) {
            self.git_history.hovered_tip = Some(oid.to_owned());
            self.refresh_graph_hover(cx);
        } else if !hovered && current == Some(oid) {
            self.clear_graph_hover();
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
        self.leave_inline_commit();
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
        let selected = ui
            .detail
            .as_ref()
            .filter(|_| !self.inline_commit_active())
            .map(|detail| detail.commit.oid.clone());
        let menu_root = ui.root.clone();
        let trace = ui.hovered_tip.as_ref().map(|_| {
            ui.hover_anchor
                .as_deref()
                .and_then(|oid| BranchTrace::from_tip(&entries, oid))
                .unwrap_or_default()
        });
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
                        .w(px(columns.width(GraphColumn::Refs)))
                        .min_w_0()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .when_some(badge, |cell, reference| {
                            let target = weak.clone();
                            let tip = oid.clone();
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
                    let button = Button::new(("history-row", index))
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
                            Columns::row()
                                .w_full()
                                .h_full()
                                .min_w_0()
                                .child(refs_cell)
                                .child(graph_view(
                                    entry.graph.clone(),
                                    columns.width(GraphColumn::Graph),
                                    columns.lane_step,
                                    index,
                                    trace.as_ref(),
                                    palette,
                                ))
                                .child(
                                    div()
                                        .w(px(columns.width(GraphColumn::Message)))
                                        .flex_none()
                                        .min_w_0()
                                        .text_ellipsis()
                                        .text_color(palette.rgb(if related { TEXT } else { MUTED }))
                                        .child(visible(&commit.subject)),
                                )
                                .child(metadata_cell(
                                    columns.width(GraphColumn::Author),
                                    visible(&commit.author),
                                    palette,
                                ))
                                .child(metadata_cell(
                                    columns.width(GraphColumn::Date),
                                    commit.date.get(..10).unwrap_or(&commit.date).to_owned(),
                                    palette,
                                ))
                                .child(
                                    metadata_cell(
                                        columns.width(GraphColumn::Hash),
                                        short(&oid).to_owned(),
                                        palette,
                                    )
                                    .font_family("JetBrains Mono"),
                                ),
                        )
                        .on_click(move |_, w, cx| {
                            let _ = target.update(cx, |app, cx| {
                                app.reveal_history_commit(oid.clone(), w, cx)
                            });
                        });
                    div()
                        .id(("commit-context", index))
                        .w_full()
                        .child(button)
                        .context_menu(commit_context_menu(
                            weak.clone(),
                            menu_root.clone(),
                            commit.clone(),
                        ))
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
