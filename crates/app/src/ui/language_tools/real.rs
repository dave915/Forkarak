//! Opt-in frontend dispatch regression over explicitly installed real tools.
//! This proves the production request/edit adapter, not native AX interaction.
use super::{
    Reply, Tool, execute,
    model::{Identity, completion_edit, parameter_text},
};
use crate::ui::code_edit::FileKey;
use gpui_kit::EntityId;
use pokarak_navigation::{
    DocumentSnapshot, FormattingOptions, ManagedTools, NavigationKind, Navigator, NavigatorOptions,
    Position,
};
use std::{fs, path::PathBuf, process::Command, time::Duration};
#[test]
#[ignore = "requires explicitly installed managed TypeScript; does not install tools"]
fn real_managed_typescript_completion_insertion_and_dirty_intelligence() {
    let root =
        std::env::var_os("FORKARAK_MANAGED_TOOLS_DIR").expect("set FORKARAK_MANAGED_TOOLS_DIR");
    let tools = ManagedTools::new(PathBuf::from(root), None).unwrap();
    let installed = tools
        .status("typescript")
        .unwrap()
        .installed
        .expect("install managed TypeScript explicitly before this test");
    assert!(
        installed
            .servers
            .iter()
            .all(|s| std::path::Path::new(&s.program).is_absolute())
    );
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    assert!(
        Command::new("/usr/bin/git")
            .args(["init", "-q", "--"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"target":"ES2022","strict":true},"include":["entry.ts"]}"#,
    )
    .unwrap();
    let disk = "const disk = 0;\n";
    fs::write(root.join("entry.ts"), disk).unwrap();
    let source = "const dirty = 'hello';\nconst result = dirty.rep";
    let snapshot =
        DocumentSnapshot::new(root.clone(), "entry.ts".into(), source.into(), 1).unwrap();
    let mut nav = Navigator::with_options(
        root.clone(),
        installed.servers,
        NavigatorOptions {
            config_file: None,
            allow_project_imports: true,
            initialize_timeout: Duration::from_secs(45),
            request_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(2),
        },
    );
    let options = FormattingOptions::default();
    let position = Position {
        line: 1,
        character: 24,
    };
    let Reply::Complete(candidates) =
        execute(&mut nav, &snapshot, &[], Tool::Complete, position, &options).unwrap()
    else {
        panic!("completion dispatch");
    };
    let item = candidates
        .items
        .iter()
        .find(|i| i.label == "repeat")
        .expect("real repeat completion");
    let item = nav
        .resolve_completion_buffer(&snapshot, item)
        .unwrap()
        .value;
    assert!(item.detail.is_some());
    assert!(item.documentation.is_some());
    let identity = Identity {
        snapshot: snapshot.clone(),
        key: FileKey {
            root: root.clone(),
            path: "entry.ts".into(),
        },
        editor: EntityId::from(1),
        selection: source.len()..source.len(),
        cursor: source.len(),
        settings: serde_json::json!({}),
        view: 1,
    };
    let (inserted, caret) = completion_edit(&identity, &item).unwrap();
    assert!(inserted.ends_with("dirty.repeat"), "{inserted}");
    assert_eq!(caret, inserted.len());
    let Reply::Hover(Some(hover)) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Hover,
        Position {
            line: 0,
            character: 7,
        },
        &options,
    )
    .unwrap() else {
        panic!("hover");
    };
    assert!(hover.contents.iter().any(|m| m.value.contains("dirty")));
    let Reply::Symbols(symbols) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Symbols,
        Position::default(),
        &options,
    )
    .unwrap() else {
        panic!("symbols");
    };
    assert!(symbols.iter().any(|s| s.name == "dirty"));
    snapshot.invalidate();
    let source = "/** Greet a name repeatedly. */\nfunction greet(name: string, count: number) { return name.repeat(count); }\nconst output = greet('hello', 2);\nconst wrong: number = 'bad';\n";
    let snapshot =
        DocumentSnapshot::new(root.clone(), "entry.ts".into(), source.into(), 2).unwrap();
    let Reply::Navigation(result, _) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Navigate(NavigationKind::Definition),
        Position {
            line: 2,
            character: 17,
        },
        &options,
    )
    .unwrap() else {
        panic!("navigation");
    };
    assert!(
        result.semantic
            && result
                .locations
                .iter()
                .any(|h| h.line == 1 && h.preview.contains("greet"))
    );
    let Reply::Navigation(result, _) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Navigate(NavigationKind::References),
        Position {
            line: 1,
            character: 11,
        },
        &options,
    )
    .unwrap() else {
        panic!("references");
    };
    assert!(result.locations.iter().any(|h| h.line == 2));
    let Reply::Signature(Some(help)) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Signature,
        Position {
            line: 2,
            character: 30,
        },
        &options,
    )
    .unwrap() else {
        panic!("signature");
    };
    let signature = &help.signatures[help.active_signature.unwrap_or(0) as usize];
    let active = signature
        .active_parameter
        .or(help.active_parameter)
        .unwrap();
    assert_eq!(active, 1);
    assert!(
        parameter_text(
            &signature.label,
            &signature.parameters[active as usize].label
        )
        .unwrap()
        .contains("count")
    );
    let Reply::Diagnostics(diagnostics) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Diagnostics,
        Position::default(),
        &options,
    )
    .unwrap() else {
        panic!("diagnostics");
    };
    assert!(
        diagnostics
            .iter()
            .any(|d| d.range.start.line == 3 && d.severity == Some(1))
    );
    let Reply::Format(edits) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Format {
            save_after: false,
            commit: false,
        },
        Position::default(),
        &options,
    )
    .unwrap() else {
        panic!("format");
    };
    assert!(
        pokarak_navigation::TextEdit::apply_all(source, &edits)
            .unwrap()
            .contains("greet")
    );
    assert_eq!(fs::read_to_string(root.join("entry.ts")).unwrap(), disk);
    nav.shutdown();
}
