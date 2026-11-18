use super::*;
use gpui_kit::component::menu::PopupMenu;
use std::rc::Rc;

type ActivateTab = Rc<dyn Fn(&mut Pokarak, &mut Window, &mut Context<Pokarak>) -> bool>;

impl Pokarak {
    pub(in crate::ui) fn active_file_tab(&self) -> Option<PathBuf> {
        if self.git_history.active || self.opening_path.is_some() || self.result_tab_active() {
            return None;
        }
        let session = self.session()?;
        session
            .saved
            .selected
            .as_ref()
            .filter(|path| session.saved.tabs.contains(path))
            .cloned()
    }
}

pub(in crate::ui) fn tab_side_menu(
    owner: WeakEntity<Pokarak>,
    root: Option<PathBuf>,
    request: u64,
    path: PathBuf,
    activate: impl Fn(&mut Pokarak, &mut Window, &mut Context<Pokarak>) -> bool + 'static,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu {
    side_menu(owner, root, request, path, Some(Rc::new(activate)))
}

pub(in crate::ui) fn tree_side_menu(
    owner: WeakEntity<Pokarak>,
    root: Option<PathBuf>,
    request: u64,
    path: PathBuf,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu {
    side_menu(owner, root, request, path, None)
}

fn side_menu(
    owner: WeakEntity<Pokarak>,
    root: Option<PathBuf>,
    request: u64,
    path: PathBuf,
    tab: Option<ActivateTab>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu {
    move |menu, _, _| {
        let item = |label, move_tab| {
            let owner = owner.clone();
            let root = root.clone();
            let path = path.clone();
            let tab = tab.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let _ = owner.update(cx, |app, cx| {
                    if app.git_history.active
                        || app.opening_path.is_some()
                        || root.is_none()
                        || app.current != root
                        || app.request != request
                    {
                        return;
                    }
                    if let Some(activate) = &tab
                        && (!activate(app, window, cx)
                            || !app.session().is_some_and(|s| s.saved.tabs.contains(&path)))
                    {
                        return;
                    }
                    // Dismiss restores the previous focus if the menu still owns it.
                    // Give the file load a stable starting focus before it picks its reader.
                    app.focus.focus(window, cx);
                    if move_tab {
                        app.move_to_other(path.clone(), window, cx);
                    } else {
                        app.open_to_side(path.clone(), window, cx);
                    }
                });
            })
        };
        let menu = menu
            .label(path.to_string_lossy().into_owned())
            .item(item("옆에 열기", false));
        if tab.is_some() {
            menu.item(item("다른 쪽으로 이동", true))
        } else {
            menu
        }
    }
}
