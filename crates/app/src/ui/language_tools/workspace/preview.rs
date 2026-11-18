use super::*;
use crate::ui::dialogs::DialogWindowExt;
use pokarak_navigation::FileEdits;

struct Preview {
    owner: WeakEntity<Pokarak>,
    identity: Identity,
    request: u64,
    label: &'static str,
    edit: WorkspaceEdit,
    expanded: HashMap<usize, Vec<Excerpt>>,
    error: Option<String>,
}

struct Excerpt {
    first_line: usize,
    old: String,
    new: String,
}

/// Merge edits that touch the same source line so both sides show the actual line,
/// even when multiple identifiers change or a replacement spans a newline.
fn excerpts(file: &FileEdits) -> anyhow::Result<Vec<Excerpt>> {
    let lines: Vec<_> = file.original.split('\n').collect();
    let mut edits: Vec<_> = file.edits.iter().collect();
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    let mut result = vec![];
    let mut remaining = edits.as_slice();
    while let Some(first) = remaining.first() {
        let start = first.range.start.line;
        let mut end = first.range.end.line;
        let mut count = 1;
        for edit in &remaining[1..] {
            if edit.range.start.line > end {
                break;
            }
            end = end.max(edit.range.end.line);
            count += 1;
        }
        let old = lines
            .get(start as usize..=end as usize)
            .ok_or_else(|| anyhow::anyhow!("미리보기 줄 범위가 변경되었습니다."))?
            .join("\n");
        let relative: Vec<_> = remaining[..count]
            .iter()
            .map(|item| {
                let mut edit = (*item).clone();
                edit.range.start.line -= start;
                edit.range.end.line -= start;
                edit
            })
            .collect();
        let new = TextEdit::apply_all(&old, &relative)?;
        result.push(Excerpt {
            first_line: start as usize + 1,
            old,
            new,
        });
        remaining = &remaining[count..];
    }
    Ok(result)
}

impl Pokarak {
    pub(super) fn open_workspace_preview(
        &mut self,
        identity: Identity,
        label: &'static str,
        edit: WorkspaceEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.weak_entity();
        let request = self.language_tools.request;
        let preview = cx.new(|_| Preview {
            owner,
            identity,
            request,
            label,
            edit,
            expanded: HashMap::new(),
            error: None,
        });
        window.open_app_dialog(cx, move |dialog, w, _| {
            dialog
                .title(format!("{label} 미리보기"))
                .width(ui_px(680.).min(w.viewport_size().width - ui_px(32.)))
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .child(preview.clone())
        });
    }
}

impl Preview {
    fn verify(&mut self, cx: &mut Context<Self>) -> bool {
        let current = self
            .owner
            .update(cx, |app, cx| {
                app.edit_request_current(self.request, &self.identity, cx)
            })
            .unwrap_or(false);
        if !current {
            self.error = Some("편집 중 내용이 바뀌어 적용하지 않았습니다. 다시 시도하세요.".into());
            cx.notify();
        }
        current
    }
    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.verify(cx) {
            return;
        }
        if self.expanded.remove(&index).is_none() {
            match excerpts(&self.edit.files[index]) {
                Ok(lines) => {
                    self.expanded.insert(index, lines);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        cx.notify();
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.error.is_some() || !self.verify(cx) {
            return;
        }
        window.close_dialog(cx);
        let _ = self.owner.update(cx, |app, cx| {
            app.apply_language_edit(&self.identity, self.label, &self.edit, window, cx);
            if let Some(editor) = app.active_editor() {
                editor.read(cx).focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        });
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let mut files = div()
            .id("workspace-preview-files")
            .v_flex()
            .max_h(window.viewport_size().height * 0.48)
            .overflow_y_scroll();
        for (index, file) in self.edit.files.iter().enumerate() {
            let path = file
                .path
                .strip_prefix(self.identity.snapshot.root())
                .unwrap_or(&file.path);
            let label = format!("{} · {}곳", path.display(), file.edits.len());
            let expanded = self.expanded.get(&index);
            files = files.child(
                Button::new(("workspace-preview-file", index))
                    .ghost()
                    .small()
                    .w_full()
                    .accessibility_label(label.clone())
                    .child(dialogs::choice_body().child(format!(
                        "{}  {label}",
                        if expanded.is_some() { "▾" } else { "▸" }
                    )))
                    .on_click(cx.listener(move |view, _, _, cx| view.toggle(index, cx))),
            );
            if let Some(lines) = expanded {
                for excerpt in lines {
                    files = files.child(
                        div()
                            .px_3()
                            .text_color(palette.rgb(MUTED))
                            .child(format!("{}줄", excerpt.first_line)),
                    );
                    for (sign, color, text) in
                        [("−", RED, &excerpt.old), ("+", GREEN, &excerpt.new)]
                    {
                        for line in text.split('\n') {
                            files = files.child(
                                div()
                                    .px_3()
                                    .font_family("JetBrains Mono")
                                    .text_color(palette.rgb(color))
                                    .child(format!("{sign} {}", line.trim_end_matches('\r'))),
                            );
                        }
                    }
                }
            }
        }
        div()
            .id("workspace-edit-preview")
            .v_flex()
            .gap_2()
            .child(files)
            .children(
                self.error
                    .as_ref()
                    .map(|e| div().text_color(palette.rgb(RED)).child(e.clone())),
            )
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("workspace-preview-cancel")
                            .label("취소")
                            .on_click(|_, w, cx| w.close_dialog(cx)),
                    )
                    .child(
                        Button::new("workspace-preview-apply")
                            .label("초안에 적용")
                            .primary()
                            .disabled(self.error.is_some())
                            .on_click(cx.listener(|s, _, w, cx| s.apply(w, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{FileEdits, LspPosition, TextEdit, TextRange, excerpts};

    #[test]
    fn grouped_preview_shows_real_unicode_lines_and_multiline_replacements() {
        let pos = |line, character| LspPosition { line, character };
        let edit = |start, end, text: &str| TextEdit {
            range: TextRange { start, end },
            new_text: text.into(),
        };
        let file = FileEdits {
            path: "demo.ts".into(),
            original: "😀 old(old)\r\nuntouched\r\nremove\r\nrest\r\n".into(),
            edits: vec![
                edit(pos(0, 7), pos(0, 10), "new"),
                edit(pos(2, 0), pos(3, 2), "first\r\nsecond "),
                edit(pos(0, 3), pos(0, 6), "new"),
            ],
        };
        let view = excerpts(&file).unwrap();
        assert_eq!(view.len(), 2);
        assert_eq!(view[0].first_line, 1);
        assert_eq!(view[0].old, "😀 old(old)\r");
        assert_eq!(view[0].new, "😀 new(new)\r");
        assert_eq!(view[1].first_line, 3);
        assert_eq!(view[1].old, "remove\r\nrest\r");
        assert_eq!(view[1].new, "first\r\nsecond st\r");
    }
}
