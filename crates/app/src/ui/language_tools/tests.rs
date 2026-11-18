use super::model::{Identity, completion_edit, configs, parameter_text, symbol_rows};
use super::{Reply, Tool, execute};
use crate::ui::code_edit::FileKey;
use gpui_kit::EntityId;
use pokarak_navigation::{
    DocumentSnapshot, DocumentSymbol, FormattingOptions, NavigationKind, Navigator,
    Position as LspPosition, TextEdit, TextRange,
};
use std::{collections::HashSet, path::PathBuf, time::Duration};
use std::{fs, process::Command};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    snapshot: DocumentSnapshot,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("authored source 한 😀");
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "--"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let root = root.canonicalize().unwrap();
        fs::write(
            root.join("entry.ts"),
            "// disk source\nconst disk = 0;\ndisk",
        )
        .unwrap();
        fs::write(root.join("peer.py"), include_str!("test_peer.py")).unwrap();
        let snapshot = DocumentSnapshot::new(
            root.clone(),
            "entry.ts".into(),
            "// 😀\nconst dirty = 1;\ndirty".into(),
            1,
        )
        .unwrap();
        Self {
            _temp: temp,
            root,
            snapshot,
        }
    }
    fn nav(&self) -> Navigator {
        Navigator::with_options(
            self.root.clone(),
            vec![pokarak_navigation::ServerConfig {
                language: "typescript".into(),
                program: "/usr/bin/python3".into(),
                args: vec![self.root.join("peer.py").to_str().unwrap().into()],
            }],
            pokarak_navigation::NavigatorOptions {
                config_file: None,
                initialize_timeout: Duration::from_secs(3),
                request_timeout: Duration::from_secs(3),
                shutdown_timeout: Duration::from_millis(50),
                ..Default::default()
            },
        )
    }
    fn identity(&self) -> Identity {
        Identity {
            snapshot: self.snapshot.clone(),
            key: FileKey {
                root: self.root.clone(),
                path: "entry.ts".into(),
            },
            editor: EntityId::from(1),
            selection: self.snapshot.text().len()..self.snapshot.text().len(),
            cursor: self.snapshot.text().len(),
            settings: serde_json::json!({}),
            view: 1,
        }
    }
}

#[test]
fn symbol_click_dispatch_collects_both_roles_at_the_same_clicked_dirty_position_once() {
    let f = Fixture::new();
    let mut nav = f.nav();
    let position = LspPosition {
        line: 2,
        character: 3,
    };
    let Reply::Symbol(result, _) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Symbol,
        position,
        &FormattingOptions::default(),
    )
    .unwrap() else {
        panic!("combined symbol reply");
    };
    assert_eq!(result.definitions.len(), 1);
    assert_eq!(result.unique_definition().unwrap().line, 1);
    assert_eq!(result.usages.len(), 1);
    assert_eq!(result.usages[0].line, 2);
    assert!(result.definitions[0].preview.contains("dirty"));
    assert!(result.usages[0].preview.contains("dirty"));
    nav.shutdown();
    let messages: Vec<serde_json::Value> = fs::read_to_string(f.root.join("protocol.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let navigation: Vec<_> = messages
        .iter()
        .filter(|m| {
            matches!(
                m["method"].as_str(),
                Some("textDocument/definition" | "textDocument/references")
            )
        })
        .collect();
    assert_eq!(navigation.len(), 2);
    assert_eq!(navigation[0]["method"], "textDocument/definition");
    assert_eq!(navigation[1]["method"], "textDocument/references");
    assert_eq!(
        navigation[0]["params"]["position"],
        serde_json::json!({"line":2,"character":3})
    );
    assert_eq!(
        navigation[0]["params"]["position"],
        navigation[1]["params"]["position"]
    );
    let opens: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 1);
    assert_eq!(
        opens[0]["params"]["textDocument"]["text"],
        f.snapshot.text()
    );
    assert_eq!(
        fs::read_to_string(f.root.join("entry.ts")).unwrap(),
        "// disk source\nconst disk = 0;\ndisk"
    );
}

#[test]
fn stale_clicked_source_does_not_start_a_server() {
    let f = Fixture::new();
    let mut nav = f.nav();
    f.snapshot.invalidate();
    assert!(
        execute(
            &mut nav,
            &f.snapshot,
            &[],
            Tool::Symbol,
            LspPosition {
                line: 2,
                character: 3
            },
            &FormattingOptions::default()
        )
        .is_err()
    );
    assert!(!f.root.join("protocol.jsonl").exists());
}

