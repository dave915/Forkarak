use super::*;

impl Pokarak {
    pub(in crate::ui) fn language_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        let full =
            self.command_bar_density.get("working-reader") == Some(&reader_toolbar::Density::Full);
        Button::new("language-tools")
            .icon(IconName::Waypoints)
            .when(full, |b| b.label("코드 지원…"))
            .accessibility_label("코드 지원")
            .tooltip("코드 지원 · 자동완성, 오류 확인, 함수 도움말, 코드 탐색")
            .ghost()
            .small()
            .flex_none()
            .h(ui_px(28.))
            .when(!full, |b| b.w(ui_px(28.)))
            .loading(self.language_tools.pending.is_some())
            .disabled(!self.code_support_available(cx))
            .on_click(cx.listener(|app, _, w, cx| app.show_code_support_menu(w, cx)))
            .into_any_element()
    }
    pub(in crate::ui) fn code_support_available(&self, cx: &App) -> bool {
        self.language_target(cx).is_some()
    }
    pub(super) fn prepare_language_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.language_tools.panel.as_mut()
            && panel.is_caret_popup()
        {
            if let Reply::Signature(Some(help)) = &panel.value {
                panel.cursor = help.active_signature.unwrap_or(0) as usize;
            } else if matches!(panel.value, Reply::Complete(_)) {
                self.language_tools
                    .scroll
                    .scroll_to_item(panel.cursor, ScrollStrategy::Top);
            }
            if self.popup_editor(window, cx).is_none() {
                self.language_tools.panel = None;
            }
            return;
        }
        self.language_tools
            .focus
            .get_or_insert_with(|| cx.focus_handle());
        if self.language_tools.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("이름으로 검색…"));
            let subscription =
                cx.subscribe_in(&input, window, |a, _, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    if let Some(panel) = a.language_tools.panel.as_mut()
                        && matches!(panel.value, Reply::Symbols(_))
                    {
                        panel.cursor = 0;
                    }
                    cx.notify();
                });
            self.language_tools.input = Some(input);
            self.language_tools.input_subscription = Some(subscription);
        }
        if let Some(input) = &self.language_tools.input {
            input.update(cx, |e, cx| e.set_value("", window, cx));
        }
        if self
            .language_tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.value, Reply::Symbols(_)))
        {
            if let Some(input) = &self.language_tools.input {
                input.read(cx).focus_handle(cx).focus(window, cx);
            }
        } else if let Some(focus) = &self.language_tools.focus {
            focus.focus(window, cx);
        }
    }
    fn language_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.language_tools.panel.as_ref() else {
            return;
        };
        let key = event.keystroke.key.as_str();
        if !matches!(key, "up" | "down" | "enter" | "escape") {
            return;
        }
        cx.stop_propagation();
        if key == "escape" {
            self.dismiss_language(window, cx);
            return;
        }
        let query = self
            .language_tools
            .input
            .as_ref()
            .map(|i| i.read(cx).value().to_lowercase())
            .unwrap_or_default();
        let count = match &panel.value {
            Reply::Symbols(v) => symbol_rows(v, &query, &panel.collapsed).len(),
            Reply::Diagnostics(v) => v.len(),
            _ => 0,
        };
        if key == "enter" {
            match &panel.value {
                Reply::Symbols(v) => {
                    if let Some(row) = symbol_rows(v, &query, &panel.collapsed).get(panel.cursor) {
                        self.jump_language(row.position, window, cx);
                    }
                }
                Reply::Diagnostics(v) => {
                    if let Some(item) = v.get(panel.cursor) {
                        self.jump_language(item.range.start, window, cx);
                    }
                }
                _ => {}
            }
            return;
        }
        if count == 0 {
            return;
        }
        let old = panel.cursor;
        let next = if key == "up" {
            old.saturating_sub(1)
        } else {
            (old + 1).min(count - 1)
        };
        if let Some(panel) = &mut self.language_tools.panel {
            panel.cursor = next;
        }
        self.language_tools
            .scroll
            .scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }
    fn symbols_sidebar(&self) -> bool {
        self.language_tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.value, Reply::Symbols(_)))
            && self.prefs.option_string(
                self.current_project.as_deref(),
                "navigation.symbols_placement",
                "sidebar",
            ) == "sidebar"
    }
    pub(in crate::ui) fn with_language_sidebar(
        &self,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.symbols_sidebar() {
            return content;
        }
        div()
            .h_flex()
            .size_full()
            .child(
                div()
                    .w(ui_px(250.))
                    .min_w(ui_px(150.))
                    .h_full()
                    .flex_none()
                    .border_r_1()
                    .border_color(Palette::new(cx).rgb(LINE))
                    .child(self.render_language_panel(cx)),
            )
            .child(div().flex_1().min_w_0().h_full().child(content))
            .into_any_element()
    }
    pub(in crate::ui) fn language_bottom_panel(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.language_tools.panel.as_ref()?;
        if panel.is_caret_popup() || matches!(panel.value, Reply::Symbols(_)) {
            return None;
        }
        Some(
            div()
                .h(ui_px(235.))
                .max_h(relative(0.4))
                .min_h_0()
                .flex_none()
                .child(self.render_language_panel(cx))
                .into_any_element(),
        )
    }
    pub(in crate::ui) fn language_popover(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.language_tools.panel.as_ref()?;
        if panel.is_caret_popup() {
            if self.popup_editor(window, cx).is_none() {
                self.language_tools.panel = None;
                return None;
            }
            return Some(self.caret_popup(cx));
        }
        if !matches!(panel.value, Reply::Symbols(_)) || self.symbols_sidebar() {
            return None;
        }
        let width = ui_px(470.).min(window.viewport_size().width - ui_px(24.));
        Some(
            deferred(
                div()
                    .absolute()
                    .top(ui_px(112.))
                    .right(ui_px(20.))
                    .w(width)
                    .h(ui_px(360.))
                    .shadow_lg()
                    .child(self.render_language_panel(cx)),
            )
            .into_any_element(),
        )
    }
    fn render_language_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(panel) = self.language_tools.panel.as_ref() else {
            return div().into_any_element();
        };
        let palette = Palette::new(cx);
        let title = panel.tool.title();
        let query = self
            .language_tools
            .input
            .as_ref()
            .map(|i| i.read(cx).value().to_lowercase())
            .unwrap_or_default();
        let owner = cx.entity();
        let body = match &panel.value {
            Reply::Diagnostics(items) => {
                let count = items.len();
                div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .when(count == 0, |d| {
                        d.child(
                            div()
                                .p_3()
                                .child("이 소스 버전에서 보고된 진단이 없습니다."),
                        )
                    })
                    .child(
                        uniform_list("language-diagnostics", count, move |range, _, cx| {
                            owner.update(cx, |a, cx| {
                                range
                                    .filter_map(|index| {
                                        let p = a.language_tools.panel.as_ref()?;
                                        let Reply::Diagnostics(v) = &p.value else {
                                            return None;
                                        };
                                        let item = v.get(index)?;
                                        let position = item.range.start;
                                        let (label, color) = severity(item.severity);
                                        Some(
                                            div()
                                                .h(ui_px(48.))
                                                .px_3()
                                                .v_flex()
                                                .child(
                                                    Button::new(("diagnostic", index))
                                                        .ghost()
                                                        .small()
                                                        .w_full()
                                                        .selected(index == p.cursor)
                                                        .label(format!(
                                                            "{label}  {}:{}  {}",
                                                            position.line + 1,
                                                            position.character + 1,
                                                            item.message
                                                                .lines()
                                                                .next()
                                                                .unwrap_or("")
                                                        ))
                                                        .tooltip(item.message.clone())
                                                        .on_click(cx.listener(
                                                            move |a, _, w, cx| {
                                                                if let Some(p) =
                                                                    a.language_tools.panel.as_mut()
                                                                {
                                                                    p.cursor = index;
                                                                }
                                                                a.jump_language(position, w, cx);
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(ui_text(11.))
                                                        .text_color(rgb(color))
                                                        .child(format!(
                                                            "{} · {}",
                                                            item.source
                                                                .as_deref()
                                                                .unwrap_or("언어 서버"),
                                                            item.code
                                                                .as_ref()
                                                                .map(|c| c
                                                                    .as_str()
                                                                    .map(str::to_owned)
                                                                    .unwrap_or_else(
                                                                        || c.to_string()
                                                                    ))
                                                                .unwrap_or_default()
                                                        )),
                                                )
                                                .into_any_element(),
                                        )
                                    })
                                    .collect()
                            })
                        })
                        .flex_1()
                        .min_h_0()
                        .track_scroll(&self.language_tools.scroll),
                    )
                    .into_any_element()
            }
            Reply::Symbols(symbols) => {
                let rows = Arc::new(symbol_rows(symbols, &query, &panel.collapsed));
                let count = rows.len();
                div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .when_some(self.language_tools.input.as_ref(), |d, i| {
                        d.child(Input::new(i).small())
                    })
                    .when(count == 0, |d| {
                        d.child(div().p_3().child("일치하는 문서 심볼이 없습니다."))
                    })
                    .child(
                        uniform_list("language-symbols", count, move |range, _, cx| {
                            owner.update(cx, |a, cx| {
                                range
                                    .filter_map(|index| {
                                        let row = rows.get(index)?.clone();
                                        let p = a.language_tools.panel.as_ref()?;
                                        let expanded = !p.collapsed.contains(&row.id);
                                        let id = row.id;
                                        let pos = row.position;
                                        Some(
                                            div()
                                                .h(ui_px(28.))
                                                .pl(ui_px((row.depth.min(12) * 10) as f32))
                                                .h_flex()
                                                .when(row.branch, |d| {
                                                    d.child(
                                                        Button::new(("symbol-expand", id))
                                                            .small()
                                                            .ghost()
                                                            .label(if expanded {
                                                                "▾"
                                                            } else {
                                                                "▸"
                                                            })
                                                            .on_click(cx.listener(
                                                                move |a, _, _, cx| {
                                                                    if let Some(p) =
                                                                        &mut a.language_tools.panel
                                                                        && !p.collapsed.remove(&id)
                                                                    {
                                                                        p.collapsed.insert(id);
                                                                    }
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                })
                                                .child(
                                                    Button::new(("symbol-target", id))
                                                        .small()
                                                        .ghost()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .selected(index == p.cursor)
                                                        .label(row.name)
                                                        .tooltip(format!(
                                                            "{} · {}:{}",
                                                            row.detail,
                                                            pos.line + 1,
                                                            pos.character + 1
                                                        ))
                                                        .on_click(cx.listener(
                                                            move |a, _, w, cx| {
                                                                if let Some(p) =
                                                                    &mut a.language_tools.panel
                                                                {
                                                                    p.cursor = index;
                                                                }
                                                                a.jump_language(pos, w, cx);
                                                            },
                                                        )),
                                                )
                                                .into_any_element(),
                                        )
                                    })
                                    .collect()
                            })
                        })
                        .flex_1()
                        .min_h_0()
                        .track_scroll(&self.language_tools.scroll),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        };
        let tool = panel.tool.clone();
        div()
            .id("language-panel")
            // Keep lists first in the bubble phase,
            // while excluding the editor beneath a floating language panel.
            // Also consume unused deltas over short lists and the panel header.
            .occlude()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .key_context("ForkarakLanguage")
            .when_some(self.language_tools.focus.as_ref(), |d, f| d.track_focus(f))
            .capture_key_down(cx.listener(Self::language_key))
            .size_full()
            .v_flex()
            .min_h_0()
            .bg(palette.rgb(SIDE))
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .child(
                div()
                    .h(ui_px(32.))
                    .px_2()
                    .flex_none()
                    .h_flex()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .text_size(ui_text(12.))
                            .child(format!("{title} · 현재 파일")),
                    )
                    .child(
                        Button::new("refresh-language-panel")
                            .label("새로 요청")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |a, _, w, cx| {
                                a.request_language(tool.clone(), None, w, cx)
                            })),
                    )
                    .child(
                        Button::new("close-language-panel")
                            .icon(IconName::X)
                            .tooltip("코드 지원 닫기")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|a, _, w, cx| a.dismiss_language(w, cx))),
                    ),
            )
            .child(body)
            .into_any_element()
    }
    pub(in crate::ui) fn language_retry(&self, cx: &mut Context<Self>) -> Option<Button> {
        let tool = self.language_tools.retry.clone()?;
        if !self
            .error
            .as_deref()?
            .starts_with(&format!("{}:", tool.title()))
        {
            return None;
        }
        Some(
            Button::new("retry-language-tool")
                .label("다시 시도")
                .ghost()
                .small()
                .disabled(self.language_tools.pending.is_some())
                .on_click(
                    cx.listener(move |a, _, w, cx| a.request_language(tool.clone(), None, w, cx)),
                ),
        )
    }
}
