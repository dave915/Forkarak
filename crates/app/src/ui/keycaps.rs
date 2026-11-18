//! Shortcut explanations are GPUI keycaps, independent of native menu equivalents.
use super::*;
use gpui_kit::component::{kbd::Kbd, tooltip::Tooltip};

fn single(key: &str) -> Keystroke {
    Keystroke {
        key: key.into(),
        ..Keystroke::default()
    }
}

fn keys(stroke: &Keystroke) -> Vec<Keystroke> {
    let m = stroke.modifiers;
    // GPUI can store a modifier in the key field as well as in the flags.
    let key = stroke.key.as_str();
    let mut keys = Vec::new();
    for (pressed, cap) in [
        (m.control || key == "control", "ctrl"),
        (m.alt || key == "alt", "alt"),
        (m.shift || key == "shift", "shift"),
        (m.platform || key == "platform", "cmd"),
        (m.function || key == "function", "fn"),
    ] {
        if pressed {
            keys.push(single(cap));
        }
    }
    if !key.is_empty() && !stroke.is_modifier_key() {
        keys.push(single(key));
    }
    keys
}

fn strokes(binding: &str) -> Option<Vec<Keystroke>> {
    binding
        .split_whitespace()
        .map(|s| Keystroke::parse_with_os(s, OperatingSystem::Mac))
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

fn stroke_caps(stroke: &Keystroke, palette: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(ui_px(3.))
        .flex_none()
        .children(keys(stroke).into_iter().map(|key| {
            Kbd::new(key)
                .outline()
                .min_w(ui_px(19.))
                .min_h(ui_px(20.))
                .px(ui_px(4.))
                .py(ui_px(2.))
                .rounded(ui_px(4.))
                .border_b(ui_px(2.))
                .border_color(palette.rgb(0x52687c))
                .bg(palette.rgb(PANEL))
                .text_color(palette.rgb(TEXT))
                .text_size(ui_text(11.))
        }))
}

/// Preserve chord boundaries for multi-stroke custom bindings; punctuation,
/// function keys and named keys are parsed by GPUI rather than split on '-'.
pub(super) fn binding(binding: &str, palette: Palette) -> Div {
    let mut row = div().flex().items_center().gap(ui_px(8.)).flex_none();
    match strokes(binding) {
        Some(strokes) => {
            for stroke in strokes {
                row = row.child(stroke_caps(&stroke, palette));
            }
        }
        None => {
            row = row.child(binding.to_owned());
        }
    }
    row
}

pub(super) fn effective<'a>(
    prefs: &'a Preferences,
    project: Option<&Path>,
    id: &str,
    fallback: &'a str,
) -> &'a str {
    prefs
        .option(project, "keyboard.bindings")
        .and_then(|v| v.get(id))
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
}

pub(super) fn hint(label: impl Into<SharedString>, binding_text: &str, palette: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(ui_px(6.))
        .flex_none()
        .child(label.into())
        .child(binding(binding_text, palette))
}

#[derive(Clone, Debug, PartialEq)]
enum Part {
    Text(String),
    Binding(String),
}

fn explanation_parts(text: &str, prefs: &Preferences, project: Option<&Path>) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['⌘', '⇧', '⌥', '⌃']) {
        if start > 0 {
            parts.push(Part::Text(rest[..start].into()));
        }
        rest = &rest[start..];
        let mut chord = String::new();
        let mut end = 0;
        for (i, ch) in rest.char_indices() {
            let modifier = match ch {
                '⌘' => "cmd-",
                '⇧' => "shift-",
                '⌥' => "alt-",
                '⌃' => "ctrl-",
                _ => break,
            };
            chord.push_str(modifier);
            end = i + ch.len_utf8();
        }
        let tail = &rest[end..];
        let key_end = if tail.starts_with("Space") {
            5
        } else if tail.starts_with('F') && tail.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
            tail.char_indices()
                .skip(1)
                .take_while(|(_, ch)| ch.is_ascii_digit())
                .last()
                .map_or(1, |(i, ch)| i + ch.len_utf8())
        } else {
            tail.chars()
                .next()
                .filter(|ch| ch.is_ascii_alphanumeric() || "/[]↑↓←→⏎".contains(*ch))
                .map_or(0, char::len_utf8)
        };
        if key_end == 0 {
            // Modifier-click instructions keep the mouse label as Korean text.
            for modifier in chord.trim_end_matches('-').split('-') {
                parts.push(Part::Binding(modifier.into()));
            }
        } else {
            let raw = &tail[..key_end];
            chord.push_str(match raw {
                "↑" => "up",
                "↓" => "down",
                "←" => "left",
                "→" => "right",
                "⏎" => "enter",
                "Space" => "space",
                _ => raw,
            });
            chord.make_ascii_lowercase();
            let normalized = strokes(&chord);
            let override_key = super::settings::KEYBOARD_DEFAULTS
                .iter()
                .find(|(id, _, default)| {
                    // Option-arrow also belongs to DiffReview. That fixed hint
                    // must not inherit an editor-only move-line override.
                    !["move_line_up", "move_line_down"].contains(id)
                        && strokes(default) == normalized
                })
                .map(|(id, _, default)| effective(prefs, project, id, default))
                .unwrap_or(&chord);
            parts.push(Part::Binding(override_key.into()));
        }
        rest = &tail[key_end..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.into()));
    }
    parts
}

