//! Persistent workspace status is read-only; actions retain the inspected state.
use super::*;
impl Pokarak {
    pub(in crate::ui) fn refresh_workflow_status(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.current.clone() else {
            return;
        };
        if self.active_worktree_has_git_job() {
            return;
        }
        self.workflow.status_request += 1;
        let token = self.workflow.status_request;
        let generation = self.request;
        let target = root.clone();
        let task =
            cx.background_spawn(async move { GitOperations::open(&target)?.workflow_state() });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, _, cx| {
                if app.workflow.status_request != token
                    || app.request != generation
                    || app.current.as_ref() != Some(&root)
                    || app.opening_path.is_some()
                    || app.active_worktree_has_git_job()
                {
                    return;
                }
                app.workflow
                    .status
                    .insert(root, result.map_err(|e| format!("{e:#}")));
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::ui) fn render_workflow_status(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.opening_path.is_some() {
            return None;
        }
        let root = self.current.as_ref()?;
        let state = self.workflow.status.get(root)?;
        let weak = cx.entity().downgrade();
        let state = match state {
            Ok(state) => state,
            Err(error) => {
                return Some(
                    div()
                        .flex_none()
                        .flex()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .child(format!("Git 작업 상태 확인 실패: {error}"))
                        .child(wf_action(
                            &weak,
                            "workflow-status-retry",
                            "다시 확인",
                            self.git_is_busy(),
                            |app, w, cx| app.refresh_workflow_status(w, cx),
                        ))
                        .into_any_element(),
                );
            }
        };
        let progress = state.progress.as_ref()?;
        let root = root.clone();
        let abort_root = root.clone();
        let continued = progress.clone();
        let aborted = progress.clone();
        let busy = self.git_is_busy();
        Some(div().flex_none().flex().h(ui_px(32.)).items_center().gap_2().px_3()
            .child(div().min_w_0().flex_1().text_ellipsis().child(format!("{} 진행 중 · 미해결 파일 {}개 · {}", progress.kind.label(), state.conflicts.len(), root.display())))
            .child(wf_action(&weak, "workflow-status-review", "충돌 / 변경 확인…", busy, |app,w,cx| app.open_git_workflow(w,cx)))
            .child(wf_action(&weak, "workflow-status-continue", "계속…", busy || !state.conflicts.is_empty(), move |app,w,cx| app.confirm_workflow(root.clone(), format!("{} 계속", continued.kind.label()), "확인한 진행 상태에서 스테이징된 결과로 계속합니다. Git hooks가 실행되고 커밋이 만들어질 수 있습니다.".into(), WorkflowWrite::Finish { progress: continued.clone(), abort: false }, w,cx)))
            .child(wf_action(&weak, "workflow-status-abort", "중단…", busy, move |app,w,cx| app.confirm_workflow(abort_root.clone(), format!("{} 중단", aborted.kind.label()), "Git이 작업 이전 상태로 복원합니다. 충돌 해결 중 작성한 내용이 사라질 수 있습니다. 대상 워크트리를 확인하세요.".into(), WorkflowWrite::Finish { progress: aborted.clone(), abort: true }, w,cx)))
            .into_any_element())
    }
}
