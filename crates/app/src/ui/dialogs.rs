//! Application dialog defaults. Keep placement, focus and action layout in GPUI.
use super::*;
use gpui_kit::component::dialog::Dialog;

pub(super) trait DialogWindowExt {
    /// Inherit left-aligned headings, body text and field labels, preserving the
    /// native popup position, focus handling and footer/button layout.
    fn open_app_dialog(
        &mut self,
        cx: &mut App,
        build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    );
}

impl DialogWindowExt for Window {
    fn open_app_dialog(
        &mut self,
        cx: &mut App,
        build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    ) {
        self.open_dialog(cx, move |dialog, window, cx| {
            build(dialog.text_left(), window, cx)
        });
    }
}

/// Buttons center their inner content independently of the outer flex layout.
/// A full-width child gives textual choice rows a shared left edge while keeping
/// the button's hit target, keyboard behavior and selected styling intact.
pub(super) fn choice_body() -> Div {
    div().w_full().min_w_0().text_left()
}
