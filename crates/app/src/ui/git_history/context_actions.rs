//! Row actions capture the clicked commit independently of the detail reader.
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

pub(super) fn commit_context_menu(
    weak: WeakEntity<Pokarak>,
    root: Option<PathBuf>,
    commit: Commit,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu {
    move |mut menu, _, cx| {
        let disabled = weak.upgrade().is_none_or(|view| {
            let app = view.read(cx);
            app.git_is_busy()
                || app.loading
                || app.current != root
                || !app.git_history.active
                || commit.parents.len() > 1
                || app
                    .snapshot()
                    .is_none_or(|s| s.worktree.is_detached || s.worktree.head.is_empty())
        });
        menu = menu.label(format!("커밋 {}", short(&commit.oid)));
        for (action, label) in [
            (HistoryAction::CherryPick, "Cherry-pick…"),
            (HistoryAction::Revert, "Revert…"),
        ] {
            let weak = weak.clone();
            let oid = commit.oid.clone();
            let root = root.clone();
            menu = menu.item(PopupMenuItem::new(label).disabled(disabled).on_click(
                move |_, w, cx| {
                    if let Some(root) = &root {
                        let _ = weak.update(cx, |app, cx| {
                            app.prepare_commit_by_oid(action, root, &oid, w, cx)
                        });
                    }
                },
            ));
        }
        menu
    }
}

impl HistoryUi {
    fn action_commit(&self, root: &Path, oid: &str) -> Option<&Commit> {
        if !self.active || self.root.as_deref() != Some(root) {
            return None;
        }
        self.entries
            .iter()
            .map(|entry| &entry.commit)
            .find(|commit| commit.oid == oid && commit.parents.len() <= 1)
    }
}

impl Pokarak {
    pub(super) fn prepare_commit_by_oid(
        &mut self,
        action: HistoryAction,
        root: &Path,
        oid: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading || self.git_is_busy() || self.current.as_deref() != Some(root) {
            return;
        }
        let Some(commit) = self.git_history.action_commit(root, oid).cloned() else {
            return;
        };
        self.workflow_read(
            root.to_owned(),
            |ops| ops.history_target(),
            move |app, target, w, cx| {
                if app
                    .git_history
                    .action_commit(&target.root, &commit.oid)
                    .is_some()
                {
                    app.confirm_history_action(action, commit, target, w, cx);
                }
            },
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Commit, GraphRow, HistoryEntry, HistoryUi};
    use std::path::Path;

    #[test]
    fn pending_detail_does_not_redirect_row_actions() {
        let root = Path::new("/repo/worktree");
        let entry = |oid: &str, parents: &[&str]| HistoryEntry {
            commit: Commit {
                oid: oid.into(),
                parents: parents.iter().map(|s| (*s).to_owned()).collect(),
                subject: oid.into(),
                author: String::new(),
                date: String::new(),
                refs: Vec::new(),
            },
            graph: GraphRow::default(),
        };
        let mut ui = HistoryUi {
            active: true,
            loading: true,
            root: Some(root.to_owned()),
            entries: vec![
                entry("merge", &["a", "b"]),
                entry("a", &[]),
                entry("b", &[]),
            ],
            ..Default::default()
        };
        assert_eq!(ui.action_commit(root, "b").unwrap().oid, "b");
        assert!(ui.action_commit(root, "merge").is_none());
        assert!(ui.action_commit(Path::new("/other"), "b").is_none());
        assert!(ui.action_commit(root, "stale").is_none());
        ui.active = false;
        assert!(ui.action_commit(root, "b").is_none());
    }
}
