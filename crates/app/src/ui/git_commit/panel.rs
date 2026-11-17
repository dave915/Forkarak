//! The graph and the standalone dialog use the same file lists, diff and composer.
use super::*;

impl Pokarak {
    pub(in crate::ui) fn render_inline_commit(
        &self,
        _: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.commit_panel(&cx.entity().downgrade(), true, cx)
            .into_any_element()
    }

    pub(super) fn commit_panel(&self, weak: &WeakEntity<Self>, inline: bool, cx: &App) -> Div {
        let palette = Palette::new(cx);
        let root = self.commit_ui.root.as_deref().unwrap_or(Path::new(""));
        let snapshot = self.commit_ui.snapshot.as_deref();
        let branch = snapshot.map(|s| {
            s.branch
                .as_deref()
                .map(|n| n.strip_prefix("refs/heads/").unwrap_or(n))
                .unwrap_or("detached HEAD")
        });
        let submit = weak.clone();
        let pane = div()
            .flex()
            .min_h_0()
            .min_w_0()
            .gap_2()
            .when(inline, |d| d.flex_1())
            .when(!inline, |d| d.h(ui_px(236.)))
            .child(
                div()
                    .v_flex()
                    .min_h_0()
                    .w(ui_px(if inline { 210. } else { 270. }))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(palette.rgb(LINE))
                    .pr_2()
                    .gap_1()
                    .child(self.stage_group(weak, Side::Unstaged, cx))
                    .child(self.stage_group(weak, Side::Staged, cx)),
            )
            .child(self.index_preview_panel(weak, cx));
        let mut panel = div()
            .v_flex()
            .min_h_0()
            .w_full()
            .gap_1()
            .when(inline, |d| d.h_full().bg(palette.rgb(BG)).px_2().py_1())
            .key_context("CommitDialog")
            .on_action(move |_: &SubmitCommit, w, cx| {
                let _ = submit.update(cx, |app, cx| {
                    app.submit_index_commit(CommitMode::New, w, cx)
                });
            })
            .on_action(|_: &FindDefinition, _, cx| cx.stop_propagation())
            .on_action(|_: &FindReferences, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex_none()
                    .text_size(ui_text(12.))
                    .text_ellipsis()
                    .child(match (inline, branch) {
                        (true, Some(branch)) => format!("미커밋 변경  ·  {branch}"),
                        (true, None) => "미커밋 변경".into(),
                        (false, Some(branch)) => format!("{branch}  ·  {}", root.display()),
                        (false, None) => root.display().to_string(),
                    }),
            )
            .child(pane);
        let feedback = self.commit_ui.error.as_deref().or(self.error.as_deref());
        let message = feedback.or_else(|| {
            if self.git_is_busy() {
                Some(self.status.as_str())
            } else if self.commit_ui.progress.is_some() {
                Some("진행 중인 Git 작업은 작업 상태의 계속 / 중단을 사용하세요. Stage / Unstage는 사용할 수 있습니다.")
            } else if snapshot.is_some_and(|s| s.has_conflicts) {
                Some("미해결 충돌이 있습니다. Stash / 충돌 해결에서 먼저 해결하세요.")
            } else {
                None
            }
        });
        if let Some(text) = message {
            panel = panel.child(
                div()
                    .id("commit-feedback")
                    .max_h(ui_px(42.))
                    .flex_none()
                    .overflow_y_scroll()
                    .text_size(ui_text(11.))
                    .text_color(palette.rgb(if feedback.is_some() { RED } else { MUTED }))
                    .child(text.to_owned()),
            );
        }
        let disabled = self.commit_disabled();
        let actions = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .when(!inline, |bar| {
                bar.child(commit_button(
                    weak,
                    "commit-close",
                    "닫기",
                    false,
                    |app, w, cx| {
                        app.close_commit_view(cx);
                        w.close_dialog(cx);
                    },
                ))
            })
            .child(commit_button(
                weak,
                "commit-amend",
                "이전 커밋 수정…",
                disabled
                    || self.commit_ui.progress.is_some()
                    || snapshot.is_none_or(|s| s.has_conflicts || s.previous.is_none()),
                |app, w, cx| app.confirm_index_amend(w, cx),
            ))
            .child(
                commit_button(
                    weak,
                    "commit-create",
                    "Staged 커밋",
                    disabled
                        || self.commit_ui.progress.is_some()
                        || snapshot.is_none_or(|s| s.has_conflicts || s.staged.is_empty()),
                    |app, w, cx| app.submit_index_commit(CommitMode::New, w, cx),
                )
                .primary()
                .tooltip_with_action("Staged 변경 커밋", &SubmitCommit, None),
            );
        let mut composer = div()
            .flex_none()
            .gap_2()
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .pt_1()
            .when(inline, |d| d.flex().items_center())
            .when(!inline, |d| d.v_flex());
        if let Some(editor) = self.commit_ui.messages.get(root) {
            let message = Editor::new(editor)
                .aria_label("커밋 메시지")
                .disabled(self.git_is_busy())
                .h(ui_px(if inline { 42. } else { 62. }))
                .w_full()
                .font_family("IBM Plex Sans")
                .text_size(ui_text(13.));
            composer = composer.child(
                div()
                    .v_flex()
                    .min_w_0()
                    .gap_1()
                    .when(inline, |d| d.flex_1())
                    .child(
                        div()
                            .text_size(ui_text(11.))
                            .text_color(palette.rgb(MUTED))
                            .child("커밋 메시지"),
                    )
                    .child(message),
            );
        }
        panel.child(composer.child(div().flex().justify_end().child(actions)))
    }

