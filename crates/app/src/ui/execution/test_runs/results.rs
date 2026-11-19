use super::*;

impl Pokarak {
    fn current_test_result(&self, root: &Path, workspace: u64, id: RunId) -> bool {
        self.current.as_deref() == Some(root)
            && self.opening_path.is_none()
            && self.request == workspace
            && self
                .execution
                .tests
                .results
                .get(root)
                .is_some_and(|v| v.id == id)
    }

    pub(in crate::ui) fn test_footer_button(
        &self,
        root: &Path,
        cx: &mut Context<Self>,
    ) -> Option<Button> {
        let view = self.execution.tests.results.get(root)?;
        let (owner, workspace, id) = (root.to_owned(), self.request, view.id);
        Some(
            Button::new("test-result-footer")
                .label(view.label())
                .tooltip("테스트 결과 열기")
                .small()
                .ghost()
                .on_click(cx.listener(move |app, _, w, cx| {
                    if app.current_test_result(&owner, workspace, id) {
                        app.open_tasks(w, cx);
                        app.execution
                            .views
                            .entry(owner.clone())
                            .or_default()
                            .selected = Some(LogSource::Test(id));
                        cx.notify();
                    }
                })),
        )
    }

    pub(in crate::ui::execution) fn test_result_panel(
        &self,
        root: &Path,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.execution.tests.results.get(root)?;
        let id = view.id;
        let workspace = self.request;
        let owner = root.to_owned();
        let stop_owner = root.to_owned();
        let rerun_owner = root.to_owned();
        let mut panel = div().px_2().v_flex().gap_1().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(view.target.profile.name.clone()),
                )
                .child(
                    Button::new("test-show-logs")
                        .label("테스트 로그")
                        .small()
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if app.current_test_result(&owner, workspace, id) {
                                app.execution
                                    .views
                                    .entry(owner.clone())
                                    .or_default()
                                    .selected = Some(LogSource::Test(id));
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new("test-stop")
                        .label("중지")
                        .disabled(!view.status.active())
                        .small()
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if app.current_test_result(&stop_owner, workspace, id) {
                                app.execution.tests.cancel(&stop_owner, id);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new("test-rerun")
                        .label("다시 실행")
                        .small()
                        .on_click(cx.listener(move |app, _, w, cx| {
                            if app.current_test_result(&rerun_owner, workspace, id) {
                                app.run_editor_test(TestCommand::Rerun, w, cx);
                            }
                        })),
                ),
        );
        if let Some(error) = &view.report_error {
            panel = panel.child(error.clone());
        }
        if let Some(summary) = &view.summary {
            let label = if summary.skipped > 0 {
                format!("{} · {}개 건너뜀", view.label(), summary.skipped)
            } else {
                view.label()
            };
            panel = panel.child(div().id("test-summary").child(label));
            let mut failures = div()
                .id("test-failures")
                .max_h(ui_px(152.))
                .overflow_y_scroll()
                .v_flex();
            for (index, failure) in summary.failures.iter().enumerate() {
                let root = root.to_owned();
                let enabled = failure
                    .location
                    .as_ref()
                    .is_some_and(|location| view.revisions.contains_key(&location.path));
                let message = failure.message.lines().next().unwrap_or("");
                let label = if message.is_empty() {
                    failure.name.clone()
                } else {
                    format!("{} · {message}", failure.name)
                };
                failures = failures.child(
                    Button::new(("test-failure", index))
                        .label(label.clone())
                        .tooltip(label)
                        .small()
                        .ghost()
                        .disabled(!enabled)
                        .on_click(cx.listener(move |app, _, w, cx| {
                            if app.current_test_result(&root, workspace, id) {
                                app.open_test_failure(&root, index, w, cx);
                            }
                        })),
                );
            }
            panel = panel.child(failures);
        }
        Some(panel.into_any_element())
    }

    fn open_test_failure(
        &mut self,
        root: &Path,
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.execution.tests.results.get(root) else {
            return;
        };
        let Some(location) = view
            .summary
            .as_ref()
            .and_then(|s| s.failures.get(index))
            .and_then(|failure| failure.location.clone())
        else {
            return;
        };
        let Ok(relative) = location.path.strip_prefix(root) else {
            return;
        };
        if location.path.canonicalize().ok().as_ref() != Some(&location.path)
            || view
                .revisions
                .get(&location.path)
                .is_none_or(|expected| file_revision(&location.path).as_ref() != Some(expected))
        {
            self.execution.message =
                Some("실패 위치의 파일이 변경되었습니다. 테스트를 다시 실행하세요.".into());
            cx.notify();
            return;
        }
        self.open_execution_location(relative, &location, w, cx);
    }
}
