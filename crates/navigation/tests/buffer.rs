#![cfg(feature = "test-server")]
use pokarak_navigation::{
    DocumentSnapshot, FormatterConfig, FormattingOptions, NavigationKind, Navigator,
    NavigatorOptions, Position, ServerConfig, TextEdit, TextRange,
};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
const SOURCE: &str = "// 😀한\r\nconst dirty = 1;\r\ndirty;\r\n";
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root 한 #😀");
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "--"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("entry.ts"), "// disk\nconst disk = 0;\n").unwrap();
        fs::write(root.join("target.ts"), "// target\n  const disk = 1;\n").unwrap();
        Self {
            _temp: temp,
            root: root.canonicalize().unwrap(),
        }
    }
    fn snapshot(&self, version: i64) -> DocumentSnapshot {
        DocumentSnapshot::new(self.root.clone(), "entry.ts".into(), SOURCE.into(), version).unwrap()
    }
    fn nav(&self, mode: &str) -> Navigator {
        Navigator::with_options(
            self.root.clone(),
            vec![ServerConfig {
                language: "ts".into(),
                program: env!("CARGO_BIN_EXE_pokarak-mock-lsp").into(),
                args: vec![mode.into()],
            }],
            NavigatorOptions {
                config_file: None,
                initialize_timeout: Duration::from_secs(2),
                request_timeout: Duration::from_millis(800),
                shutdown_timeout: Duration::from_millis(30),
                ..Default::default()
            },
        )
    }
    fn log(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("protocol.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}
#[test]
fn dirty_navigation_completion_diagnostics_format_symbols_use_exact_snapshot() {
    for mode in ["buffer", "full"] {
        let f = Fixture::new();
        let mut nav = f.nav(mode);
        let snapshot = f.snapshot(7);
        assert!(!f.root.join("mock.pid").exists());
        let result = nav
            .navigate_buffer(&snapshot, 0, 5, NavigationKind::References)
            .unwrap();
        assert!(result.value.semantic);
        assert_eq!(result.snapshot.version(), 7);
        if mode == "buffer" {
            assert!(result.value.locations[0].preview.contains("dirty"));
        }
        let completions = nav.complete_buffer(&snapshot, 0, 5).unwrap();
        assert!(completions.value.is_incomplete);
        assert_eq!(completions.value.items[0].label, "dirty");
        assert_eq!(completions.value.items[1].insert_text_format, 2);
        assert!(
            TextEdit::apply_all(
                SOURCE,
                &[completions.value.items[0].text_edit.clone().unwrap()]
            )
            .unwrap()
            .contains("🦀한")
        );
        assert_eq!(nav.diagnostics(&snapshot).unwrap().value[0].message, SOURCE);
        let edits = nav
            .format_buffer(&snapshot, &FormattingOptions::default())
            .unwrap();
        assert!(
            TextEdit::apply_all(SOURCE, &edits.value)
                .unwrap()
                .contains("🦀한")
        );
        let symbols = nav.document_symbols(&snapshot).unwrap();
        assert_eq!(symbols.value[0].children[0].name, "dirty");
        fs::write(
            f.root.join("entry.ts"),
            "disk changed during editor session",
        )
        .unwrap();
        nav.refresh().unwrap();
        nav.complete_buffer(&snapshot, 0, 5).unwrap();
        let changed = DocumentSnapshot::new(
            f.root.clone(),
            "entry.ts".into(),
            SOURCE.replace("dirty", "dirty2"),
            23,
        )
        .unwrap();
        nav.navigate_buffer(&changed, 0, 5, NavigationKind::Definition)
            .unwrap();
        assert!(!snapshot.is_current());
        assert!(nav.complete_buffer(&snapshot, 0, 5).is_err());
        assert!(
            nav.navigate(
                std::path::Path::new("entry.ts"),
                0,
                0,
                NavigationKind::Definition
            )
            .is_err()
        );
        nav.release_buffer(std::path::Path::new("entry.ts"))
            .unwrap();
        nav.navigate(
            std::path::Path::new("entry.ts"),
            0,
            0,
            NavigationKind::Definition,
        )
        .unwrap();
        nav.shutdown();
        let log = f.log();
        let open = log
            .iter()
            .find(|v| v["method"] == "textDocument/didOpen")
            .unwrap();
        assert_eq!(open["params"]["textDocument"]["text"], SOURCE);
        assert_eq!(open["params"]["textDocument"]["version"], 7);
        assert!(log.iter().any(|v| v["method"] == "textDocument/didChange"
            && v["params"]["textDocument"]["version"] == 23));
        assert_eq!(
            fs::read_to_string(f.root.join("entry.ts")).unwrap(),
            "disk changed during editor session"
        );
    }
}
#[test]
fn push_diagnostics_are_bound_to_the_exact_version() {
    let f = Fixture::new();
    let mut nav = f.nav("push");
    let snapshot = f.snapshot(2);
    assert_eq!(nav.diagnostics(&snapshot).unwrap().value[0].message, SOURCE);
    let next = DocumentSnapshot::new(
        f.root.clone(),
        "entry.ts".into(),
        SOURCE.replace("dirty", "dirty2"),
        9,
    )
    .unwrap();
    assert_eq!(
        nav.diagnostics(&next).unwrap().value[0].message,
        next.text()
    );
    let f = Fixture::new();
    let mut nav = f.nav("push-unversioned");
    assert!(
        nav.diagnostics(&f.snapshot(0))
            .unwrap_err()
            .to_string()
            .contains("unversioned")
    );
}
#[test]
fn roots_versions_utf16_and_import_permissions_are_checked_before_startup() {
    let f = Fixture::new();
    let other = Fixture::new();
    let mut nav = f.nav("buffer");
    assert!(nav.complete_buffer(&other.snapshot(1), 0, 5).is_err());
    assert!(nav.complete_buffer(&f.snapshot(1), 0, 4).is_err());
    let invalid = f.snapshot(1);
    invalid.invalidate();
    assert!(nav.document_symbols(&invalid).is_err());
    assert!(!f.root.join("mock.pid").exists());
    let mut second = other.nav("buffer");
    nav.navigate_buffer(&f.snapshot(5), 0, 5, NavigationKind::Definition)
        .unwrap();
    second
        .navigate_buffer(&other.snapshot(5), 0, 5, NavigationKind::Definition)
        .unwrap();
    let reused = DocumentSnapshot::new(
        f.root.clone(),
        "entry.ts".into(),
        SOURCE.replace("dirty", "other"),
        5,
    )
    .unwrap();
    assert!(
        nav.document_symbols(&reused)
            .unwrap_err()
            .to_string()
            .contains("version")
    );
    assert!(nav.document_symbols(&f.snapshot(4)).is_err());
    assert_ne!(
        f.log()[0]["params"]["rootUri"],
        other.log()[0]["params"]["rootUri"]
    );
    let f = Fixture::new();
    fs::write(f.root.join("entry.kt"), SOURCE).unwrap();
    let snapshot =
        DocumentSnapshot::new(f.root.clone(), "entry.kt".into(), SOURCE.into(), 1).unwrap();
    let mut nav = Navigator::with_options(
        f.root.clone(),
        vec![ServerConfig {
            language: "kotlin".into(),
            program: env!("CARGO_BIN_EXE_pokarak-mock-lsp").into(),
            args: vec!["buffer".into()],
        }],
        NavigatorOptions {
            config_file: None,
            ..Default::default()
        },
    );
    assert!(
        nav.document_symbols(&snapshot)
            .unwrap_err()
            .to_string()
            .contains("allowProjectImports")
    );
    assert!(!f.root.join("mock.pid").exists());
}
fn await_file(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn invalidation_and_cancellation_interrupt_outstanding_calls_including_initialize() {
    for mode in ["buffer-hang", "init-hang"] {
        let f = Fixture::new();
        let mut nav = f.nav(mode);
        let snapshot = f.snapshot(1);
        let invalidate = snapshot.clone();
        let worker = std::thread::spawn(move || nav.complete_buffer(&snapshot, 0, 5));
        await_file(&f.root.join(if mode == "init-hang" {
            "initialize.started"
        } else {
            "request.started"
        }));
        let start = Instant::now();
        invalidate.invalidate();
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("Stale")
        );
        assert!(start.elapsed() < Duration::from_millis(500));
    }
    let f = Fixture::new();
    let mut nav = f.nav("buffer-hang");
    let snapshot = f.snapshot(1);
    let cancel = nav.cancellation_handle();
    let worker = std::thread::spawn(move || nav.diagnostics(&snapshot));
    await_file(&f.root.join("request.started"));
    cancel.cancel().unwrap();
    assert!(worker.join().unwrap().is_err());
}
#[test]
fn failures_are_explicit_and_edits_symbols_are_validated() {
    for (mode, expected) in [
        ("editor-error", "deliberate editor failure"),
        ("unsupported-editor", "does not support"),
        ("buffer-hang", "timed out"),
    ] {
        let f = Fixture::new();
        let mut nav = f.nav(mode);
        assert!(
            nav.complete_buffer(&f.snapshot(1), 0, 5)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
    let f = Fixture::new();
    let mut nav = f.nav("overlap-edits");
    assert!(
        nav.format_buffer(&f.snapshot(1), &FormattingOptions::default())
            .unwrap_err()
            .to_string()
            .contains("Overlapping")
    );
    let f = Fixture::new();
    let mut nav = f.nav("flat-symbol");
    assert_eq!(
        nav.document_symbols(&f.snapshot(1)).unwrap().value[0].name,
        "dirty"
    );
    let edit = TextEdit {
        range: TextRange {
            start: Position {
                line: 0,
                character: 4,
            },
            end: Position {
                line: 0,
                character: 5,
            },
        },
        new_text: "a".into(),
    };
    assert!(TextEdit::apply_all(SOURCE, &[edit]).is_err());
}
#[test]
fn configured_stdin_formatter_uses_argv_and_never_saves() {
    for mode in ["formatter", "formatter-error", "formatter-hang"] {
        let f = Fixture::new();
        let mut nav = f.nav("buffer");
        let snapshot = f.snapshot(1);
        nav.set_formatter_configs(vec![FormatterConfig {
            language: "ts".into(),
            program: env!("CARGO_BIN_EXE_pokarak-mock-lsp").into(),
            args: vec![mode.into(), "--".into(), "{path}".into()],
        }])
        .unwrap();
        let result = nav.format_buffer(&snapshot, &FormattingOptions::default());
        match mode {
            "formatter" => assert!(
                TextEdit::apply_all(SOURCE, &result.unwrap().value)
                    .unwrap()
                    .contains("formatted")
            ),
            "formatter-error" => assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("deliberate formatter failure")
            ),
            _ => assert!(result.unwrap_err().to_string().contains("timed out")),
        }
        assert!(!f.root.join("mock.pid").exists());
        assert_eq!(
            fs::read_to_string(f.root.join("entry.ts")).unwrap(),
            "// disk\nconst disk = 0;\n"
        );
    }
}

#[test]
fn synchronous_typescript_diagnostics_are_whitelisted_and_versioned() {
    let f = Fixture::new();
    let mut nav = f.nav("ts-diagnostics");
    let first = f.snapshot(4);
    let diagnostics = nav.diagnostics(&first).unwrap();
    assert_eq!(diagnostics.value.len(), 2);
    assert_eq!(
        diagnostics.value[0].range.start,
        Position {
            line: 0,
            character: 3
        }
    );
    assert_eq!(diagnostics.value[1].severity, Some(1));
    let second = DocumentSnapshot::new(
        f.root.clone(),
        "entry.ts".into(),
        SOURCE.replace("dirty", "dirty2"),
        14,
    )
    .unwrap();
    assert!(nav.diagnostics(&second).unwrap().value.is_empty());
    nav.shutdown();
    let log = f.log();
    let commands: Vec<_> = log
        .iter()
        .filter(|v| v["method"] == "workspace/executeCommand")
        .collect();
    assert_eq!(commands.len(), 4);
    assert!(
        commands
            .iter()
            .all(|v| v["params"]["command"] == "typescript.tsserverRequest")
    );
    assert_eq!(
        log.iter().filter(|v| v["method"] == "initialize").count(),
        1
    );
    for mode in ["ts-diagnostics-error", "ts-diagnostics-hang"] {
        let f = Fixture::new();
        let mut nav = f.nav(mode);
        let snapshot = f.snapshot(1);
        if mode.ends_with("error") {
            assert!(
                nav.diagnostics(&snapshot)
                    .unwrap_err()
                    .to_string()
                    .contains("deliberate TS diagnostics failure")
            );
        } else {
            let invalidate = snapshot.clone();
            let worker = std::thread::spawn(move || nav.diagnostics(&snapshot));
            await_file(&f.root.join("request.started"));
            invalidate.invalidate();
            assert!(
                worker
                    .join()
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("Stale")
            );
        }
    }
}

#[test]
fn cancellation_and_buffer_changes_do_not_affect_another_root() {
    let first = Fixture::new();
    let second = Fixture::new();
    let mut a = first.nav("buffer-hang");
    let mut b = second.nav("buffer");
    let second_snapshot = second.snapshot(1);
    b.complete_buffer(&second_snapshot, 0, 5).unwrap();
    let cancel = a.cancellation_handle();
    let snapshot = first.snapshot(1);
    let worker = std::thread::spawn(move || a.complete_buffer(&snapshot, 0, 5));
    await_file(&first.root.join("request.started"));
    cancel.cancel().unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(b.complete_buffer(&second_snapshot, 0, 5).is_ok());
    assert!(second_snapshot.is_current());
}

#[test]
fn edit_application_handles_adjacent_ranges_multiline_utf16_and_final_newline() {
    let source = "😀한\r\nlast\r\n";
    let edits = [
        TextEdit {
            range: TextRange {
                start: Position {
                    line: 0,
                    character: 2,
                },
                end: Position {
                    line: 0,
                    character: 3,
                },
            },
            new_text: "글".into(),
        },
        TextEdit {
            range: TextRange {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 4,
                },
            },
            new_text: "끝😀".into(),
        },
        TextEdit {
            range: TextRange {
                start: Position {
                    line: 2,
                    character: 0,
                },
                end: Position {
                    line: 2,
                    character: 0,
                },
            },
            new_text: "tail".into(),
        },
    ];
    assert_eq!(
        TextEdit::apply_all(source, &edits).unwrap(),
        "😀글\r\n끝😀\r\ntail"
    );
    let edits = [TextEdit {
        range: TextRange {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 1,
                character: 0,
            },
        },
        new_text: String::new(),
    }];
    assert_eq!(TextEdit::apply_all(source, &edits).unwrap(), "last\r\n");
}

#[test]
fn retains_cross_file_drafts_lazily_and_rejects_invalidated_context() {
    let f = Fixture::new();
    let mut nav = f.nav("buffer-delayed");
    let first = f.snapshot(1);
    let dependency =
        DocumentSnapshot::new(f.root.clone(), "target.ts".into(), SOURCE.into(), 7).unwrap();
    nav.retain_buffer(&dependency).unwrap();
    assert!(!f.root.join("mock.pid").exists());
    let invalidation = dependency.clone();
    let worker = std::thread::spawn(move || nav.complete_buffer(&first, 0, 5));
    await_file(&f.root.join("request.started"));
    invalidation.invalidate();
    fs::write(f.root.join("respond.now"), "ready").unwrap();
    let result = worker.join().unwrap();
    assert!(result.unwrap_err().to_string().contains("Stale"));
    let log = f.log();
    assert!(
        log.iter().any(|v| v["method"] == "textDocument/didOpen"
            && v["params"]["textDocument"]["version"] == 7)
    );
}

#[test]
fn signature_hover_and_completion_documentation_use_snapshot_and_capabilities() {
    use pokarak_navigation::ParameterLabel;
    let f = Fixture::new();
    let mut nav = f.nav("buffer");
    let snapshot = f.snapshot(1);
    let signature = nav
        .signature_help_buffer(&snapshot, 0, 5)
        .unwrap()
        .value
        .unwrap();
    assert_eq!(signature.active_signature, Some(0));
    assert_eq!(
        signature.signatures[0].parameters[0].label,
        ParameterLabel::Offsets { start: 6, end: 19 }
    );
    assert_eq!(
        signature.signatures[0].documentation.as_ref().unwrap().kind,
        "markdown"
    );
    let hover = nav.hover_buffer(&snapshot, 0, 5).unwrap().value.unwrap();
    assert!(hover.contents[0].value.contains("const dirty: number"));
    assert_eq!(hover.range.unwrap().start.character, 3);
    let completion = nav.complete_buffer(&snapshot, 0, 5).unwrap();
    let item = &completion.value.items[0];
    assert_eq!(
        item.documentation.as_ref().unwrap().value,
        "candidate summary"
    );
    let resolved = nav.resolve_completion_buffer(&snapshot, item).unwrap();
    assert_eq!(
        resolved.value.documentation.as_ref().unwrap().value,
        "**dirty** documentation"
    );
    assert_eq!(
        resolved.value.detail.as_deref(),
        Some("const dirty: number")
    );
    let other = Fixture::new();
    let mut other_nav = other.nav("buffer");
    assert!(
        other_nav
            .resolve_completion_buffer(&other.snapshot(1), item)
            .unwrap_err()
            .to_string()
            .contains("mismatched")
    );
    assert!(!other.root.join("mock.pid").exists());
    snapshot.invalidate();
    assert!(nav.resolve_completion_buffer(&snapshot, item).is_err());
    for mode in ["unsupported-editor", "editor-null", "invalid-signature"] {
        let f = Fixture::new();
        let mut nav = f.nav(mode);
        let snapshot = f.snapshot(1);
        match mode {
            "editor-null" => {
                assert!(
                    nav.signature_help_buffer(&snapshot, 0, 5)
                        .unwrap()
                        .value
                        .is_none()
                );
                assert!(nav.hover_buffer(&snapshot, 0, 5).unwrap().value.is_none());
            }
            "invalid-signature" => assert!(
                nav.signature_help_buffer(&snapshot, 0, 5)
                    .unwrap_err()
                    .to_string()
                    .contains("UTF-16")
            ),
            _ => {
                assert!(
                    nav.signature_help_buffer(&snapshot, 0, 5)
                        .unwrap_err()
                        .to_string()
                        .contains("does not support")
                );
                assert!(nav.hover_buffer(&snapshot, 0, 5).is_err());
            }
        }
    }
}

#[path = "buffer/workspace_edits.rs"]
mod workspace_edits;
