//! Managed shortcut overrides use GPUI targeted Unbind entries. Native and
//! unrelated application bindings, their metadata and disabled state stay intact.
use super::*;
use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
use gpui_kit::component::input::Replace;

struct RegisteredBindings(Vec<KeyBinding>);
impl Global for RegisteredBindings {}

struct Request {
    keys: String,
    action: Box<dyn Action>,
    context: Option<&'static str>,
}

fn requests(prefs: &Preferences, project: Option<&Path>) -> Vec<Request> {
    let overrides = prefs
        .option(project, "keyboard.bindings")
        .and_then(|v| v.as_object());
    let mut result = Vec::new();
    macro_rules! bind {
        ($id:literal,$key:literal,$action:expr,$context:expr) => {
            let key = overrides
                .and_then(|v| v.get($id))
                .and_then(|v| v.as_str())
                .unwrap_or($key);
            if !key.trim().is_empty() {
                result.push(Request {
                    keys: key.to_owned(),
                    action: Box::new($action),
                    context: $context,
                });
            }
        };
    }
    bind!("command_palette", "cmd-shift-p", CommandPalette, None);
    bind!(
        "toggle_comment",
        "cmd-/",
        ToggleComment,
        Some("ForkarakCode > Input")
    );
    bind!(
        "move_line_up",
        "alt-up",
        MoveLineUp,
        Some("ForkarakCode > Input")
    );
    bind!(
        "move_line_down",
        "alt-down",
        MoveLineDown,
        Some("ForkarakCode > Input")
    );
    bind!(
        "duplicate_line_up",
        "alt-shift-up",
        DuplicateLineUp,
        Some("ForkarakCode > Input")
    );
    bind!(
        "duplicate_line_down",
        "alt-shift-down",
        DuplicateLineDown,
        Some("ForkarakCode > Input")
    );
    bind!(
        "select_next_word",
        "cmd-d",
        SelectNextWord,
        Some("ForkarakCode > Input")
    );
    bind!(
        "replace_file",
        "cmd-alt-f",
        ReplaceFile,
        Some("ForkarakCode > Input")
    );
    bind!(
        "go_to_line",
        "cmd-l",
        GoToLine,
        Some("ForkarakCode > Input")
    );
    bind!(
        "word_wrap",
        "alt-z",
        ToggleWordWrap,
        Some("ForkarakCode > Input")
    );
    bind!(
        "pin_tab",
        "cmd-shift-enter",
        PinTab,
        Some("ForkarakCode > Input")
    );
    bind!(
        "format_file",
        "alt-shift-f",
        FormatFile,
        Some("ForkarakCode > Input")
    );
    bind!("find_file", "cmd-p", FindFile, None);
    bind!("find_text", "cmd-shift-f", FindText, None);
    bind!("find_text", "cmd-shift-f", FindText, Some("Input"));
    if let Some(key) = settings::navigation_alias("find_definition", overrides) {
        result.push(Request {
            keys: key.into(),
            action: Box::new(FindDefinition),
            context: Some("ForkarakCode"),
        });
    }
    if let Some(key) = settings::navigation_alias("find_references", overrides) {
        result.push(Request {
            keys: key.into(),
            action: Box::new(FindReferences),
            context: Some("ForkarakCode"),
        });
    }
    bind!("find_definition", "cmd-b", FindDefinition, None);
    bind!("find_references", "alt-f7", FindReferences, None);
    bind!("next_worktree", "cmd-alt-down", NextWorktree, None);
    bind!("next_worktree", "cmd-alt-down", NextWorktree, Some("Input"));
    bind!("previous_worktree", "cmd-alt-up", PreviousWorktree, None);
    bind!(
        "previous_worktree",
        "cmd-alt-up",
        PreviousWorktree,
        Some("Input")
    );
    bind!("save_file", "cmd-s", SaveCodeFile, None);
    bind!("toggle_code", "cmd-alt-d", ToggleCode, None);
    // Worktree switching and multiple cursors use distinct modifier chords.
    bind!(
        "add_cursor_above",
        "ctrl-alt-up",
        AddCursorAbove,
        Some("ForkarakCode > Input")
    );
    bind!(
        "add_cursor_below",
        "ctrl-alt-down",
        AddCursorBelow,
        Some("ForkarakCode > Input")
    );
    result
}

