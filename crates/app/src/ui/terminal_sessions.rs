//! Transient terminal tabs. Each worktree owns one container; no PTY is serialized.
use super::*;
use crate::terminal::{DockAction, TerminalDock};

actions!(
    terminal_sessions,
    [
        NewTerminal,
        NextTerminal,
        PreviousTerminal,
        CloseTerminal,
        FocusTerminalTabs
    ]
);

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-alt-t", NewTerminal, Some("PokarakTerminalSessions")),
        KeyBinding::new("cmd-shift-]", NextTerminal, Some("PokarakTerminalSessions")),
        KeyBinding::new(
            "cmd-shift-[",
            PreviousTerminal,
            Some("PokarakTerminalSessions"),
        ),
        KeyBinding::new("cmd-alt-w", CloseTerminal, Some("PokarakTerminalSessions")),
        KeyBinding::new(
            "cmd-alt-0",
            FocusTerminalTabs,
            Some("PokarakTerminalSessions"),
        ),
    ]);
}

struct Tab<T> {
    id: u64,
    value: T,
}

/// Stable IDs prevent a late close/output notification from targeting a replacement tab.
struct Tabs<T> {
    entries: Vec<Tab<T>>,
    selected: Option<u64>,
    next_id: u64,
}

impl<T> Default for Tabs<T> {
    fn default() -> Self {
        Self {
            entries: vec![],
            selected: None,
            next_id: 1,
        }
    }
}

impl<T> Tabs<T> {
    fn push(&mut self, value: T) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(Tab { id, value });
        self.selected = Some(id);
        id
    }

    fn current(&self) -> Option<&T> {
        self.entries
            .iter()
            .find(|tab| Some(tab.id) == self.selected)
            .map(|tab| &tab.value)
    }

    fn any(&self, mut predicate: impl FnMut(&T) -> bool) -> bool {
        self.entries.iter().any(|tab| predicate(&tab.value))
    }

    fn select(&mut self, id: u64) {
        if self.entries.iter().any(|tab| tab.id == id) {
            self.selected = Some(id);
        }
    }

    fn cycle(&mut self, next: bool) {
        let Some(index) = self
            .entries
            .iter()
            .position(|tab| Some(tab.id) == self.selected)
        else {
            return;
        };
        let count = self.entries.len();
        self.selected = Some(
            self.entries[if next {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            }]
            .id,
        );
    }

    fn remove(&mut self, id: u64) -> Option<T> {
        let index = self.entries.iter().position(|tab| tab.id == id)?;
        let tab = self.entries.remove(index);
        if self.selected == Some(id) {
            self.selected = self
                .entries
                .get(index.min(self.entries.len().saturating_sub(1)))
                .map(|tab| tab.id);
        }
        Some(tab.value)
    }
}

pub(super) struct TerminalSessions {
    root: PathBuf,
    config: crate::terminal::TerminalConfig,
    tabs: Tabs<Entity<TerminalDock>>,
    controls_focus: FocusHandle,
}

impl EventEmitter<DockAction> for TerminalSessions {}
impl Focusable for TerminalSessions {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tabs
            .current()
            .map(|dock| dock.focus_handle(cx))
            .unwrap_or_else(|| self.controls_focus.clone())
    }
}

