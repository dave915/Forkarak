use super::*;
use gpui_kit::component::menu::DropdownMenu;

impl Pokarak {
    pub(in crate::ui) fn language_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let enabled = self.language_target(cx).is_some();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("language-tools")
                    .label("언어 도구…")
                    .ghost()
                    .small()
                    .disabled(!enabled)
                    .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                        for (label, tool) in [
                            ("코드 완성  ⌃Space", Tool::Complete),
                            ("현재 파일 진단", Tool::Diagnostics),
                            ("문서 심볼 검색  ⌘⇧O", Tool::Symbols),
                            ("함수 시그니처  ⌃⇧Space", Tool::Signature),
                            ("타입 · 문서  ⌥F1", Tool::Hover),
                        ] {
                            let owner = weak.clone();
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                                    let _ = owner.update(cx, |a, cx| {
                                        a.request_language(tool.clone(), None, w, cx)
                                    });
                                }));
                        }
                        let owner = weak.clone();
                        menu.item(PopupMenuItem::new("언어 분석 설정…").on_click(
                            move |_, w, cx| {
                                let _ = owner
                                    .update(cx, |a, cx| a.open_settings_section("language", w, cx));
                            },
                        ))
                    }),
            )
            .when_some(self.language_tools.pending.as_ref(), |d, tool| {
                d.child(
                    div()
                        .text_size(ui_text(11.))
                        .child(format!("{}…", tool.title())),
                )
            })
            .into_any_element()
    }
    pub(super) fn prepare_language_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.language_tools
            .focus
            .get_or_insert_with(|| cx.focus_handle());
        if self.language_tools.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("이름으로 검색…"));
            let subscription = cx.subscribe_in(
                &input,
                window,
                |a, input, event: &InputEvent, window, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    let query = input.read(cx).value().to_lowercase();
                    if let Some(panel) = a.language_tools.panel.as_mut() {
                        panel.cursor = match &panel.value {
                            Reply::Complete(items) => items
                                .items
                                .iter()
                                .position(|i| i.label.to_lowercase().contains(query.as_str()))
                                .unwrap_or(0),
                            _ => 0,
                        };
                    }
                    a.resolve_candidate(window, cx);
                    cx.notify();
                },
            );
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
        let rows = match &panel.value {
            Reply::Complete(v) => v
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| i.label.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            Reply::Symbols(v) => (0..symbol_rows(v, &query, &panel.collapsed).len()).collect(),
            Reply::Diagnostics(v) => (0..v.len()).collect(),
            _ => vec![],
        };
        if key == "enter" {
            match &panel.value {
                Reply::Complete(_) => self.apply_completion(window, cx),
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
        if rows.is_empty() {
            return;
        }
        let old = rows.iter().position(|i| *i == panel.cursor).unwrap_or(0);
        let next = if key == "up" {
            old.saturating_sub(1)
        } else {
            (old + 1).min(rows.len() - 1)
        };
        if let Some(panel) = &mut self.language_tools.panel {
            panel.cursor = rows[next];
        }
        self.language_tools
            .scroll
            .scroll_to_item(next, ScrollStrategy::Nearest);
        self.resolve_candidate(window, cx);
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
        if self.symbols_sidebar() || matches!(panel.value, Reply::Symbols(_)) {
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
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.language_tools.panel.as_ref()?;
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
            Reply::Complete(result) => {
                let rows = Arc::new(
                    result
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| item.label.to_lowercase().contains(&query))
                        .map(|(i, _)| i)
                        .collect::<Vec<_>>(),
                );
                let count = rows.len();
                let details = result
                    .items
                    .get(panel.cursor)
                    .map(|item| {
                        format!(
                            "{}\n{}\n{}",
                            item.label,
                            item.detail.as_deref().unwrap_or(""),
                            item.documentation
                                .as_ref()
                                .map(|m| m.value.as_str())
                                .unwrap_or("")
                        )
                    })
                    .unwrap_or_default();
                let detail_note = panel.details.clone().unwrap_or_default();
                let incomplete = result.is_incomplete;
                div()
                    .h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(relative(0.45))
                            .h_full()
                            .v_flex()
                            .when_some(self.language_tools.input.as_ref(), |d, i| {
                                d.child(Input::new(i).small())
                            })
                            .when(count == 0, |d| {
                                d.child("완성 후보가 없습니다. 커서 위치에서 다시 요청하세요.")
                            })
                            .child(
                                uniform_list("language-completions", count, move |range, _, cx| {
                                    owner.update(cx, |a, cx| {
                                        range
                                            .filter_map(|row| {
                                                let index = *rows.get(row)?;
                                                let p = a.language_tools.panel.as_ref()?;
                                                let Reply::Complete(v) = &p.value else {
                                                    return None;
                                                };
                                                let item = v.items.get(index)?;
                                                Some(
                                                    Button::new(("completion-item", index))
                                                        .label(item.label.clone())
                                                        .tooltip(
                                                            item.detail.clone().unwrap_or_else(
                                                                || item.label.clone(),
                                                            ),
                                                        )
                                                        .small()
                                                        .ghost()
                                                        .w_full()
                                                        .selected(index == p.cursor)
                                                        .on_click(cx.listener(
                                                            move |a, _, w, cx| {
                                                                if let Some(p) =
                                                                    a.language_tools.panel.as_mut()
                                                                {
                                                                    p.cursor = index;
                                                                    p.details = None;
                                                                }
                                                                a.resolve_candidate(w, cx);
                                                                cx.notify();
                                                            },
                                                        ))
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
                            .when(incomplete, |d| {
                                d.child(
                                    div()
                                        .text_size(ui_text(11.))
                                        .child("일부 후보 · 입력 후 다시 요청"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("completion-documentation")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .p_3()
                            .v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_family("JetBrains Mono")
                                    .text_size(ui_text(12.))
                                    .child(details),
                            )
                            .child(
                                div()
                                    .text_size(ui_text(11.))
                                    .text_color(palette.rgb(MUTED))
                                    .child(detail_note),
                            )
                            .child(
                                Button::new("apply-completion")
                                    .label("선택한 완성 적용  ↵")
                                    .small()
                                    .disabled(count == 0)
                                    .on_click(cx.listener(|a, _, w, cx| a.apply_completion(w, cx))),
                            ),
                    )
                    .into_any_element()
            }
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
            Reply::Signature(help) => {
                let mut body = div()
                    .id("signature-documentation")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .v_flex()
                    .gap_2();
                if let Some(help) = help {
                    if let Some(sig) = help
                        .signatures
                        .get(help.active_signature.unwrap_or(0) as usize)
                    {
                        body = body
                            .child(div().font_family("JetBrains Mono").child(sig.label.clone()));
                        let parameter = sig
                            .active_parameter
                            .or(help.active_parameter)
                            .and_then(|i| sig.parameters.get(i as usize).map(|p| (i, p)));
                        if let Some((i, p)) = parameter {
                            body = body.child(div().text_color(palette.rgb(BLUE)).child(format!(
                                "활성 매개변수 {}: {}",
                                i + 1,
                                parameter_text(&sig.label, &p.label).unwrap_or_default()
                            )));
                            if let Some(doc) = &p.documentation {
                                body = body.child(doc.value.clone());
                            }
                        }
                        if let Some(doc) = &sig.documentation {
                            body = body.child(doc.value.clone());
                        }
                        body = body.child(div().text_size(ui_text(11.)).child(format!(
                            "오버로드 {} / {} · 커서에서 다시 요청해 갱신",
                            help.active_signature.unwrap_or(0) + 1,
                            help.signatures.len()
                        )));
                    } else {
                        body = body.child("이 커서 위치에서 함수 시그니처가 없습니다.");
                    }
                } else {
                    body = body.child("이 커서 위치에서 함수 시그니처가 없습니다.");
                }
                body.into_any_element()
            }
            Reply::Hover(hover) => div()
                .id("hover-documentation")
                .overflow_y_scroll()
                .flex_1()
                .min_h_0()
                .p_3()
                .v_flex()
                .gap_2()
                .children(
                    hover
                        .as_ref()
                        .map(|h| {
                            h.contents
                                .iter()
                                .map(|m| div().child(m.value.clone()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_else(|| {
                            vec![div().child("이 위치에서 타입·문서 정보가 없습니다.")]
                        }),
                )
                .into_any_element(),
            _ => div().into_any_element(),
        };
        let tool = panel.tool.clone();
        div()
            .id("language-panel")
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
                            .child(format!("{title} · 의미 분석")),
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
                            .tooltip("언어 도구 닫기")
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
                .label("분석 다시 시도")
                .ghost()
                .small()
                .disabled(self.language_tools.pending.is_some())
                .on_click(
                    cx.listener(move |a, _, w, cx| a.request_language(tool.clone(), None, w, cx)),
                ),
        )
    }
}
