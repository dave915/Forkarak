//! Native editor commands. The reader keeps its draft/save and trust contracts.
mod changes;
mod edit;
mod files;
mod formatter;
mod indent;
mod palette;
mod shortcuts;
mod wrap;
use super::*;
use code_edit::FileKey;
use gpui_kit::component::input::{GutterDecoration, GutterDecorationCollection};
use std::ops::Range;

use edit::LineCommand;
actions!(
    forkarak_editor,
    [
        CommandPalette,
        ToggleComment,
        MoveLineUp,
        MoveLineDown,
        DuplicateLineUp,
        DuplicateLineDown,
        SelectNextWord,
        ReplaceFile,
        GoToLine,
        ToggleWordWrap,
        PinTab,
        ManageFiles,
        PreviousCode,
        FormatFile
    ]
);

#[derive(Default)]
pub(super) struct Tools {
    preview: HashMap<PathBuf, PathBuf>,
    wrap: HashMap<FileKey, bool>,
    gutters: HashMap<FileKey, (EntityId, GutterDecorationCollection)>,
    changes: HashMap<FileKey, Vec<changes::Change>>,
    expanded: HashSet<FileKey>,
    indent_request: HashMap<FileKey, u64>,
    marker_request: HashMap<FileKey, u64>,
    formatting: HashSet<FileKey>,
    bindings: Option<(Option<PathBuf>, serde_json::Value)>,
    configuration: Option<(FileKey, EntityId, serde_json::Value)>,
}

pub(super) fn bind_keys(cx: &mut App) {
    shortcuts::install(&Preferences::load(), None, cx);
}

pub(super) fn actions(
    app: &Pokarak,
    root: Stateful<Div>,
    cx: &mut Context<Pokarak>,
) -> Stateful<Div> {
    let code = app.code_tools_target(cx).is_some();
    let editable = app
        .current
        .as_ref()
        .zip(app.selected().as_ref())
        .is_some_and(|(root, path)| app.code_edits.can_edit(root, path))
        && code;
    root.on_action(cx.listener(|a, _: &CommandPalette, w, cx| a.open_command_palette(w, cx)))
        .when(
            app.current.is_some()
                && app.opening_path.is_none()
                && !app.loading
                && !app.git_is_busy(),
            |root| {
                root.on_action(cx.listener(|a, _: &ManageFiles, w, cx| a.open_file_tools(w, cx)))
            },
        )
        .when(code, |root| {
            root.on_action(cx.listener(|a, _: &SelectNextWord, w, cx| a.select_next_word(w, cx)))
                .on_action(cx.listener(|a, _: &GoToLine, w, cx| a.open_go_to_line(w, cx)))
                .on_action(cx.listener(|a, _: &ToggleWordWrap, w, cx| a.toggle_word_wrap(w, cx)))
                .on_action(cx.listener(|a, _: &PinTab, w, cx| a.pin_code_tab(w, cx)))
                .on_action(cx.listener(|a, _: &PreviousCode, w, cx| a.toggle_previous_code(w, cx)))
        })
        .when(editable, |root| {
            root.on_action(
                cx.listener(|a, _: &ToggleComment, w, cx| {
                    a.edit_lines(LineCommand::Comment, w, cx)
                }),
            )
            .on_action(cx.listener(|a, _: &MoveLineUp, w, cx| a.edit_lines(LineCommand::Up, w, cx)))
            .on_action(
                cx.listener(|a, _: &MoveLineDown, w, cx| a.edit_lines(LineCommand::Down, w, cx)),
            )
            .on_action(cx.listener(|a, _: &DuplicateLineUp, w, cx| {
                a.edit_lines(LineCommand::DuplicateUp, w, cx)
            }))
            .on_action(cx.listener(|a, _: &DuplicateLineDown, w, cx| {
                a.edit_lines(LineCommand::DuplicateDown, w, cx)
            }))
            .on_action(cx.listener(|a, _: &ReplaceFile, w, cx| a.replace_in_file(w, cx)))
            .on_action(
                cx.listener(|a, _: &FormatFile, w, cx| a.format_code_file(false, false, w, cx)),
            )
        })
}

