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
#[ignore = "requires FORKARAK_BUNDLED_RESOURCES; real bundled JDT, authored Eclipse fixture, no Maven/Gradle import"]
fn real_bundled_java_symbol_click_collects_definitions_and_usages() {
    let resources = PathBuf::from(
        std::env::var_os("FORKARAK_BUNDLED_RESOURCES").expect("set FORKARAK_BUNDLED_RESOURCES"),
    )
    .canonicalize()
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("symbol fixture 한 😀");
    fs::create_dir_all(root.join("src/demo")).unwrap();
    let root = root.canonicalize().unwrap();
    assert!(
        Command::new("/usr/bin/git")
            .args(["init", "-q", "--"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    fs::write(root.join(".project"), r#"<projectDescription><name>symbol-click</name><projects/><buildSpec><buildCommand><name>org.eclipse.jdt.core.javabuilder</name><arguments/></buildCommand></buildSpec><natures><nature>org.eclipse.jdt.core.javanature</nature></natures></projectDescription>"#).unwrap();
    fs::write(root.join(".classpath"), r#"<classpath><classpathentry kind="src" path="src"/><classpathentry kind="con" path="org.eclipse.jdt.launching.JRE_CONTAINER"/><classpathentry kind="output" path="bin"/></classpath>"#).unwrap();
    let greeter = "package demo;\npublic class Greeter {\n    public String greet(String name) { return name; }\n}\n";
    let disk = "package demo;\nclass Caller { String diskOnly; }\n";
    let draft = "package demo;\nclass Caller {\n    String first() { return new Greeter().greet(\"first\"); }\n    String second() { return new Greeter().greet(\"second\"); }\n    String textOnly = \"greet\";\n}\n";
    fs::write(root.join("src/demo/Greeter.java"), greeter).unwrap();
    fs::write(root.join("src/demo/Caller.java"), disk).unwrap();
    let state = root.parent().unwrap().join("managed-state");
    let tools = ManagedTools::new(state.clone(), Some(resources)).unwrap();
    let java = tools
        .status("java")
        .unwrap()
        .installed
        .expect("bundled Java");
    assert_eq!(
        java.distribution,
        pokarak_navigation::ToolDistribution::Bundled
    );
    let mut nav = Navigator::with_options(
        root.clone(),
        java.servers,
        NavigatorOptions {
            config_file: None,
            allow_project_imports: false,
            initialize_timeout: Duration::from_secs(90),
            request_timeout: Duration::from_secs(45),
            shutdown_timeout: Duration::from_secs(3),
        },
    );
    let snapshot =
        DocumentSnapshot::new(root.clone(), "src/demo/Caller.java".into(), draft.into(), 9)
            .unwrap();
    let position = Position {
        line: 2,
        character: 43,
    }; // inside the clicked greet, not the old primary cursor
    let Reply::Symbol(result, _) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Symbol,
        position,
        &FormattingOptions::default(),
    )
    .unwrap() else {
        panic!("symbol reply")
    };
    eprintln!(
        "BUNDLED JAVA SYMBOL CLICK: {}\ndefinitions={:?}\nusages={:?}",
        result.status, result.definitions, result.usages
    );
    assert_eq!(
        result.definitions.len(),
        1,
        "{}\n{}\n{}",
        result.status,
        nav.server_messages(),
        nav.stderr()
    );
    let definition = result.unique_definition().unwrap();
    assert_eq!(definition.path, root.join("src/demo/Greeter.java"));
    assert_eq!((definition.line, definition.column), (2, 18));
    assert_eq!(
        result
            .usages
            .iter()
            .map(|h| (&h.path, h.line))
            .collect::<Vec<_>>(),
        vec![
            (&snapshot.path().to_owned(), 2),
            (&snapshot.path().to_owned(), 3)
        ]
    );
    assert!(
        result
            .usages
            .iter()
            .all(|h| h.preview.contains("Greeter().greet"))
    );
    assert!(!result.usages.iter().any(|h| h.line == 4));
    // Explicit Definition and References still use their original single-tool dispatch.
    let Reply::Navigation(explicit, _) = execute(
        &mut nav,
        &snapshot,
        &[],
        Tool::Navigate(NavigationKind::References),
        position,
        &FormattingOptions::default(),
    )
    .unwrap() else {
        panic!("explicit references")
    };
    assert!(explicit.semantic);
    assert!(
        explicit
            .locations
            .iter()
            .any(|h| h.path == definition.path && h.line == 2)
    );
    assert!(
        explicit
            .locations
            .iter()
            .any(|h| h.path == snapshot.path() && h.line == 3)
    );
    snapshot.invalidate();
    assert!(
        execute(
            &mut nav,
            &snapshot,
            &[],
            Tool::Symbol,
            position,
            &FormattingOptions::default()
        )
        .is_err()
    );
    nav.shutdown();
    assert_eq!(
        fs::read_to_string(root.join("src/demo/Caller.java")).unwrap(),
        disk
    );
    assert_eq!(
        fs::read_to_string(root.join("src/demo/Greeter.java")).unwrap(),
        greeter
    );
    assert!(!root.join("pom.xml").exists());
    assert!(!root.join("build.gradle").exists());
    assert!(!state.exists());
}
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
