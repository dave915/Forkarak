use super::model::{Identity, completion_edit, configs, parameter_text, snippet, symbol_rows};
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
fn snippet_and_utf16_parameter_handling_never_insert_protocol_syntax() {
    assert_eq!(snippet("call(${1:value}, $2)$0").unwrap(), "call(value, )");
    assert_eq!(snippet(r"\$literal ${1:a\}b}").unwrap(), "$literal a}b");
    for unsupported in ["${VAR}", "${1|a,b|}", "${1:call(${2:x})}", "${1/"] {
        assert!(snippet(unsupported).is_err(), "{unsupported}");
    }
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
