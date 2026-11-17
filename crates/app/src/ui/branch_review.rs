//! Explicit, read-only aggregate branch review, independent of graph filtering.
use super::*;

fn baseline_label(mode: &CompareMode) -> String {
    match mode {
        CompareMode::Uncommitted => "HEAD → 현재 파일".into(),
        CompareMode::Branch(base) => format!("공통 조상({base}, HEAD) → 현재 파일"),
    }
}

struct ComparisonRequest {
    root: PathBuf,
    generation: u64,
}

impl ComparisonRequest {
    fn accepts(&self, current: Option<&Path>, generation: u64, opening: bool) -> bool {
        current == Some(self.root.as_path()) && self.generation == generation && !opening
    }
}

impl Pokarak {
    pub(super) fn applied_comparison_label(&self) -> String {
        baseline_label(
            &self
                .session()
                .map(|session| session.saved.comparison.clone())
                .unwrap_or_default(),
        )
    }

    pub(super) fn branch_review_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.current.as_ref()?;
        if self.opening_path.is_some() {
            return None;
        }
        let branch = self
            .session()
            .is_some_and(|session| matches!(session.saved.comparison, CompareMode::Branch(_)));
        let busy = self.loading || self.git_is_busy() || self.file_loading;
        Some(
            div()
                .id("branch-review-controls")
                .flex_none()
                .min_h(ui_px(32.))
                .px_3()
                .py_1()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(Palette::new(cx).rgb(LINE))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(ui_text(11.))
                        .text_ellipsis()
                        .child(format!("코드 비교 · {}", self.applied_comparison_label())),
                )
                .child(
                    Button::new("compare-branch")
                        .label("브랜치 비교…")
                        .accessibility_label("브랜치 전체 변경 비교")
                        .tooltip(
                            "기준 브랜치와 HEAD의 공통 조상부터 현재 파일까지 코드 보기에서 비교",
                        )
                        .ghost()
                        .small()
                        .selected(branch && !self.git_history.active)
                        .disabled(busy)
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.open_branch_review(window, cx);
                        })),
                )
                .when(branch, |bar| {
                    bar.child(
                        Button::new("compare-head")
                            .label("HEAD로 돌아가기")
                            .accessibility_label("비교 기준을 HEAD로 변경")
                            .ghost()
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(|app, _, window, cx| {
                                if let Some(root) = app.current.clone() {
                                    app.apply_branch_review(
                                        root,
                                        CompareMode::Uncommitted,
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn open_branch_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.git_is_busy() || window.has_active_dialog(cx) {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        let initial = self.session().and_then(|session| {
            if let CompareMode::Branch(base) = &session.saved.comparison {
                Some(base.clone())
            } else {
                None
            }
        });
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("기준 브랜치 · 예: main, origin/main")
                .default_value(initial.unwrap_or_default())
        });
        let owner = cx.weak_entity();
        let scope = root.clone();
        let field = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = owner.clone();
            let target = scope.clone();
            let value = field.clone();
            let submit_button = owner.clone();
            let button_root = scope.clone();
            let button_input = field.clone();
            dialog
                .title("브랜치 전체 변경 비교")
                .width(ui_px(580.))
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(div().text_size(ui_text(11.)).child(scope.display().to_string()))
                        .child("기준 브랜치와 현재 HEAD의 공통 조상부터 작업 파일까지 비교합니다. 커밋된 변경과 미커밋 변경을 함께 표시합니다.")
                        .child(Input::new(&field).id("branch-review-base"))
                        .child(div().text_size(ui_text(11.)).child("브랜치를 전환하거나 파일을 변경하지 않습니다. 그래프의 브랜치 필터와 별개입니다.")),
                )
                .on_ok(move |_, window, cx| {
                    submit_review(&submit, &target, &value, window, cx)
                })
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(Button::new("cancel-branch-review").label("취소").ghost().on_click(
                            |_, window, cx| window.close_dialog(cx),
                        ))
                        .child(Button::new("apply-branch-review").label("비교 적용").on_click(
                            move |_, window, cx| {
                                if submit_review(&submit_button, &button_root, &button_input, window, cx) {
                                    window.close_dialog(cx);
                                }
                            },
                        )),
                )
        });
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn apply_branch_review(
        &mut self,
        root: PathBuf,
        compare: CompareMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current.as_ref() != Some(&root)
            || self.opening_path.is_some()
            || self.loading
            || self.git_is_busy()
        {
            return;
        }
        self.persist(cx);
        self.request += 1;
        self.file_request += 1;
        self.navigation_request += 1;
        self.navigating = false;
        self.file_loading = false;
        self.pending_cli_file = None;
        self.checking_changes = false;
        self.count_pending = false;
        self.loading = true;
        self.error = None;
        self.status = format!("비교 중 · {}", baseline_label(&compare));
        let request = ComparisonRequest {
            root: root.clone(),
            generation: self.request,
        };
        let task = cx.background_spawn(async move { pokarak_core::snapshot(&root, &compare) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if !request.accepts(
                    app.current.as_deref(),
                    app.request,
                    app.opening_path.is_some(),
                ) {
                    return;
                }
                app.loading = false;
                match result {
                    Ok(snapshot) => {
                        // Neither the selected baseline nor the existing reader is
                        // replaced before successful validation of this exact root.
                        app.leave_git_graph();
                        app.external_change = false;
                        app.install_snapshot(snapshot, window, cx);
                        app.persist(cx);
                    }
                    Err(error) => {
                        app.error = Some(format!(
                            "비교 기준을 적용하지 못했습니다. 이전 비교를 유지합니다.\n{error:#}"
                        ));
                        app.status = "브랜치 비교 실패 · 이전 비교 유지".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn submit_review(
    owner: &WeakEntity<Pokarak>,
    root: &Path,
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let base = input.read(cx).value().trim().to_owned();
    if base.is_empty() {
        input.update(cx, |input, cx| input.focus(window, cx));
        return false;
    }
    owner
        .update(cx, |app, cx| {
            if app.current.as_deref() != Some(root) || app.loading || app.git_is_busy() {
                return false;
            }
            app.apply_branch_review(root.to_path_buf(), CompareMode::Branch(base), window, cx);
            true
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{ComparisonRequest, baseline_label};
    use pokarak_core::CompareMode;
    use std::path::Path;

    #[test]
    fn late_comparison_cannot_install_into_a_different_or_reopened_worktree() {
        let request = ComparisonRequest {
            root: "/fixture/worktree-a".into(),
            generation: 9,
        };
        assert!(request.accepts(Some(Path::new("/fixture/worktree-a")), 9, false));
        assert!(!request.accepts(Some(Path::new("/fixture/worktree-b")), 9, false));
        assert!(!request.accepts(Some(Path::new("/fixture/worktree-a")), 10, false));
        assert!(!request.accepts(Some(Path::new("/fixture/worktree-a")), 9, true));
        assert!(!request.accepts(None, 9, false));
    }

    #[test]
    fn applied_label_distinguishes_merge_base_from_the_branch_tip_and_graph_filter() {
        assert_eq!(
            baseline_label(&CompareMode::Uncommitted),
            "HEAD → 현재 파일"
        );
        assert_eq!(
            baseline_label(&CompareMode::Branch("origin/main".into())),
            "공통 조상(origin/main, HEAD) → 현재 파일"
        );
    }
}
