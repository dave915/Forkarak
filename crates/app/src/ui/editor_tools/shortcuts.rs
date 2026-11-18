//! Replace the managed part of GPUI's application-wide keymap when its settings change.
use super::*;
use gpui_kit::component::input::Replace;
use keyboard::{KEYBOARD_DEFAULTS, Keyboard, Preset};
use serde_json::{Map, Value};

struct RegisteredBindings {
    preset: Preset,
    global: Option<Map<String, Value>>,
    project: Option<Map<String, Value>>,
}

impl Global for RegisteredBindings {}

struct Request {
    keys: String,
    action: Box<dyn Action>,
    context: Option<&'static str>,
}

fn requests(prefs: &Preferences, project: Option<&Path>) -> Vec<Request> {
    let keyboard = Keyboard::new(prefs, project);
    let mut requests = Vec::new();
    // A preset switch may introduce a conflict with an existing override.
    // GPUI dispatches later bindings first at the same depth.
    let mut shortcuts: Vec<_> = KEYBOARD_DEFAULTS.iter().collect();
    shortcuts.sort_by_key(|s| {
        (
            keyboard.project.is_some_and(|m| m.contains_key(s.id)),
            keyboard.custom(s.id).is_some(),
        )
    });
    for shortcut in shortcuts {
        let key = keyboard.key(shortcut);
        if !key.is_empty() {
            for &context in shortcut.contexts {
                requests.push(Request {
                    keys: key.into(),
                    action: (shortcut.action)(),
                    context,
                });
            }
        }
        if let Some(keys) = keyboard.alias(shortcut.id) {
            requests.push(Request {
                keys: keys.into(),
                action: (shortcut.action)(),
                context: Some("ForkarakCode"),
            });
        }
    }
    requests
}

fn replace_managed(keymap: &mut Keymap, active: Vec<KeyBinding>) {
    // Old global Unbind markers can outrank a newer editor-only binding, and
    // a retired multi-stroke prefix can still swallow a keystroke. Rebuild in
    // one update, retaining unrelated bindings, metadata and masks in order.
    let mut managed: HashSet<_> = KEYBOARD_DEFAULTS
        .iter()
        .map(|s| (s.action)().as_ref().type_id())
        .collect();
    // GPUI's built-in Replace uses the project-search chord in ordinary inputs.
    managed.insert(std::any::TypeId::of::<Replace>());
    let retained: Vec<_> = keymap
        .bindings()
        .filter(|b| !managed.contains(&b.action().type_id()))
        .cloned()
        .collect();
    keymap.clear();
    keymap.add_bindings(retained.into_iter().chain(active));
}

