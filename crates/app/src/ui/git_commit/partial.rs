use super::*;

impl Pokarak {
    pub(super) fn toggle_partial_split(&mut self, cx: &mut Context<Self>) {
        if self.commit_ui.split.take().is_none()
            && let Some(preview) = &self.commit_ui.partial
        {
            let document = Arc::new(DiffDocument {
                lines: preview.lines().to_vec(),
                ..Default::default()
            });
            self.commit_ui.split = Some(self.new_split_diff(document, cx));
        }
        cx.notify();
    }

    pub(super) fn apply_line_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_disabled() || self.commit_ui.marked.is_empty() {
            return;
        }
        let (Some(preview), Some(root)) =
            (self.commit_ui.partial.clone(), self.commit_ui.root.clone())
        else {
            return;
        };
        let selected = self.commit_ui.marked.iter().copied().collect::<Vec<_>>();
        let action = if self.commit_ui.side == Some(Side::Unstaged) {
            "선택한 줄 Stage"
        } else {
            "선택한 줄 Unstage"
        };
        self.commit_ui.request += 1;
        self.commit_ui.preview_request += 1;
        self.run_git_job(
            root,
            action.into(),
            move |ops| ops.apply_selected_lines(&preview, &selected),
            window,
            cx,
        );
    }

    fn toggle_lines(&mut self, rows: &[usize], cx: &mut Context<Self>) {
        if self.commit_disabled() {
            return;
        }
        let remove = rows.iter().all(|r| self.commit_ui.marked.contains(r));
        for row in rows {
            if remove {
                self.commit_ui.marked.remove(row);
            } else {
                self.commit_ui.marked.insert(*row);
            }
        }
        cx.notify();
    }

    pub(super) fn partial_rows(
        &self,
        preview: Arc<PartialPreview>,
        weak: WeakEntity<Pokarak>,
        palette: Palette,
    ) -> AnyElement {
        let disabled = self.commit_disabled();
        let font_size = ui_px(self.prefs.display.code_font_size);
        let marked = self.commit_ui.marked.clone();
        let token = self.commit_ui.preview_request;
        uniform_list("partial-diff", preview.lines().len(), move |range, _, _| {
            range
                .map(|i| {
                    let line = &preview.lines()[i];
                    let (prefix, color, selectable) = match line.kind {
                        DiffKind::Addition => ("+", GREEN, true),
                        DiffKind::Deletion => ("−", RED, true),
                        DiffKind::Header if line.text.starts_with("@@ ") => ("블록", BLUE, true),
                        DiffKind::Header => ("", MUTED, false),
                        DiffKind::Context => ("", TEXT, false),
                    };
                    let hunk = prefix == "블록";
                    let checked = if hunk {
                        let rows = preview.hunk_lines(i);
                        !rows.is_empty() && rows.iter().all(|row| marked.contains(row))
                    } else {
                        marked.contains(&i)
                    };
                    let marker = if !selectable {
                        ""
                    } else if checked {
                        "☑"
                    } else {
                        "☐"
                    };
                    let label = if let Some(number) = line.new_line.or(line.old_line) {
                        format!("{marker} {number:>4}  {prefix} {}", line.text)
                    } else {
                        format!("{marker} {prefix} {}", line.text)
                    };
                    if !selectable {
                        return div()
                            .h(theme::code_row_height(font_size))
                            .px_2()
                            .whitespace_nowrap()
                            .font_family("JetBrains Mono")
                            .text_size(font_size)
                            .text_color(palette.rgb(color))
                            .child(label)
                            .into_any_element();
                    }
                    let target = preview.clone();
                    commit_button(
                        &weak,
                        ("partial-row", i),
                        label,
                        disabled,
                        move |app, _, cx| {
                            if token == app.commit_ui.preview_request {
                                if hunk {
                                    app.toggle_lines(&target.hunk_lines(i), cx);
                                } else {
                                    app.toggle_lines(&[i], cx);
                                }
                            }
                        },
                    )
                    .ghost()
                    .selected(checked)
                    .h(theme::code_row_height(font_size))
                    .min_w_full()
                    .justify_start()
                    .font_family("JetBrains Mono")
                    .text_size(font_size)
                    .text_color(palette.rgb(color))
                    .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .h_full()
        .w_full()
        .into_any_element()
    }
}