#[test]
fn first_symbol_gesture_resumes_once_after_approval_at_its_exact_dirty_position() {
    use super::model::PendingSymbolRequest;
    let f = Fixture::new();
    let mut identity = f.identity();
    identity.cursor -= 2;
    identity.selection = identity.cursor..identity.cursor;
    let requested = gpui_kit::component::input::Position::new(2, 3);
    let mut pending =
        PendingSymbolRequest::remember(&Tool::Symbol, identity.clone(), Some(requested));
    assert!(pending.is_some());
    let mut nav = f.nav();
    assert!(
        !f.root.join("protocol.jsonl").exists(),
        "prompting neither starts nor trusts a server"
    );
    // This is the approval handler's take-and-validate boundary, followed by
    // the same dispatcher it invokes. A consumed gesture cannot run twice.
    let resume = pending.take().unwrap().resume(Some(&identity)).unwrap();
    assert_eq!(resume, requested);
    assert!(pending.is_none());
    let Reply::Symbol(result, _) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Symbol,
        LspPosition {
            line: resume.line,
            character: resume.character,
        },
        &FormattingOptions::default(),
    )
    .unwrap() else {
        panic!("resumed symbol reply")
    };
    assert_eq!(result.definitions.len(), 1);
    assert_eq!(result.usages.len(), 1);
    nav.shutdown();
    let messages = fs::read_to_string(f.root.join("protocol.jsonl")).unwrap();
    assert_eq!(
        messages
            .lines()
            .filter(|line| line.contains("textDocument/definition"))
            .count(),
        1
    );
    assert_eq!(
        messages
            .lines()
            .filter(|line| line.contains("textDocument/references"))
            .count(),
        1
    );
}