pub(super) fn install(prefs: &Preferences, project: Option<&Path>, cx: &mut App) {
    let keyboard = Keyboard::new(prefs, project);
    if cx
        .try_global::<RegisteredBindings>()
        .is_some_and(|installed| {
            installed.preset == keyboard.preset
                && installed.global.as_ref() == keyboard.global
                && installed.project.as_ref() == keyboard.project
        })
    {
        return;
    }
    let active = requests(prefs, project)
        .into_iter()
        .filter_map(|request| {
            let predicate = request.context.map(|c| {
                KeyBindingContextPredicate::parse(c)
                    .expect("registered shortcut context")
                    .into()
            });
            KeyBinding::load(&request.keys, request.action, predicate, false, None, cx).ok()
        })
        .collect();
    replace_managed(&mut cx.key_bindings().borrow_mut(), active);
    install_menus(keyboard, cx);
    cx.set_global(RegisteredBindings {
        preset: keyboard.preset,
        global: keyboard.global.cloned(),
        project: keyboard.project.cloned(),
    });
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::{replace_managed, requests};
    use crate::prefs::Preferences;
    use crate::ui::{
        FindDefinition, FindFile, FindReferences, FindText, NextWorktree, PreviousWorktree,
        SaveCodeFile, ToggleCode, ToggleSidebar,
    };
    use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
    use gpui_kit::component::input::{Replace, Search, Undo};
    use gpui_kit::{
        Action, AsKeystroke, KeyBinding, KeyContext, Keymap, Keystroke, NoAction, OperatingSystem,
    };
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
    fn replacing_a_keymap_retires_contextual_and_global_keys_aliases_and_chord_prefixes() {
        use crate::ui::editor_tools::RunTest;
        let code = ["ForkarakCode", "Input"];
        let generic = ["Input"];
        let mut map = Keymap::new(vec![
            KeyBinding::new("cmd-shift-f", Replace, Some("Input")),
            KeyBinding::new("cmd-alt-up", AddCursorAbove, Some("Input")),
            KeyBinding::new("cmd-alt-down", AddCursorBelow, Some("Input")),
            KeyBinding::new("cmd-f", Search, Some("Input")),
            KeyBinding::new("cmd-z", Undo, Some("Input")),
            KeyBinding::new("cmd-z", NoAction, Some("Input")),
            KeyBinding::new("ctrl-s", SaveCodeFile, Some("Input")),
            KeyBinding::new("cmd-alt-b", ToggleSidebar, None),
            KeyBinding::new("f12", FindDefinition, Some("ForkarakCode")),
        ]);
        let vscode = || {
            vec![
                KeyBinding::new("f12", FindDefinition, None),
                KeyBinding::new("cmd-b", ToggleSidebar, None),
                KeyBinding::new("cmd-alt-up", AddCursorAbove, Some("ForkarakCode > Input")),
                KeyBinding::new("cmd-; c", RunTest, Some("ForkarakCode")),
            ]
        };
        replace_managed(&mut map, vscode());
        assert_eq!(resolves(&map, "f12", &generic), [FindDefinition.name()]);
        assert_eq!(resolves(&map, "cmd-alt-up", &code), [AddCursorAbove.name()]);
        assert!(resolves(&map, "cmd-alt-up", &generic).is_empty());
        assert!(resolves(&map, "cmd-alt-b", &code).is_empty());
        assert!(resolves(&map, "ctrl-s", &code).is_empty());
        assert!(resolves(&map, "cmd-shift-f", &generic).is_empty());
        let prefix = [Keystroke::parse("cmd-;").unwrap()];
        let stack = code.map(|c| KeyContext::parse(c).unwrap());
        assert!(map.bindings_for_input(&prefix, &stack).1);
        let binding_count = map.bindings().len();

        replace_managed(
            &mut map,
            vec![KeyBinding::new("f12", FindDefinition, Some("ForkarakCode"))],
        );
        assert!(!map.bindings_for_input(&prefix, &stack).1);
        assert!(resolves(&map, "f12", &generic).is_empty());
        assert_eq!(resolves(&map, "f12", &code), [FindDefinition.name()]);
        assert!(map.bindings_for_action(&ToggleSidebar).next().is_none());
        replace_managed(&mut map, vscode());
        assert_eq!(map.bindings().len(), binding_count);
        // AppKit takes the first eligible item from this same iterator.
        let equivalent = map.bindings_for_action(&ToggleSidebar).next().unwrap();
        assert_eq!(
            equivalent.keystrokes()[0]
                .as_keystroke()
                .unparse(OperatingSystem::Mac),
            "cmd-b"
        );
        assert_eq!(resolves(&map, "cmd-f", &generic), [Search.name()]);
        assert!(resolves(&map, "cmd-z", &generic).is_empty());
    }

    #[test]
    fn distributed_features_have_stable_dispatched_types_and_contexts() {
        let mut prefs = Preferences::default();
        prefs.set_option(None, "keyboard.preset", json!("vscode"));
        let active = requests(&prefs, None);
        for (key, expected, context) in [
            ("cmd-alt-s", "SaveAll", None),
            ("f2", "RenameSymbol", Some("ForkarakCode > Input")),
            ("cmd-.", "QuickFix", Some("ForkarakCode > Input")),
            ("cmd-; c", "RunTest", Some("ForkarakCode")),
            ("cmd-; f", "RunFileTests", Some("ForkarakCode")),
            ("cmd-; l", "RerunTest", Some("ForkarakCode")),
            ("cmd-\\", "SplitRight", Some("ForkarakCode")),
            ("cmd-1", "FocusGroup1", None),
            ("cmd-2", "FocusGroup2", None),
            (
                "cmd-enter",
                "OpenToSide",
                Some("ForkarakFileSearch > Input"),
            ),
        ] {
            let req = active.iter().find(|r| r.keys == key).unwrap();
            assert_eq!(req.action.name(), format!("forkarak_editor::{expected}"));
            assert_eq!(req.context, context);
            for stroke in key.split_whitespace() {
                Keystroke::parse(stroke).unwrap();
            }
        }
        assert!(active.iter().any(
            |r| r.keys == "cmd-f12" && r.action.name() == crate::ui::FindImplementation.name()
        ));
        for (preset, definition, references) in [
            ("intellij", "cmd-b", "alt-f7"),
            ("vscode", "f12", "shift-f12"),
        ] {
            prefs.set_option(None, "keyboard.preset", json!(preset));
            let active = requests(&prefs, None);
            for (action, key) in [
                (FindDefinition.name(), definition),
                (FindReferences.name(), references),
            ] {
                assert_eq!(
                    active
                        .iter()
                        .filter(|r| r.action.name() == action)
                        .map(|r| r.keys.as_str())
                        .collect::<Vec<_>>(),
                    [key]
                );
            }
        }
    }
}