pub(super) fn install(prefs: &Preferences, project: Option<&Path>, cx: &mut App) {
    // Keep only our current registration, rather than rediscovering retired keys
    // from the append-only GPUI keymap on every project switch.
    let previous = cx
        .try_global::<RegisteredBindings>()
        .map(|v| v.0.clone())
        .unwrap_or_else(|| {
            let mut managed: Vec<_> = requests(&Preferences::default(), None)
                .iter()
                .map(|r| r.action.as_ref().type_id())
                .collect();
            managed.push(std::any::TypeId::of::<Replace>());
            cx.key_bindings()
                .borrow()
                .bindings()
                .filter(|binding| managed.contains(&binding.action().type_id()))
                .cloned()
                .collect()
        });
    let mut updates: Vec<_> = previous
        .iter()
        .filter_map(|binding| {
            let key = binding
                .keystrokes()
                .iter()
                .map(|key| key.as_keystroke().unparse(cx.platform_info().os))
                .collect::<Vec<_>>()
                .join(" ");
            KeyBinding::load(
                &key,
                Box::new(Unbind(binding.action().name().into())),
                binding.predicate(),
                false,
                None,
                cx,
            )
            .ok()
        })
        .collect();
    let active: Vec<_> = requests(prefs, project)
        .into_iter()
        .filter_map(|request| {
            let predicate = request
                .context
                .and_then(|context| KeyBindingContextPredicate::parse(context).ok())
                .map(Into::into);
            KeyBinding::load(&request.keys, request.action, predicate, false, None, cx).ok()
        })
        .collect();
    // New bindings follow their retirement markers, so the current command may
    // reuse the old chord. Unbind targets the old action, not unrelated commands.
    updates.extend(active.iter().cloned());
    cx.bind_keys(updates);
    cx.set_global(RegisteredBindings(active));
}

#[cfg(test)]
mod tests {
    use super::requests;
    use crate::prefs::Preferences;
    use crate::ui::{
        FindDefinition, FindFile, FindReferences, FindText, NextWorktree, PreviousWorktree,
        Refresh, SaveCodeFile, ToggleCode,
    };
    use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
    use gpui_kit::component::input::{Replace, Search, Undo};
    use gpui_kit::{Action, KeyBinding, KeyContext, Keymap, Keystroke, NoAction, Unbind};
    use serde_json::json;

    fn resolves(keymap: &Keymap, key: &'static str, contexts: &[&str]) -> Vec<&'static str> {
        let contexts: Vec<_> = contexts
            .iter()
            .map(|text| KeyContext::parse(text).unwrap())
            .collect();
        keymap
            .bindings_for_input(&[Keystroke::parse(key).unwrap()], &contexts)
            .0
            .iter()
            .map(|binding| binding.action().name())
            .collect()
    }

    #[test]
    fn function_keys_are_reader_only_and_yield_to_user_bindings() {
        let mut prefs = Preferences::default();
        let defaults = requests(&prefs, None);
        for (key, action) in [
            ("f12", FindDefinition.name()),
            ("alt-shift-f12", FindReferences.name()),
        ] {
            let registration = defaults.iter().find(|r| r.keys == key).unwrap();
            assert_eq!(registration.action.name(), action);
            assert_eq!(registration.context, Some("ForkarakCode"));
            assert!(Keystroke::parse(key).is_ok());
        }
        prefs.set_option(
            None,
            "keyboard.bindings",
            json!({"find_file":"f12","find_references":""}),
        );
        let custom = requests(&prefs, None);
        assert!(
            custom
                .iter()
                .all(|r| r.keys != "f12" || r.action.name() == FindFile.name())
        );
        assert!(
            custom
                .iter()
                .all(|r| r.action.name() != FindReferences.name())
        );
    }

    #[test]
    fn every_persisted_root_command_consumes_canonical_project_overrides_and_reset() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().canonicalize().unwrap();
        let other = project.join("different-project");
        let mut prefs = Preferences::default();
        prefs.set_option(None,"keyboard.bindings",json!({"find_file":"ctrl-1","find_text":"ctrl-2","find_definition":"ctrl-3","find_references":"ctrl-4","next_worktree":"ctrl-5","previous_worktree":"ctrl-6","save_file":"ctrl-7","toggle_code":"ctrl-8"}));
        prefs.set_option(Some(&project),"keyboard.bindings",json!({"find_file":"alt-1","find_text":"alt-2","find_definition":"alt-3","find_references":"alt-4","next_worktree":"alt-5","previous_worktree":"alt-6","save_file":"alt-7","toggle_code":"alt-8"}));
        let names = [
            FindFile.name(),
            FindText.name(),
            FindDefinition.name(),
            FindReferences.name(),
            NextWorktree.name(),
            PreviousWorktree.name(),
            SaveCodeFile.name(),
            ToggleCode.name(),
        ];
        for (root, modifier) in [
            (Some(project.as_path()), "alt"),
            (Some(other.as_path()), "ctrl"),
            (None, "ctrl"),
        ] {
            let registered = requests(&prefs, root);
            for (index, name) in names.iter().enumerate() {
                let matching: Vec<_> = registered
                    .iter()
                    .filter(|r| r.action.name() == *name)
                    .collect();
                assert!(!matching.is_empty(), "missing runtime command: {name}");
                assert!(
                    matching
                        .iter()
                        .all(|r| r.keys == format!("{modifier}-{}", index + 1)),
                    "inert override for {name}"
                );
            }
        }
        prefs.set_option(Some(&project), "keyboard.bindings", json!({"find_file":""}));
        assert!(
            requests(&prefs, Some(&project))
                .iter()
                .all(|r| r.action.name() != FindFile.name())
        );
        prefs.remove_option(Some(&project), "keyboard.bindings");
        assert!(
            requests(&prefs, Some(&project))
                .iter()
                .filter(|r| r.action.name() == FindFile.name())
                .all(|r| r.keys == "ctrl-1")
        );
        prefs.remove_option(None, "keyboard.bindings");
        assert!(
            requests(&prefs, Some(&project))
                .iter()
                .filter(|r| r.action.name() == FindFile.name())
                .all(|r| r.keys == "cmd-p")
        );
    }