impl TerminalSessions {
    #[allow(dead_code)] // Retained constructor for existing integrations.
    pub(super) fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::configured(
            root,
            crate::terminal::TerminalConfig::default(),
            None,
            window,
            cx,
        )
    }

    pub(super) fn configured(
        root: PathBuf,
        config: crate::terminal::TerminalConfig,
        command: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            root,
            config,
            tabs: Tabs::default(),
            controls_focus: cx.focus_handle(),
        };
        this.add_command(command, window, cx);
        this
    }

    /// Includes inactive tabs, even while this worktree or its panel is hidden.
    pub(super) fn has_running_shell(&self, cx: &App) -> bool {
        self.tabs.any(|dock| dock.read(cx).has_running_shell())
    }

    pub(super) fn shutdown(&mut self, cx: &mut App) {
        let tabs = std::mem::take(&mut self.tabs);
        for tab in tabs.entries {
            tab.value.update(cx, |dock, _| dock.shutdown());
        }
    }

    fn focus_selected(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle(cx).focus(window, cx);
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config = crate::terminal::TerminalConfig::from_preferences(
            &Preferences::load(),
            self.config.project.as_deref(),
        );
        self.add_command(None, window, cx);
    }

    pub(super) fn set_launch_config(&mut self, config: crate::terminal::TerminalConfig) {
        self.config = config;
    }

    pub(super) fn add_command(
        &mut self,
        command: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut config = self.config.clone();
        if let Some(command) = command {
            // A separate foreground login shell loads PATH and startup files, then execs
            // the explicitly requested CLI. Never inject into an existing session.
            config.shell = "/bin/zsh".into();
            config.args = vec![
                "-l".into(),
                "-i".into(),
                "-c".into(),
                format!("exec {command}"),
            ];
        }
        let dock =
            cx.new(|cx| TerminalDock::embedded_configured(self.root.clone(), config, window, cx));
        let id = self.tabs.push(dock.clone());
        // Observe exit/restart status on background tabs as well as the selected tab.
        cx.observe(&dock, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &dock,
            window,
            move |this, _, action, window, cx| match action {
                DockAction::Hide => cx.emit(DockAction::Hide),
                DockAction::Close => this.close(id, window, cx),
            },
        )
        .detach();
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn select(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.select(id);
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn cycle(&mut self, next: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.cycle(next);
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dock) = self.tabs.remove(id) {
            // Explicitly detach the shell now, even if GPUI retains the old rendered entity.
            dock.update(cx, |dock, _| {
                dock.shutdown();
            });
        }
        if self.tabs.entries.is_empty() {
            // Last close hides the panel. Reopening explicitly creates a fresh first tab.
            cx.emit(DockAction::Close);
        } else {
            self.focus_selected(window, cx);
        }
        cx.notify();
    }
}

