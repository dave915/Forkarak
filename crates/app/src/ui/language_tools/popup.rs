use super::*;
use gpui_kit::component::input::{Enter, Escape, MoveDown, MoveUp};

mod completion;
mod edits;
mod signature;
pub(super) use completion::resolved_candidate;

impl Panel {
    pub(super) fn is_caret_popup(&self) -> bool {
        matches!(
            self.value,
            Reply::Complete(_)
                | Reply::Signature(_)
                | Reply::Hover(_)
                | Reply::RenameInput(..)
                | Reply::Actions(..)
        )
    }
}

impl LanguageTools {
    pub(super) fn dismiss_panel(&mut self) {
        self.on_type_task = None;
        self.rename_input = None;
        if self.panel.take().is_some_and(|p| p.is_caret_popup()) {
            self.documentation_request = self.documentation_request.wrapping_add(1);
            // Invalidate in-flight or debounced popup requests, even at the same caret.
            // Preserve independent pending work such as an explicit format or navigation.
            if matches!(
                self.pending,
                None | Some(Tool::Complete | Tool::Signature | Tool::Hover)
            ) {
                self.request = self.request.wrapping_add(1);
                self.pending = None;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum PopupKey {
    Up,
    Down,
    Enter,
    Escape,
}

#[derive(Debug, PartialEq, Eq)]
enum PopupCommand {
    Stay,
    Dismiss,
    Accept,
    Select(usize),
}

impl PopupKey {
    fn action_command(
        self,
        cursor: usize,
        count: usize,
        enabled: impl Fn(usize) -> bool,
    ) -> PopupCommand {
        match self {
            Self::Escape => PopupCommand::Dismiss,
            Self::Enter if cursor < count && enabled(cursor) => PopupCommand::Accept,
            Self::Up => (0..cursor.min(count))
                .rev()
                .find(|i| enabled(*i))
                .map_or(PopupCommand::Stay, PopupCommand::Select),
            Self::Down => (cursor.saturating_add(1)..count)
                .find(|i| enabled(*i))
                .map_or(PopupCommand::Stay, PopupCommand::Select),
            Self::Enter => PopupCommand::Stay,
        }
    }
    fn command(self, value: &Reply, cursor: usize) -> Option<PopupCommand> {
        if let Reply::Actions(actions, _) = value {
            return Some(self.action_command(cursor, actions.items.len(), |i| {
                actions.items[i].disabled.is_none()
            }));
        }
        if matches!(value, Reply::RenameInput(..)) {
            return match self {
                Self::Escape => Some(PopupCommand::Dismiss),
                Self::Enter => Some(PopupCommand::Accept),
                _ => None,
            };
        }
        let count = match value {
            Reply::Complete(result) => result.items.len(),
            Reply::Signature(Some(help)) => help.signatures.len(),
            Reply::Signature(None) | Reply::Hover(_) => 0,
            _ => return None,
        };
        let completion = matches!(value, Reply::Complete(_));
        match self {
            Self::Escape => Some(PopupCommand::Dismiss),
            Self::Enter if completion && cursor < count => Some(PopupCommand::Accept),
            Self::Up | Self::Down if count > 1 || (completion && count == 1) => {
                let index = if matches!(self, Self::Up) {
                    cursor.saturating_sub(1)
                } else {
                    (cursor + 1).min(count - 1)
                };
                Some(PopupCommand::Select(index))
            }
            _ => None,
        }
    }
}

pub(super) fn actions(
    app: &Pokarak,
    root: Stateful<Div>,
    cx: &mut Context<Pokarak>,
) -> Stateful<Div> {
    // Input consumes these bindings as actions before raw key-down listeners run.
    root.when(
        app.language_tools
            .panel
            .as_ref()
            .is_some_and(Panel::is_caret_popup),
        |root| {
            root.capture_action(
                cx.listener(|a, _: &MoveUp, w, cx| a.popup_key(PopupKey::Up, w, cx)),
            )
            .capture_action(
                cx.listener(|a, _: &MoveDown, w, cx| a.popup_key(PopupKey::Down, w, cx)),
            )
            .capture_action(cx.listener(|a, action: &Enter, w, cx| {
                if !action.secondary && !action.shift {
                    a.popup_key(PopupKey::Enter, w, cx);
                }
            }))
            .capture_action(
                cx.listener(|a, _: &Escape, w, cx| a.popup_key(PopupKey::Escape, w, cx)),
            )
        },
    )
}

impl Pokarak {
    pub(super) fn popup_editor(&self, window: &Window, cx: &App) -> Option<Entity<EditorState>> {
        let panel = self.language_tools.panel.as_ref()?;
        let id = &panel.identity;
        let (key, editor) = self.language_target(cx)?;
        let state = editor.read(cx);
        (panel.is_caret_popup()
            && self.language_trusted()
            && id.view == self.language_tools.view
            && id.key == key
            && id.editor == editor.entity_id()
            && id.snapshot.is_current()
            && state.cursor() == id.cursor
            && state.selected_range() == id.selection
            && window.is_window_active()
            && (state.focus_handle(cx).is_focused(window)
                || (matches!(panel.value, Reply::RenameInput(..))
                    && self
                        .language_tools
                        .rename_input
                        .as_ref()
                        .is_some_and(|i| i.read(cx).focus_handle(cx).is_focused(window)))))
        .then_some(editor)
    }

    fn popup_space(&self, window: &Window, cx: &App) -> Option<PopupSpace> {
        let editor = self.popup_editor(window, cx)?;
        let editor = editor.read(cx);
        let anchor = match &self.language_tools.panel.as_ref()?.value {
            Reply::RenameInput(range, _) => offset(editor.text(), range.start)?,
            _ => editor.cursor(),
        };
        let caret = editor.range_to_bounds(&(anchor..anchor))?;
        let input = editor.input_bounds();
        if !input.contains(&caret.center())
            || caret.left() < input.left() + editor.text_origin_offset()?
        {
            return None;
        }
        PopupSpace::new(caret, window.viewport_size())
    }

    fn popup_key(&mut self, key: PopupKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.language_tools.panel.as_ref() else {
            return;
        };
        let Some(command) = key.command(&panel.value, panel.cursor) else {
            return;
        };
        if self.popup_space(window, cx).is_none() && !matches!(command, PopupCommand::Dismiss) {
            return;
        }
        match command {
            PopupCommand::Stay => {}
            PopupCommand::Dismiss => {
                self.dismiss_language(window, cx);
            }
            PopupCommand::Accept => match &panel.value {
                Reply::RenameInput(..) => self.submit_rename(window, cx),
                Reply::Actions(..) => {
                    self.pick_code_action(self.language_tools.request, panel.cursor, window, cx);
                }
                _ => self.apply_completion(window, cx),
            },
            PopupCommand::Select(index) => self.select_popup_item(index, window, cx),
        }
        cx.stop_propagation();
    }

    fn select_popup_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.language_tools.panel.as_mut() else {
            return;
        };
        if panel.cursor == index {
            return;
        }
        panel.cursor = index;
        if let Reply::Actions(actions, _) = &panel.value {
            self.language_tools
                .scroll
                .scroll_to_item(actions.row_for(index), ScrollStrategy::Nearest);
        }
        if matches!(panel.value, Reply::Complete(_)) {
            self.language_tools
                .scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
            self.resolve_candidate(window, cx);
        }
        cx.notify();
    }

    pub(super) fn caret_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = cx.entity();
        // Prepaint after the editor has published this frame's scrolled/wrapped glyph bounds.
        deferred(
            canvas(
                move |_, window, cx| {
                    let (space, above, mut element) = owner.update(cx, |app, cx| {
                        let space = app.popup_space(window, cx)?;
                        let panel = app.language_tools.panel.as_ref()?;
                        let body = match &panel.value {
                            Reply::RenameInput(..) => app.rename_popup(&space)?,
                            Reply::Actions(actions, _) => {
                                app.code_actions_popup(panel, actions, &space, cx)
                            }
                            Reply::Complete(result) => {
                                app.completion_popup(panel, result, &space, cx)
                            }
                            Reply::Signature(help) => {
                                app.signature_popup(panel, help.as_ref(), &space, cx)
                            }
                            Reply::Hover(hover) => hover_popup(hover.as_ref(), &space),
                            _ => return None,
                        };
                        let palette = Palette::new(cx);
                        let element = div()
                            .id("caret-language-popup")
                            .occlude()
                            .when(!matches!(panel.value, Reply::RenameInput(..)), |d| {
                                d.on_any_mouse_down(|_, w, _| w.prevent_default())
                            })
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                                if app
                                    .language_tools
                                    .panel
                                    .as_ref()
                                    .is_some_and(Panel::is_caret_popup)
                                {
                                    app.language_tools.dismiss_panel();
                                    cx.notify();
                                }
                            }))
                            .max_w(space.width)
                            .max_h(space.max_height())
                            .rounded(ui_px(5.))
                            .border_1()
                            .border_color(palette.rgb(LINE))
                            .bg(palette.rgb(PANEL))
                            .text_color(palette.rgb(TEXT))
                            .text_size(ui_text(12.))
                            .shadow_lg()
                            .overflow_hidden()
                            .child(body)
                            .into_any_element();
                        Some((
                            space,
                            matches!(panel.value, Reply::Signature(_) | Reply::Hover(_)),
                            element,
                        ))
                    })?;
                    let measured = element.layout_as_root(AvailableSpace::min_size(), window, cx);
                    element.prepaint_at(space.origin(measured, above), window, cx);
                    Some(element)
                },
                |_, element: Option<AnyElement>, window, cx| {
                    if let Some(mut element) = element {
                        element.paint(window, cx);
                    }
                },
            )
            .absolute(),
        )
        .with_priority(100)
        .into_any_element()
    }
}

