//! Explicit editor actions. Every follow-up belongs to the same displayed source.
use super::*;
use gpui_kit::component::native_menu::NativeMenu;
use serde::Deserialize;

mod bulbs;
mod preview;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
enum ContextCommand {
    Definition,
    Rename,
    QuickFix,
}

#[derive(Action, Clone, Debug, PartialEq, Deserialize)]
#[action(namespace = forkarak_language, no_json)]
pub(super) struct EditAt {
    view: u64,
    version: i64,
    cursor: usize,
    selection: std::ops::Range<usize>,
    offset: usize,
    command: ContextCommand,
}

impl Pokarak {
    pub(in crate::ui) fn editor_context_menu(
        &mut self,
        mut menu: NativeMenu,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) -> NativeMenu {
        use gpui_kit::base::input::{Copy, Cut, Paste, SelectAll};
        let state = editor.read(cx);
        let selected = !state.selected_range().is_empty();
        let readonly = self
            .current
            .as_ref()
            .zip(self.selected().as_ref())
            .is_none_or(|(root, path)| !self.code_edits.editing(root, path));
        let offset = state.context_menu_offset().unwrap_or(state.cursor());
        if self.active_editor().as_ref() == Some(editor)
            && let Ok(identity) = self.language_identity(cx)
        {
            for (label, id, command) in [
                ("정의로 이동", "find_definition", ContextCommand::Definition),
                ("이름 바꾸기…", "rename_symbol", ContextCommand::Rename),
                ("빠른 수정…", "quick_fix", ContextCommand::QuickFix),
            ] {
                menu = menu.menu(
                    keycaps::command_label(label, id, &self.prefs, self.current_project.as_deref()),
                    Box::new(EditAt {
                        view: identity.view,
                        version: identity.snapshot.version(),
                        cursor: identity.cursor,
                        selection: identity.selection.clone(),
                        offset,
                        command,
                    }),
                );
            }
            menu = menu.separator();
        }
        menu.menu_with_disabled("잘라내기", readonly || !selected, Box::new(Cut))
            .menu_with_disabled("복사", !selected, Box::new(Copy))
            .menu_with_disabled("붙여넣기", readonly, Box::new(Paste))
            .separator()
            .menu("전체 선택", Box::new(SelectAll))
    }

    pub(super) fn edit_at(&mut self, action: &EditAt, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(identity) = self.language_request_identity(cx) else {
            return;
        };
        if identity.view != action.view
            || identity.snapshot.version() != action.version
            || identity.cursor != action.cursor
            || identity.selection != action.selection
            || !identity.snapshot.text().is_char_boundary(action.offset)
        {
            return;
        }
        let Some(editor) = self.active_editor() else {
            return;
        };
        editor.update(cx, |s, cx| {
            s.focus(window, cx);
            s.set_selected_range(action.offset..action.offset, cx);
        });
        let tool = match action.command {
            ContextCommand::Definition => Tool::Navigate(NavigationKind::Definition),
            ContextCommand::Rename => Tool::PrepareRename,
            ContextCommand::QuickFix => Tool::QuickFix,
        };
        self.request_language(tool, None, window, cx);
    }

    fn edit_request_current(&mut self, token: u64, identity: &Identity, cx: &App) -> bool {
        token == self.language_tools.request
            && self.navigation.is_some()
            && !self.git_is_busy()
            && !self.active_worktree_has_git_job()
            && self.accepts_language(identity, true, cx)
    }