impl Pokarak {
    pub(super) fn sync_editor_bindings(&mut self, cx: &mut Context<Self>) {
        let project = self.current_project.clone();
        let value = self
            .prefs
            .option(project.as_deref(), "keyboard.bindings")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        if self.code_edits.tools.bindings.as_ref() != Some(&(project.clone(), value.clone())) {
            shortcuts::install(&self.prefs, project.as_deref(), cx);
            self.code_edits.tools.bindings = Some((project, value));
        }
    }

    pub(super) fn sync_editor_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some((root, path, editor)) = self.code_tools_target(cx) else {
            return;
        };
        let Some(key) = FileKey::new(&root, &path) else {
            return;
        };
        let project = self.current_project.as_deref();
        let keys = [
            "editor.direct_edit",
            "editor.editorconfig",
            "editor.detect_indent",
            "editor.tab_size",
            "editor.use_spaces",
            "editor.word_wrap",
            "editor.wrap_patterns",
            "editor.inline_changes",
        ];
        let values = serde_json::Value::Array(
            keys.iter()
                .map(|key| {
                    self.prefs
                        .option(project, key)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        );
        let identity = (key, editor.entity_id(), values);
        if self.code_edits.tools.configuration.as_ref() != Some(&identity) {
            self.configure_code_tools(&root, &path, &editor, window, cx);
            self.code_edits.tools.configuration = Some(identity);
        }
    }

    fn code_tools_target(&self, cx: &App) -> Option<(PathBuf, PathBuf, Entity<EditorState>)> {
        if self.git_history.active
            || self.loading
            || self.file_loading
            || self.opening_path.is_some()
            || !self.session().is_some_and(|s| s.saved.code)
        {
            return None;
        }
        let (root, path, editor) = (
            self.current.clone()?,
            self.selected()?,
            self.active_editor()?,
        );
        let _ = editor.read(cx);
        Some((root, path, editor))
    }

    fn edit_lines(&mut self, command: LineCommand, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) || self.git_is_busy() {
            return;
        }
        let Some((root, path, editor)) = self.code_tools_target(cx) else {
            return;
        };
        if !self.code_edits.editing(&root, &path) {
            self.start_code_edit(window, cx);
        }
        if !self.code_edits.editing(&root, &path) {
            self.status = "‘편집’을 먼저 선택하세요. 과거 코드는 읽기 전용입니다.".into();
            cx.notify();
            return;
        }
        let text = editor.read(cx).value().to_string();
        match edit::transform(&text, editor.read(cx).selected_range(), &path, command) {
            Ok(Some((range, replacement, selection))) => {
                let scroll = editor.read(cx).scroll_offset();
                editor.update(cx, |e, cx| {
                    e.set_selected_range(range, cx);
                    e.replace(replacement, window, cx);
                    e.set_selected_range(selection, cx);
                    e.set_scroll_offset(scroll, cx);
                });
                self.capture_code_buffers(cx);
                self.pin_preview(&root, &path);
                self.checkpoint_code_drafts(cx);
                self.refresh_inline_tools(&root, &path, &editor, cx);
            }
            Err(message) => self.status = message,
            _ => {}
        }
        editor.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn select_next_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some((_, _, editor)) = self.code_tools_target(cx) else {
            return;
        };
        let text = editor.read(cx).value();
        let selected = editor.read(cx).selected_range();
        if let Some(next) = edit::word_or_next(&text, selected.clone()) {
            editor.update(cx, |e, cx| {
                if selected.is_empty() {
                    e.set_selected_range(next, cx)
                } else {
                    e.add_selection(next, cx)
                }
            });
        }
        editor.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn replace_in_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some((root, path, editor)) = self.code_tools_target(cx) else {
            return;
        };
        if !self.code_edits.editing(&root, &path) {
            self.start_code_edit(window, cx);
        }
        if !self.code_edits.editing(&root, &path) {
            return;
        }
        editor.read(cx).focus_handle(cx).focus(window, cx);
        editor.update(cx, |e, cx| e.open_search(true, cx));
    }

