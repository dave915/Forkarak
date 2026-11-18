use super::*;

struct Bulb {
    row: usize,
    position: LspPosition,
    origin: Point<Pixels>,
    side: Pixels,
}

/// Use the editor's actual layout, including wrapping, folds and horizontal scroll.
fn visible_bulbs(state: &EditorState, items: &[Diagnostic]) -> Vec<Bulb> {
    let Some(rows) = state.visible_row_range() else {
        return vec![];
    };
    let viewport = state.input_bounds();
    let gutter = state.text_origin_offset().unwrap_or_default();
    let mut marked = std::collections::BTreeMap::new();
    for diagnostic in items {
        let range = diagnostic.range;
        let first = rows.start.max(range.start.line as usize);
        let last = rows.end.min(range.end.line as usize + 1);
        for row in first..last {
            if row > range.start.line as usize
                && row == range.end.line as usize
                && range.end.character == 0
            {
                continue;
            }
            let character = if row == range.start.line as usize {
                range.start.character
            } else {
                0
            };
            marked.entry(row).or_insert(LspPosition {
                line: row as u32,
                character,
            });
        }
    }
    let mut targets = Vec::<Bulb>::new();
    for (row, position) in marked {
        let start = state.text().line_start_offset(row);
        let Some(bounds) = state.range_to_bounds(&(start..start)) else {
            continue;
        };
        let side = bounds.size.height.min(gutter).min(ui_px(19.));
        if side <= px(0.) || bounds.top() < viewport.top() || bounds.bottom() > viewport.bottom() {
            continue;
        }
        let origin = point(viewport.left(), bounds.top());
        if targets
            .last()
            .is_none_or(|prior| prior.origin.y != origin.y)
        {
            targets.push(Bulb {
                row,
                position,
                origin,
                side,
            });
        }
    }
    targets
}

impl Pokarak {
    pub(in crate::ui) fn diagnostic_bulbs(
        &self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (identity, items) = self.language_tools.diagnostics.as_ref()?;
        if identity.editor != editor.entity_id()
            || !identity.snapshot.is_current()
            || items.is_empty()
        {
            return None;
        }
        let owner = cx.entity();
        let editor = editor.clone();
        Some(
            canvas(
                move |_, window, cx| {
                    owner.update(cx, |app, cx| {
                        let Some((identity, items)) = &app.language_tools.diagnostics else {
                            return vec![];
                        };
                        if !identity.snapshot.is_current()
                            || identity.editor != editor.entity_id()
                            || app.language_tools.view != identity.view
                        {
                            return vec![];
                        }
                        visible_bulbs(editor.read(cx), items)
                            .into_iter()
                            .map(|bulb| {
                                let expected = identity.clone();
                                let label = format!("{}줄 빠른 수정", bulb.row + 1);
                                let mut element = div()
                                    .occlude()
                                    .child(
                                        Button::new(("diagnostic-lightbulb", bulb.row))
                                            .accessibility_label(label.clone())
                                            .tooltip(label)
                                            .ghost()
                                            .small()
                                            .w(bulb.side)
                                            .h(bulb.side)
                                            .p_0()
                                            .child(div().text_size(bulb.side * 0.78).child("💡"))
                                            .on_click(cx.listener(move |app, _, w, cx| {
                                                if !app.accepts_language(&expected, false, cx) {
                                                    return;
                                                }
                                                let Some(editor) = app.active_editor() else {
                                                    return;
                                                };
                                                let Some(at) =
                                                    offset(editor.read(cx).text(), bulb.position)
                                                else {
                                                    return;
                                                };
                                                editor.update(cx, |s, cx| {
                                                    s.set_selected_range(at..at, cx);
                                                    s.focus(w, cx);
                                                });
                                                app.request_language(Tool::QuickFix, None, w, cx);
                                            })),
                                    )
                                    .into_any_element();
                                element.layout_as_root(AvailableSpace::min_size(), window, cx);
                                element.prepaint_at(bulb.origin, window, cx);
                                element
                            })
                            .collect::<Vec<_>>()
                    })
                },
                |_, bulbs, window, cx| {
                    for mut bulb in bulbs {
                        bulb.paint(window, cx);
                    }
                },
            )
            .absolute()
            .into_any_element(),
        )
    }
}