impl Render for TerminalSessions {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.tabs.selected;
        let tabs = self
            .tabs
            .entries
            .iter()
            .map(|tab| {
                let id = tab.id;
                let status = tab.value.read(cx).session_status();
                let label = format!("터미널 {id} · {status}");
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .child(
                        Button::new(SharedString::from(format!("terminal-tab-{id}")))
                            .ghost()
                            .small()
                            .selected(selected == Some(id))
                            .label(label.clone())
                            .tooltip(format!("{label} 선택 · {}", self.root.display()))
                            .accessibility_label(format!("{label} 선택"))
                            .tab_stop(true)
                            .on_click(cx.listener(move |this, _, w, cx| this.select(id, w, cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("terminal-close-{id}")))
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip(format!(
                                "터미널 {id} 닫기 · 이 세션의 셸과 실행 중인 명령 종료 (선택 탭 ⌘⌥W)"
                            ))
                            .accessibility_label(format!("터미널 {id} 닫기"))
                            .tab_stop(true)
                            .on_click(cx.listener(move |this, _, w, cx| this.close(id, w, cx))),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .id("terminal-sessions")
            .key_context("PokarakTerminalSessions")
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(rgb(0x1c2128))
            .on_action(cx.listener(|this, _: &NewTerminal, w, cx| this.add(w, cx)))
            .on_action(cx.listener(|this, _: &NextTerminal, w, cx| this.cycle(true, w, cx)))
            .on_action(cx.listener(|this, _: &PreviousTerminal, w, cx| this.cycle(false, w, cx)))
            .on_action(cx.listener(|this, _: &CloseTerminal, w, cx| {
                if let Some(id) = this.tabs.selected {
                    this.close(id, w, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &FocusTerminalTabs, w, cx| this.controls_focus.focus(w, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(ui_px(32.))
                    .px_1()
                    .gap_1()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .bg(rgb(0x242a31))
                    .child(
                        div()
                            .id("terminal-tab-strip")
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .children(tabs),
                    )
                    .child(
                        Button::new("new-terminal")
                            .ghost()
                            .small()
                            .label("+ 터미널")
                            .track_focus(&self.controls_focus)
                            .tab_stop(true)
                            .tooltip("새 터미널 (⌘⌥T) · 탭 버튼에 포커스 (⌘⌥0)")
                            .accessibility_label("새 터미널")
                            .on_click(cx.listener(|this, _, w, cx| this.add(w, cx))),
                    )
                    .child(
                        Button::new("previous-terminal")
                            .ghost()
                            .small()
                            .label("‹")
                            .disabled(self.tabs.entries.len() < 2)
                            .tooltip("이전 터미널 (⌘⇧[)")
                            .accessibility_label("이전 터미널")
                            .on_click(cx.listener(|this, _, w, cx| this.cycle(false, w, cx))),
                    )
                    .child(
                        Button::new("next-terminal")
                            .ghost()
                            .small()
                            .label("›")
                            .disabled(self.tabs.entries.len() < 2)
                            .tooltip("다음 터미널 (⌘⇧])")
                            .accessibility_label("다음 터미널")
                            .on_click(cx.listener(|this, _, w, cx| this.cycle(true, w, cx))),
                    )
                    .child(
                        Button::new("hide-terminals")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronDown)
                            .tooltip("모든 터미널 숨기기 · 세션은 계속 실행됩니다")
                            .accessibility_label("터미널 패널 숨기기")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DockAction::Hide))),
                    ),
            )
            .when_some(self.tabs.current().cloned(), |v, dock| {
                v.child(div().flex_1().min_h_0().child(dock))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::Tabs;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn selected_tab_survives_hidden_worktree_and_background_close() {
        let mut first = Tabs::default();
        let one = first.push("first");
        let two = first.push("second");
        let three = first.push("third");
        first.select(two);
        let mut other_worktree = Tabs::default();
        other_worktree.push("other root");
        other_worktree.cycle(true);
        assert_eq!(first.current(), Some(&"second"));
        assert_eq!(first.remove(one), Some("first"));
        assert_eq!(first.selected, Some(two));
        assert_eq!(first.remove(two), Some("second"));
        assert_eq!(first.selected, Some(three));
        assert_eq!(other_worktree.current(), Some(&"other root"));
    }

    #[test]
    fn cycling_wraps_and_last_close_has_no_selection_or_reused_id() {
        let mut tabs = Tabs::default();
        tabs.cycle(false);
        let one = tabs.push(1);
        let two = tabs.push(2);
        tabs.cycle(true);
        assert_eq!(tabs.selected, Some(one));
        tabs.cycle(false);
        assert_eq!(tabs.selected, Some(two));
        tabs.remove(two);
        assert_eq!(tabs.selected, Some(one));
        tabs.remove(one);
        assert!(tabs.current().is_none());
        tabs.cycle(true);
        let fresh = tabs.push(3);
        assert!(fresh > two);
        tabs.select(two);
        tabs.remove(two);
        assert_eq!(tabs.selected, Some(fresh));
    }

    #[test]
    fn update_guard_sees_live_background_tabs_when_selected_tab_has_exited() {
        use std::cell::Cell;
        let mut tabs = Tabs::default();
        let live = tabs.push(Cell::new(true));
        tabs.push(Cell::new(false));
        assert!(!tabs.current().unwrap().get());
        assert!(tabs.any(Cell::get));
        tabs.entries
            .iter()
            .find(|tab| tab.id == live)
            .unwrap()
            .value
            .set(false);
        assert!(!tabs.any(Cell::get));
        tabs.push(Cell::new(true));
        assert!(tabs.any(Cell::get));
    }

    #[test]
    fn closing_one_tab_drops_only_its_owned_resource_and_project_drop_cleans_all() {
        struct Owned(u8, Rc<RefCell<Vec<u8>>>);
        impl Drop for Owned {
            fn drop(&mut self) {
                self.1.borrow_mut().push(self.0);
            }
        }
        let closed = Rc::new(RefCell::new(vec![]));
        let mut tabs = Tabs::default();
        let one = tabs.push(Owned(1, closed.clone()));
        tabs.push(Owned(2, closed.clone()));
        tabs.push(Owned(3, closed.clone()));
        drop(tabs.remove(one));
        assert_eq!(*closed.borrow(), [1]);
        drop(tabs);
        assert_eq!(*closed.borrow(), [1, 2, 3]);
    }
}
