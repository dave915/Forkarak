use super::*;

impl Pokarak {
    pub(super) fn rename_popup(&self, space: &PopupSpace) -> Option<AnyElement> {
        let input = self.language_tools.rename_input.as_ref()?;
        Some(
            div()
                .id("rename-symbol-popover")
                .w(ui_px(300.).min(space.body_width()))
                .p_2()
                .v_flex()
                .gap_1()
                .child("이름 바꾸기")
                .child(
                    Input::new(input)
                        .small()
                        .id("rename-symbol-input")
                        .accessibility_id("rename-symbol-input")
                        .aria_label("새 심볼 이름"),
                )
                .child(div().text_size(ui_text(11.)).child("Enter 적용 · Esc 취소"))
                .into_any_element(),
        )
    }

    pub(super) fn code_actions_popup(
        &self,
        panel: &Panel,
        actions: &workspace::GroupedActions,
        space: &PopupSpace,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use workspace::ActionRow;
        let palette = Palette::new(cx);
        let has_reasons = actions.items.iter().any(|a| a.disabled.is_some());
        let row_height = ui_px(if has_reasons { 36. } else { 27. });
        let owner = cx.entity();
        let token = self.language_tools.request;
        let count = actions.rows.len();
        let list = uniform_list("quick-fix-list", count, move |range, _, cx| {
            owner.update(cx, |app, cx| {
                let Some(panel) = app.language_tools.panel.as_ref() else {
                    return vec![];
                };
                let Reply::Actions(actions, _) = &panel.value else {
                    return vec![];
                };
                range
                    .filter_map(|row| {
                        let index = match *actions.rows.get(row)? {
                            ActionRow::Heading(group) => {
                                return Some(
                                    div()
                                        .id(("quick-fix-heading", row))
                                        .role(Role::Heading)
                                        .aria_label(group.label())
                                        .h(row_height)
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .text_size(ui_text(11.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(palette.rgb(MUTED))
                                        .bg(palette.rgb(SIDE))
                                        .child(group.label())
                                        .into_any_element(),
                                );
                            }
                            ActionRow::Item(index) => index,
                        };
                        let item = actions.items.get(index)?;
                        let reason = item.disabled.as_deref();
                        let selected = index == panel.cursor && reason.is_none();
                        let title = &item.title;
                        let label = match reason {
                            Some(reason) => format!("{title} · {reason}"),
                            None => title.clone(),
                        };
                        let body = dialogs::choice_body()
                            .v_flex()
                            .text_color(palette.rgb(if reason.is_some() { MUTED } else { TEXT }))
                            .child(div().text_ellipsis().child(title.clone()))
                            .when_some(reason, |d, reason| {
                                d.child(
                                    div()
                                        .text_size(ui_text(10.))
                                        .text_ellipsis()
                                        .child(reason.to_owned()),
                                )
                            });
                        Some(
                            Button::new(("quick-fix-action", index))
                                .ghost()
                                .small()
                                .accessibility_label(label.clone())
                                .selected(selected)
                                .disabled(reason.is_some())
                                .h(row_height)
                                .w_full()
                                .child(body)
                                .tooltip(label)
                                .on_click(cx.listener(move |app, _, w, cx| {
                                    app.pick_code_action(token, index, w, cx)
                                }))
                                .into_any_element(),
                        )
                    })
                    .collect()
            })
        })
        .track_scroll(&self.language_tools.scroll)
        .h((row_height * count.min(11) as f32).min(space.body_height()))
        .w_full();
        div()
            .id("quick-fix-popover")
            .aria_label(panel.tool.title())
            .w(ui_px(415.).min(space.body_width()))
            .max_h(space.body_height())
            .when(count == 0, |d| {
                d.child(div().p_2().child("이 위치에 적용할 빠른 수정이 없습니다."))
            })
            .when(count > 0, |d| d.child(list))
            .into_any_element()
    }
}
