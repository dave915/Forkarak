//! Exact job ownership for live output and explicit cancellation.
use super::*;

pub(super) fn job_matches(ui: &GitUi, root: &Path, id: u64) -> bool {
    ui.job_id == id && ui.busy.as_ref().is_some_and(|(r, _)| r == root)
}

impl Pokarak {
    pub(super) fn drain_git_progress(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(progress) = &self.git_ui.progress else {
            return;
        };
        let Some((root, _)) = &self.git_ui.busy else {
            return;
        };
        history::append(
            &mut self.git_ui.history,
            root,
            self.git_ui.job_id,
            progress.drain(),
            progress.dropped_bytes(),
        );
        self.update_git_live_editor(window, cx);
    }

    pub(super) fn update_git_live_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((root, id, editor)) = &self.git_ui.live_editor else {
            return;
        };
        let Some(record) = self
            .git_ui
            .history
            .get(root)
            .and_then(|records| records.iter().find(|r| r.id == *id))
        else {
            return;
        };
        let text = record.text(root);
        if editor.read(cx).value().as_ref() != text {
            editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
    }

    pub(super) fn cancel_git_action(&mut self, root: &Path, id: u64, cx: &mut Context<Self>) {
        if !job_matches(&self.git_ui, root, id) {
            return;
        }
        if let Some(cancel) = &self.git_ui.cancellation {
            cancel.cancel();
        }
        self.status = format!(
            "Git 취소 요청 · 종료 후 상태를 다시 확인합니다 — {}",
            root.display()
        );
        cx.notify();
    }

    pub(in crate::ui) fn render_git_activity(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (root, label) = self.git_ui.busy.as_ref()?;
        if !self.git_ui.setup_job && self.current.as_ref() != Some(root) && self.current.is_some() {
            return None;
        }
        let root = root.clone();
        let cancel_root = root.clone();
        let id = self.git_ui.job_id;
        Some(
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .child(format!("{label} 실행 중 · {}", root.display()))
                .child(
                    Button::new("git-live-output")
                        .label("실시간 출력…")
                        .small()
                        .on_click(cx.listener(move |app, _, w, cx| {
                            if let Some(record) = app
                                .git_ui
                                .history
                                .get(&root)
                                .and_then(|records| records.iter().find(|r| r.id == id))
                                .cloned()
                            {
                                app.show_git_output(root.clone(), record, w, cx);
                            }
                        })),
                )
                .child(
                    Button::new("git-cancel-operation")
                        .label("작업 취소")
                        .small()
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.cancel_git_action(&cancel_root, id, cx)
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn progress_and_cancel_require_exact_job_and_root() {
        let ui = GitUi {
            job_id: 7,
            busy: Some(("/a".into(), "Fetch".into())),
            ..Default::default()
        };
        assert!(job_matches(&ui, Path::new("/a"), 7));
        assert!(!job_matches(&ui, Path::new("/b"), 7));
        assert!(!job_matches(&ui, Path::new("/a"), 6));
    }
}
