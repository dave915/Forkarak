use super::toolbar::toolbar_icon;
use super::*;
use gpui_kit::component::{
    popover::Popover,
    tab::{Tab, TabBar},
};

pub(super) struct ProjectTabs {
    scroll: ScrollHandle,
    list_scroll: ScrollHandle,
    focus: FocusHandle,
    open: bool,
    cursor: Option<PathBuf>,
    return_focus: Option<FocusHandle>,
    dragging: bool,
    drag_scroll: Option<Task<()>>,
}

impl ProjectTabs {
    pub(super) fn new(cx: &App) -> Self {
        Self {
            scroll: ScrollHandle::new(),
            list_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            open: false,
            cursor: None,
            return_focus: None,
            dragging: false,
            drag_scroll: None,
        }
    }
}

#[derive(Clone)]
struct ProjectDrag {
    owner: EntityId,
    root: PathBuf,
}

#[derive(Clone, Copy)]
enum Surface {
    Tabs,
    List,
}

impl Surface {
    fn after(self, bounds: Bounds<Pixels>, position: Point<Pixels>) -> bool {
        match self {
            Self::Tabs => position.x >= bounds.center().x,
            Self::List => position.y >= bounds.center().y,
        }
    }
}

fn accepts(projects: &[SavedProject], owner: EntityId, drag: &ProjectDrag, target: &Path) -> bool {
    drag.owner == owner
        && projects.iter().any(|p| p.root == drag.root)
        && projects.iter().any(|p| p.root == target)
}

// Resolve both identities at drop time. Removing the source shifts a later
// insertion slot; adjacent slots and either half of the source are no-ops.
fn move_project(
    projects: &mut Vec<SavedProject>,
    prefs: &mut Preferences,
    owner: EntityId,
    drag: &ProjectDrag,
    target: &Path,
    after: bool,
) -> bool {
    if !accepts(projects, owner, drag, target) {
        return false;
    }
    let source = projects.iter().position(|p| p.root == drag.root).unwrap();
    let target = projects.iter().position(|p| p.root == target).unwrap();
    let slot = target + usize::from(after);
    let destination = slot - usize::from(source < slot);
    if source == destination {
        return false;
    }
    let project = projects.remove(source);
    projects.insert(destination, project);
    // Only order preferences change. In particular, do not persist/capture the
    // reader, reopen a repository, or invalidate any in-flight request here.
    prefs.open_projects = projects.clone();
    true
}

fn project_label(root: &Path) -> String {
    root.file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy()
        .into_owned()
}

struct ProjectPreview(PathBuf);

impl Render for ProjectPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        div()
            .v_flex()
            .w(ui_px(320.))
            .p_2()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(palette.rgb(BLUE))
            .bg(palette.rgb(PANEL))
            .text_color(palette.rgb(TEXT))
            .shadow_md()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::FolderGit2).size(ui_px(14.)))
                    .child(div().truncate().child(project_label(&self.0))),
            )
            .child(
                div()
                    .text_size(ui_text(11.))
                    .text_color(palette.rgb(MUTED))
                    .child(self.0.to_string_lossy().into_owned()),
            )
    }
}