    #[test]
    fn targeted_retirement_switches_projects_without_resetting_native_or_disabled_bindings() {
        let mut map = Keymap::new(vec![
            KeyBinding::new("cmd-p", FindFile, None),
            KeyBinding::new("cmd-f", Search, Some("Input")),
            KeyBinding::new("cmd-z", Undo, Some("Input")),
            KeyBinding::new("ctrl-r", Refresh, None),
            KeyBinding::new("ctrl-r", NoAction, Some("Input")),
        ]);
        map.add_bindings([
            KeyBinding::new("cmd-p", Unbind(FindFile.name().into()), None),
            KeyBinding::new("ctrl-1", FindFile, None),
        ]);
        map.add_bindings([
            KeyBinding::new("ctrl-1", Unbind(FindFile.name().into()), None),
            KeyBinding::new("ctrl-2", FindFile, None),
        ]);
        let contexts = ["Forkarak", "ForkarakCode", "Input"];
        assert!(resolves(&map, "cmd-p", &contexts).is_empty());
        assert!(resolves(&map, "ctrl-1", &contexts).is_empty());
        assert_eq!(resolves(&map, "ctrl-2", &contexts), vec![FindFile.name()]);
        assert_eq!(resolves(&map, "cmd-f", &contexts), vec![Search.name()]);
        assert_eq!(resolves(&map, "cmd-z", &contexts), vec![Undo.name()]);
        assert!(resolves(&map, "ctrl-r", &contexts).is_empty());
        // Returning to a previous project reactivates its chord after its old
        // targeted marker without resurrecting either obsolete registration.
        map.add_bindings([
            KeyBinding::new("ctrl-2", Unbind(FindFile.name().into()), None),
            KeyBinding::new("ctrl-1", FindFile, None),
        ]);
        assert_eq!(resolves(&map, "ctrl-1", &contexts), vec![FindFile.name()]);
        assert!(resolves(&map, "ctrl-2", &contexts).is_empty());
        assert_eq!(resolves(&map, "cmd-z", &contexts), vec![Undo.name()]);
    }

    #[test]
    fn native_input_shortcuts_do_not_steal_project_search_or_worktree_commands() {
        let mut map = Keymap::new(vec![
            KeyBinding::new("cmd-shift-f", Replace, Some("Input")),
            KeyBinding::new("cmd-alt-up", AddCursorAbove, Some("Input")),
            KeyBinding::new("cmd-alt-down", AddCursorBelow, Some("Input")),
        ]);
        map.add_bindings([
            KeyBinding::new("cmd-shift-f", Unbind(Replace.name().into()), Some("Input")),
            KeyBinding::new(
                "cmd-alt-up",
                Unbind(AddCursorAbove.name().into()),
                Some("Input"),
            ),
            KeyBinding::new(
                "cmd-alt-down",
                Unbind(AddCursorBelow.name().into()),
                Some("Input"),
            ),
            KeyBinding::new("cmd-shift-f", FindText, None),
            KeyBinding::new("cmd-shift-f", FindText, Some("Input")),
            KeyBinding::new("cmd-alt-up", PreviousWorktree, Some("Input")),
            KeyBinding::new("cmd-alt-down", NextWorktree, Some("Input")),
            KeyBinding::new("ctrl-alt-up", AddCursorAbove, Some("ForkarakCode > Input")),
            KeyBinding::new(
                "ctrl-alt-down",
                AddCursorBelow,
                Some("ForkarakCode > Input"),
            ),
        ]);
        let contexts = ["Forkarak", "ForkarakCode", "Input"];
        assert!(
            resolves(&map, "cmd-shift-f", &contexts)
                .iter()
                .all(|name| *name == FindText.name())
        );
        assert_eq!(
            resolves(&map, "cmd-alt-up", &contexts),
            vec![PreviousWorktree.name()]
        );
        assert_eq!(
            resolves(&map, "cmd-alt-down", &contexts),
            vec![NextWorktree.name()]
        );
        assert_eq!(
            resolves(&map, "ctrl-alt-up", &contexts),
            vec![AddCursorAbove.name()]
        );
        assert_eq!(
            resolves(&map, "ctrl-alt-down", &contexts),
            vec![AddCursorBelow.name()]
        );
    }
}