    pub(super) fn submit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.language_tools.panel.as_ref() else {
            return;
        };
        let Reply::RenameInput(_, old) = &panel.value else {
            return;
        };
        let Some(input) = self.language_tools.rename_input.as_ref() else {
            return;
        };
        let name = input.read(cx).value().to_string();
        if name.trim().is_empty() {
            self.status = "새 이름을 입력하세요.".into();
            cx.notify();
            return;
        }
        let identity = panel.identity.clone();
        let unchanged = old == &name;
        let current = self.edit_request_current(self.language_tools.request, &identity, cx);
        self.dismiss_language(window, cx);
        if unchanged || !current {
            return;
        }
        self.run_language_request(Tool::Rename(name), None, false, window, cx);
    }

    pub(super) fn pick_code_action(
        &mut self,
        token: u64,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.language_tools.panel.as_ref() else {
            return;
        };
        let Reply::Actions(actions, _) = &panel.value else {
            return;
        };
        let Some(action) = actions
            .items
            .get(index)
            .filter(|a| a.disabled.is_none())
            .cloned()
        else {
            return;
        };
        let identity = panel.identity.clone();
        if !self.edit_request_current(token, &identity, cx) {
            return;
        }
        self.dismiss_language(window, cx);
        if let Some(edit) = action.edit {
            self.present_workspace_edit(identity, Tool::QuickFix, edit, window, cx);
        } else if action.resolve_data.is_some() {
            self.run_language_request(Tool::Resolve(Arc::new(action)), None, false, window, cx);
        }
    }

    pub(super) fn present_workspace_edit(
        &mut self,
        identity: Identity,
        tool: Tool,
        mut edit: WorkspaceEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx)
            || !self.edit_request_current(self.language_tools.request, &identity, cx)
        {
            return;
        }
        edit.files.retain(|f| !f.edits.is_empty());
        if edit.files.is_empty() {
            self.status = "적용할 수정이 없습니다.".into();
        } else if needs_preview(&edit, identity.snapshot.path()) {
            edit.files.sort_by(|a, b| a.path.cmp(&b.path));
            self.status = format!("{} · 초안에 적용할 변경을 확인하세요.", tool.title());
            self.open_workspace_preview(identity, tool.title(), edit, window, cx);
        } else {
            self.apply_language_edit(&identity, tool.title(), &edit, window, cx);
        }
        cx.notify();
    }

    fn apply_language_edit(
        &mut self,
        identity: &Identity,
        label: &str,
        edit: &WorkspaceEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.apply_workspace_edit(&identity.key.root, edit, label, window, cx) {
            Ok(()) => {
                self.error = None;
                self.status = format!(
                    "{label} · {}개 파일의 초안에 적용했습니다.",
                    edit.files.len()
                );
            }
            Err(reason) => self.error = Some(reason),
        }
    }
}

