use super::*;
use gpui_kit::component::resizable::v_resizable;

impl Pokarak {
    pub(in crate::ui) fn leave_git_graph(&mut self) {
        self.git_history.active = false;
        self.git_history.request += 1;
        self.git_history.loading = false;
    }

    pub(in crate::ui) fn reset_git_history(&mut self) {
        self.git_history.reset(true);
    }

    pub(in crate::ui) fn sync_git_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.git_history.active;
        self.git_history.reset(active);
        if active {
            self.open_git_history(window, cx);
        }
    }

    pub(in crate::ui) fn show_code_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_history.active {
            self.leave_git_graph();
            if let Some(path) = self.selected() {
                self.open_file(path, false, None, window, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::ui) fn toggle_history_code(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .git_history
            .document
            .as_ref()
            .is_some_and(|doc| !doc.binary)
        {
            self.git_history.code = !self.git_history.code;
            self.update_history_editor(window, cx);
        }
    }

    pub(in crate::ui) fn render_workspace_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        div()
            .h(px(38.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(palette.rgb(LINE))
            .bg(palette.rgb(SIDE))
            .child(
                Button::new("view-graph")
                    .label("Git 그래프")
                    .ghost()
                    .small()
                    .selected(self.git_history.active)
                    .disabled(self.loading || self.git_is_busy())
                    .tooltip("커밋 그래프  ⌘⌥H")
                    .on_click(cx.listener(|app, _, window, cx| app.open_git_history(window, cx))),
            )
            .child(
                Button::new("view-code")
                    .label("코드 보기")
                    .ghost()
                    .small()
                    .selected(!self.git_history.active)
                    .disabled(self.loading)
                    .on_click(cx.listener(|app, _, window, cx| app.show_code_view(window, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("graph-commit")
                    .label("Stage / 커밋…")
                    .small()
                    .disabled(self.loading || self.git_is_busy())
                    .on_click(cx.listener(|app, _, window, cx| app.open_git_commit(window, cx))),
            )
            .into_any_element()
    }

    pub(in crate::ui) fn render_git_graph(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let ui = &self.git_history;
        let busy = history_disabled(self);
        let weak = cx.entity().downgrade();
        let graph_width = (ui
            .entries
            .iter()
            .map(|entry| entry.graph.columns)
            .max()
            .unwrap_or(1)
            * 14
            + 20)
            .max(72) as f32;
        let entries = ui.entries.clone();
        let selected = ui.detail.as_ref().map(|detail| detail.commit.oid.clone());
        let row_target = weak.clone();
        let list = uniform_list("history-commits", entries.len(), move |range, _, _| {
            range
                .map(|index| {
                    let entry = &entries[index];
                    let commit = &entry.commit;
                    let oid = commit.oid.clone();
                    let target = row_target.clone();
                    let label = format!(
                        "{}  {}  {}  {}  {}",
                        short(&oid),
                        visible(&commit.subject),
                        visible(&commit.author),
                        commit.date,
                        commit.refs.join(", ")
                    );
                    let mut message = div().flex_1().min_w_0().flex().items_center().gap_2();
                    for reference in commit.refs.iter().take(2) {
                        let name = reference
                            .strip_prefix("refs/heads/")
                            .or_else(|| reference.strip_prefix("refs/remotes/"))
                            .or_else(|| reference.strip_prefix("refs/tags/"))
                            .unwrap_or(reference)
                            .to_owned();
                        message = message.child(
                            div()
                                .flex_none()
                                .max_w(px(175.))
                                .px_1()
                                .rounded(px(3.))
                                .text_size(px(10.))
                                .bg(palette.rgb(0x314353))
                                .text_color(palette.rgb(BLUE))
                                .text_ellipsis()
                                .child(name),
                        );
                    }
                    message = message.child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .child(visible(&commit.subject)),
                    );
                    Button::new(("history-row", index))
                        .ghost()
                        .small()
                        .w_full()
                        .h(px(30.))
                        .px_0()
                        .justify_start()
                        .selected(selected.as_ref() == Some(&oid))
                        .disabled(busy)
                        .tooltip(label.clone())
                        .accessibility_label(label)
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(graph_view(entry.graph.clone(), graph_width, palette))
                                .child(message)
                                .child(
                                    div()
                                        .w(px(110.))
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(palette.rgb(MUTED))
                                        .text_ellipsis()
                                        .child(visible(&commit.author)),
                                )
                                .child(
                                    div()
                                        .w(px(85.))
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(palette.rgb(MUTED))
                                        .child(
                                            commit
                                                .date
                                                .get(..10)
                                                .unwrap_or(&commit.date)
                                                .to_owned(),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(px(98.))
                                        .flex_none()
                                        .text_size(px(11.))
                                        .font_family("JetBrains Mono")
                                        .text_color(palette.rgb(MUTED))
                                        .child(short(&oid).to_owned()),
                                ),
                        )
                        .on_click(move |_, window, cx| {
                            let _ = target.update(cx, |app, cx| {
                                app.load_history_detail(oid.clone(), 0, window, cx)
                            });
                        })
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&ui.scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        let count = self
            .snapshot()
            .map(|snapshot| snapshot.worktree.changed_files)
            .unwrap_or_default();
        let status = if self.loading || ui.loading {
            "읽는 중…".into()
        } else if ui.limit_reached {
            format!("{}개 · 표시 한도 도달", ui.entries.len())
        } else {
            format!(
                "{}개 커밋{}",
                ui.entries.len(),
                if ui.shallow { " · Shallow" } else { "" }
            )
        };
        let graph = div()
            .size_full()
            .v_flex()
            .overflow_hidden()
            .child(
                div()
                    .h(px(38.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(palette.rgb(LINE))
                    .child(div().font_weight(FontWeight::MEDIUM).child("커밋 그래프"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette.rgb(MUTED))
                            .child(status),
                    )
                    .child(div().flex_1())
                    .child(
                        history_button(
                            &weak,
                            "history-reload",
                            "새로고침",
                            busy,
                            |app, window, cx| app.sync_git_history(window, cx),
                        )
                        .ghost(),
                    )
                    .child(
                        history_button(
                            &weak,
                            "history-more",
                            "다음 100개",
                            busy || ui.cursor.is_none(),
                            |app, window, cx| app.load_history_page(false, window, cx),
                        )
                        .ghost(),
                    ),
            )
            .child(
                Button::new("graph-working-changes")
                    .ghost()
                    .small()
                    .h(px(32.))
                    .w_full()
                    .justify_start()
                    .disabled(self.loading)
                    .label(format!("●  미커밋 변경   {count}개 파일"))
                    .tooltip("현재 작업 파일의 staged·unstaged 변경 보기")
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.show_code_view(window, cx);
                        app.compare(false, window, cx);
                    })),
            )
            .child(
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
                    .child(div().w(px(graph_width)).flex_none().child("그래프"))
                    .child(div().flex_1().child("커밋 메시지 / 브랜치"))
                    .child(div().w(px(110.)).flex_none().child("작성자"))
                    .child(div().w(px(85.)).flex_none().child("날짜"))
                    .child(div().w(px(98.)).flex_none().child("커밋")),
            )
            .child(if ui.entries.is_empty() {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .text_color(palette.rgb(MUTED))
                    .child(if self.loading || ui.loading {
                        "커밋을 불러오는 중…"
                    } else if ui.error.is_some() {
                        "기록을 불러오지 못했습니다. 새로고침으로 다시 시도하세요."
                    } else {
                        "아직 커밋이 없습니다. 미커밋 변경에서 첫 커밋을 만들 수 있습니다."
                    })
                    .into_any_element()
            } else {
                list.into_any_element()
            })
            .when_some(ui.error.clone(), |view, error| {
                view.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .text_color(palette.rgb(RED))
                        .child(history_error(error)),
                )
            });
        v_resizable("graph-detail-panes")
            .child(
                resizable_panel()
                    .size(px(310.))
                    .size_range(px(150.)..px(1200.))
                    .child(graph),
            )
            .child(
                resizable_panel()
                    .size_range(px(180.)..px(1600.))
                    .child(self.render_graph_detail(cx)),
            )
            .into_any_element()
    }

    fn render_graph_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let ui = &self.git_history;
        let Some(detail) = &ui.detail else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_color(palette.rgb(MUTED))
                .child(if ui.loading {
                    "커밋을 읽는 중…"
                } else {
                    "커밋을 선택하면 변경 파일과 diff를 볼 수 있습니다."
                })
                .into_any_element();
        };
        let busy = history_disabled(self);
        let weak = cx.entity().downgrade();
        let binary = ui.document.as_ref().is_some_and(|doc| doc.binary);
        let merge = detail.commit.parents.len() > 1;
        let mut parents = div().flex().items_center().gap_1();
        if merge {
            for (index, oid) in detail.commit.parents.iter().enumerate() {
                let commit = detail.commit.oid.clone();
                parents = parents.child(
                    history_button(
                        &weak,
                        ("history-parent", index),
                        format!("부모 {}", index + 1),
                        busy,
                        move |app, window, cx| {
                            app.load_history_detail(commit.clone(), index, window, cx)
                        },
                    )
                    .ghost()
                    .selected(detail.parent.as_ref() == Some(oid))
                    .tooltip(oid.clone()),
                );
            }
        }
        let changed_files = detail.files.clone();
        let selected = ui.selected.clone();
        let target = weak.clone();
        let files = uniform_list("history-files", changed_files.len(), move |range, _, _| {
            range
                .map(|index| {
                    let file = &changed_files[index];
                    let path = file.path.clone();
                    let (status, _) = file_status(&file.status);
                    let label = format!("{status}  {}", visible(&path.to_string_lossy()));
                    history_button(
                        &target,
                        ("history-file", index),
                        label.clone(),
                        busy,
                        move |app, window, cx| app.load_history_file(path.clone(), window, cx),
                    )
                    .ghost()
                    .h(px(29.))
                    .w_full()
                    .justify_start()
                    .tooltip(label)
                    .selected(selected.as_ref() == Some(&file.path))
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&ui.files_scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        let file_pane = div()
            .size_full()
            .v_flex()
            .bg(palette.rgb(SIDE))
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .border_b_1()
                    .border_color(palette.rgb(LINE))
                    .child(format!("변경 파일  {}", detail.files.len())),
            )
            .child(files);
        let preview = if let Some(doc) = ui.document.as_ref().filter(|doc| !ui.code && self.prefs.split_diff && !doc.binary) {
            doc.split(cx)
        } else if let Some(editor) = &ui.editor {
            Editor::new(editor)
                .readonly(true)
                .bordered(false)
                .h_full()
                .w_full()
                .font_family("JetBrains Mono")
                .text_size(px(12.))
                .into_any_element()
        } else {
            div()
                .size_full()
                .p_3()
                .text_color(palette.rgb(MUTED))
                .child(if ui.loading {
                    "파일을 읽는 중…"
                } else if detail.files.is_empty() {
                    "이 부모와 비교한 파일 변경이 없습니다."
                } else {
                    "파일을 선택하세요."
                })
                .into_any_element()
        };
        let reader = div()
            .size_full()
            .v_flex()
            .overflow_hidden()
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(palette.rgb(LINE))
                    .child(
                        history_button(&weak, "history-diff", "Diff", busy, |app, window, cx| {
                            app.set_split_diff(false);
                            app.git_history.code = false;
                            app.update_history_editor(window, cx);
                        })
                        .ghost()
                        .selected(!ui.code && !self.prefs.split_diff),
                    )
                    .child(
                        history_button(&weak, "history-split", "좌우", busy || binary, |app, _, cx| {
                            app.set_split_diff(true);
                            app.git_history.code = false;
                            cx.notify();
                        }).ghost().selected(!ui.code && self.prefs.split_diff),
                    )
                    .child(
                        history_button(
                            &weak,
                            "history-code",
                            "커밋의 파일",
                            busy || binary,
                            |app, window, cx| {
                                app.git_history.code = true;
                                app.update_history_editor(window, cx);
                            },
                        )
                        .ghost()
                        .selected(ui.code),
                    )
                    .child(
                        div()
                            .ml_2()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_ellipsis()
                            .child(
                                ui.selected
                                    .as_ref()
                                    .map(|path| visible(&path.to_string_lossy()))
                                    .unwrap_or_default(),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(preview));
        div().size_full().v_flex().overflow_hidden()
            .child(div().h(px(38.)).flex_none().px_3().flex().items_center().gap_2().border_b_1().border_color(palette.rgb(LINE))
                .child(div().flex_1().min_w_0().text_ellipsis().font_weight(FontWeight::MEDIUM).child(visible(&detail.commit.subject)))
                .child(parents)
                .child(history_button(&weak, "history-pick", "Cherry-pick…", busy || merge, |app, window, cx| app.prepare_history_action(HistoryAction::CherryPick, window, cx)).ghost())
                .child(history_button(&weak, "history-revert", "Revert…", busy || merge, |app, window, cx| app.prepare_history_action(HistoryAction::Revert, window, cx)).ghost()))
            .child(div().h(px(26.)).flex_none().px_3().flex().items_center().text_size(px(11.)).text_color(palette.rgb(MUTED))
                .child(format!("{}  ·  {}  ·  {}", short(&detail.commit.oid), visible(&detail.commit.author), detail.commit.date)))
            .child(div().flex_1().min_h_0().child(h_resizable("history-file-panes")
                .child(resizable_panel().size(px(230.)).size_range(px(170.)..px(500.)).child(file_pane))
                .child(resizable_panel().child(reader))))
            .child(div().h(px(24.)).flex_none().px_3().flex().items_center().text_size(px(10.)).text_color(palette.rgb(MUTED))
                .child(if ui.document.as_ref().is_some_and(|doc| doc.truncated) { "일부 미리보기 · 2 MiB / 50,000줄 제한 · 읽기 전용" }
                    else if merge { "선택한 부모 대비 변경 · merge 커밋의 cherry-pick/revert는 외부 Git에서 실행하세요." }
                    else { "커밋 시점의 파일 · 읽기 전용" }))
            .into_any_element()
    }
}
