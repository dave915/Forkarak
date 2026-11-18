//! One command registry for dispatch, preferences and shortcut presentation.
use super::*;
use editor_tools::*;
use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Preset {
    #[default]
    Forkarak,
    IntelliJ,
    VsCode,
}

pub(super) struct Shortcut {
    pub id: &'static str,
    pub label: &'static str,
    pub forkarak: &'static str,
    intellij: &'static str,
    vscode: &'static str,
    pub action: fn() -> Box<dyn Action>,
    pub contexts: &'static [Option<&'static str>],
}

const GLOBAL: &[Option<&str>] = &[None];
const GLOBAL_INPUT: &[Option<&str>] = &[None, Some("Input")];
const CODE: &[Option<&str>] = &[Some("ForkarakCode")];
const EDITOR: &[Option<&str>] = &[Some("ForkarakCode > Input")];
const FILE_SEARCH: &[Option<&str>] = &[Some("ForkarakFileSearch > Input")];

macro_rules! shortcut {
    ($id:literal, $label:literal, $forkarak:literal, $intellij:literal, $vscode:literal, $action:path, $contexts:ident) => {
        Shortcut {
            id: $id,
            label: $label,
            forkarak: $forkarak,
            intellij: $intellij,
            vscode: $vscode,
            action: || Box::new($action),
            contexts: $contexts,
        }
    };
}

// macOS reference cards: JetBrains IntelliJ IDEA and Microsoft's doc.keybindings.osx.json.
// Forkarak keeps a preview open; VS Code calls this Keep Editor (Pin Editor is a different command).
#[rustfmt::skip]
pub(super) static KEYBOARD_DEFAULTS: &[Shortcut] = &[
    shortcut!("find_file", "파일 찾기", "cmd-p", "cmd-shift-o", "cmd-p", FindFile, GLOBAL),
    shortcut!("find_text", "프로젝트 검색", "cmd-shift-f", "cmd-shift-f", "cmd-shift-f", FindText, GLOBAL_INPUT),
    shortcut!("replace_file", "파일 내 바꾸기", "cmd-alt-f", "cmd-r", "cmd-alt-f", ReplaceFile, EDITOR),
    shortcut!("command_palette", "명령 팔레트", "cmd-shift-p", "cmd-shift-a", "cmd-shift-p", CommandPalette, GLOBAL),
    shortcut!("find_definition", "정의로 이동", "cmd-b", "cmd-b", "f12", FindDefinition, GLOBAL),
    shortcut!("find_references", "사용처 찾기", "alt-f7", "alt-f7", "shift-f12", FindReferences, GLOBAL),
    shortcut!("find_implementation", "구현으로 이동", "", "cmd-alt-b", "cmd-f12", FindImplementation, CODE),
    shortcut!("go_to_line", "줄로 이동", "cmd-l", "cmd-l", "ctrl-g", GoToLine, EDITOR),
    shortcut!("select_next_word", "다음 같은 단어 선택", "cmd-d", "ctrl-g", "cmd-d", SelectNextWord, EDITOR),
    shortcut!("add_cursor_above", "위에 커서 추가", "ctrl-alt-up", "", "cmd-alt-up", AddCursorAbove, EDITOR),
    shortcut!("add_cursor_below", "아래에 커서 추가", "ctrl-alt-down", "", "cmd-alt-down", AddCursorBelow, EDITOR),
    shortcut!("move_line_up", "줄 위로 이동", "alt-up", "alt-shift-up", "alt-up", MoveLineUp, EDITOR),
    shortcut!("move_line_down", "줄 아래로 이동", "alt-down", "alt-shift-down", "alt-down", MoveLineDown, EDITOR),
    shortcut!("duplicate_line_up", "줄 위로 복제", "alt-shift-up", "", "alt-shift-up", DuplicateLineUp, EDITOR),
    shortcut!("duplicate_line_down", "줄 아래로 복제", "alt-shift-down", "cmd-d", "alt-shift-down", DuplicateLineDown, EDITOR),
    shortcut!("toggle_comment", "주석 토글", "cmd-/", "cmd-/", "cmd-/", ToggleComment, EDITOR),
    shortcut!("format_file", "파일 포맷", "alt-shift-f", "cmd-alt-l", "alt-shift-f", FormatFile, EDITOR),
    shortcut!("word_wrap", "화면 줄바꿈", "alt-z", "", "alt-z", ToggleWordWrap, EDITOR),
    shortcut!("pin_tab", "미리보기 탭 고정", "cmd-shift-enter", "", "cmd-k enter", PinTab, EDITOR),
    shortcut!("save_file", "파일 저장", "cmd-s", "", "cmd-s", SaveCodeFile, GLOBAL),
    shortcut!("save_all", "모두 저장", "cmd-alt-s", "cmd-s", "cmd-alt-s", SaveAll, GLOBAL),
    shortcut!("rename_symbol", "이름 바꾸기", "shift-f6", "shift-f6", "f2", RenameSymbol, EDITOR),
    shortcut!("quick_fix", "빠른 수정", "alt-enter", "alt-enter", "cmd-.", QuickFix, EDITOR),
    shortcut!("run_test", "현재 테스트 실행", "ctrl-shift-r", "ctrl-shift-r", "cmd-; c", RunTest, CODE),
    shortcut!("run_file_tests", "파일의 테스트 실행", "", "", "cmd-; f", RunFileTests, CODE),
    shortcut!("rerun_test", "마지막 테스트 다시 실행", "ctrl-r", "ctrl-r", "cmd-; l", RerunTest, CODE),
    shortcut!("split_right", "편집기 오른쪽으로 분할", "cmd-\\", "", "cmd-\\", SplitRight, CODE),
    shortcut!("move_to_other", "다른 쪽으로 이동", "", "", "", MoveToOther, CODE),
    shortcut!("focus_group_1", "왼쪽 그룹으로", "", "", "cmd-1", FocusGroup1, GLOBAL),
    shortcut!("focus_group_2", "오른쪽 그룹으로", "", "", "cmd-2", FocusGroup2, GLOBAL),
    shortcut!("open_to_side", "파일 찾기에서 옆에 열기", "cmd-enter", "shift-enter", "cmd-enter", OpenToSide, FILE_SEARCH),
    shortcut!("next_worktree", "다음 워크트리", "cmd-alt-down", "cmd-alt-down", "ctrl-cmd-down", NextWorktree, GLOBAL_INPUT),
    shortcut!("previous_worktree", "이전 워크트리", "cmd-alt-up", "cmd-alt-up", "ctrl-cmd-up", PreviousWorktree, GLOBAL_INPUT),
    shortcut!("toggle_sidebar", "Git 사이드바", "cmd-alt-b", "cmd-9", "cmd-b", ToggleSidebar, GLOBAL),
    shortcut!("toggle_files", "파일 목록", "cmd-shift-b", "cmd-1", "cmd-shift-e", ToggleFiles, GLOBAL),
    shortcut!("refresh", "새로고침", "cmd-r", "cmd-alt-y", "cmd-r", Refresh, GLOBAL),
    shortcut!("open_git_commit", "커밋", "cmd-shift-k", "cmd-k", "cmd-shift-k", OpenGitCommit, GLOBAL),
    shortcut!("toggle_code", "코드/diff", "cmd-alt-d", "cmd-alt-d", "cmd-alt-d", ToggleCode, GLOBAL),
    shortcut!("language_symbols", "파일 구조 검색", "cmd-shift-o", "cmd-f12", "cmd-shift-o", language_tools::Symbols, CODE),
    shortcut!("language_signature", "함수 도움말", "cmd-i", "cmd-i", "cmd-i", language_tools::Signature, CODE),
];

