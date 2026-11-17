use super::*;
use crate::prefs::{UpdateView, UpdateWorkspace};

impl Pokarak {
    pub(super) fn remember_update_workspace(&mut self, cx: &App) -> Result<(), String> {
        if !self.retain_code_drafts(cx) {
            return Err("편집 초안을 보관할 수 없어 재시작을 중단했습니다.".into());
        }
        self.remember_view(cx);
        self.prefs.last_root = self.current.clone();
        self.prefs.open_projects = self.projects.clone();
        let view = if self.inline_commit_active() {
            UpdateView::Changes
        } else if self.git_history.active {
            UpdateView::Graph(self.git_history.selection())
        } else {
            UpdateView::Code
        };
        self.prefs.update_workspace = Some(UpdateWorkspace {
            root: self.current.clone(),
            view,
            messages: self.commit_ui.draft_messages(cx),
        });
        self.prefs.try_save().map_err(|e| {
            self.prefs.update_workspace = None;
            format!("작업 화면을 저장할 수 없습니다. {e}")
        })
    }

    pub(super) fn forget_update_workspace(&mut self) {
        self.prefs.update_workspace = None;
        self.prefs.save();
    }

    pub(super) fn open_restored_view(
        &mut self,
        view: Option<UpdateView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match view {
            Some(UpdateView::Code) => self.show_code_view(window, cx),
            Some(UpdateView::Changes) => {
                self.open_inline_commit(window, cx);
                self.open_git_history(window, cx);
            }
            Some(UpdateView::Graph(target)) => {
                self.git_history.resume_selection(target);
                self.open_git_history(window, cx);
            }
            None => self.open_git_history(window, cx),
        }
    }
}
