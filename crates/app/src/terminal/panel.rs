use super::{
    Shell, TermMode,
    input::key_bytes,
    screen::{BACKGROUND, FOREGROUND, Frame, Metrics},
};
use crate::theme::{ui_px, ui_text};
use alacritty_terminal::{
    grid::Scroll,
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        ActiveTheme, Sizable, StyledExt,
        button::{Button, ButtonVariants},
    },
    *,
};
use std::{ops::Range, path::PathBuf, time::Duration};

actions!(terminal, [CopyTerminal, PasteTerminal]);
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-c", CopyTerminal, Some("PokarakTerminal")),
        KeyBinding::new("cmd-v", PasteTerminal, Some("PokarakTerminal")),
    ]);
}
pub enum DockAction {
    Hide,
    Close,
}
pub struct TerminalDock {
    root: PathBuf,
    pub(super) shell: Option<Shell>,
    error: Option<String>,
    wake: Option<Task<()>>,
    focus: FocusHandle,
    pub(super) metrics: Metrics,
    pub(super) cursor: (usize, usize),
    pub(super) marked: String,
    pub(super) ime_selection: Range<usize>,
    wheel_remainder: f32,
}
impl Focusable for TerminalDock {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EventEmitter<DockAction> for TerminalDock {}
impl TerminalDock {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut dock = Self {
            root,
            shell: None,
            error: None,
            wake: None,
            focus: cx.focus_handle(),
            metrics: Metrics::default(),
            cursor: (0, 0),
            marked: String::new(),
            ime_selection: 0..0,
            wheel_remainder: 0.,
        };
        cx.on_focus(&dock.focus, window, |dock, _, cx| {
            dock.report_focus(true);
            cx.notify();
        })
        .detach();
        cx.on_blur(&dock.focus, window, |dock, _, cx| {
            dock.report_focus(false);
            cx.notify();
        })
        .detach();
        dock.start(cx);
        dock
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        self.wake = None;
        self.shell = None;
        self.marked.clear();
        self.ime_selection = 0..0;
        match Shell::open(&self.root, Default::default()) {
            Ok((shell, events)) => {
                self.shell = Some(shell);
                self.error = None;
                self.wake = Some(cx.spawn(async move |view, cx| {
                    while events.recv().await.is_ok() {
                        if view.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                        smol::Timer::after(Duration::from_millis(16)).await;
                    }
                }));
            }
            Err(err) => self.error = Some(format!("{err:#}")),
        }
    }
    fn report_focus(&self, focused: bool) {
        if let Some(shell) = &self.shell
            && shell.mode().contains(TermMode::FOCUS_IN_OUT)
        {
            let bytes = if focused { b"\x1b[I" } else { b"\x1b[O" };
            let _ = shell.io.send(alacritty_terminal::event_loop::Msg::Input(
                bytes.as_slice().into(),
            ));
        }
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.marked.is_empty() {
            return;
        }
        if let Some(shell) = &self.shell
            && let Some(bytes) = key_bytes(&event.keystroke, shell.mode())
        {
            shell.write(bytes);
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
        }
    }
    fn point(&self, pos: Point<Pixels>) -> (usize, usize) {
        let (col, row) = self.metrics.coords(pos);
        let size = self.metrics.size();
        (
            col.min(size.num_cols as usize - 1),
            row.min(size.num_lines as usize - 1),
        )
    }
    fn mouse_packet(&self, code: u8, pressed: bool, pos: Point<Pixels>, m: Modifiers) -> bool {
        let Some(shell) = &self.shell else {
            return false;
        };
        let mode = shell.mode();
        if m.shift || !mode.intersects(TermMode::MOUSE_MODE) {
            return false;
        }
        let (col, row) = self.point(pos);
        let code = code + 8 * u8::from(m.alt) + 16 * u8::from(m.control);
        if mode.contains(TermMode::SGR_MOUSE) {
            shell.write(
                format!(
                    "\x1b[<{code};{};{}{}",
                    col + 1,
                    row + 1,
                    if pressed { 'M' } else { 'm' }
                )
                .into_bytes(),
            );
        } else if row < 223 && col < 223 {
            shell.write(vec![
                27,
                91,
                77,
                32 + if pressed { code } else { 3 },
                33 + col as u8,
                33 + row as u8,
            ]);
        }
        true
    }
    fn mouse_down(&mut self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(w, cx);
        if self.mouse_packet(0, true, e.position, e.modifiers) {
            return;
        }
        let Some(shell) = &self.shell else {
            return;
        };
        let (col, row) = self.point(e.position);
        let mut term = shell.term.lock();
        let point = GridPoint::new(
            Line(row as i32 - term.grid().display_offset() as i32),
            Column(col),
        );
        let ty = match e.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        term.selection = Some(Selection::new(ty, point, Side::Left));
        cx.notify();
    }
    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(shell) = &self.shell else {
            return;
        };
        let mode = shell.mode();
        let pressed = e.pressed_button == Some(MouseButton::Left);
        if (mode.contains(TermMode::MOUSE_MOTION) || pressed && mode.contains(TermMode::MOUSE_DRAG))
            && self.mouse_packet(if pressed { 32 } else { 35 }, true, e.position, e.modifiers)
        {
            return;
        }
        if !pressed || (!e.modifiers.shift && mode.intersects(TermMode::MOUSE_MODE)) {
            return;
        }
        let (col, row) = self.point(e.position);
        let mut term = shell.term.lock();
        let point = GridPoint::new(
            Line(row as i32 - term.grid().display_offset() as i32),
            Column(col),
        );
        if let Some(selection) = &mut term.selection {
            selection.update(point, Side::Right);
            cx.notify();
        }
    }
    fn scroll(&mut self, e: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(shell) = &self.shell else {
            return;
        };
        self.wheel_remainder += match e.delta {
            ScrollDelta::Pixels(p) => p.y / self.metrics.cell.height.max(px(1.)),
            ScrollDelta::Lines(p) => p.y,
        };
        let steps = self.wheel_remainder.trunc() as i32;
        self.wheel_remainder -= steps as f32;
        if steps == 0 {
            return;
        }
        let mode = shell.mode();
        if mode.intersects(TermMode::MOUSE_MODE) && !e.modifiers.shift {
            for _ in 0..steps.unsigned_abs().min(100) {
                self.mouse_packet(
                    if steps > 0 { 64 } else { 65 },
                    true,
                    e.position,
                    e.modifiers,
                );
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
            && !e.modifiers.shift
        {
            let seq = if mode.contains(TermMode::APP_CURSOR) {
                "\x1bO"
            } else {
                "\x1b["
            };
            shell.write(
                format!("{seq}{}", if steps > 0 { 'A' } else { 'B' })
                    .repeat(steps.unsigned_abs().min(100) as usize)
                    .into_bytes(),
            );
        } else {
            shell.term.lock().scroll_display(Scroll::Delta(steps));
        }
        cx.stop_propagation();
        cx.notify();
    }
}
impl Render for TerminalDock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let focus = self.focus.clone();
        let exited = self.shell.as_ref().is_none_or(Shell::exited);
        let font_size = cx.theme().mono_font_size;
        let surface = canvas(
            |_, _, _| (),
            move |bounds, _, w, cx| {
                let _ = weak.update(cx, |dock, cx| {
                    let metrics = Metrics::measured(bounds, font_size, w);
                    dock.metrics = metrics;
                    w.handle_input(&focus, ElementInputHandler::new(bounds, cx.entity()), cx);
                    if let Some(shell) = &dock.shell {
                        shell.resize(metrics.size());
                        let focused = focus.is_focused(w);
                        let frame = Frame::read(&shell.term.lock(), focused);
                        if let Some((col, row, _)) = frame.cursor {
                            dock.cursor = (col, row);
                        }
                        frame.paint(metrics, &dock.marked, focused, w, cx);
                    }
                });
            },
        )
        .size_full();
        div()
            .id("worktree-terminal")
            .v_flex()
            .size_full()
            .min_h_0()
            .bg(rgb(BACKGROUND))
            .child(
                div()
                    .flex_none()
                    .h(ui_px(29.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(0x37404b))
                    .bg(rgb(0x242a31))
                    .text_color(rgb(FOREGROUND))
                    .text_size(ui_text(12.))
                    .child("터미널")
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .child(format!("zsh · {}", self.root.display())),
                    )
                    .when(exited, |bar| {
                        bar.child(
                            Button::new("terminal-restart")
                                .ghost()
                                .small()
                                .label("새 셸")
                                .on_click(cx.listener(|dock, _, w, cx| {
                                    dock.start(cx);
                                    dock.focus.focus(w, cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("terminal-hide")
                            .ghost()
                            .small()
                            .icon(gpui_kit::assets::IconName::ChevronDown)
                            .tooltip("터미널 숨기기")
                            .accessibility_label("터미널 숨기기")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DockAction::Hide))),
                    )
                    .child(
                        Button::new("terminal-close")
                            .ghost()
                            .small()
                            .icon(gpui_kit::assets::IconName::X)
                            .tooltip("터미널 세션 닫기")
                            .accessibility_label("터미널 세션 닫기")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DockAction::Close))),
                    ),
            )
            .when_some(self.error.clone(), |v, err| {
                v.child(div().px_2().text_color(rgb(0xf4a5a5)).child(err))
            })
            .when(exited && self.error.is_none(), |v| {
                v.child(
                    div()
                        .px_2()
                        .text_size(ui_text(12.))
                        .text_color(rgb(0xb3bec9))
                        .child("셸이 종료되었습니다"),
                )
            })
            .child(
                div()
                    .id("terminal-grid")
                    .key_context("PokarakTerminal")
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .cursor_text()
                    .on_action(cx.listener(|dock, _: &PasteTerminal, _, cx| {
                        if let (Some(s), Some(text)) =
                            (&dock.shell, cx.read_from_clipboard().and_then(|i| i.text()))
                        {
                            s.paste(&text);
                            cx.notify();
                        }
                    }))
                    .on_action(cx.listener(|dock, _: &CopyTerminal, _, cx| {
                        if let Some(text) = dock
                            .shell
                            .as_ref()
                            .and_then(|s| s.term.lock().selection_to_string())
                            .filter(|s| !s.is_empty())
                        {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    }))
                    .on_key_down(cx.listener(Self::key))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|dock, e: &MouseUpEvent, _, _| {
                            dock.mouse_packet(0, false, e.position, e.modifiers);
                        }),
                    )
                    .on_scroll(cx.listener(Self::scroll))
                    .child(surface),
            )
    }
}