pub(super) fn shortcut(id: &str) -> Option<&'static Shortcut> {
    KEYBOARD_DEFAULTS.iter().find(|s| s.id == id)
}

#[derive(Clone, Copy)]
pub(super) struct Keyboard<'a> {
    pub preset: Preset,
    pub global: Option<&'a Map<String, Value>>,
    pub project: Option<&'a Map<String, Value>>,
}

impl<'a> Keyboard<'a> {
    pub fn new(prefs: &'a Preferences, root: Option<&Path>) -> Self {
        let preset = match prefs
            .option(root, "keyboard.preset")
            .and_then(Value::as_str)
        {
            Some("intellij") => Preset::IntelliJ,
            Some("vscode") => Preset::VsCode,
            _ => Preset::Forkarak,
        };
        Self {
            preset,
            global: prefs
                .options
                .get("keyboard.bindings")
                .and_then(Value::as_object),
            project: root
                .and_then(|p| prefs.project_options.get(p))
                .and_then(|m| m.get("keyboard.bindings"))
                .and_then(Value::as_object),
        }
    }

    pub fn custom(self, id: &str) -> Option<&'a str> {
        [self.project, self.global]
            .into_iter()
            .find_map(|m| m?.get(id)?.as_str())
    }

    pub fn key(self, s: &Shortcut) -> &'a str {
        self.custom(s.id)
            .map(str::trim)
            .unwrap_or(match self.preset {
                Preset::Forkarak => s.forkarak,
                Preset::IntelliJ => s.intellij,
                Preset::VsCode => s.vscode,
            })
    }

    pub fn alias(self, id: &str) -> Option<&'static str> {
        let key = match (id, self.preset) {
            ("find_definition", Preset::Forkarak) => "f12",
            ("find_references", Preset::Forkarak) => "alt-shift-f12",
            ("language_signature", _) => "ctrl-shift-space",
            _ => return None,
        };
        if self.custom(id).is_some()
            || KEYBOARD_DEFAULTS.iter().any(|s| {
                self.custom(s.id)
                    .and_then(|k| k.split_whitespace().next())
                    .is_some_and(|first| normalize_key(first) == normalize_key(key))
            })
        {
            return None;
        }
        Some(key)
    }

    pub fn custom_count(self) -> usize {
        KEYBOARD_DEFAULTS
            .iter()
            .filter(|s| self.custom(s.id).is_some())
            .count()
    }
}