fn render_parts(parts: &[Part], palette: Palette) -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .gap(ui_px(3.))
        .children(parts.iter().map(|part| match part {
            Part::Text(text) => div().child(text.clone()).into_any_element(),
            Part::Binding(keys) => binding(keys, palette).into_any_element(),
        }))
}

/// Korean prose and mouse instructions stay intact; managed chords use overrides.
pub(super) fn explanation(
    text: &str,
    prefs: &Preferences,
    project: Option<&Path>,
    palette: Palette,
) -> Div {
    render_parts(&explanation_parts(text, prefs, project), palette)
}

/// Native menus keep system text/key equivalents; embedded explanatory chords
/// use the same glyphs and current overrides, separated for readability.
pub(super) fn menu_label(text: &str, prefs: &Preferences, project: Option<&Path>) -> String {
    explanation_parts(text, prefs, project)
        .into_iter()
        .map(|part| match part {
            Part::Text(text) => text,
            Part::Binding(binding) => strokes(&binding)
                .unwrap_or_default()
                .iter()
                .map(|stroke| {
                    keys(stroke)
                        .iter()
                        .map(Kbd::format)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join("  "),
        })
        .collect()
}

pub(super) fn popup_item(text: &str) -> PopupMenuItem {
    let parts = explanation_parts(text, &Preferences::default(), None);
    if !parts.iter().any(|p| matches!(p, Part::Binding(_))) {
        return PopupMenuItem::new(text.to_owned());
    }
    let label = SharedString::from(text.to_owned());
    PopupMenuItem::element(move |_, cx| {
        div()
            .id(label.clone())
            .aria_label(label.clone())
            .child(render_parts(&parts, Palette::new(cx)))
    })
}

pub(super) trait ShortcutTooltip {
    fn shortcut_tooltip(
        self,
        label: impl Into<SharedString>,
        action: &dyn Action,
        context: Option<&str>,
    ) -> AnyElement;
    fn keycap_tooltip(self, label: impl Into<SharedString>, keys: &str) -> AnyElement;
    fn explanation_tooltip(
        self,
        text: &str,
        prefs: &Preferences,
        project: Option<&Path>,
    ) -> AnyElement;
}

impl ShortcutTooltip for Button {
    fn shortcut_tooltip(
        self,
        label: impl Into<SharedString>,
        action: &dyn Action,
        context: Option<&str>,
    ) -> AnyElement {
        let label = label.into();
        let action = action.boxed_clone();
        let context = context.map(str::to_owned);
        div()
            .id(label.clone())
            .flex_none()
            .child(self.accessibility_label(label.clone()))
            .tooltip(move |w, cx| {
                let key_context = context.as_deref().and_then(|s| KeyContext::parse(s).ok());
                let binding = match key_context {
                    Some(context) => {
                        w.highest_precedence_binding_for_action_in_context(action.as_ref(), context)
                    }
                    None => w.highest_precedence_binding_for_action(action.as_ref()),
                };
                let keys = binding
                    .map(|b| {
                        b.keystrokes()
                            .iter()
                            .map(|s| s.as_keystroke().unparse(cx.platform_info().os))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let label = label.clone();
                Tooltip::element(move |_, cx| hint(label.clone(), &keys, Palette::new(cx)))
                    .build(w, cx)
            })
            .into_any_element()
    }

    fn keycap_tooltip(self, label: impl Into<SharedString>, keys: &str) -> AnyElement {
        let label = label.into();
        let keys = keys.to_owned();
        div()
            .id(label.clone())
            .flex_none()
            .child(self.accessibility_label(label.clone()))
            .tooltip(move |w, cx| {
                let label = label.clone();
                let keys = keys.clone();
                Tooltip::element(move |_, cx| hint(label.clone(), &keys, Palette::new(cx)))
                    .build(w, cx)
            })
            .into_any_element()
    }
    fn explanation_tooltip(
        self,
        text: &str,
        prefs: &Preferences,
        project: Option<&Path>,
    ) -> AnyElement {
        let parts = explanation_parts(text, prefs, project);
        let label = SharedString::from(text.to_owned());
        div()
            .id(SharedString::from(text.to_owned()))
            .flex_none()
            .child(self.accessibility_label(label))
            .tooltip(move |w, cx| {
                let parts = parts.clone();
                Tooltip::element(move |_, cx| render_parts(&parts, Palette::new(cx))).build(w, cx)
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Part, explanation_parts, keys, menu_label, strokes};
    use crate::prefs::Preferences;
    use gpui_kit::component::kbd::Kbd;

    #[test]
    fn modifier_only_keycaps_render_glyphs_for_click_hints() {
        let hint = "⌘클릭 / ⌃클릭 / ⌥클릭 / ⇧클릭";
        assert_eq!(menu_label(hint, &Preferences::default(), None), hint);

        for (binding, expected) in [
            ("cmd-ctrl", vec!["⌃", "⌘"]),
            ("cmd-ctrl-", vec!["⌃", "⌘"]),
            ("fn", vec!["Fn"]),
        ] {
            let parsed = strokes(binding).unwrap();
            let labels: Vec<_> = keys(&parsed[0]).iter().map(Kbd::format).collect();
            assert_eq!(labels, expected, "{binding}");
        }
    }

    #[test]
    fn splits_modifiers_without_splitting_named_or_punctuation_keys() {
        for (binding, expected) in [
            ("cmd-shift-p", vec!["⇧", "⌘", "P"]),
            ("alt-f7", vec!["⌥", "F7"]),
            ("cmd--", vec!["⌘", "-"]),
            ("ctrl-alt-down", vec!["⌃", "⌥", "↓"]),
            ("fn-space", vec!["Fn", "Space"]),
        ] {
            let parsed = strokes(binding).unwrap();
            let labels: Vec<_> = keys(&parsed[0]).iter().map(Kbd::format).collect();
            assert_eq!(labels, expected, "{binding}");
        }
        assert_eq!(strokes("cmd-k cmd-f7").unwrap().len(), 2);
        assert!(strokes("not-a-modifier-f7").is_none());
    }

    #[test]
    fn explanations_follow_project_bindings_and_keep_korean_mouse_instructions() {
        let project = std::path::Path::new("/fixture/project");
        let mut prefs = Preferences::default();
        prefs.set_option(
            None,
            "keyboard.bindings",
            serde_json::json!({"find_file": "ctrl-f7", "move_line_down": "ctrl-down"}),
        );
        prefs.set_option(
            Some(project),
            "keyboard.bindings",
            serde_json::json!({"find_file": "cmd-k cmd-p", "save_file": "alt-f12"}),
        );
        assert_eq!(
            explanation_parts("⌘클릭 정의 / ⌘P 파일 찾기 / ⌘S 저장", &prefs, Some(project)),
            vec![
                Part::Binding("cmd".into()),
                Part::Text("클릭 정의 / ".into()),
                Part::Binding("cmd-k cmd-p".into()),
                Part::Text(" 파일 찾기 / ".into()),
                Part::Binding("alt-f12".into()),
                Part::Text(" 저장".into()),
            ]
        );
        assert_eq!(menu_label("파일 찾기  ⌘P", &prefs, None), "파일 찾기  ⌃ F7");
        assert!(strokes("cmd").is_some());
        assert_eq!(
            explanation_parts("사용처 ⌥F7 · 이동 ⌥↓", &prefs, None),
            vec![
                Part::Text("사용처 ".into()),
                Part::Binding("alt-f7".into()),
                Part::Text(" · 이동 ".into()),
                Part::Binding("alt-down".into()),
            ]
        );
    }
}
