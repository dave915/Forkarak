use super::terminal_sessions::TerminalSessions;
use super::*;
use crate::terminal::DockAction;
use gpui_kit::component::resizable::v_resizable;

impl Pokarak {
    pub(super) fn has_live_terminals(&self, cx: &App) -> bool {
        self.sessions
            .values()
            .filter_map(|session| session.terminal.as_ref())
            .any(|terminals| terminals.read(cx).has_running_shell(cx))
    }

    pub(super) fn shutdown_terminals(&mut self, cx: &mut App) -> Vec<crate::terminal::Shutdown> {
        for terminals in self
            .sessions
            .values_mut()
            .filter_map(|session| session.terminal.take())
        {
            terminals.update(cx, |terminals, cx| {
                terminals.shutdown(cx);
            });
        }
        crate::terminal::finish_pending_shutdowns()
    }

    pub(super) fn close_worktree_terminals(&mut self, root: &Path, cx: &mut App) {
        if let Some(terminals) = self
            .sessions
            .get_mut(root)
            .and_then(|session| session.terminal.take())
        {
            terminals.update(cx, |terminals, cx| {
                terminals.shutdown(cx);
            });
        }
    }

    pub(super) fn open_worktree_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone().filter(|_| self.opening_path.is_none()) else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&root) else {
            return;
        };
        if session.terminal_visible {
            session.terminal_visible = false;
            self.focus.focus(window, cx);
            cx.notify();
            return;
        }
        let terminal = session.terminal.get_or_insert_with(|| {
            let dock = cx.new(|cx| TerminalSessions::new(root.clone(), window, cx));
            cx.subscribe_in(&dock, window, move |app, _, action, w, cx| {
                let Some(session) = app.sessions.get_mut(&root) else {
                    return;
                };
                session.terminal_visible = false;
                if matches!(action, DockAction::Close) {
                    session.terminal = None;
                }
                if app.current.as_ref() == Some(&root) {
                    app.focus.focus(w, cx);
                }
                cx.notify();
            })
            .detach();
            dock
        });
        terminal.focus_handle(cx).focus(window, cx);
        session.terminal_visible = true;
        cx.notify();
    }

    pub(super) fn with_terminal_dock(&self, body: AnyElement, window: &Window) -> AnyElement {
        let Some(dock) = self
            .session()
            .filter(|s| s.terminal_visible && self.opening_path.is_none())
            .and_then(|s| s.terminal.as_ref())
        else {
            return body;
        };
        let target = (window.viewport_size().height * 0.30).clamp(ui_px(136.), ui_px(320.));
        v_resizable(SharedString::from(format!(
            "terminal-split:{:?}:{}",
            self.current, self.prefs.display.ui_zoom_percent
        )))
        .child(
            resizable_panel()
                .size_range(ui_px(110.)..ui_px(4000.))
                .child(body),
        )
        .child(
            resizable_panel()
                .size(target)
                .size_range(ui_px(96.)..ui_px(2000.))
                .child(dock.clone()),
        )
        .into_any_element()
    }
}
