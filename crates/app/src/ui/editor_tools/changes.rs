use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Change {
    pub range: Range<usize>,
    pub old: String,
    pub line: u32,
    pub added: bool,
}

// Bound the LCS matrix. For very large unmatched regions the whole region is
// represented by one accurate change instead of blocking the native UI.
pub(in crate::ui) fn compare(old: &str, new: &str) -> Vec<Change> {
    let before: Vec<_> = old.split_inclusive('\n').collect();
    let after: Vec<_> = new.split_inclusive('\n').collect();
    let mut prefix = 0;
    while prefix < before.len().min(after.len()) && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut a = before.len();
    let mut b = after.len();
    while a > prefix && b > prefix && before[a - 1] == after[b - 1] {
        a -= 1;
        b -= 1;
    }
    if a == prefix && b == prefix {
        return vec![];
    }
    let offsets: Vec<_> = std::iter::once(0)
        .chain(after.iter().scan(0, |sum, s| {
            *sum += s.len();
            Some(*sum)
        }))
        .collect();
    if (a - prefix + 1).saturating_mul(b - prefix + 1) > 750_000 {
        return vec![Change {
            range: offsets[prefix]..offsets[b],
            old: before[prefix..a].concat(),
            line: prefix as u32,
            added: a == prefix,
        }];
    }
    let width = b - prefix + 1;
    let height = a - prefix + 1;
    let mut lcs = vec![0u32; width * height];
    for i in (0..height - 1).rev() {
        for j in (0..width - 1).rev() {
            lcs[i * width + j] = if before[prefix + i] == after[prefix + j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (prefix, prefix);
    let mut changes = vec![];
    while i < a || j < b {
        if i < a && j < b && before[i] == after[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (start_i, start_j) = (i, j);
        while i < a || j < b {
            if i < a && j < b && before[i] == after[j] {
                break;
            }
            if j == b
                || (i < a
                    && lcs[(i - prefix + 1) * width + j - prefix]
                        >= lcs[(i - prefix) * width + j - prefix + 1])
            {
                i += 1;
            } else {
                j += 1;
            }
        }
        changes.push(Change {
            range: offsets[start_j]..offsets[j],
            old: before[start_i..i].concat(),
            line: start_j as u32,
            added: i == start_i,
        });
    }
    changes
}

impl Pokarak {
    pub(in crate::ui) fn refresh_inline_tools(
        &mut self,
        root: &Path,
        path: &Path,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = FileKey::new(root, path) else {
            return;
        };
        let request = self
            .code_edits
            .tools
            .marker_request
            .entry(key.clone())
            .or_default();
        *request += 1;
        let request = *request;
        let enabled = self.prefs.option_bool(
            self.current_project.as_deref(),
            "editor.inline_changes",
            true,
        );
        if !enabled {
            if let Some((_, gutter)) = self.code_edits.tools.gutters.remove(&key) {
                gutter.dispose(cx);
            }
            self.code_edits.tools.changes.remove(&key);
            return;
        }
        let Some(old) = self
            .sessions
            .get(root)
            .and_then(|s| s.diffs.get(path))
            .map(|d| d.old_text.clone())
            .or_else(|| {
                self.sessions
                    .get(root)
                    .and_then(|s| s.snapshot.as_ref())
                    .filter(|snapshot| !snapshot.changes.iter().any(|file| file.path == path))
                    .and_then(|_| self.code_edits.expected_text(root, path))
            })
        else {
            if let Some((_, gutter)) = self.code_edits.tools.gutters.remove(&key) {
                gutter.dispose(cx);
            }
            self.code_edits.tools.changes.remove(&key);
            return;
        };
        let text = editor.read(cx).value().to_string();
        let expected = text.clone();
        let id = editor.entity_id();
        let editor = editor.clone();
        let task = cx.background_spawn(async move { compare(&old, &text) });
        cx.spawn(async move |app, cx| {
            let changes = task.await;
            let _ = app.update(cx, |app, cx| {
                if app.code_edits.tools.marker_request.get(&key) != Some(&request)
                    || app.current.as_ref() != Some(&key.root)
                    || app
                        .selected()
                        .as_ref()
                        .and_then(|p| FileKey::new(&key.root, p))
                        .as_ref()
                        != Some(&key)
                    || app.active_editor().is_none_or(|e| e.entity_id() != id)
                    || editor.read(cx).value().as_ref() != expected
                {
                    return;
                }
                let marks = changes
                    .iter()
                    .map(|c| {
                        GutterDecoration::new(c.range.clone()).with_color(
                            rgb(if c.added {
                                GREEN
                            } else if c.range.is_empty() {
                                RED
                            } else {
                                BLUE
                            })
                            .into(),
                        )
                    })
                    .collect();
                if let Some((old_id, collection)) = app
                    .code_edits
                    .tools
                    .gutters
                    .get(&key)
                    .filter(|(old_id, _)| *old_id == id)
                {
                    let _ = old_id;
                    collection.set(marks, cx);
                } else {
                    if let Some((_, old)) = app.code_edits.tools.gutters.remove(&key) {
                        old.dispose(cx);
                    }
                    let collection = editor.update(cx, |e, cx| {
                        e.create_gutter_decorations_collection(marks, cx)
                    });
                    app.code_edits
                        .tools
                        .gutters
                        .insert(key.clone(), (id, collection));
                }
                app.code_edits.tools.changes.insert(key, changes);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn toggle_previous_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some((root, path, editor)) = self.code_tools_target(cx) else {
            return;
        };
        let Some(key) = FileKey::new(&root, &path) else {
            return;
        };
        if self.code_edits.tools.expanded.remove(&key) {
            cx.notify();
            return;
        }
        let cursor = editor.read(cx).cursor();
        if let Some(change) = self
            .code_edits
            .tools
            .changes
            .get(&key)
            .and_then(|items| items.iter().min_by_key(|c| c.range.start.abs_diff(cursor)))
        {
            let position = Position::new(change.line, 0);
            editor.update(cx, |e, cx| e.set_cursor_position(position, window, cx));
            self.code_edits.tools.expanded.insert(key);
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = owner.update(cx, |_, cx| cx.notify());
            });
            cx.notify();
        }
    }

    pub(in crate::ui) fn code_previous_panel(
        &self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = FileKey::new(self.current.as_deref()?, path)?;
        if !self.code_edits.tools.expanded.contains(&key) {
            return None;
        }
        let editor = self.active_editor()?;
        let cursor = editor.read(cx).cursor();
        let change = self
            .code_edits
            .tools
            .changes
            .get(&key)?
            .iter()
            .min_by_key(|c| c.range.start.abs_diff(cursor))?;
        let old = if change.old.is_empty() {
            "(이 위치에 이전 코드가 없습니다 — 추가된 줄)".to_string()
        } else {
            change.old.clone()
        };
        let anchor = editor.read(cx).cursor_layout()?.0.bottom_left();
        Some(
            deferred(
                anchored()
                    .position(anchor)
                    .offset(point(px(0.), ui_px(6.)))
                    .snap_to_window_with_margin(ui_px(8.))
                    .child(
                        div()
                            .id("editor-previous-code")
                            .occlude()
                            .w(ui_px(580.))
                            .max_h(ui_px(220.))
                            .overflow_y_scroll()
                            .px_3()
                            .py_2()
                            .border_1()
                            .border_color(Palette::new(cx).rgb(LINE))
                            .bg(Palette::new(cx).rgb(PANEL))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(ui_text(11.))
                                            .text_color(Palette::new(cx).rgb(RED))
                                            .child(format!(
                                                "{}행 변경 구간 · 비교 기준의 이전 코드",
                                                change.line + 1
                                            )),
                                    )
                                    .child(
                                        Button::new("close-previous-code")
                                            .small()
                                            .ghost()
                                            .label("닫기")
                                            .on_click(cx.listener(|a, _, w, cx| {
                                                a.toggle_previous_code(w, cx)
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .font_family("JetBrains Mono")
                                    .text_size(ui_px(self.prefs.display.code_font_size))
                                    .whitespace_normal()
                                    .child(old),
                            ),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::compare;
    #[test]
    fn changed_ranges_and_prior_content_are_real() {
        let changes = compare("one\ntwo\nthree\n", "one\nnew\nthree\nadded\n");
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].old, "two\n");
        assert_eq!(changes[0].range, 4..8);
        assert_eq!(changes[1].old, "");
        assert!(changes[1].added);
    }
    #[test]
    fn pure_deletion_keeps_boundary_at_end() {
        let c = compare("one\ntwo\n", "one\n");
        assert_eq!(c[0].range, 4..4);
        assert_eq!(c[0].old, "two\n");
        assert!(compare("same", "same").is_empty());
    }
}
