use super::*;
use gpui_kit::component::resizable::v_resizable;

impl Pokarak {
    pub(in crate::ui) fn leave_git_graph(&mut self) {
        self.leave_inline_commit();
        self.clear_graph_hover();
        if self.git_history.active {
            self.git_history.request += 1;
        }
        self.git_history.active = false;
        self.git_history.loading = false;
    }

    pub(in crate::ui) fn reset_git_history(&mut self) {
        self.leave_inline_commit();
        self.git_history.reset(true);
    }

    pub(in crate::ui) fn sync_git_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.git_history.active;
        self.git_history.reset(active);
        if active {
            self.open_git_history(window, cx);
            self.refresh_inline_commit(window, cx);
        } else {
            self.load_code_sidebar(cx);
        }
    }

    pub(in crate::ui) fn show_code_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening_path.is_some() {
            return;
        }
        let was_graph = self.git_history.active;
        if was_graph {
            self.leave_git_graph();
        }
        let needs_snapshot = self
            .current
            .as_ref()
            .and_then(|root| self.sessions.get_mut(root))
            .is_some_and(Session::prepare_code_snapshot);
        if needs_snapshot {
            if !self.loading {
                self.refresh(window, cx);
            }
        } else if was_graph && let Some(path) = self.selected() {
            self.open_file(path, false, None, window, cx);
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

    pub(in crate::ui) fn render_git_graph(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = Palette::new(cx);
        let ui = &self.git_history;
        let busy = history_disabled(self);
        let weak = cx.entity().downgrade();
        let table = self.commit_table(window, cx);
        let saved = self.session().map(|session| &session.saved);
        let collapsed = saved.is_some_and(|view| view.graph_detail_collapsed);
        let count = match self.change_count {
            Some(count) => format!("{count}개 파일"),
            None if self.count_pending => "변경 수 확인 중…".into(),
            None => "변경 수 미확인".into(),
        };
        let working_title = if self.graph_branch().is_some() {
            self.active_worktree().map(|tree| {
                format!(
                    "미커밋 변경 · {}",
                    if tree.is_detached {
                        short(&tree.head)
                    } else {
                        &tree.branch
                    }
                )
            })
        } else {
            None
        }
        .unwrap_or_else(|| "미커밋 변경".into());
        let status = if self.loading || ui.loading {
            "읽는 중…".into()
        } else if matches!(ui.next_page, NextPage::Loading) {
            "다음 커밋을 읽는 중…".into()
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
                    .id("graph-controls")
                    .overflow_x_scroll()
                    .h(ui_px(38.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(palette.rgb(LINE))
                    .child(self.branch_filter_control(cx))
                    .child(
                        div()
                            .text_size(ui_text(11.))
                            .text_color(palette.rgb(MUTED))
                            .child(status),
                    )
                    .child(div().flex_1())
                    .child(self.graph_search_control(cx))
                    .child(
                        history_button(
                            &weak,
                            "toggle-graph-detail",
                            if collapsed {
                                "상세 펼치기"
                            } else {
                                "상세 접기"
                            },
                            false,
                            move |app, _, cx| app.set_graph_detail_collapsed(!collapsed, cx),
                        )
                        .ghost(),
                    )
                    .child(
                        history_button(
                            &weak,
                            "history-reload",
                            "새로고침",
                            busy,
                            |app, window, cx| app.refresh(window, cx),
                        )
                        .ghost(),
                    ),
            )
            .child(
                Button::new("graph-working-changes")
                    .ghost()
                    .small()
                    .h(ui_px(32.))
                    .w_full()
                    .selected(self.inline_commit_active())
                    .disabled(self.loading || self.git_is_busy())
                    .accessibility_label(format!("{working_title} {count}"))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::GitCommitHorizontal)
                                    .size(ui_px(15.))
                                    .text_color(palette.rgb(BLUE)),
                            )
                            .child(working_title)
                            .child(div().text_color(palette.rgb(MUTED)).child(count)),
                    )
                    .tooltip("변경 사항 검토 · 스테이징 · 커밋")
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.set_graph_detail_collapsed(false, cx);
                        app.open_inline_commit(window, cx);
                    })),
            )
            .child(table)
            .when_some(
                match &ui.next_page {
                    NextPage::Failed(error) => Some(error.clone()),
                    _ => None,
                },
                |view, error| {
                    view.child(
                        div()
                            .flex_none()
                            .px_3()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_ellipsis()
                                    .text_color(palette.rgb(RED))
                                    .child(format!("다음 커밋을 읽을 수 없습니다 · {error}")),
                            )
                            .child(
                                history_button(
                                    &weak,
                                    "history-page-retry",
                                    "다시 시도",
                                    self.loading || self.git_is_busy(),
                                    |app, _, cx| {
                                        if matches!(app.git_history.next_page, NextPage::Failed(_))
                                        {
                                            app.git_history.next_page = NextPage::Idle;
                                            app.load_next_history_page(cx);
                                        }
                                    },
                                )
                                .ghost(),
                            ),
                    )
                },
            )
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
        if collapsed {
            return graph.into_any_element();
        }
        let graph_height = saved
            .and_then(|s| s.graph_height)
            .map(ui_px)
            .unwrap_or(window.viewport_size().height * 0.38)
            .clamp(
                ui_px(150.),
                (window.viewport_size().height - ui_px(245.)).max(ui_px(150.)),
            );
        let resize = weak.clone();
        let resize_root = self.current.clone();
        v_resizable(SharedString::from(format!(
            "graph-detail-panes:{resize_root:?}:{}",
            self.prefs.display.ui_zoom_percent
        )))
        .on_resize(move |state, _, cx| {
            if let Some(height) = state.read(cx).sizes().first().copied() {
                let _ = resize.update(cx, |app, _| {
                    if let Some(session) =
                        resize_root.as_ref().and_then(|r| app.sessions.get_mut(r))
                    {
                        session.saved.graph_height = Some(theme::logical_px(height));
                    }
                });
            }
        })
        .child(
            resizable_panel()
                .size(graph_height)
                .size_range(ui_px(150.)..ui_px(1200.))
                .child(graph),
        )
        .child(
            resizable_panel()
                .size_range(ui_px(180.)..ui_px(1600.))
                .child(if self.inline_commit_active() {
                    self.render_inline_commit(window, cx)
                } else {
                    self.render_graph_detail(cx)
                }),
        )
        .into_any_element()
    }

    pub(super) fn set_graph_detail_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if let Some(session) = self.current.as_ref().and_then(|r| self.sessions.get_mut(r)) {
            session.saved.graph_detail_collapsed = collapsed;
        }
        cx.notify();
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
        let files_width = self
            .session()
            .and_then(|s| s.saved.graph_files_width)
            .map(ui_px)
            .unwrap_or(ui_px(230.));
        let busy = history_disabled(self);
        let weak = cx.entity().downgrade();
        let resize = weak.clone();
        let resize_root = self.current.clone();
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
        let files = uniform_list("history-files", changed_files.len(), move |range, _, cx| {
            range
                .map(|index| {
                    let file = &changed_files[index];
                    let path = file.path.clone();
                    let (status, _) = file_status(&file.status);
                    let display_path = visible(&path.to_string_lossy());
                    let target = target.clone();
                    file_icons::changed_file_button(
                        ("history-file", index),
                        &path,
                        status,
                        display_path,
                        cx,
                    )
                    .small()
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        let _ = target.update(cx, |app, cx| {
                            app.load_history_file(path.clone(), window, cx)
                        });
                    })
                    .ghost()
                    .h(ui_px(29.))
                    .w_full()
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
                    .h(ui_px(34.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(ui_text(12.))
                    .border_b_1()
                    .border_color(palette.rgb(LINE))
                    .child(format!("변경 파일  {}", detail.files.len())),
            )
            .child(files);
        let preview = if let Some(doc) = ui
            .document
            .as_ref()
            .filter(|doc| !ui.code && self.prefs.split_diff && !doc.binary)
        {
            doc.split(self, cx)
        } else if let Some(editor) = &ui.editor {
            Editor::new(editor)
                .readonly(true)
                .bordered(false)
                .h_full()
                .w_full()
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
                    .id("history-file-controls")
                    .overflow_x_scroll()
                    .h(ui_px(34.))
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
                        history_button(
                            &weak,
                            "history-split",
                            "좌우",
                            busy || binary,
                            |app, _, cx| {
                                app.set_split_diff(true);
                                app.git_history.code = false;
                                cx.notify();
                            },
                        )
                        .ghost()
                        .selected(!ui.code && self.prefs.split_diff),
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
                    .when_some(ui.selected.as_ref(), |bar, path| {
                        let deleted = detail
                            .files
                            .iter()
                            .any(|f| &f.path == path && f.status == FileStatus::Deleted);
                        bar.child(self.file_inspection_buttons(
                            file_inspector::Target {
                                path: path.clone(),
                                revision: Some(detail.commit.oid.clone()),
                                blame_revision: if deleted {
                                    detail.parent.clone()
                                } else {
                                    Some(detail.commit.oid.clone())
                                },
                            },
                            cx,
                        ))
                    })
                    .child(
                        div()
                            .ml_2()
                            .flex_1()
                            .min_w_0()
                            .text_size(ui_text(11.))
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
            .child(div().h(ui_px(38.)).flex_none().px_3().flex().items_center().gap_2().border_b_1().border_color(palette.rgb(LINE))
                .child(div().flex_1().min_w_0().text_ellipsis().font_weight(FontWeight::MEDIUM).child(visible(&detail.commit.subject)))
                .child(parents))
            .child(div().h(ui_px(26.)).flex_none().px_3().flex().items_center().text_size(ui_text(11.)).text_color(palette.rgb(MUTED))
                .child(format!("{}  ·  {}  ·  {}", short(&detail.commit.oid), visible(&detail.commit.author), detail.commit.date)))
            .child(div().flex_1().min_h_0().child(h_resizable(SharedString::from(format!("history-file-panes:{resize_root:?}:{}", self.prefs.display.ui_zoom_percent)))
                .on_resize(move |state,_,cx| {
                    if let Some(width) = state.read(cx).sizes().first().copied() {
                        let _=resize.update(cx, |app,_| {
                            if let Some(session)=resize_root.as_ref().and_then(|r|app.sessions.get_mut(r)) {
                                session.saved.graph_files_width=Some(theme::logical_px(width));
                            }
                        });
                    }
                })
                .child(resizable_panel().size(files_width).size_range(ui_px(170.)..ui_px(500.)).child(file_pane))
                .child(resizable_panel().child(reader))))
            .child(div().h(ui_px(24.)).flex_none().px_3().flex().items_center().text_size(ui_text(10.)).text_color(palette.rgb(MUTED))
                .child(if ui.document.as_ref().is_some_and(|doc| doc.truncated) { "일부 미리보기 · 2 MiB / 50,000줄 제한 · 읽기 전용" }
                    else if merge { "선택한 부모 대비 변경 · merge 커밋의 cherry-pick/revert는 외부 Git에서 실행하세요." }
                    else { "커밋 시점의 파일 · 읽기 전용" }))
            .into_any_element()
    }
}
