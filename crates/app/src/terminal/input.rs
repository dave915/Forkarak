use super::{TermMode, TerminalDock};
use gpui_kit::*;
use std::ops::Range;

pub(super) fn key_bytes(key: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = key.modifiers;
    if m.platform {
        return None;
    }
    let modifiers = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let k = key.key.as_str();
    let enhanced =
        mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC);
    let code = match k {
        "up" | "down" | "right" | "left" | "home" | "end" | "f1" | "f2" | "f3" | "f4" => {
            let suffix = match k {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                "end" => 'F',
                "f1" => 'P',
                "f2" => 'Q',
                "f3" => 'R',
                _ => 'S',
            };
            if modifiers != 1 {
                format!("\x1b[1;{modifiers}{suffix}")
            } else if mode.contains(TermMode::APP_CURSOR) || k.starts_with('f') {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[{suffix}")
            }
        }
        "insert" | "delete" | "pageup" | "pagedown" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10"
        | "f11" | "f12" => {
            let n = match k {
                "insert" => 2,
                "delete" => 3,
                "pageup" => 5,
                "pagedown" => 6,
                "f5" => 15,
                "f6" => 17,
                "f7" => 18,
                "f8" => 19,
                "f9" => 20,
                "f10" => 21,
                "f11" => 23,
                _ => 24,
            };
            if modifiers != 1 {
                format!("\x1b[{n};{modifiers}~")
            } else {
                format!("\x1b[{n}~")
            }
        }
        "escape" | "enter" | "backspace" | "tab" => {
            let b = match k {
                "escape" => 27,
                "enter" => 13,
                "backspace" => 127,
                _ => 9,
            };
            if enhanced
                && (modifiers != 1
                    || k == "escape"
                    || mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC))
            {
                format!("\x1b[{b};{modifiers}u")
            } else if k == "tab" && m.shift {
                "\x1b[Z".into()
            } else {
                format!("{}{}", if m.alt { "\x1b" } else { "" }, char::from(b))
            }
        }
        _ if m.control || m.alt => {
            let ch = if k == "space" {
                ' '
            } else {
                let mut chars = k.chars();
                let ch = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                ch
            };
            if enhanced {
                format!("\x1b[{};{modifiers}u", ch as u32)
            } else if m.control {
                let ctrl = match ch.to_ascii_uppercase() {
                    ' ' | '@' | '2' => 0,
                    'A'..='_' => (ch.to_ascii_uppercase() as u8) & 31,
                    '?' => 127,
                    _ => return None,
                };
                format!("{}{}", if m.alt { "\x1b" } else { "" }, char::from(ctrl))
            } else {
                format!("\x1b{}", if m.shift { ch.to_ascii_uppercase() } else { ch })
            }
        }
        _ => return None, // GPUI's native text input handles printable text and IME composition.
    };
    Some(code.into_bytes())
}

impl EntityInputHandler for TerminalDock {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<u16> = self.marked.encode_utf16().collect();
        let start = range.start.min(utf16.len());
        let end = range.end.clamp(start, utf16.len());
        *actual = Some(start..end);
        Some(String::from_utf16_lossy(&utf16[start..end]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.ime_selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let text = std::mem::take(&mut self.marked);
        self.ime_selection = 0..0;
        if let Some(shell) = &self.shell {
            shell.write(text.into_bytes());
        }
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        self.ime_selection = 0..0;
        if let Some(shell) = &self.shell {
            shell.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut before: Vec<u16> = self.marked.encode_utf16().collect();
        let replace = range.unwrap_or(0..before.len());
        let start = replace.start.min(before.len());
        let end = replace.end.clamp(start, before.len());
        before.splice(start..end, text.encode_utf16());
        self.marked = String::from_utf16_lossy(&before);
        let len = self.marked.encode_utf16().count();
        self.ime_selection = selection
            .map(|r| (start + r.start).min(len)..(start + r.end).min(len))
            .unwrap_or(len..len);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            self.metrics.at(self.cursor.0, self.cursor.1),
            self.metrics.cell,
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
    fn paste(&mut self, item: ClipboardItem, _: &mut Window, cx: &mut Context<Self>) {
        if let (Some(shell), Some(text)) = (&self.shell, item.text()) {
            shell.paste(&text);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TermMode, key_bytes};
    use gpui_kit::Keystroke;

    #[test]
    fn navigation_keys_follow_the_running_programs_modes() {
        let key = |k: &'static str, mode| key_bytes(&Keystroke::parse(k).unwrap(), mode);
        assert_eq!(key("up", TermMode::empty()).unwrap(), b"\x1b[A");
        assert_eq!(key("up", TermMode::APP_CURSOR).unwrap(), b"\x1bOA");
        assert_eq!(key("shift-left", TermMode::empty()).unwrap(), b"\x1b[1;2D");
        assert_eq!(key("ctrl-c", TermMode::empty()).unwrap(), [3]);
        assert_eq!(
            key("shift-enter", TermMode::DISAMBIGUATE_ESC_CODES).unwrap(),
            b"\x1b[13;2u"
        );
        assert!(key("a", TermMode::empty()).is_none());
    }
}
