use super::*;
use pokarak_navigation::CompletionResult;

// Resolution is optional. An unavailable resolve leaves the originally
// supplied candidate (including its edits, detail and documentation) intact.
pub(in crate::ui::language_tools) fn resolved_candidate(
    nav: &mut Navigator,
    snapshot: &DocumentSnapshot,
    mut item: CompletionItem,
) -> Option<CompletionItem> {
    let resolved = nav.resolve_completion_buffer(snapshot, &item).ok()?.value;
    if resolved.label != item.label {
        return None;
    }
    // These are the three resolve properties advertised by our LSP client.
    item.documentation = resolved.documentation.or(item.documentation);
    item.detail = resolved.detail.or(item.detail);
    if !resolved.additional_text_edits.is_empty() {
        item.additional_text_edits = resolved.additional_text_edits;
    }
    Some(item)
}

impl Pokarak {
    pub(super) fn completion_popup(
        &self,
        panel: &Panel,
        result: &CompletionResult,
        space: &PopupSpace,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = Palette::new(cx);
        let count = result.items.len();
        let Some(selected) = result.items.get(panel.cursor) else {
            return div()
                .w(ui_px(180.).min(space.body_width()))
                .px_2()
                .py_1()
                .text_ellipsis()
                .child("완성 후보가 없습니다.")
                .into_any_element();
        };
        let detail = selected.detail.as_deref().filter(|s| !s.trim().is_empty());
        let documentation = selected
            .documentation
            .as_ref()
            .filter(|d| !d.value.trim().is_empty());
        let has_docs = detail.is_some() || documentation.is_some();
        let side_by_side = has_docs && space.body_width() >= ui_px(560.);
        let width = space.body_width().min(if side_by_side {
            ui_px(660.)
        } else {
            ui_px(350.)
        });
        let height = space.body_height();
        let row = ui_text(25.);
        let footer_height = ui_text(24.);
        let with_footer = height >= row + footer_height;
        let available = if with_footer {
            height - footer_height
        } else {
            height
        };
        let stacked_docs = !side_by_side && has_docs && available >= row * 5.;
        let list_height = (row * count.min(8) as f32).min(if stacked_docs {
            available * 0.65
        } else {
            available
        });
        let body_height = if side_by_side {
            list_height.max(ui_px(105.)).min(available)
        } else {
            list_height
        };

        let owner = cx.entity();
        let token = self.language_tools.documentation_request;
        let list = uniform_list("caret-completions", count, move |range, _, cx| {
            owner.update(cx, |app, cx| {
                let Some(panel) = app.language_tools.panel.as_ref() else {
                    return vec![];
                };
                let Reply::Complete(result) = &panel.value else {
                    return vec![];
                };
                range
                    .filter_map(|index| {
                        let item = result.items.get(index)?;
                        let selected = index == panel.cursor;
                        let detail = item
                            .detail
                            .as_deref()
                            .filter(|s| !s.is_empty())
                            .or_else(|| completion_kind(item.kind));
                        Some(
                            div()
                                .id(("caret-completion", index))
                                .role(Role::ListBoxOption)
                                .aria_label(item.label.clone())
                                .aria_selected(selected)
                                .h(row)
                                .px_2()
                                .h_flex()
                                .gap_2()
                                .cursor_pointer()
                                .when(selected, |d| d.bg(palette.selected()))
                                .hover(move |d| {
                                    d.bg(if selected {
                                        palette.selected_hover()
                                    } else {
                                        palette.hover()
                                    })
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_ellipsis()
                                        .font_family("JetBrains Mono")
                                        .child(item.label.clone()),
                                )
                                .when_some(detail, |d, detail| {
                                    d.child(
                                        div()
                                            .max_w(relative(0.43))
                                            .min_w_0()
                                            .text_ellipsis()
                                            .text_size(ui_text(11.))
                                            .text_color(palette.rgb(MUTED))
                                            .child(
                                                detail
                                                    .lines()
                                                    .next()
                                                    .unwrap_or_default()
                                                    .to_owned(),
                                            ),
                                    )
                                })
                                .on_click(cx.listener(move |app, event: &ClickEvent, w, cx| {
                                    if app.matches_completion_row(token, index)
                                        && app.popup_space(w, cx).is_some()
                                    {
                                        app.select_popup_item(index, w, cx);
                                        if event.click_count() > 1 {
                                            app.popup_key(PopupKey::Enter, w, cx);
                                        }
                                    }
                                }))
                                .into_any_element(),
                        )
                    })
                    .collect()
            })
        })
        .track_scroll(&self.language_tools.scroll)
        .h_full()
        .w_full();
        let docs = div()
            .id(("caret-completion-doc", panel.cursor))
            .overflow_y_scroll()
            .p_2()
            .text_size(ui_text(12.))
            .v_flex()
            .gap_1()
            .children(detail.map(|text| div().font_family("JetBrains Mono").child(text.to_owned())))
            .children(documentation.map(documentation_view));
        let body = div().h_flex().w_full().h(body_height).child(
            div()
                .id("completion-listbox")
                .role(Role::ListBox)
                .aria_label("자동완성 후보")
                .h_full()
                .min_w_0()
                .when(side_by_side, |d| d.w(relative(0.53)))
                .when(!side_by_side, |d| d.w_full())
                .child(list),
        );
        let mut popup = div().v_flex().w(width);
        if side_by_side {
            popup = popup.child(
                body.child(
                    docs.flex_1()
                        .min_w_0()
                        .h_full()
                        .border_l_1()
                        .border_color(palette.rgb(LINE)),
                ),
            );
        } else {
            popup = popup.child(body).when(stacked_docs, |p| {
                p.child(
                    docs.max_h((available - list_height).min(ui_px(100.)))
                        .border_t_1()
                        .border_color(palette.rgb(LINE)),
                )
            });
        }
        let selected_index = panel.cursor;
        popup
            .when(with_footer, |p| {
                p.child(
                    div()
                        .h(footer_height)
                        .w_full()
                        .h_flex()
                        .px_2()
                        .justify_between()
                        .border_t_1()
                        .border_color(palette.rgb(LINE))
                        .text_color(palette.rgb(MUTED))
                        .text_size(ui_text(11.))
                        .child(format!(
                            "{} / {count}{}",
                            panel.cursor + 1,
                            if result.is_incomplete {
                                " · 일부 후보"
                            } else {
                                ""
                            }
                        ))
                        .child(
                            Button::new("accept-caret-completion")
                                .small()
                                .ghost()
                                .tab_stop(false)
                                .h(footer_height)
                                .label("적용 ↵")
                                .accessibility_label("선택한 자동완성 적용")
                                .on_click(cx.listener(move |app, _, w, cx| {
                                    if app.matches_completion_row(token, selected_index) {
                                        app.popup_key(PopupKey::Enter, w, cx);
                                    }
                                })),
                        ),
                )
            })
            .into_any_element()
    }
    // Mouse hitboxes belong to the last painted frame. A newer reply may
    // have replaced the rows before a click reaches its listener.
    fn matches_completion_row(&self, token: u64, index: usize) -> bool {
        self.language_tools.documentation_request == token
            && self
                .language_tools
                .panel
                .as_ref()
                .is_some_and(|p| matches!(&p.value, Reply::Complete(r) if index < r.items.len()))
    }
}

fn completion_kind(kind: Option<u32>) -> Option<&'static str> {
    // LSP CompletionItemKind, used when the server supplies no detail.
    const KINDS: [&str; 25] = [
        "텍스트",
        "메서드",
        "함수",
        "생성자",
        "필드",
        "변수",
        "클래스",
        "인터페이스",
        "모듈",
        "속성",
        "단위",
        "값",
        "열거형",
        "키워드",
        "스니펫",
        "색상",
        "파일",
        "참조",
        "폴더",
        "열거형 멤버",
        "상수",
        "구조체",
        "이벤트",
        "연산자",
        "타입 매개변수",
    ];
    KINDS.get(kind?.checked_sub(1)? as usize).copied()
}
