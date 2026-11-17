//! Row actions capture the clicked commit independently of the detail reader.
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use pokarak_core::operations::branches::BranchStart;

pub(super) fn commit_context_menu(
    weak: WeakEntity<Pokarak>,
    root: Option<PathBuf>,
    commit: Commit,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu {
    move |mut menu, _, cx| {
        let branch_disabled = weak.upgrade().is_none_or(|view| {
            let app = view.read(cx);
            app.git_is_busy()
                || app.loading
                || app.current != root
                || root
                    .as_deref()
                    .is_none_or(|root| app.git_history.branch_start(root, &commit.oid).is_none())
        });
        let disabled = weak.upgrade().is_none_or(|view| {
            let app = view.read(cx);
            app.git_is_busy()
                || app.loading
                || app.current != root
                || !app.git_history.active
                || commit.parents.len() > 1
                || app
                    .active_worktree()
                    .is_none_or(|tree| tree.is_detached || tree.head.is_empty())
        });
        menu = menu.label(format!("커밋 {}", short(&commit.oid)));
        let (branch_weak, branch_root, oid) = (weak.clone(), root.clone(), commit.oid.clone());
        menu = menu.item(
            PopupMenuItem::new("여기서 브랜치 만들기…")
                .disabled(branch_disabled)
                .on_click(move |_, w, cx| {
                    if let Some(root) = &branch_root {
                        let _ = branch_weak.update(cx, |app, cx| {
                            if app.current.as_ref() != Some(root)
                                || app.loading
                                || app.git_is_busy()
                            {
                                return;
                            }
                            if let Some(start) = app.git_history.branch_start(root, &oid) {
                                app.open_branch_create(Some(start), w, cx);
                            }
                        });
                    }
                }),
        );
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
    fn branch_start(&self, root: &Path, oid: &str) -> Option<BranchStart> {
        if !self.active || self.root.as_deref() != Some(root) {
            return None;
        }
        self.entries
            .iter()
            .find(|entry| entry.commit.oid == oid)
            .map(|entry| BranchStart {
                reference: entry.commit.oid.clone(),
                oid: entry.commit.oid.clone(),
            })
    }

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
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(ui.action_commit(root, "b").unwrap().oid, "b");
        assert_eq!(ui.branch_start(root, "b").unwrap().oid, "b");
        assert_eq!(ui.branch_start(root, "merge").unwrap().reference, "merge");
        assert!(ui.branch_start(Path::new("/other"), "b").is_none());
        assert!(ui.branch_start(root, "stale").is_none());
        assert!(ui.action_commit(root, "merge").is_none());
        assert!(ui.action_commit(Path::new("/other"), "b").is_none());
        assert!(ui.action_commit(root, "stale").is_none());
        ui.active = false;
        assert!(ui.action_commit(root, "b").is_none());
        assert!(ui.branch_start(root, "b").is_none());
    }
}