pub(super) fn action_context(
    position: LspPosition,
    items: &[Diagnostic],
) -> (TextRange, Vec<Diagnostic>) {
    let diagnostics: Vec<_> = items
        .iter()
        .filter(|d| {
            d.range.start.line <= position.line
                && position.line <= d.range.end.line
                && !(d.range.start.line < position.line
                    && d.range.end
                        == (LspPosition {
                            line: position.line,
                            character: 0,
                        }))
        })
        .cloned()
        .collect();
    let range = diagnostics
        .iter()
        .find(|d| d.range.start <= position && position <= d.range.end)
        .map(|d| d.range)
        .unwrap_or(TextRange {
            start: position,
            end: position,
        });
    (range, diagnostics)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ActionGroup {
    Preferred,
    QuickFix,
    Refactor,
    Source,
    Other,
}
impl ActionGroup {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Preferred => "추천",
            Self::QuickFix => "빠른 수정",
            Self::Refactor => "리팩터링",
            Self::Source => "소스",
            Self::Other => "기타",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActionRow {
    Heading(ActionGroup),
    Item(usize),
}

pub(super) struct GroupedActions {
    pub items: Vec<CodeAction>,
    pub rows: Vec<ActionRow>,
}
impl GroupedActions {
    pub fn new(mut items: Vec<CodeAction>) -> Self {
        items.sort_by_key(action_group);
        let rows = action_rows(items.iter().map(action_group));
        Self { items, rows }
    }
    pub fn row_for(&self, index: usize) -> usize {
        self.rows
            .iter()
            .position(|r| *r == ActionRow::Item(index))
            .unwrap_or_default()
    }
}

fn action_rows(groups: impl Iterator<Item = ActionGroup>) -> Vec<ActionRow> {
    let mut previous = None;
    let mut rows = Vec::new();
    for (index, group) in groups.enumerate() {
        if previous != Some(group) {
            rows.push(ActionRow::Heading(group));
            previous = Some(group);
        }
        rows.push(ActionRow::Item(index));
    }
    rows
}

fn action_group(action: &CodeAction) -> ActionGroup {
    ActionGroup::of(action.preferred, action.kind.as_deref())
}

impl ActionGroup {
    fn of(preferred: bool, kind: Option<&str>) -> Self {
        if preferred {
            return Self::Preferred;
        }
        match kind.and_then(|k| k.split('.').next()) {
            Some("quickfix") => ActionGroup::QuickFix,
            Some("refactor") => ActionGroup::Refactor,
            Some("source") => ActionGroup::Source,
            _ => ActionGroup::Other,
        }
    }
}

fn needs_preview(edit: &WorkspaceEdit, active: &Path) -> bool {
    edit.files.iter().any(|file| file.path != active)
}

#[cfg(test)]
mod tests {
    use super::{
        ActionGroup, Diagnostic, LspPosition, TextEdit, TextRange, WorkspaceEdit, action_context,
        needs_preview,
    };
    use std::path::Path;

    #[test]
    fn quick_fix_groups_prefer_server_recommendations_then_preserve_server_order() {
        let mut actions = [
            ("Organize imports", false, Some("source.organizeImports")),
            ("Extract", false, Some("refactor.extract.function")),
            ("Import A", false, Some("quickfix")),
            ("Move", true, Some("refactor.rewrite")),
            ("Import B", false, Some("quickfix")),
            ("Other", false, None),
        ];
        actions.sort_by_key(|(_, preferred, kind)| ActionGroup::of(*preferred, *kind));
        assert_eq!(
            actions.map(|a| a.0),
            [
                "Move",
                "Import A",
                "Import B",
                "Extract",
                "Organize imports",
                "Other"
            ]
        );
    }

    #[test]
    fn one_heading_per_group_keeps_keyboard_action_indices_independent_of_display_rows() {
        use super::{ActionGroup::*, ActionRow::*, action_rows};
        assert_eq!(
            action_rows([Preferred, QuickFix, QuickFix, QuickFix, Source].into_iter()),
            [
                Heading(Preferred),
                Item(0),
                Heading(QuickFix),
                Item(1),
                Item(2),
                Item(3),
                Heading(Source),
                Item(4)
            ]
        );
    }

    #[test]
    fn quick_fix_context_selects_the_diagnostic_under_the_cursor_and_sends_its_line() {
        let pos = |line, character| LspPosition { line, character };
        let diagnostic = |name, start, end| Diagnostic {
            range: TextRange { start, end },
            severity: None,
            code: None,
            source: None,
            message: String::from(name),
        };
        let items = vec![
            diagnostic("previous", pos(0, 0), pos(2, 0)),
            diagnostic("first", pos(2, 1), pos(2, 3)),
            diagnostic("target", pos(2, 5), pos(2, 9)),
            diagnostic("next", pos(3, 1), pos(3, 5)),
        ];
        let (range, context) = action_context(pos(2, 6), &items);
        assert_eq!(range, items[2].range);
        assert_eq!(
            context
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>(),
            ["first", "target"]
        );
        let cursor = pos(4, 1);
        let (range, context) = action_context(cursor, &items);
        assert!(context.is_empty());
        assert_eq!(
            range,
            TextRange {
                start: cursor,
                end: cursor
            }
        );
    }

    #[test]
    fn preview_is_required_even_if_the_only_affected_file_is_unopened() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("service.ts");
        let other = dir.path().join("usage.ts");
        let file = |path: &Path| pokarak_navigation::FileEdits {
            path: path.to_owned(),
            original: "old()".into(),
            edits: vec![TextEdit {
                range: TextRange {
                    start: LspPosition::default(),
                    end: LspPosition {
                        line: 0,
                        character: 3,
                    },
                },
                new_text: "new".into(),
            }],
        };
        assert!(!needs_preview(
            &WorkspaceEdit {
                files: vec![file(&active)]
            },
            &active
        ));
        assert!(needs_preview(
            &WorkspaceEdit {
                files: vec![file(&other)]
            },
            &active
        ));
        assert!(needs_preview(
            &WorkspaceEdit {
                files: vec![file(&other), file(&active)]
            },
            &active
        ));
    }
}