fn documentation_view(content: &pokarak_navigation::MarkupContent) -> Div {
    if content.kind != "markdown" {
        return div().child(content.value.clone());
    }
    // Keep server-provided examples passive. Fenced code belongs in the same
    // typeface as the editor, with the surrounding explanation in the UI font.
    let mut view = div().v_flex().gap_1();
    let mut fence: Option<(char, usize)> = None;
    let mut block = Vec::new();
    let flush = |view: Div, lines: &mut Vec<&str>, code: bool| {
        if lines.is_empty() {
            return view;
        }
        let text = lines.join("\n");
        lines.clear();
        view.child(
            div()
                .when(code, |d| d.font_family("JetBrains Mono"))
                .child(text),
        )
    };
    for line in content.value.lines() {
        let trimmed = line.trim();
        let Some(ch) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) else {
            block.push(line);
            continue;
        };
        let count = trimmed.chars().take_while(|c| *c == ch).count();
        match fence {
            None if count >= 3 => {
                view = flush(view, &mut block, false);
                fence = Some((ch, count));
            }
            Some((start, len)) if ch == start && count >= len && trimmed.len() == count => {
                view = flush(view, &mut block, true);
                fence = None;
            }
            _ => block.push(line),
        }
    }
    flush(view, &mut block, fence.is_some())
}

