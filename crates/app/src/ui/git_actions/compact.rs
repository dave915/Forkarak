//! Shared layout for compact Git operation dialogs.
use super::*;
use gpui_kit::component::{dialog::Dialog, tooltip::Tooltip};
use std::{cell::Cell, rc::Rc};

pub(super) fn dialog(dialog: Dialog, owner: &WeakEntity<Pokarak>) -> Dialog {
    let owner = owner.clone();
    dialog
        .centered()
        .width(ui_px(510.))
        .on_close(move |_, _, cx| {
            let _ = owner.update(cx, |app, _| app.invalidate_workflow_read());
        })
}

pub(super) fn row(label: &'static str, value: impl IntoElement, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .w(ui_px(94.))
                .flex_none()
                .text_size(ui_text(12.))
                .text_color(Palette::new(cx).rgb(MUTED))
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(value))
}

pub(super) fn worktree(root: &Path, cx: &App) -> impl IntoElement {
    let path = root.display().to_string();
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| path.as_str().into());
    div()
        .id("git-dialog-worktree")
        .text_ellipsis()
        .text_size(ui_text(12.))
        .text_color(Palette::new(cx).rgb(MUTED))
        .child(format!("워크트리 · {name}"))
        .tooltip(move |w, cx| Tooltip::new(path.clone()).build(w, cx))
}

pub(super) fn disclosure(id: &'static str, label: String, expanded: &Rc<Cell<bool>>) -> Button {
    let expanded = expanded.clone();
    theme::action_button(id, false)
        .small()
        .ghost()
        .icon(if expanded.get() {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        })
        .label(label)
        .on_click(move |_, w, _| {
            expanded.set(!expanded.get());
            w.refresh();
        })
}