    fn word_wrap(&self, root: &Path, path: &Path) -> bool {
        if let Some(value) =
            FileKey::new(root, path).and_then(|key| self.code_edits.tools.wrap.get(&key))
        {
            return *value;
        }
        wrap::configured(&self.prefs, self.current_project.as_deref(), path)
    }

    fn toggle_word_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some((root, path, editor)) = self.code_tools_target(cx) else {
            return;
        };
        let wrap = !self.word_wrap(&root, &path);
        if let Some(key) = FileKey::new(&root, &path) {
            self.code_edits.tools.wrap.insert(key, wrap);
        }
        editor.update(cx, |e, cx| e.set_soft_wrap(wrap, window, cx));
        cx.notify();
    }

    pub(super) fn prepare_preview_tab(
        &mut self,
        root: &Path,
        path: &Path,
        history: bool,
        cx: &App,
    ) {
        if !history
            || !self
                .prefs
                .option_bool(self.current_project.as_deref(), "editor.preview_tabs", true)
        {
            return;
        }
        let Some(session) = self.sessions.get(root) else {
            return;
        };
        if session.saved.tabs.iter().any(|p| p == path) {
            return;
        }
        if let Some(previous) = self.code_edits.tools.preview.remove(root) {
            if !self.code_edits.dirty(root, &previous)
                && !self.code_edits.editing(root, &previous)
                && let Some(session) = self.sessions.get_mut(root)
            {
                // Positions/history are retained; only the reusable tab is replaced.
                session.saved.tabs.retain(|p| p != &previous);
                session.documents.remove(&previous);
                session.diffs.remove(&previous);
            }
        }
        let _ = cx;
        self.code_edits
            .tools
            .preview
            .insert(root.to_owned(), path.to_owned());
    }

    pub(super) fn is_preview_tab(&self, root: &Path, path: &Path) -> bool {
        self.code_edits
            .tools
            .preview
            .get(root)
            .is_some_and(|p| p == path)
    }
    pub(super) fn pin_preview(&mut self, root: &Path, path: &Path) {
        if self.is_preview_tab(root, path) {
            self.code_edits.tools.preview.remove(root);
        }
    }
    fn pin_code_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        if let (Some(root), Some(path)) = (self.current.clone(), self.selected()) {
            self.pin_preview(&root, &path);
            self.persist(cx);
            cx.notify();
        }
    }

    pub(super) fn configure_code_tools(
        &mut self,
        root: &Path,
        path: &Path,
        editor: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wrap = self.word_wrap(root, path);
        self.enable_direct_code_edit(root, path, editor, cx);
        editor.update(cx, |e, cx| e.set_soft_wrap(wrap, window, cx));
        self.refresh_inline_tools(root, path, editor, cx);
        let Some(key) = FileKey::new(root, path) else {
            return;
        };
        let request = self
            .code_edits
            .tools
            .indent_request
            .entry(key.clone())
            .or_default();
        *request += 1;
        let request = *request;
        let project = self.current_project.as_deref();
        let config = self.prefs.option_bool(project, "editor.editorconfig", true);
        let detect = self
            .prefs
            .option_bool(project, "editor.detect_indent", true);
        let fallback = indent::Indent {
            size: self
                .prefs
                .option_usize(project, "editor.tab_size", 4)
                .clamp(1, 16),
            tabs: !self.prefs.option_bool(project, "editor.use_spaces", true),
        };
        let text = editor.read(cx).value().to_string();
        editor.update(cx, |e, cx| e.set_tab_size(fallback.tab_size(), cx));
        let id = editor.entity_id();
        let editor = editor.clone();
        let target = key.clone();
        let task = cx.background_spawn(async move {
            indent::resolve(&target.root, &target.path, &text, config, detect, fallback)
        });
        cx.spawn(async move |app, cx| {
            let indent = task.await;
            let _ = app.update(cx, |app, cx| {
                if app.current.as_ref() != Some(&key.root)
                    || app
                        .selected()
                        .as_ref()
                        .and_then(|p| FileKey::new(&key.root, p))
                        .as_ref()
                        != Some(&key)
                    || app.code_edits.tools.indent_request.get(&key) != Some(&request)
                    || app.active_editor().is_none_or(|e| e.entity_id() != id)
                {
                    return;
                }
                editor.update(cx, |e, cx| e.set_tab_size(indent.tab_size(), cx));
            });
        })
        .detach();
    }

    /// Shared formatter/LSP adapter: exact worktree, file, entity and source snapshot
    /// are checked before replacing the retained draft. Disk remains untouched.
    pub(super) fn apply_editor_draft(
        &mut self,
        root: &Path,
        path: &Path,
        id: EntityId,
        expected: &str,
        replacement: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some((active_root, active_path, editor)) = self.code_tools_target(cx) else {
            return Err("현재 코드 파일이 없습니다.".into());
        };
        if active_root != root
            || FileKey::new(root, &active_path) != FileKey::new(root, path)
            || editor.entity_id() != id
            || !self.code_edits.editing(root, path)
            || self.git_is_busy()
            || editor.read(cx).value().as_ref() != expected
        {
            return Err(
                "워크트리·파일·초안이 변경되어 오래된 편집 결과를 적용하지 않았습니다.".into(),
            );
        }
        let position = editor.read(cx).cursor_position();
        let scroll = editor.read(cx).scroll_offset();
        editor.update(cx, |e, cx| {
            e.replace_all(replacement, window, cx);
            e.set_cursor_position(position, window, cx);
            e.set_scroll_offset(scroll, cx);
        });
        self.capture_code_buffers(cx);
        self.pin_preview(root, path);
        self.checkpoint_code_drafts(cx);
        self.refresh_inline_tools(root, path, &editor, cx);
        cx.notify();
        Ok(())
    }

    pub(super) fn editor_tool_buttons(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(
                Button::new("editor-palette")
                    .small()
                    .ghost()
                    .label("명령…")
                    .tooltip("명령 팔레트  ⌘⇧P")
                    .on_click(cx.listener(|a, _, w, cx| a.open_command_palette(w, cx))),
            )
            .child(
                Button::new("editor-files")
                    .small()
                    .ghost()
                    .label("파일 관리…")
                    .on_click(cx.listener(|a, _, w, cx| a.open_file_tools(w, cx))),
            )
            .child(
                Button::new("editor-line")
                    .small()
                    .ghost()
                    .label("줄 이동…")
                    .tooltip("⌘L")
                    .on_click(cx.listener(|a, _, w, cx| a.open_go_to_line(w, cx))),
            )
            .child(
                Button::new("editor-wrap")
                    .small()
                    .ghost()
                    .label("줄바꿈")
                    .selected(
                        self.current
                            .as_ref()
                            .is_some_and(|r| self.word_wrap(r, path)),
                    )
                    .on_click(cx.listener(|a, _, w, cx| a.toggle_word_wrap(w, cx))),
            )
            .when(
                self.current
                    .as_ref()
                    .is_some_and(|r| self.is_preview_tab(r, path)),
                |bar| {
                    bar.child(
                        Button::new("editor-pin")
                            .small()
                            .ghost()
                            .label("탭 고정")
                            .on_click(cx.listener(|a, _, w, cx| a.pin_code_tab(w, cx))),
                    )
                },
            )
            .child(
                Button::new("editor-previous")
                    .small()
                    .ghost()
                    .label("이전 코드")
                    .tooltip("커서가 있는 변경 구간의 이전 코드 펼치기")
                    .disabled(
                        !self
                            .current
                            .as_ref()
                            .and_then(|r| FileKey::new(r, path))
                            .is_some_and(|k| {
                                self.code_edits
                                    .tools
                                    .changes
                                    .get(&k)
                                    .is_some_and(|c| !c.is_empty())
                            }),
                    )
                    .on_click(cx.listener(|a, _, w, cx| a.toggle_previous_code(w, cx))),
            )
            .into_any_element()
    }
}