pub(super) fn normalize_key(key: &str) -> String {
    key.split_whitespace()
        .map(|s| {
            Keystroke::parse_with_os(s, OperatingSystem::Mac)
                .map(|k| k.unparse(OperatingSystem::Mac))
                .unwrap_or_else(|_| s.to_owned())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{KEYBOARD_DEFAULTS, Keyboard, Preset, shortcut};
    use crate::prefs::Preferences;
    use serde_json::json;
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn legacy_session_preserves_every_existing_primary_and_navigation_alias() {
        let prefs: Preferences =
            serde_json::from_str(r#"{"worktrees":{},"options":{"editor.word_wrap":true}}"#)
                .unwrap();
        let keyboard = Keyboard::new(&prefs, None);
        assert_eq!(keyboard.preset, Preset::Forkarak);
        let existing = [
            ("find_file", "cmd-p"),
            ("find_text", "cmd-shift-f"),
            ("find_definition", "cmd-b"),
            ("find_references", "alt-f7"),
            ("next_worktree", "cmd-alt-down"),
            ("previous_worktree", "cmd-alt-up"),
            ("save_file", "cmd-s"),
            ("toggle_code", "cmd-alt-d"),
            ("replace_file", "cmd-alt-f"),
            ("select_next_word", "cmd-d"),
            ("add_cursor_above", "ctrl-alt-up"),
            ("add_cursor_below", "ctrl-alt-down"),
            ("move_line_up", "alt-up"),
            ("move_line_down", "alt-down"),
            ("duplicate_line_up", "alt-shift-up"),
            ("duplicate_line_down", "alt-shift-down"),
            ("go_to_line", "cmd-l"),
            ("word_wrap", "alt-z"),
            ("command_palette", "cmd-shift-p"),
            ("toggle_comment", "cmd-/"),
            ("pin_tab", "cmd-shift-enter"),
            ("format_file", "alt-shift-f"),
            ("toggle_sidebar", "cmd-alt-b"),
            ("toggle_files", "cmd-shift-b"),
            ("refresh", "cmd-r"),
            ("open_git_commit", "cmd-shift-k"),
            ("language_symbols", "cmd-shift-o"),
            ("language_signature", "cmd-i"),
        ];
        for (id, key) in existing {
            assert_eq!(keyboard.key(shortcut(id).unwrap()), key, "{id}");
        }
        assert_eq!(keyboard.alias("find_definition"), Some("f12"));
        assert_eq!(keyboard.alias("find_references"), Some("alt-shift-f12"));
        assert_eq!(
            keyboard.alias("language_signature"),
            Some("ctrl-shift-space")
        );
        assert_eq!(
            KEYBOARD_DEFAULTS
                .iter()
                .map(|s| s.id)
                .collect::<HashSet<_>>()
                .len(),
            KEYBOARD_DEFAULTS.len()
        );
    }

    #[test]
    fn per_command_overrides_survive_preset_and_project_switches_and_empty_means_unbound() {
        let project = Path::new("/fixture/project");
        let mut prefs = Preferences::default();
        prefs.set_option(None, "keyboard.preset", json!("intellij"));
        prefs.set_option(
            None,
            "keyboard.bindings",
            json!({"find_file":"f8","refresh":"f9","find_definition":""}),
        );
        prefs.set_option(
            Some(project),
            "keyboard.bindings",
            json!({"refresh":"","save_file":"f10"}),
        );
        for preset in ["forkarak", "intellij", "vscode"] {
            prefs.set_option(Some(project), "keyboard.preset", json!(preset));
            let keyboard = Keyboard::new(&prefs, Some(project));
            assert_eq!(keyboard.custom_count(), 4);
            for (id, key) in [
                ("find_file", "f8"),
                ("refresh", ""),
                ("find_definition", ""),
                ("save_file", "f10"),
            ] {
                assert_eq!(keyboard.key(shortcut(id).unwrap()), key);
            }
            assert!(keyboard.alias("find_definition").is_none());
        }
        prefs.remove_option(Some(project), "keyboard.bindings");
        let project_keys = Keyboard::new(&prefs, Some(project));
        assert_eq!(project_keys.key(shortcut("save_file").unwrap()), "cmd-s");
        assert_eq!(project_keys.key(shortcut("refresh").unwrap()), "f9");
        assert_eq!(
            Keyboard::new(&prefs, None).key(shortcut("save_file").unwrap()),
            ""
        );
        prefs.remove_option(Some(project), "keyboard.preset");
        assert_eq!(
            Keyboard::new(&prefs, Some(project)).preset,
            Preset::IntelliJ
        );
    }
}