#[test]
fn pending_symbol_approval_discards_changed_scope_source_position_and_non_gesture_actions() {
    use super::model::PendingSymbolRequest;
    let f = Fixture::new();
    let mut identity = f.identity();
    identity.cursor -= 2;
    identity.selection = identity.cursor..identity.cursor;
    let requested = gpui_kit::component::input::Position::new(2, 3);
    let pending = || {
        PendingSymbolRequest::remember(&Tool::Symbol, identity.clone(), Some(requested)).unwrap()
    };
    assert!(pending().resume(None).is_none()); // navigation/loading has no current target
    let mut changed = identity.clone();
    changed.key.root = f.root.join("another-worktree");
    assert!(pending().resume(Some(&changed)).is_none());
    changed = identity.clone();
    changed.key.path = "another.ts".into();
    assert!(pending().resume(Some(&changed)).is_none());
    changed = identity.clone();
    changed.cursor -= 1;
    changed.selection = changed.cursor..changed.cursor;
    assert!(pending().resume(Some(&changed)).is_none());
    changed = identity.clone();
    changed.view += 1;
    assert!(pending().resume(Some(&changed)).is_none());
    changed = identity.clone();
    changed.editor = EntityId::from(2);
    assert!(pending().resume(Some(&changed)).is_none());
    changed = identity.clone();
    changed.snapshot = DocumentSnapshot::new(
        f.root.clone(),
        "entry.ts".into(),
        "// replaced source\nconst dirty = 1;\ndirty".into(),
        2,
    )
    .unwrap();
    assert!(pending().resume(Some(&changed)).is_none());
    assert!(
        PendingSymbolRequest::remember(
            &Tool::Navigate(NavigationKind::Definition),
            identity.clone(),
            Some(requested)
        )
        .is_none()
    );
    assert!(
        PendingSymbolRequest::remember(
            &Tool::Navigate(NavigationKind::References),
            identity.clone(),
            Some(requested)
        )
        .is_none()
    );
    assert!(
        PendingSymbolRequest::remember(&Tool::Complete, identity.clone(), Some(requested))
            .is_none()
    );
    assert!(PendingSymbolRequest::remember(&Tool::Symbol, identity.clone(), None).is_none());
    assert!(
        PendingSymbolRequest::remember(
            &Tool::Symbol,
            identity.clone(),
            Some(gpui_kit::component::input::Position::new(2, 4))
        )
        .is_none()
    );
    let invalidated = pending();
    identity.snapshot.invalidate();
    assert!(invalidated.resume(Some(&identity)).is_none());
    assert!(
        !f.root.join("protocol.jsonl").exists(),
        "discarding pending gestures never starts a process"
    );
}
#[test]
fn native_command_dispatch_uses_dirty_source_for_every_tool_and_never_saves() {
    let f = Fixture::new();
    let mut nav = f.nav();
    let id = f.identity();
    let position = LspPosition {
        line: 2,
        character: 5,
    };
    let options = FormattingOptions::default();
    assert!(!f.root.join("protocol.jsonl").exists());
    let Reply::Navigation(result, _) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Navigate(NavigationKind::Definition),
        position,
        &options,
    )
    .unwrap() else {
        panic!()
    };
    assert!(result.semantic);
    assert!(result.locations[0].preview.contains("dirty"));
    let Reply::Complete(result) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Complete,
        position,
        &options,
    )
    .unwrap() else {
        panic!()
    };
    let item = nav
        .resolve_completion_buffer(&f.snapshot, &result.items[0])
        .unwrap()
        .value;
    assert!(
        item.documentation
            .as_ref()
            .unwrap()
            .value
            .contains("documentation")
    );
    let (replacement, caret) = completion_edit(&id, &item).unwrap();
    assert_eq!(
        replacement,
        "// 😀\n// import\nconst dirty = 1;\ndirtyMethod(count)"
    );
    assert_eq!(caret, replacement.len());
    let mirror = &result.items[1];
    let (plain, _) = completion_edit(&id, mirror).unwrap();
    assert!(plain.ends_with("<one>inside</one>"));
    assert!(completion_edit(&id, &result.items[2]).is_err());
    let Reply::Diagnostics(items) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Diagnostics,
        position,
        &options,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(items[0].severity, Some(2));
    assert_eq!(items[0].range.start.character, 6);
    let Reply::Symbols(items) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Symbols,
        position,
        &options,
    )
    .unwrap() else {
        panic!()
    };
    let rows = symbol_rows(&items, "dirty", &HashSet::new());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].position.line, 2);
    assert_eq!(rows[1].depth, 1);
    let Reply::Signature(Some(help)) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Signature,
        position,
        &options,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(help.active_parameter, Some(1));
    assert_eq!(
        parameter_text(
            &help.signatures[0].label,
            &help.signatures[0].parameters[1].label
        )
        .unwrap(),
        "count: number"
    );
    let Reply::Hover(Some(hover)) =
        execute(&mut nav, &f.snapshot, &[], Tool::Hover, position, &options).unwrap()
    else {
        panic!()
    };
    assert!(hover.contents[0].value.contains("number"));
    let Reply::Format(edits) = execute(
        &mut nav,
        &f.snapshot,
        &[],
        Tool::Format {
            save_after: false,
            commit: false,
        },
        position,
        &options,
    )
    .unwrap() else {
        panic!()
    };
    assert!(
        TextEdit::apply_all(f.snapshot.text(), &edits)
            .unwrap()
            .contains("formatted")
    );
    assert!(
        fs::read_to_string(f.root.join("entry.ts"))
            .unwrap()
            .contains("const disk")
    );
    let messages = fs::read_to_string(f.root.join("protocol.jsonl")).unwrap();
    let did_open: serde_json::Value = messages
        .lines()
        .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
        .find(|v| v["method"] == "textDocument/didOpen")
        .unwrap();
    assert_eq!(
        did_open["params"]["textDocument"]["text"],
        f.snapshot.text()
    );
    nav.shutdown();
}
#[test]
fn responses_reject_worktree_file_editor_version_settings_view_and_selection_changes() {
    let f = Fixture::new();
    let expected = f.identity();
    let mut current = expected.clone();
    assert!(expected.matches(&current, true));
    current.cursor -= 1;
    assert!(!expected.matches(&current, true));
    assert!(expected.matches(&current, false));
    current = expected.clone();
    current.selection = 0..1;
    assert!(!expected.matches(&current, true));
    current = expected.clone();
    current.key.root = f.root.join("other");
    assert!(!expected.matches(&current, true));
    current = expected.clone();
    current.key.path = "other.ts".into();
    assert!(!expected.matches(&current, true));
    current = expected.clone();
    current.editor = EntityId::from(2);
    assert!(!expected.matches(&current, true));
    current = expected.clone();
    current.view += 1;
    assert!(!expected.matches(&current, true));
    current = expected.clone();
    current.settings = serde_json::json!({"language.unsaved":false});
    assert!(!expected.matches(&current, true));
    current = expected.clone();
    current.snapshot = DocumentSnapshot::new(
        f.root.clone(),
        "entry.ts".into(),
        f.snapshot.text().into(),
        2,
    )
    .unwrap();
    assert!(!expected.matches(&current, true));
    f.snapshot.invalidate();
    assert!(!expected.matches(&expected, true));
    let mut nav = f.nav();
    assert!(
        execute(
            &mut nav,
            &f.snapshot,
            &[],
            Tool::Complete,
            LspPosition::default(),
            &FormattingOptions::default()
        )
        .is_err()
    );
    assert!(!f.root.join("protocol.jsonl").exists());
}
#[test]
fn symbol_tree_search_keeps_parents_and_collapsed_branches_only_hide_when_unfiltered() {
    let range = TextRange {
        start: LspPosition::default(),
        end: LspPosition {
            line: 1,
            character: 0,
        },
    };
    let child = DocumentSymbol {
        name: "method".into(),
        detail: Some("returns boolean".into()),
        kind: 6,
        range,
        selection_range: range,
        children: vec![],
    };
    let parent = DocumentSymbol {
        name: "Class".into(),
        detail: None,
        kind: 5,
        range,
        selection_range: range,
        children: vec![child],
    };
    let collapsed = HashSet::from([0]);
    assert_eq!(
        symbol_rows(std::slice::from_ref(&parent), "", &collapsed).len(),
        1
    );
    assert_eq!(
        symbol_rows(std::slice::from_ref(&parent), "BOOLEAN", &collapsed).len(),
        2
    );
    assert!(symbol_rows(&[parent], "no match", &collapsed).is_empty());
}
#[test]
fn utf16_parameter_labels_preserve_astral_text_boundaries() {
    assert_eq!(
        parameter_text(
            "f(😀: str, count: num)",
            &pokarak_navigation::ParameterLabel::Offsets { start: 11, end: 21 }
        )
        .unwrap(),
        "count: num"
    );
    assert!(
        parameter_text(
            "f(😀)",
            &pokarak_navigation::ParameterLabel::Offsets { start: 3, end: 4 }
        )
        .is_none()
    );
}
#[test]
fn custom_server_argv_is_preserved_and_configuration_errors_are_actionable() {
    let value = serde_json::json!({"rust":{"path":"/tmp/a path/server","args":["--literal","$(touch should-not-exist)"]},"python":{"path":"pyright","args":[]}});
    let parsed = configs(Some(&value)).unwrap();
    let rust = parsed.iter().find(|c| c.language == "rust").unwrap();
    assert_eq!(rust.program, "/tmp/a path/server");
    assert_eq!(rust.args[1], "$(touch should-not-exist)");
    assert!(
        configs(Some(
            &serde_json::json!({"rust":{"path":"server","args":[9]}})
        ))
        .is_err()
    );
    assert!(configs(Some(&serde_json::json!({"rust":{"args":[]}}))).is_err());
}

#[test]
fn custom_language_families_replace_lower_exact_defaults_but_keep_same_layer_exact_choices() {
    let config = |language: &str, program: &str| pokarak_navigation::ServerConfig {
        language: language.into(),
        program: program.into(),
        args: vec![],
    };
    let mut all = vec![
        config("typescript", "managed-ts"),
        config("javascriptreact", "managed-jsx"),
        config("typescriptreact", "managed-tsx"),
        config("java", "managed-java"),
    ];
    super::model::overlay_configs(
        &mut all,
        vec![config("ts", "custom-family"), config("tsx", "custom-exact")],
    );
    assert_eq!(all.len(), 3);
    assert!(
        all.iter()
            .all(|c| !c.program.starts_with("managed-") || c.language == "java")
    );
    assert!(
        all.iter()
            .any(|c| c.language == "typescriptreact" && c.program == "custom-exact")
    );
    assert!(
        all.iter()
            .any(|c| c.language == "typescript" && c.program == "custom-family")
    );
    super::model::overlay_configs(&mut all, vec![config("kt", "kotlin-custom")]);
    assert!(all.iter().any(|c| c.language == "kotlin"));
}