fn hover_popup(hover: Option<&HoverResult>, space: &PopupSpace) -> AnyElement {
    let contents = hover.map_or(&[][..], |result| result.contents.as_slice());
    div()
        .id("caret-hover-content")
        .aria_label("타입 · 설명")
        .w(ui_px(415.).min(space.body_width()))
        .max_h(ui_px(185.).min(space.body_height()))
        .overflow_y_scroll()
        .v_flex()
        .px_2()
        .py_1()
        .gap_1()
        .when(contents.is_empty(), |d| {
            d.child("이 위치에 타입·설명 정보가 없습니다.")
        })
        .children(contents.iter().map(documentation_view))
        .into_any_element()
}

struct PopupSpace {
    caret: Bounds<Pixels>,
    width: Pixels,
    above: Pixels,
    below: Pixels,
}

impl PopupSpace {
    fn new(caret: Bounds<Pixels>, viewport: Size<Pixels>) -> Option<Self> {
        if !Bounds::new(Point::default(), viewport).contains(&caret.center()) {
            return None;
        }
        let edge = ui_px(6.);
        let gap = ui_px(3.);
        let space = Self {
            caret,
            width: viewport.width - edge * 2.,
            above: caret.top() - edge - gap,
            below: viewport.height - edge - caret.bottom() - gap,
        };
        (space.width >= ui_px(48.) && space.max_height() >= ui_px(30.)).then_some(space)
    }

    fn max_height(&self) -> Pixels {
        self.above.max(self.below)
    }

    // Leave room for the shared border around the popup body.
    fn body_height(&self) -> Pixels {
        self.max_height() - px(2.)
    }

    fn body_width(&self) -> Pixels {
        self.width - px(2.)
    }

