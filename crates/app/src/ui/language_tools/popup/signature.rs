use super::*;
use pokarak_navigation::{ParameterLabel, SignatureInformation};
use std::ops::Range;

impl Pokarak {
    pub(super) fn signature_popup(
        &self,
        panel: &Panel,
        help: Option<&SignatureHelp>,
        space: &PopupSpace,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = Palette::new(cx);
        let Some((help, sig)) = help.and_then(|h| Some((h, h.signatures.get(panel.cursor)?)))
        else {
            return div()
                .w(ui_px(230.).min(space.body_width()))
                .px_2()
                .py_1()
                .text_ellipsis()
                .child("이 위치에 함수 시그니처가 없습니다.")
                .into_any_element();
        };
        let index = sig.active_parameter.or(help.active_parameter).unwrap_or(0) as usize;
        let parameter = sig.parameters.get(index);
        let span = parameter_range(sig, index);
        // A text label that the server did not include verbatim can still identify
        // the active argument below the signature.
        let fallback = if span.is_none() {
            parameter.and_then(|p| parameter_text(&sig.label, &p.label))
        } else {
            None
        };
        let highlight = HighlightStyle {
            color: Some(palette.rgb(BLUE).into()),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let label = StyledText::new(sig.label.clone())
            .with_highlights(span.into_iter().map(|r| (r, highlight)));
        let count = help.signatures.len();
        let controls = count > 1;
        let header_height = ui_text(24.).min(space.body_height() / 2.);
        div()
            .v_flex()
            .w(ui_px(440.).min(space.body_width()))
            .when(controls, |p| {
                p.child(
                    div()
                        .h_flex()
                        .h(header_height)
                        .flex_none()
                        .px_2()
                        .gap_1()
                        .border_b_1()
                        .border_color(palette.rgb(LINE))
                        .child(
                            div()
                                .flex_1()
                                .text_color(palette.rgb(MUTED))
                                .text_size(ui_text(11.))
                                .child(format!("오버로드 {} / {count}", panel.cursor + 1)),
                        )
                        .child(
                            Button::new("previous-signature")
                                .small()
                                .ghost()
                                .tab_stop(false)
                                .h(header_height)
                                .icon(IconName::ChevronUp)
                                .tooltip("이전 오버로드 ↑")
                                .accessibility_label("이전 오버로드")
                                .disabled(panel.cursor == 0)
                                .on_click(
                                    cx.listener(|app, _, w, cx| app.popup_key(PopupKey::Up, w, cx)),
                                ),
                        )
                        .child(
                            Button::new("next-signature")
                                .small()
                                .ghost()
                                .tab_stop(false)
                                .h(header_height)
                                .icon(IconName::ChevronDown)
                                .tooltip("다음 오버로드 ↓")
                                .accessibility_label("다음 오버로드")
                                .disabled(panel.cursor + 1 == count)
                                .on_click(cx.listener(|app, _, w, cx| {
                                    app.popup_key(PopupKey::Down, w, cx)
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id(("signature-content", panel.cursor))
                    .overflow_y_scroll()
                    .max_h(
                        (space.body_height() - if controls { header_height } else { px(0.) })
                            .min(ui_px(175.)),
                    )
                    .px_2()
                    .py_1()
                    .v_flex()
                    .gap_1()
                    .child(div().font_family("JetBrains Mono").child(label))
                    .when_some(fallback, |d, name| {
                        d.child(
                            div()
                                .text_color(palette.rgb(BLUE))
                                .child(format!("활성 매개변수 · {name}")),
                        )
                    })
                    .children(
                        parameter
                            .and_then(|p| p.documentation.as_ref())
                            .map(|d| div().child(d.value.clone())),
                    )
                    .children(
                        sig.documentation
                            .as_ref()
                            .map(|d| div().text_color(palette.rgb(MUTED)).child(d.value.clone())),
                    ),
            )
            .into_any_element()
    }
}

// LSP offsets count UTF-16 code units across the entire signature, including newlines.
fn utf16_boundary(label: &str, units: u32) -> Option<usize> {
    let mut position = 0;
    for (byte, ch) in label.char_indices() {
        if position == units {
            return Some(byte);
        }
        position += ch.len_utf16() as u32;
    }
    (position == units).then_some(label.len())
}

fn parameter_range(sig: &SignatureInformation, active: usize) -> Option<Range<usize>> {
    let label = &sig.label;
    let offsets = |start, end| {
        let range = utf16_boundary(label, start)?..utf16_boundary(label, end)?;
        (!range.is_empty()).then_some(range)
    };
    if let ParameterLabel::Offsets { start, end } = sig.parameters.get(active)?.label {
        return offsets(start, end);
    }
    // When a server sends strings, match parameters in declaration order so
    // identical labels refer to the right occurrence.
    let mut start = label.find('(').map_or(0, |p| p + 1);
    for (i, param) in sig.parameters.iter().enumerate().take(active + 1) {
        let range = match &param.label {
            ParameterLabel::Offsets { start, end } => offsets(*start, *end)?,
            ParameterLabel::Text(text) if !text.is_empty() => {
                let pos = start + label.get(start..)?.find(text)?;
                pos..pos + text.len()
            }
            _ => return None,
        };
        if i == active {
            return Some(range);
        }
        start = range.end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ParameterLabel, SignatureInformation, parameter_range, utf16_boundary};
    use pokarak_navigation::ParameterInformation;

    #[test]
    fn highlights_the_selected_parameter_with_repeated_labels_and_multiline_utf16_offsets() {
        let signature = |label: &str, parameters: Vec<ParameterLabel>| SignatureInformation {
            label: label.into(),
            parameters: parameters
                .into_iter()
                .map(|label| ParameterInformation {
                    label,
                    documentation: None,
                })
                .collect(),
            documentation: None,
            active_parameter: None,
        };
        let sig = signature("foo(foo, foo)", vec![ParameterLabel::Text("foo".into()); 2]);
        assert_eq!(parameter_range(&sig, 0), Some(4..7));
        assert_eq!(parameter_range(&sig, 1), Some(9..12));
        let sig = signature(
            "f(😀,\n 안녕)",
            vec![ParameterLabel::Offsets { start: 7, end: 9 }],
        );
        assert_eq!(&sig.label[parameter_range(&sig, 0).unwrap()], "안녕");
        assert_eq!(utf16_boundary(&sig.label, 3), None);
    }
}