impl Pokarak {
    pub(super) fn project_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> TabBar {
        let palette = Palette::new(cx);
        let active = self.active_project_index();
        // Reveal on identity, order, or viewport changes, including when a
        // background tab is moved across the active tab without selecting it.
        let reveal = (
            active.and_then(|i| self.projects.get(i).map(|p| p.root.clone())),
            self.projects
                .iter()
                .map(|p| p.root.clone())
                .collect::<Vec<_>>(),
            window.viewport_size().width,
            self.prefs.display.ui_zoom_percent,
            self.prefs.display.ui_font_size,
        );
        let state = window.use_keyed_state("project-tab-reveal", cx, |_, _| None);
        if state.read(cx).as_ref() != Some(&reveal) {
            state.update(cx, |state, _| *state = Some(reveal));
            if let Some(index) = active {
                self.project_tabs.scroll.scroll_to_item(index);
            }
        }

        TabBar::new("project-tabs")
            .segmented()
            .bg(palette.rgb(SIDE))
            .min_w_0()
            .flex_initial()
            .menu(false)
            .max_width(ui_px(if window.viewport_size().width < ui_px(1180.) {
                160.
            } else {
                200.
            }))
            .track_scroll(&self.project_tabs.scroll)
            .when_some(active, |bar, index| bar.selected_index(index))
            .children(self.projects.iter().enumerate().map(|(index, project)| {
                let root = project.root.clone();
                let path = root.to_string_lossy().into_owned();
                let tab =
                    Tab::new()
                        .prefix(Icon::new(IconName::FolderGit2).size(ui_px(18.)).text_color(
                            palette.rgb(if active == Some(index) { BLUE } else { MUTED }),
                        ))
                        .label(project_label(&root))
                        .aria_label(format!("프로젝트 {path} · 드래그로 순서 변경"))
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(w, cx)
                        })
                        // Inactive tabs stay quiet; they close from the project list.
                        .when(active == Some(index), |tab| {
                            tab.suffix(self.project_close_button(&root, cx))
                        })
                        .on_click(cx.listener(move |app, _, w, cx| {
                            app.select_project(&root, w, cx);
                        }));
                self.project_drop_target(tab, project, Surface::Tabs, cx)
            }))
            .when(self.projects.len() > 1, |bar| {
                bar.suffix(self.project_picker(window, cx))
            })
    }

    fn select_project(&mut self, root: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_project_picker_focus(window, cx);
        self.project_tabs.open = false;
        if let Some(project) = self.projects.iter().find(|p| p.root == root)
            && self.opening_path.as_ref().or(self.current.as_ref()) != Some(&project.last_worktree)
        {
            // A different project hides the old editor. Keep the normal
            // workspace focus path while its reader is restored.
            self.focus.focus(window, cx);
            self.open_repository(project.last_worktree.clone(), window, cx);
        }
        cx.notify();
    }

    fn restore_project_picker_focus(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.project_tabs.return_focus.take()
            && self.project_tabs.focus.contains_focused(w, cx)
        {
            previous.focus(w, cx);
        }
    }

    pub(super) fn cancel_project_drag(
        &mut self,
        event: &KeyDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project_tabs.dragging && cx.has_active_drag() && event.keystroke.key == "escape" {
            cx.stop_active_drag(w);
            self.project_tabs.drag_scroll = None;
            self.project_tabs.dragging = false;
            cx.stop_propagation();
            return true;
        }
        false
    }

    fn project_close_button(&self, root: &Path, cx: &mut Context<Self>) -> Button {
        let root = root.to_path_buf();
        let label = project_label(&root);
        Button::new(format!("close-project:{}", root.display()))
            .icon(toolbar::toolbar_glyph(IconName::X))
            .ghost()
            .with_size(ui_px(20.))
            .px_0()
            .accessibility_label(format!("{label} 프로젝트 닫기"))
            .tooltip(format!("{label} 프로젝트 닫기"))
            // A close press must not arm the enclosing row's click or drag.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |app, _, w, cx| {
                cx.stop_propagation();
                let active = app.current_project.as_ref() == Some(&root);
                let picker_open = app.project_tabs.open;
                app.close_project(root.clone(), w, cx);
                if app.projects.iter().any(|p| p.root == root) {
                    return;
                }
                if active {
                    app.project_tabs.return_focus = Some(app.focus.clone());
                }
                if app.projects.len() < 2 {
                    app.project_tabs.open = false;
                    if picker_open {
                        app.project_tabs.focus.focus(w, cx);
                        app.restore_project_picker_focus(w, cx);
                    }
                } else if app.project_tabs.open {
                    if !app
                        .projects
                        .iter()
                        .any(|p| Some(&p.root) == app.project_tabs.cursor.as_ref())
                    {
                        app.project_tabs.cursor = app
                            .active_project_index()
                            .map(|i| app.projects[i].root.clone());
                    }
                    app.project_tabs.focus.focus(w, cx);
                }
            }))
    }

    fn project_picker(&self, window: &mut Window, cx: &mut Context<Self>) -> Popover {
        let active = self.active_project_index();
        let previous_focus = window.focused(cx);
        let palette = Palette::new(cx);
        let width = ui_px(440.).min(window.viewport_size().width - ui_px(100.));
        let height = ui_px((self.projects.len().min(10) * 36) as f32)
            .min(window.viewport_size().height - ui_px(160.));
        let rows = self
            .projects
            .iter()
            .enumerate()
            .map(|(index, project)| {
                let root = project.root.clone();
                let path = root.to_string_lossy().into_owned();
                let selected = active == Some(index);
                let cursor = self.project_tabs.cursor.as_ref() == Some(&root);
                let row = div()
                    .id(format!("project-picker:{}", root.display()))
                    .relative()
                    .h(ui_px(36.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .role(Role::ListBoxOption)
                    .aria_label(format!("프로젝트 {path}"))
                    .aria_description("선택하여 열기 · 드래그로 순서 변경")
                    .aria_selected(selected)
                    .text_color(palette.rgb(TEXT))
                    .when(selected || cursor, |row| row.bg(palette.rgb(PANEL)))
                    .hover(|row| row.bg(palette.rgb(PANEL)))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(w, cx)
                    })
                    .child(
                        Icon::new(if selected {
                            IconName::Check
                        } else {
                            IconName::FolderGit2
                        })
                        .size(ui_px(14.))
                        .text_color(palette.rgb(if selected { BLUE } else { MUTED })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(project_label(&root)),
                    )
                    .child(self.project_close_button(&root, cx))
                    .on_click(cx.listener(move |app, _, w, cx| app.select_project(&root, w, cx)));
                self.project_drop_target(row, project, Surface::List, cx)
            })
            .collect::<Vec<_>>();

        Popover::new("project-picker")
            .open(self.project_tabs.open)
            .track_focus(&self.project_tabs.focus)
            .trigger(
                toolbar_icon("project-list", IconName::ChevronDown, "프로젝트 목록")
                    .tooltip("모든 프로젝트 · 드래그로 순서 변경"),
            )
            .on_open_change(cx.listener(move |app, open, w, cx| {
                app.project_tabs.open = *open;
                if *open {
                    app.project_tabs.return_focus = previous_focus.clone();
                    let index = app.active_project_index().unwrap_or(0);
                    app.project_tabs.cursor = app.projects.get(index).map(|p| p.root.clone());
                    app.project_tabs.list_scroll.scroll_to_item(index);
                } else {
                    app.restore_project_picker_focus(w, cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("project-picker-body")
                    .v_flex()
                    .w(width)
                    .gap_1()
                    .track_focus(&self.project_tabs.focus)
                    .on_key_down(cx.listener(Self::project_picker_key))
                    .child(
                        div()
                            .text_size(ui_text(11.))
                            .text_color(palette.rgb(MUTED))
                            .child("프로젝트 · 드래그로 순서 변경"),
                    )
                    .child(
                        div()
                            .id("project-picker-scroll")
                            .v_flex()
                            .h(height)
                            .overflow_y_scroll()
                            .role(Role::ListBox)
                            .aria_label("열린 프로젝트")
                            .track_scroll(&self.project_tabs.list_scroll)
                            .children(rows),
                    ),
            )
    }

    fn project_picker_key(&mut self, event: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        if !self.project_tabs.open || self.projects.is_empty() {
            return;
        }
        let cursor = self
            .projects
            .iter()
            .position(|p| Some(&p.root) == self.project_tabs.cursor.as_ref())
            .unwrap_or(0);
        let index = match event.keystroke.key.as_str() {
            "up" => cursor.saturating_sub(1),
            "down" => (cursor + 1).min(self.projects.len() - 1),
            "home" => 0,
            "end" => self.projects.len() - 1,
            "enter" | "space" => {
                self.select_project(&self.projects[cursor].root.clone(), w, cx);
                cx.stop_propagation();
                return;
            }
            "escape" => {
                self.restore_project_picker_focus(w, cx);
                self.project_tabs.open = false;
                cx.stop_propagation();
                cx.notify();
                return;
            }
            _ => return,
        };
        self.project_tabs.cursor = Some(self.projects[index].root.clone());
        self.project_tabs.list_scroll.scroll_to_item(index);
        cx.stop_propagation();
        cx.notify();
    }

    fn project_drop_target<E>(
        &self,
        element: E,
        project: &SavedProject,
        surface: Surface,
        cx: &mut Context<Self>,
    ) -> E
    where
        E: StatefulInteractiveElement + ParentElement + Styled,
    {
        let root = project.root.clone();
        let scroll = match surface {
            Surface::Tabs => self.project_tabs.scroll.clone(),
            Surface::List => self.project_tabs.list_scroll.clone(),
        };
        let styled_scroll = scroll.clone();
        let styled_root = root.clone();
        let owner = cx.entity_id();
        let view = cx.weak_entity();
        let styled_view = view.clone();
        let validate_root = root.clone();
        let scroll_owner = cx.weak_entity();
        let color = Palette::new(cx).rgb(BLUE);
        element
            .relative()
            .on_drag(
                ProjectDrag {
                    owner,
                    root: project.root.clone(),
                },
                move |drag, _, w, cx| {
                    let _ = scroll_owner.update(cx, |app, cx| {
                        app.start_project_drag_scroll(drag.clone(), w, cx)
                    });
                    cx.new(|_| ProjectPreview(drag.root.clone()))
                },
            )
            .can_drop(move |value, _, cx| {
                value.downcast_ref::<ProjectDrag>().is_some_and(|drag| {
                    view.upgrade().is_some_and(|app| {
                        accepts(&app.read(cx).projects, owner, drag, &validate_root)
                    })
                })
            })
            .drag_over::<ProjectDrag>(move |style, _, w, cx| {
                let Some(bounds) = styled_view.upgrade().and_then(|app| {
                    project_bounds(&app.read(cx).projects, &styled_scroll, &styled_root)
                }) else {
                    return style;
                };
                let after = surface.after(bounds, w.mouse_position());
                match (surface, after) {
                    (Surface::Tabs, false) => style.border_l(ui_px(2.)).border_color(color),
                    (Surface::Tabs, true) => style.border_r(ui_px(2.)).border_color(color),
                    (Surface::List, false) => style.border_t(ui_px(2.)).border_color(color),
                    (Surface::List, true) => style.border_b(ui_px(2.)).border_color(color),
                }
            })
            .on_drop(cx.listener(move |app, drag: &ProjectDrag, w, cx| {
                let Some(bounds) = project_bounds(&app.projects, &scroll, &root) else {
                    return;
                };
                let after = surface.after(bounds, w.mouse_position());
                if move_project(
                    &mut app.projects,
                    &mut app.prefs,
                    cx.entity_id(),
                    drag,
                    &root,
                    after,
                ) {
                    app.prefs.save();
                    cx.notify();
                }
            }))
    }

    fn start_project_drag_scroll(
        &mut self,
        drag: ProjectDrag,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_tabs.dragging = true;
        self.project_tabs.drag_scroll = Some(cx.spawn_in(w, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep = this
                    .update_in(cx, |app, w, cx| {
                        if !cx.has_active_drag()
                            || !accepts(&app.projects, cx.entity_id(), &drag, &drag.root)
                        {
                            app.project_tabs.dragging = false;
                            return false;
                        }
                        let position = w.mouse_position();
                        let ui = &app.project_tabs;
                        for (handle, surface) in [
                            (&ui.scroll, Surface::Tabs),
                            (&ui.list_scroll, Surface::List),
                        ] {
                            if matches!(surface, Surface::List) && !ui.open {
                                continue;
                            }
                            let bounds = handle.bounds();
                            if !bounds.contains(&position) {
                                continue;
                            }
                            let mut offset = handle.offset();
                            match surface {
                                Surface::Tabs => {
                                    offset.x = (offset.x
                                        - edge_delta(position.x, bounds.left(), bounds.right()))
                                    .clamp(-handle.max_offset().x, px(0.))
                                }
                                Surface::List => {
                                    offset.y = (offset.y
                                        - edge_delta(position.y, bounds.top(), bounds.bottom()))
                                    .clamp(-handle.max_offset().y, px(0.))
                                }
                            }
                            if offset != handle.offset() {
                                handle.set_offset(offset);
                                w.refresh();
                            }
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }
}

fn project_bounds(
    projects: &[SavedProject],
    scroll: &ScrollHandle,
    root: &Path,
) -> Option<Bounds<Pixels>> {
    let index = projects.iter().position(|project| project.root == root)?;
    let mut bounds = scroll.bounds_for_item(index)?;
    // ScrollHandle children are measured before scrolling. Use the whole row
    // (including icon and close action), translated into window coordinates.
    bounds.origin += scroll.offset();
    Some(bounds)
}

fn edge_delta(position: Pixels, start: Pixels, end: Pixels) -> Pixels {
    let zone = ui_px(24.).min((end - start) / 3.);
    if position < start + zone {
        -ui_px(8.)
    } else if position > end - zone {
        ui_px(8.)
    } else {
        px(0.)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectDrag, Surface, edge_delta, move_project};
    use crate::prefs::{Preferences, SavedProject, SavedView};
    use crate::ui::project_index_for_path;
    use gpui_kit::{Bounds, EntityId, point, px, size};
    use std::path::{Path, PathBuf};

    fn fixture() -> (Vec<SavedProject>, Preferences, EntityId) {
        let projects: Vec<_> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|name| {
                let root = PathBuf::from(format!("/fixture/{name}"));
                let worktree = PathBuf::from(format!("/linked/{name}"));
                SavedProject {
                    root: root.clone(),
                    last_worktree: worktree.clone(),
                    worktrees: vec![root, worktree],
                }
            })
            .collect();
        let prefs = Preferences {
            open_projects: projects.clone(),
            ..Default::default()
        };
        (projects, prefs, EntityId::from(1))
    }

    fn drag(owner: EntityId, name: &str) -> ProjectDrag {
        ProjectDrag {
            owner,
            root: PathBuf::from(format!("/fixture/{name}")),
        }
    }

    fn order(projects: &[SavedProject]) -> Vec<String> {
        projects
            .iter()
            .map(|p| p.root.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn first_last_and_both_sides_of_interior_targets() {
        let (mut projects, mut prefs, owner) = fixture();
        for (source, target, after, expected) in [
            ("a", "d", true, ["b", "c", "d", "a"]),
            ("a", "b", false, ["a", "b", "c", "d"]),
            ("d", "b", false, ["a", "d", "b", "c"]),
            ("d", "b", true, ["a", "b", "d", "c"]),
        ] {
            assert!(move_project(
                &mut projects,
                &mut prefs,
                owner,
                &drag(owner, source),
                Path::new(&format!("/fixture/{target}")),
                after
            ));
            assert_eq!(order(&projects), expected);
            assert_eq!(order(&prefs.open_projects), expected);
        }
    }

    #[test]
    fn same_and_adjacent_slots_leave_order_and_preferences_untouched() {
        let (mut projects, mut prefs, owner) = fixture();
        let before = serde_json::to_value(&prefs).unwrap();
        for (target, after) in [("b", false), ("b", true), ("a", true), ("c", false)] {
            assert!(!move_project(
                &mut projects,
                &mut prefs,
                owner,
                &drag(owner, "b"),
                Path::new(&format!("/fixture/{target}")),
                after
            ));
            assert_eq!(order(&projects), ["a", "b", "c", "d"]);
            assert_eq!(serde_json::to_value(&prefs).unwrap(), before);
        }
    }

    #[test]
    fn missing_source_target_and_foreign_owner_are_rejected() {
        let (mut projects, mut prefs, owner) = fixture();
        let before = serde_json::to_value(&prefs).unwrap();
        for (payload, target) in [
            (drag(EntityId::from(2), "a"), "/fixture/c"),
            (drag(owner, "closed"), "/fixture/c"),
            (drag(owner, "a"), "/fixture/closed"),
        ] {
            assert!(!move_project(
                &mut projects,
                &mut prefs,
                owner,
                &payload,
                Path::new(target),
                true
            ));
        }
        assert_eq!(order(&projects), ["a", "b", "c", "d"]);
        assert_eq!(serde_json::to_value(&prefs).unwrap(), before);
        projects.clear();
        assert!(!move_project(
            &mut projects,
            &mut prefs,
            owner,
            &drag(owner, "a"),
            Path::new("/fixture/b"),
            false
        ));
    }

    #[test]
    fn drag_resolves_roots_after_an_intervening_reorder() {
        let (mut projects, mut prefs, owner) = fixture();
        let started = drag(owner, "c");
        assert!(move_project(
            &mut projects,
            &mut prefs,
            owner,
            &drag(owner, "d"),
            Path::new("/fixture/a"),
            false
        ));
        assert!(move_project(
            &mut projects,
            &mut prefs,
            owner,
            &started,
            Path::new("/fixture/b"),
            false
        ));
        assert_eq!(order(&projects), ["d", "a", "c", "b"]);
        // A closed source cannot move the project now occupying its old index.
        projects.retain(|p| p.root != started.root);
        assert!(!move_project(
            &mut projects,
            &mut prefs,
            owner,
            &started,
            Path::new("/fixture/a"),
            true
        ));
        assert_eq!(order(&projects), ["d", "a", "b"]);
    }

    #[test]
    fn active_linked_and_nested_project_identity_and_saved_reader_survive() {
        let (mut projects, mut prefs, owner) = fixture();
        projects[2].root = "/fixture/a/nested".into();
        let current_project = projects[2].root.clone();
        let current_worktree = projects[2].last_worktree.clone();
        prefs.last_root = Some(current_worktree.clone());
        prefs.worktrees.insert(
            current_worktree.clone(),
            SavedView {
                selected: Some("src/lib.rs".into()),
                tabs: vec!["src/lib.rs".into(), "draft.rs".into()],
                code: true,
                line: 37,
                column: 8,
                scroll_y: -240.,
                ..Default::default()
            },
        );
        let reader = serde_json::to_value(&prefs.worktrees).unwrap();
        let record = serde_json::to_value(&projects[2]).unwrap();
        let payload = ProjectDrag {
            owner,
            root: current_project.clone(),
        };
        assert!(move_project(
            &mut projects,
            &mut prefs,
            owner,
            &payload,
            Path::new("/fixture/a"),
            false
        ));
        assert_eq!(projects[0].root, current_project);
        for path in [
            &current_worktree,
            &PathBuf::from("/fixture/a/nested/src/lib.rs"),
        ] {
            assert_eq!(project_index_for_path(&projects, path), Some(0));
        }
        assert_eq!(serde_json::to_value(&projects[0]).unwrap(), record);
        // The existing open_projects representation survives a restart without
        // replacing last_root or any worktree's editor/navigation preferences.
        let restored: Preferences =
            serde_json::from_value(serde_json::to_value(&prefs).unwrap()).unwrap();
        assert_eq!(restored.open_projects[0].root, current_project);
        assert_eq!(restored.last_root, Some(current_worktree));
        assert_eq!(serde_json::to_value(&restored.worktrees).unwrap(), reader);
    }

    #[test]
    fn insertion_halves_and_scroll_edges_follow_each_surface() {
        let bounds = Bounds::new(point(px(20.), px(40.)), size(px(100.), px(36.)));
        assert!(!Surface::Tabs.after(bounds, point(px(69.), px(75.))));
        assert!(Surface::Tabs.after(bounds, point(px(70.), px(41.))));
        assert!(!Surface::List.after(bounds, point(px(119.), px(57.))));
        assert!(Surface::List.after(bounds, point(px(21.), px(58.))));
        assert!(edge_delta(px(21.), px(20.), px(120.)) < px(0.));
        assert_eq!(edge_delta(px(70.), px(20.), px(120.)), px(0.));
        assert!(edge_delta(px(119.), px(20.), px(120.)) > px(0.));
    }
}
