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
    Blame {
        lines: Vec<BlameLine>,
        syntax: syntax::CodeSyntax,
    },
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
                        if mode == Mode::Blame && target.revision == target.blame_revision {
                            app.open_inline_blame(target.clone(), window, cx);
                            return;
                        }
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
                                .width(ui_px(850.))
                                .margin_top(ui_px(14.))
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
                        .map(|lines| {
                            let mut source = String::new();
                            for line in &lines {
                                source.push_str(&line.text);
                                source.push('\n');
                            }
                            let syntax = syntax::CodeSyntax::new(&target.path, &source);
                            Page::Blame { lines, syntax }
                        })
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
            let syntax = Arc::new(split_diff::DiffSyntax::new(&patch));
            anyhow::Ok((
                patch::Prepared::new(patch.patch, Some(syntax)),
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
            Page::Blame { lines, .. } if lines.is_empty() => div()
                .p_3()
                .child("표시할 줄이 없습니다.")
                .into_any_element(),
            Page::Blame { lines, .. } => {
                let font_size = cx.theme().mono_font_size;
                let scale = f32::from(font_size) / 13.;
                let this = cx.entity().downgrade();
                uniform_list("file-blame-rows", lines.len(), move |range, _, cx| {
                    let mut rows = vec![];
                    if let Some(reader) = this.upgrade()
                        && let Page::Blame { lines, syntax } = &reader.read(cx).page
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
                                    .child(
                                        div()
                                            .whitespace_nowrap()
                                            .child(syntax.styled_line(i, &line.text, true, cx)),
                                    ),
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
                        .text_size(ui_text(11.))
                        .text_ellipsis()
                        .child(label.clone()),
                )
                .child(Editor::new(editor).readonly(true).bordered(false).h_full())
                .into_any_element(),
        };
        div()
            .v_flex()
            .gap_2()
            .child(div().text_size(ui_text(12.)).text_ellipsis().child(format!(
                "{}   ·   {}",
                self.target.path.display(),
                revision.as_deref().map(|s| &s[..8]).unwrap_or("작업 트리")
            )))
            .child(bar)
            .child(div().h(ui_px(280.)).min_w_0().overflow_hidden().child(body))
    }
}

// The gutter belongs to an exact editor, worktree, revision and displayed source.
// Scroll/wrap geometry comes from the native editor's actual laid-out ranges.
pub(super) struct InlineBlame {
    owner: WeakEntity<Pokarak>,
    root: PathBuf,
    target: Target,
    editor: Entity<EditorState>,
    expected: String,
    lines: Vec<BlameLine>,
    loading: bool,
    error: Option<String>,
    full: bool,
    selected: Option<(usize, Entity<BlameDetails>)>,
    _observe: Subscription,
}

fn matches_display(lines: &[BlameLine], text: &str) -> bool {
    let source: Vec<_> = text.lines().collect();
    source.len() == lines.len()
        && source
            .iter()
            .zip(lines)
            .all(|(text, line)| *text == line.text.trim_end_matches('\r'))
}

impl Pokarak {
    fn open_inline_blame(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        if !self.git_history.active
            && (self.code_edits.dirty(&root, &target.path)
                || self.code_edits.stale(&root, &target.path))
        {
            self.status="Blame은 저장된 코드에서 사용할 수 있습니다. 초안을 저장하거나 디스크와 비교하세요.".into();
            cx.notify();
            return;
        }
        let editor = if self.git_history.active {
            self.history_blame_editor(window, cx)
        } else {
            self.select_reader_mode(true, window, cx);
            self.active_editor()
        };
        let Some(editor) = editor else {
            return;
        };
        let full = self.prefs.option_string(
            self.current_project.as_deref(),
            "navigation.blame_mode",
            "line",
        ) == "file";
        let owner = cx.weak_entity();
        let expected = editor.read(cx).value().to_string();
        let gutter = cx.new(|cx| {
            let observe = cx.observe(&editor, |this: &mut InlineBlame, _, cx| {
                if this.editor.read(cx).value().as_ref() != this.expected {
                    this.selected = None;
                }
                cx.notify();
            });
            InlineBlame {
                owner,
                root: root.clone(),
                target: target.clone(),
                editor: editor.clone(),
                expected: expected.clone(),
                lines: vec![],
                loading: true,
                error: None,
                full,
                selected: None,
                _observe: observe,
            }
        });
        self.inline_blame = Some(gutter.clone());
        let task = cx.background_spawn(async move {
            file_history::file_blame(&root, &target.path, target.blame_revision.as_deref())
        });
        gutter.update(cx, |_, cx| {
            cx.spawn(async move |view, cx| {
                let result = task.await;
                let _ = view.update(cx, |this, cx| {
                    this.loading = false;
                    let valid = this
                        .owner
                        .upgrade()
                        .is_some_and(|owner| this.accepts(owner.read(cx), &this.editor, cx));
                    if !valid {
                        this.error = Some(
                            "표시 중인 코드가 변경되었습니다. Blame을 다시 실행하세요.".into(),
                        );
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(lines) if matches_display(&lines, &this.expected) => this.lines = lines,
                        Ok(_) => this.error = Some(
                            "Blame 소스와 표시 코드가 다릅니다. 저장·새로고침 후 다시 실행하세요."
                                .into(),
                        ),
                        Err(error) => {
                            this.error = Some(format!("Blame을 읽지 못했습니다: {error:#}"))
                        }
                    }
                    cx.notify();
                });
            })
            .detach()
        });
        cx.notify();
    }

    pub(super) fn with_inline_blame(
        &self,
        editor: &Entity<EditorState>,
        content: AnyElement,
        cx: &App,
    ) -> AnyElement {
        let Some(gutter) = self
            .inline_blame
            .as_ref()
            .filter(|g| g.read(cx).accepts(self, editor, cx))
        else {
            return content;
        };
        // Prepaint/paint the editor before the gutter canvas. It then reads this
        // frame's scroll and wrap geometry, including while resizing the pane.
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .child(div().size_full().pl(ui_px(182.)).child(content))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(ui_px(182.))
                    .h_full()
                    .child(gutter.clone()),
            )
            .into_any_element()
    }

    pub(super) fn inline_blame_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let gutter = self.inline_blame.as_ref()?;
        let state = gutter.read(cx);
        if !state.identity_matches(self) {
            return None;
        }
        let stale = !state.accepts(self, &state.editor, cx);
        let status = if stale {
            "Blame 숨김 · 코드가 변경됨".to_owned()
        } else if state.loading {
            "Blame 읽는 중…".into()
        } else {
            state
                .error
                .clone()
                .unwrap_or_else(|| "Blame · 작성자 / 커밋".into())
        };
        let full = state.full;
        let target = gutter.clone();
        let close = gutter.clone();
        Some(
            div()
                .flex_none()
                .min_h(ui_px(28.))
                .px_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .text_size(ui_text(10.))
                .child(status)
                .child(
                    Button::new("blame-line-mode")
                        .label(if full {
                            "현재 줄만"
                        } else {
                            "파일 전체"
                        })
                        .ghost()
                        .xsmall()
                        .on_click(move |_, _, cx| {
                            target.update(cx, |g, cx| {
                                g.full = !g.full;
                                cx.notify();
                            })
                        }),
                )
                .child(
                    Button::new("close-inline-blame")
                        .icon(IconName::X)
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if app.inline_blame.as_ref() == Some(&close) {
                                app.inline_blame = None;
                                cx.notify();
                            }
                        })),
                )
                .into_any_element(),
        )
    }
}
impl InlineBlame {
    fn identity_matches(&self, app: &Pokarak) -> bool {
        if app.current.as_ref() != Some(&self.root) || app.opening_path.is_some() {
            return false;
        }
        if app.git_history.active {
            app.git_history.selection().is_some_and(|s| {
                s.file.as_ref() == Some(&self.target.path)
                    && Some(&s.oid) == self.target.revision.as_ref()
                    && s.code
            })
        } else {
            self.target.revision.is_none()
                && app.selected().as_ref() == Some(&self.target.path)
                && app.session().is_some_and(|s| s.saved.code)
        }
    }
    fn accepts(&self, app: &Pokarak, editor: &Entity<EditorState>, cx: &App) -> bool {
        self.identity_matches(app)
            && self.editor == *editor
            && editor.read(cx).value().as_ref() == self.expected
            && (self.target.revision.is_some()
                || (!app.code_edits.dirty(&self.root, &self.target.path)
                    && !app.code_edits.stale(&self.root, &self.target.path)
                    && !app.external_change))
    }
    fn details(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(line) = self.lines.get(row).cloned() else {
            return;
        };
        let root = self.root.clone();
        let path = self.target.path.clone();
        let details = cx.new(|cx: &mut Context<BlameDetails>| {
            let details = BlameDetails {
                line: line.clone(),
                summary: None,
                error: None,
            };
            if let Some(oid) = line.oid.clone() {
                let task = cx.background_spawn(async move {
                    file_history::file_history(&root, &path, Some(&oid))
                        .map(|h| h.entries.into_iter().find(|r| r.oid == oid))
                });
                cx.spawn(async move |view, cx| {
                    let result = task.await;
                    let _ = view.update(cx, |this, cx| {
                        match result {
                            Ok(revision) => this.summary = Some(revision.map(|r| r.subject).unwrap_or_else(|| "이 경로의 커밋 메시지를 읽을 수 없습니다. 이름 변경 전 경로일 수 있습니다.".into())),
                            Err(e) => this.error = Some(format!("{e:#}")),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            details
        });
        self.selected = Some((row, details));
        cx.notify();
    }
}
impl InlineBlame {
    fn annotations(&self, cx: &App) -> Vec<(usize, Bounds<Pixels>, String)> {
        let editor = self.editor.read(cx);
        if editor.value().as_ref() != self.expected || self.loading || self.error.is_some() {
            return vec![];
        }
        let Some(visible) = editor.visible_row_range() else {
            return vec![];
        };
        let cursor = editor.cursor_position().line as usize;
        visible
            .filter_map(|row| {
                if !self.full && row != cursor {
                    return None;
                }
                let line = self.lines.get(row)?;
                let offset = if self.full {
                    editor.text().line_start_offset(row)
                } else {
                    editor.cursor()
                };
                let bounds = editor.range_to_bounds(&(offset..offset))?;
                let hash = line
                    .oid
                    .as_deref()
                    .map(|s| &s[..s.len().min(8)])
                    .unwrap_or("미커밋");
                Some((
                    row,
                    bounds,
                    format!("{hash} · {}", line.author.replace(['\n', '\r'], " ")),
                ))
            })
            .collect()
    }
}
impl Render for InlineBlame {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::popover::Popover;
        let palette = Palette::new(cx);
        let weak = cx.weak_entity();
        let accessible = cx.weak_entity();
        let surface = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let entries = weak
                    .upgrade()
                    .map(|g| g.read(cx).annotations(cx))
                    .unwrap_or_default();
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for (_, code_bounds, label) in entries {
                        let height = code_bounds.size.height;
                        if code_bounds.bottom() < bounds.top()
                            || code_bounds.top() > bounds.bottom()
                        {
                            continue;
                        }
                        let run = TextRun {
                            len: label.len(),
                            font: font("IBM Plex Sans"),
                            color: palette.rgb(MUTED).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line =
                            window
                                .text_system()
                                .shape_line(label.into(), ui_px(10.), &[run], None);
                        let _ = line.paint(
                            point(bounds.left() + ui_px(6.), code_bounds.top()),
                            height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                });
            },
        )
        .size_full();
        let mut gutter = div()
            .id("inline-blame-gutter")
            .role(accesskit::Role::Button)
            .aria_label("Blame 작성자·커밋 · 클릭하여 상세 보기")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(palette.rgb(SIDE))
            .border_r_1()
            .border_color(palette.rgb(LINE))
            .cursor_pointer()
            .child(surface)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if let Some((row, _, _)) =
                        this.annotations(cx).into_iter().find(|(_, bounds, _)| {
                            event.position.y >= bounds.top() && event.position.y < bounds.bottom()
                        })
                    {
                        this.details(row, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_a11y_action(accesskit::Action::Click, move |_, _, cx| {
                let _ = accessible.update(cx, |this, cx| {
                    let row = this.editor.read(cx).cursor_position().line as usize;
                    this.details(row, cx);
                });
            });
        if let Some((row, details)) = &self.selected {
            let selected = *row;
            if let Some((_, bounds, _)) = self
                .annotations(cx)
                .into_iter()
                .find(|(row, _, _)| *row == selected)
            {
                let top = self.editor.read(cx).input_bounds().top();
                let details = details.clone();
                gutter = gutter.child(
                    div()
                        .absolute()
                        .top(bounds.top() - top)
                        .left_0()
                        .w_full()
                        .h(bounds.size.height)
                        .child(
                            Popover::new(("inline-blame-detail", selected))
                                .open(true)
                                .trigger(
                                    Button::new("inline-blame-popup-anchor")
                                        .ghost()
                                        .w_full()
                                        .h(bounds.size.height)
                                        .opacity(0.),
                                )
                                .content(move |_, _, _| details.clone())
                                .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                                    if !*open {
                                        this.selected = None;
                                        cx.notify();
                                    }
                                })),
                        ),
                );
            }
        }
        gutter
    }
}

struct BlameDetails {
    line: BlameLine,
    summary: Option<String>,
    error: Option<String>,
}
impl Render for BlameDetails {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(ui_px(400.))
            .p_3()
            .v_flex()
            .gap_2()
            .bg(Palette::new(cx).rgb(PANEL))
            .border_1()
            .border_color(Palette::new(cx).rgb(LINE))
            .child(
                self.line
                    .oid
                    .clone()
                    .unwrap_or_else(|| "현재 작업 파일에서 변경된 줄".into()),
            )
            .child(format!("{} · {}", self.line.author, self.line.date))
            .child(
                self.error
                    .clone()
                    .or_else(|| self.summary.clone())
                    .unwrap_or_else(|| {
                        if self.line.oid.is_some() {
                            "커밋 상세를 읽는 중…".into()
                        } else {
                            "아직 커밋되지 않은 변경입니다.".into()
                        }
                    }),
            )
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(ui_text(11.))
                    .child(self.line.text.clone()),
            )
    }
}

#[cfg(test)]
mod inline_tests {
    use super::{BlameLine, matches_display};
    #[test]
    fn blame_gutter_rejects_dirty_external_and_wrong_line_sources() {
        let line = BlameLine {
            line: 1,
            oid: Some("abc".into()),
            author: "A".into(),
            date: "date".into(),
            text: "saved".into(),
        };
        assert!(matches_display(std::slice::from_ref(&line), "saved\n"));
        assert!(!matches_display(std::slice::from_ref(&line), "draft\n"));
        assert!(!matches_display(
            std::slice::from_ref(&line),
            "saved\nmore\n"
        ));
        assert!(matches_display(
            &[BlameLine {
                text: "saved\r".into(),
                ..line
            }],
            "saved\r\n"
        ));
    }
    #[test]
    fn real_blame_matches_saved_and_historical_sources_but_never_the_draft() {
        use std::{path::Path, process::Command};
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args([
                    "-c",
                    "user.name=Gutter Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-C",
                ])
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        std::fs::write(repo.path().join("code.rs"), "fn saved() {}\n").unwrap();
        git(&["add", "--", "code.rs"]);
        git(&["commit", "-qm", "Saved author source"]);
        let oid = git(&["rev-parse", "HEAD"]);
        let saved =
            super::file_history::file_blame(repo.path(), Path::new("code.rs"), None).unwrap();
        assert!(matches_display(&saved, "fn saved() {}\n"));
        assert!(
            saved
                .iter()
                .all(|line| line.oid.as_ref() == Some(&oid) && line.author == "Gutter Fixture")
        );
        assert!(!matches_display(&saved, "fn unsaved_draft() {}\n"));
        std::fs::write(repo.path().join("code.rs"), "fn external() {}\n").unwrap();
        let current =
            super::file_history::file_blame(repo.path(), Path::new("code.rs"), None).unwrap();
        assert!(matches_display(&current, "fn external() {}\n"));
        assert!(current[0].oid.is_none());
        assert!(!matches_display(&current, "fn saved() {}\n"));
        let historical =
            super::file_history::file_blame(repo.path(), Path::new("code.rs"), Some(&oid)).unwrap();
        assert!(matches_display(&historical, "fn saved() {}\n"));
        assert!(!matches_display(&historical, "fn external() {}\n"));
    }
}