    fn stage_group(&self, weak: &WeakEntity<Self>, side: Side, cx: &App) -> Div {
        let unstaged = side == Side::Unstaged;
        let snapshot = self.commit_ui.snapshot.clone();
        let files = |s: &IndexSnapshot| {
            if unstaged {
                s.unstaged.len()
            } else {
                s.staged.len()
            }
        };
        let count = snapshot.as_deref().map_or(0, files);
        let selected_count = self.commit_ui.files.count(side);
        let disabled = self.commit_disabled();
        let palette = Palette::new(cx);
        let mut group = div()
            .v_flex()
            .min_h_0()
            .when(count > 0, |d| d.flex_1())
            .when(count == 0, |d| d.flex_none())
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(ui_text(12.))
                            .text_color(palette.rgb(TEXT))
                            .child(format!(
                                "{}  {count}",
                                if unstaged { "작업 파일" } else { "Staged" }
                            )),
                    )
                    .child(commit_button(
                        weak,
                        if unstaged { "stage-all" } else { "unstage-all" },
                        if unstaged {
                            "모두 Stage"
                        } else {
                            "모두 Unstage"
                        },
                        disabled || count == 0,
                        move |app, w, cx| app.change_commit_index(unstaged, StageScope::All, w, cx),
                    )),
            )
            .when(count > 0, |group| {
                group.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .text_size(ui_text(11.))
                                .child(format!("{selected_count}개 선택")),
                        )
                        .child(commit_button(
                            weak,
                            if unstaged {
                                "stage-clear-selection"
                            } else {
                                "unstage-clear-selection"
                            },
                            "해제",
                            disabled || selected_count == 0,
                            |app, _, cx| {
                                app.commit_ui.files.clear();
                                cx.notify();
                            },
                        ))
                        .child(commit_button(
                            weak,
                            if unstaged {
                                "stage-selected"
                            } else {
                                "unstage-selected"
                            },
                            if unstaged {
                                "선택 Stage"
                            } else {
                                "선택 Unstage"
                            },
                            disabled || selected_count == 0,
                            move |app, w, cx| app.change_selected_commit_files(side, w, cx),
                        )),
                )
            });
        if let Some(snapshot) = snapshot.filter(|_| count > 0) {
            let target = weak.clone();
            let selected = self.commit_ui.files.clone();
            let rows = uniform_list(
                if unstaged {
                    "unstaged-files"
                } else {
                    "staged-files"
                },
                count,
                move |range, _, cx| {
                    let files = if unstaged {
                        &snapshot.unstaged
                    } else {
                        &snapshot.staged
                    };
                    range
                        .map(|index| {
                            let file = &files[index];
                            let path = file.path.clone();
                            let paths: Vec<PathBuf> = file
                                .old_path
                                .iter()
                                .cloned()
                                .chain(std::iter::once(path.clone()))
                                .collect();
                            let name = display_path(&path);
                            let tooltip = file
                                .old_path
                                .as_ref()
                                .map(|old| format!("{} → {name}", display_path(old)))
                                .unwrap_or_else(|| name.clone());
                            let (status, _) = file_status(&file.status);
                            let select = target.clone();
                            let toggle_target = target.clone();
                            let toggle_path = path.clone();
                            let click_snapshot = snapshot.clone();
                            let toggle_snapshot = snapshot.clone();
                            let write_snapshot = snapshot.clone();
                            let marked = selected.contains(side, &path);
                            div()
                                .flex()
                                .w_full()
                                .h(ui_px(24.))
                                .items_center()
                                .gap_1()
                                .child(
                                    theme::action_button(
                                        (if unstaged { "work-mark" } else { "index-mark" }, index),
                                        disabled,
                                    )
                                    .label(if marked { "✓" } else { "□" })
                                    .small()
                                    .accessibility_label(format!(
                                        "{} {} 선택 전환",
                                        if unstaged { "작업 파일" } else { "Staged" },
                                        display_path(&path)
                                    ))
                                    .selected(marked)
                                    .tooltip(
                                        "파일 선택 전환 · Cmd 클릭: 추가/해제 · Shift 클릭: 범위",
                                    )
                                    .on_click(
                                        move |_, w, cx| {
                                            let _ = toggle_target.update(cx, |app, cx| {
                                                app.select_commit_file(
                                                    &toggle_snapshot,
                                                    toggle_path.clone(),
                                                    side,
                                                    selection::Gesture::Toggle,
                                                    w,
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                )
                                .child(
                                    file_icons::changed_file_button(
                                        (if unstaged { "work-file" } else { "index-file" }, index),
                                        &path,
                                        status,
                                        name,
                                        cx,
                                    )
                                    .small()
                                    .disabled(disabled)
                                    .cursor(if disabled {
                                        CursorStyle::Arrow
                                    } else {
                                        CursorStyle::PointingHand
                                    })
                                    .selected(marked)
                                    .tooltip(tooltip)
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .on_click(
                                        move |event, w, cx| {
                                            let _ = select.update(cx, |app, cx| {
                                                let modifiers = event.modifiers();
                                                app.select_commit_file(
                                                    &click_snapshot,
                                                    path.clone(),
                                                    side,
                                                    selection::Gesture::from_modifiers(
                                                        modifiers.platform,
                                                        modifiers.shift,
                                                    ),
                                                    w,
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                )
                                .child(commit_button(
                                    &target,
                                    (
                                        if unstaged {
                                            "stage-file"
                                        } else {
                                            "unstage-file"
                                        },
                                        index,
                                    ),
                                    if unstaged { "Stage" } else { "Unstage" },
                                    disabled,
                                    move |app, w, cx| {
                                        if !app.commit_snapshot_current(&write_snapshot) {
                                            return;
                                        }
                                        app.change_commit_index(
                                            unstaged,
                                            StageScope::Files(paths.clone()),
                                            w,
                                            cx,
                                        )
                                    },
                                ))
                        })
                        .collect()
                },
            )
            .flex_1()
            .min_h_0()
            .w_full();
            group = group.child(rows);
        }
        group
    }

    fn index_preview_panel(&self, weak: &WeakEntity<Self>, cx: &App) -> Div {
        let disabled = self.commit_disabled();
        let palette = Palette::new(cx);
        let side = self.commit_ui.side;
        let mut toolbar = div().flex().flex_none().items_center().gap_1().child(
            div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .text_size(ui_text(11.))
                .text_color(palette.rgb(MUTED))
                .child(side.map_or_else(String::new, |s| {
                    let pair = if s == Side::Unstaged {
                        "Index → 작업 파일"
                    } else {
                        "HEAD → Index"
                    };
                    self.commit_ui.selected.as_deref().map_or_else(
                        || pair.into(),
                        |p| format!("{pair}  ·  {}", display_path(p)),
                    )
                })),
        );
        toolbar = toolbar
            .child(commit_button(
                weak,
                "index-split",
                if self.commit_ui.split.is_some() {
                    "줄 선택"
                } else {
                    "좌우"
                },
                disabled || self.commit_ui.partial.is_none(),
                |app, _, cx| app.toggle_partial_split(cx),
            ))
            .child(commit_button(
                weak,
                "apply-selected-lines",
                format!(
                    "선택 {}줄 {}",
                    self.commit_ui.marked.len(),
                    if side == Some(Side::Staged) {
                        "Unstage"
                    } else {
                        "Stage"
                    }
                ),
                disabled || self.commit_ui.marked.is_empty() || self.commit_ui.partial.is_none(),
                |app, w, cx| app.apply_line_selection(w, cx),
            ))
            .child(commit_button(
                weak,
                "index-refresh",
                "새로고침",
                self.git_is_busy() || self.commit_ui.reading,
                |app, w, cx| app.read_commit_index(w, cx),
            ));
        let preview = if let Some(split) = &self.commit_ui.split {
            split.clone().into_any_element()
        } else if let Some(partial) = &self.commit_ui.partial {
            self.partial_rows(partial.clone(), weak.clone(), palette)
        } else if let Some(editor) = &self.commit_ui.preview {
            Editor::new(editor)
                .aria_label("Git 변경 미리보기")
                .readonly(true)
                .h_full()
                .w_full()
                .into_any_element()
        } else {
            let text = if self.commit_ui.reading || self.commit_ui.selected.is_some() {
                "변경을 읽는 중…"
            } else if self
                .commit_ui
                .snapshot
                .as_ref()
                .is_some_and(|s| s.staged.is_empty() && s.unstaged.is_empty())
            {
                "미커밋 변경이 없습니다."
            } else {
                "파일을 선택하면 변경 내용을 표시합니다."
            };
            div().p_2().child(commit_note(text)).into_any_element()
        };
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .v_flex()
            .gap_1()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(preview),
            )
    }
}
