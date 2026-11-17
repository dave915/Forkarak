use super::*;

#[derive(Clone, Copy)]
pub(super) enum Pane {
    Sidebar,
    Files,
}

impl Pane {
    fn toggle(self, saved: &mut SavedView) {
        let hidden = match self {
            Self::Sidebar => &mut saved.hide_sidebar,
            Self::Files => &mut saved.hide_files,
        };
        *hidden = !*hidden;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PaneLayout {
    graph: bool,
    sidebar: bool,
    files: bool,
}

impl PaneLayout {
    fn new(graph: bool, saved: &SavedView) -> Self {
        Self {
            graph,
            sidebar: !saved.hide_sidebar,
            files: !graph && !saved.hide_files,
        }
    }

    fn group_id(self, root: &Path, zoom: u16, saved: &SavedView) -> SharedString {
        // A retained group's slots belong to this exact layout. Widths are
        // included so a resize in another visibility/mode combination cannot
        // restore older cached widths when this combination is shown again.
        format!(
            "workspace-panes:{root:?}:{self:?}:{zoom}:{:?}:{:?}",
            saved.sidebar_width, saved.code_files_width
        )
        .into()
    }

    fn remember_widths(self, saved: &mut SavedView, sizes: &[f32]) {
        let width = |index: usize| {
            sizes
                .get(index)
                .copied()
                .filter(|width| width.is_finite() && *width > 0.)
        };
        if self.sidebar
            && let Some(width) = width(0)
        {
            saved.sidebar_width = Some(width);
        }
        if self.files
            && let Some(width) = width(usize::from(self.sidebar))
        {
            saved.code_files_width = Some(width);
        }
    }
}

impl Pokarak {
    pub(super) fn toggle_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening_path.is_some()
            || window.has_active_dialog(cx)
            || (matches!(pane, Pane::Files) && self.git_history.active)
        {
            return;
        }
        let Some(session) = self
            .current
            .as_ref()
            .and_then(|root| self.sessions.get_mut(root))
        else {
            return;
        };
        pane.toggle(&mut session.saved);
        // The hidden tree may own keyboard focus. Keep toolbar/menu shortcuts
        // reachable without reopening a file or changing its reading position.
        self.focus.focus(window, cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn pane_toggles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let saved = self.session().map(|session| &session.saved);
        let sidebar = saved.is_none_or(|saved| !saved.hide_sidebar);
        let files = saved.is_none_or(|saved| !saved.hide_files);
        let pending = self.opening_path.is_some() || saved.is_none();
        let sidebar_label = if sidebar {
            "사이드바 숨기기"
        } else {
            "사이드바 표시"
        };
        let files_label = if files {
            "파일 목록 숨기기"
        } else {
            "파일 목록 표시"
        };
        div()
            .id("pane-toggles")
            .flex_none()
            .flex()
            .items_center()
            .gap(ui_px(2.))
            .child(
                toolbar::toolbar_icon("toggle-sidebar", IconName::PanelLeft, sidebar_label)
                    .selected(sidebar)
                    .toggled(sidebar)
                    .disabled(pending)
                    .tooltip_with_action(sidebar_label, &ToggleSidebar, None)
                    .on_click(cx.listener(|app, _, w, cx| app.toggle_pane(Pane::Sidebar, w, cx))),
            )
            .child(
                toolbar::toolbar_icon("toggle-files", IconName::FolderTree, files_label)
                    .selected(!self.git_history.active && files)
                    .toggled(!self.git_history.active && files)
                    .disabled(pending || self.git_history.active)
                    .when(self.git_history.active, |button| {
                        button
                            .accessibility_label("파일 목록 표시 / 숨기기 · 코드 보기에서 사용")
                            .tooltip("파일 목록 패널은 코드 보기에서 표시 / 숨길 수 있습니다")
                    })
                    .when(!self.git_history.active, |button| {
                        button.tooltip_with_action(files_label, &ToggleFiles, None)
                    })
                    .on_click(cx.listener(|app, _, w, cx| app.toggle_pane(Pane::Files, w, cx))),
            )
    }

    pub(super) fn render_workspace_panes(
        &self,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let defaults = SavedView::default();
        let saved = self
            .session()
            .map(|session| &session.saved)
            .unwrap_or(&defaults);
        let layout = PaneLayout::new(self.git_history.active, saved);
        let zoom = self.prefs.display.ui_zoom_percent;
        let target = root.to_path_buf();
        let view = cx.weak_entity();
        let mut panels =
            h_resizable(layout.group_id(root, zoom, saved)).on_resize(move |state, _, cx| {
                let sizes: Vec<_> = state
                    .read(cx)
                    .sizes()
                    .iter()
                    .copied()
                    .map(theme::logical_px)
                    .collect();
                let _ = view.update(cx, |app, cx| {
                    if app.current.as_ref() != Some(&target)
                        || app.git_history.active != layout.graph
                        || app.prefs.display.ui_zoom_percent != zoom
                        || app.opening_path.is_some()
                    {
                        return;
                    }
                    let Some(session) = app.sessions.get_mut(&target) else {
                        return;
                    };
                    if PaneLayout::new(app.git_history.active, &session.saved) != layout {
                        return;
                    }
                    layout.remember_widths(&mut session.saved, &sizes);
                    app.persist(cx);
                    cx.notify();
                });
            });
        if layout.sidebar {
            panels = panels.child(
                resizable_panel()
                    .size(ui_px(saved.sidebar_width.unwrap_or(210.)))
                    .size_range(ui_px(160.)..ui_px(420.))
                    .fixed()
                    .child(self.render_worktrees(cx)),
            );
        }
        if layout.files {
            panels = panels.child(
                resizable_panel()
                    .size(ui_px(saved.code_files_width.unwrap_or(285.)))
                    .size_range(ui_px(200.)..ui_px(600.))
                    .fixed()
                    .child(self.render_files(window, cx)),
            );
        }
        // Omitted panels have no slot, separator or resize handle. The final
        // unconstrained pane receives all of the reclaimed space.
        panels
            .child(resizable_panel().child(if layout.graph {
                self.render_git_graph(window, cx)
            } else {
                self.render_reader(window, cx)
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Pane, PaneLayout, SavedView};
    use std::path::Path;

    #[test]
    fn resize_slots_follow_visible_panes_and_never_save_the_editor_width() {
        for graph in [false, true] {
            for hide_sidebar in [false, true] {
                for hide_files in [false, true] {
                    let mut saved = SavedView {
                        hide_sidebar,
                        hide_files,
                        sidebar_width: Some(231.),
                        code_files_width: Some(327.),
                        ..Default::default()
                    };
                    let layout = PaneLayout::new(graph, &saved);
                    let mut sizes = Vec::new();
                    if !hide_sidebar {
                        sizes.push(240.);
                    }
                    if !graph && !hide_files {
                        sizes.push(360.);
                    }
                    sizes.push(1200.); // Graph/editor must never overwrite a hidden panel.
                    layout.remember_widths(&mut saved, &sizes);
                    assert_eq!(
                        saved.sidebar_width,
                        Some(if hide_sidebar { 231. } else { 240. })
                    );
                    assert_eq!(
                        saved.code_files_width,
                        Some(if graph || hide_files { 327. } else { 360. })
                    );
                }
            }
        }
    }

    #[test]
    fn layout_cache_does_not_reuse_slots_or_widths_from_another_context() {
        let root = Path::new("/project");
        let saved = SavedView::default();
        let layout = PaneLayout::new(false, &saved);
        let initial = layout.group_id(root, 100, &saved);
        for changed in [
            SavedView {
                hide_sidebar: true,
                ..saved.clone()
            },
            SavedView {
                hide_files: true,
                ..saved.clone()
            },
            SavedView {
                hide_sidebar: true,
                hide_files: true,
                ..saved.clone()
            },
            SavedView {
                sidebar_width: Some(240.),
                ..saved.clone()
            },
            SavedView {
                code_files_width: Some(360.),
                ..saved.clone()
            },
        ] {
            assert_ne!(
                initial,
                PaneLayout::new(false, &changed).group_id(root, 100, &changed)
            );
        }
        assert_ne!(
            initial,
            PaneLayout::new(true, &saved).group_id(root, 100, &saved)
        );
        assert_ne!(
            initial,
            layout.group_id(Path::new("/linked-worktree"), 100, &saved)
        );
        assert_ne!(initial, layout.group_id(root, 125, &saved));
    }

    #[test]
    fn hide_restore_keeps_the_complete_saved_reading_context() {
        let mut saved: SavedView = serde_json::from_value(serde_json::json!({
            "selected": "src/main.rs", "tabs": ["README.md", "src/main.rs"],
            "code": true, "all_files": true, "sidebar_width": 233, "code_files_width": 319,
            "collapsed_folders": ["src/private"], "graph_branch": "feature/review",
            "filter": "main", "diff_row": 42, "line": 12, "column": 5,
            "scroll_x": -32, "scroll_y": -470,
            "positions": { "src/main.rs": { "line": 12, "column": 5, "scroll_y": -470 } }
        }))
        .unwrap();
        let before = serde_json::to_value(&saved).unwrap();
        Pane::Sidebar.toggle(&mut saved);
        Pane::Files.toggle(&mut saved);
        PaneLayout::new(false, &saved).remember_widths(&mut saved, &[940.]);
        let mut restored: SavedView =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(restored.hide_sidebar && restored.hide_files);
        Pane::Files.toggle(&mut restored);
        Pane::Sidebar.toggle(&mut restored);
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
    }
}