    fn origin(&self, size: Size<Pixels>, prefer_above: bool) -> Point<Pixels> {
        let above = match (size.height <= self.above, size.height <= self.below) {
            (true, true) => prefer_above,
            (fits_above, _) => fits_above,
        };
        let edge = ui_px(6.);
        let x = self
            .caret
            .left()
            .clamp(edge, edge + (self.width - size.width).max(px(0.)));
        let y = if above {
            self.caret.top() - ui_px(3.) - size.height
        } else {
            self.caret.bottom() + ui_px(3.)
        };
        point(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::{PopupCommand, PopupKey, PopupSpace, Reply, SignatureHelp};
    use gpui_kit::{Bounds, point, px, size};
    use pokarak_navigation::{CompletionResult, SignatureInformation};

    #[test]
    fn placement_follows_caret_flips_and_clamps_to_window_edges() {
        let viewport = size(px(600.), px(240.));
        let popup = size(px(230.), px(72.));
        let space_at = |x, y| {
            PopupSpace::new(
                Bounds::new(point(px(x), px(y)), size(px(0.), px(17.))),
                viewport,
            )
            .unwrap()
        };
        assert_eq!(
            space_at(40., 100.).origin(popup, false),
            point(px(40.), px(120.))
        );
        assert_eq!(
            space_at(40., 100.).origin(popup, true),
            point(px(40.), px(25.))
        );
        assert_eq!(
            space_at(599., 215.).origin(popup, false),
            point(px(364.), px(140.))
        );
        assert_eq!(space_at(0., 2.).origin(popup, true), point(px(6.), px(22.)));
        let short = space_at(100., 116.);
        let full_height = size(short.width, short.max_height());
        assert_eq!(short.origin(full_height, false).y, px(6.));
        assert!(PopupSpace::new(Bounds::default(), size(px(20.), px(20.))).is_none());
    }

    #[test]
    fn quick_fix_keyboard_skips_disabled_actions_and_never_edits_source_at_list_boundaries() {
        let enabled = |index| matches!(index, 0 | 2);
        assert_eq!(
            PopupKey::Down.action_command(0, 4, enabled),
            PopupCommand::Select(2)
        );
        assert_eq!(
            PopupKey::Up.action_command(2, 4, enabled),
            PopupCommand::Select(0)
        );
        assert_eq!(
            PopupKey::Up.action_command(0, 4, enabled),
            PopupCommand::Stay
        );
        assert_eq!(
            PopupKey::Down.action_command(2, 4, enabled),
            PopupCommand::Stay
        );
        assert_eq!(
            PopupKey::Enter.action_command(1, 4, enabled),
            PopupCommand::Stay
        );
        assert_eq!(
            PopupKey::Enter.action_command(0, 0, |_| true),
            PopupCommand::Stay
        );
        assert_eq!(
            PopupKey::Enter.action_command(2, 4, enabled),
            PopupCommand::Accept
        );
        assert_eq!(
            PopupKey::Escape.action_command(0, 0, enabled),
            PopupCommand::Dismiss
        );
    }

    #[test]
    fn hover_and_signatures_leave_editor_keys_available() {
        let signature = SignatureInformation {
            label: "f()".into(),
            documentation: None,
            parameters: vec![],
            active_parameter: None,
        };
        let help = |count| {
            Reply::Signature(Some(SignatureHelp {
                signatures: vec![signature.clone(); count],
                active_signature: None,
                active_parameter: None,
            }))
        };
        let empty = Reply::Complete(CompletionResult {
            items: vec![],
            is_incomplete: false,
        });
        for value in [Reply::Hover(None), help(1), empty] {
            for key in [PopupKey::Enter, PopupKey::Up, PopupKey::Down] {
                assert_eq!(key.command(&value, 0), None);
            }
            assert_eq!(
                PopupKey::Escape.command(&value, 0),
                Some(PopupCommand::Dismiss)
            );
        }
        let overloads = help(3);
        assert_eq!(
            PopupKey::Up.command(&overloads, 1),
            Some(PopupCommand::Select(0))
        );
        assert_eq!(
            PopupKey::Down.command(&overloads, 1),
            Some(PopupCommand::Select(2))
        );
        assert_eq!(PopupKey::Enter.command(&overloads, 1), None);
    }
}
