//! A window-scoped reader. Every query is tied to the file and revision that opened it.
use super::*;
use gpui_kit::component::ActiveTheme;
use pokarak_core::file_history::{self, BlameLine, FileHistory, FileRevision};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    History,
    Blame,
}

#[derive(Clone)]
pub(super) struct Target {
    pub path: PathBuf,
    pub revision: Option<String>,
    pub blame_revision: Option<String>,
}

enum Page {
    Loading,
    History(FileHistory),
    Blame(Vec<BlameLine>),
    Patch(Entity<EditorState>, String),
    Error(String),
}

struct Inspector {
    root: PathBuf,
    target: Target,
    mode: Mode,
    page: Page,
    generation: u64,
}

impl Pokarak {
    pub(super) fn file_inspection_buttons(
        &self,
        target: Target,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut row = div().flex().items_center();
        for (mode, label) in [(Mode::History, "History"), (Mode::Blame, "Blame")] {
            let target = target.clone();
            row = row.child(
                Button::new(label)
                    .label(label)
                    .ghost()
                    .small()
                    .tooltip(if mode == Mode::History {
                        "파일의 커밋 이력"
                    } else {
                        "줄마다 마지막으로 수정한 커밋과 작성자"
                    })
                    .disabled(self.loading || self.git_is_busy())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        let Some(root) = app.current.clone() else {
                            return;
                        };
                        let reader = cx.new(|cx| {
                            let mut reader = Inspector {
                                root,
                                target: target.clone(),
                                mode,
                                page: Page::Loading,
                                generation: 0,
                            };
                            reader.query(mode, cx);
                            reader
                        });
                        window.open_dialog(cx, move |dialog, _, _| {
                            dialog
                                .title("파일 이력")
                                .width(px(850.))
                                .margin_top(px(14.))
                                .child(reader.clone())
                        });
                    })),
            );
        }
        row
    }
}

impl Inspector {
    fn query(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.generation += 1;
        self.mode = mode;
        self.page = Page::Loading;
        let generation = self.generation;
        let root = self.root.clone();
        let target = self.target.clone();
        let task = cx.background_spawn(async move {
            match mode {
                Mode::History => {
                    file_history::file_history(&root, &target.path, target.revision.as_deref())
                        .map(Page::History)
                }
                Mode::Blame => {
                    file_history::file_blame(&root, &target.path, target.blame_revision.as_deref())
                        .map(Page::Blame)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.page = result.unwrap_or_else(|e| Page::Error(format!("{e:#}")));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn revision(&mut self, file: FileRevision, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.page = Page::Loading;
        let root = self.root.clone();
        let task = cx.background_spawn(async move {
            let patch = file_history::revision_diff(&root, &file)?;
            let parent = file
                .parent
                .as_deref()
                .map(|oid| &oid[..8])
                .unwrap_or("빈 트리");
            anyhow::Ok((
                patch.patch,
                format!(
                    "{} → {}  ·  {}  ·  {}",
                    parent,
                    &file.oid[..8],
                    file.path.display(),
                    file.subject
                ),
            ))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.page = match result {
                    Ok((patch, label)) => {
                        let editor = patch::editor(patch, false, window, cx);
                        Page::Patch(editor, label)
                    }
                    Err(e) => Page::Error(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for Inspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let revision = match self.mode {
            Mode::History => &self.target.revision,
            Mode::Blame => &self.target.blame_revision,
        };
        let mut bar = div().flex().items_center().gap_2();
        for (mode, label) in [(Mode::History, "History"), (Mode::Blame, "Blame")] {
            bar = bar.child(
                Button::new(("file-mode", mode as usize))
                    .label(label)
                    .small()
                    .ghost()
                    .selected(self.mode == mode && !matches!(self.page, Page::Patch(..)))
                    .on_click(cx.listener(move |this, _, _, cx| this.query(mode, cx))),
            );
        }
        let body = match &self.page {
            Page::Loading => div().p_3().child("읽는 중…").into_any_element(),
            Page::Error(error) => div()
                .p_3()
                .text_color(palette.rgb(RED))
                .child(error.clone())
                .into_any_element(),
            Page::History(history) => {
                let mut list = div()
                    .id("file-history-rows")
                    .size_full()
                    .overflow_y_scroll()
                    .v_flex();
                if history.entries.is_empty() {
                    list = list.child("이 파일에 기록된 커밋이 없습니다.");
                }
                for (i, entry) in history.entries.iter().enumerate() {
                    let revision = entry.clone();
                    list = list.child(
                        Button::new(("file-history-row", i))
                            .ghost()
                            .small()
                            .justify_start()
                            .flex_none()
                            .label(format!(
                                "{}   {}   {}   {}",
                                &entry.oid[..8],
                                entry.date.get(..10).unwrap_or(&entry.date),
                                entry.author,
                                entry.subject
                            ))
                            .tooltip(entry.path.display().to_string())
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.revision(revision.clone(), w, cx)
                            })),
                    );
                }
                if history.truncated {
                    list = list.child("최근 커밋까지만 표시했습니다.");
                }
                list.into_any_element()
            }
            Page::Blame(lines) if lines.is_empty() => div()
                .p_3()
                .child("표시할 줄이 없습니다.")
                .into_any_element(),
            Page::Blame(lines) => {
                let font_size = cx.theme().mono_font_size;
                let scale = f32::from(font_size) / 13.;
                let this = cx.entity().downgrade();
                uniform_list("file-blame-rows", lines.len(), move |range, _, cx| {
                    let mut rows = vec![];
                    if let Some(reader) = this.upgrade()
                        && let Page::Blame(lines) = &reader.read(cx).page
                    {
                        for i in range {
                            let line = &lines[i];
                            let hash = line.oid.as_deref().map(|s| &s[..8]).unwrap_or("작업 파일");
                            rows.push(
                                div()
                                    .flex()
                                    .h(theme::code_row_height(font_size))
                                    .min_w_full()
                                    .items_center()
                                    .gap_2()
                                    .text_size(font_size)
                                    .font_family("JetBrains Mono")
                                    .child(
                                        div()
                                            .w(px(78. * scale))
                                            .flex_none()
                                            .text_color(palette.rgb(BLUE))
                                            .child(hash.to_owned()),
                                    )
                                    .child(
                                        div()
                                            .w(px(104. * scale))
                                            .flex_none()
                                            .text_ellipsis()
                                            .child(line.author.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(px(88. * scale))
                                            .flex_none()
                                            .text_color(palette.rgb(MUTED))
                                            .child(
                                                line.date
                                                    .get(..10)
                                                    .unwrap_or(&line.date)
                                                    .to_owned(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w(px(39. * scale))
                                            .flex_none()
                                            .text_right()
                                            .child(line.line.to_string()),
                                    )
                                    .child(div().whitespace_nowrap().child(line.text.clone())),
                            );
                        }
                    }
                    rows
                })
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .size_full()
                .into_any_element()
            }
            Page::Patch(editor, label) => div()
                .size_full()
                .v_flex()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_ellipsis()
                        .child(label.clone()),
                )
                .child(Editor::new(editor).readonly(true).bordered(false).h_full())
                .into_any_element(),
        };
        div()
            .v_flex()
            .gap_2()
            .child(div().text_size(px(12.)).text_ellipsis().child(format!(
                "{}   ·   {}",
                self.target.path.display(),
                revision.as_deref().map(|s| &s[..8]).unwrap_or("작업 트리")
            )))
            .child(bar)
            .child(div().h(px(280.)).min_w_0().overflow_hidden().child(body))
    }
}
