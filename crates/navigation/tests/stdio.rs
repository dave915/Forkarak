#![cfg(feature = "test-server")]
use pokarak_navigation::{
    NavigationKind::*, Navigator, NavigatorOptions, ServerConfig, default_configs, path_to_uri,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work tree #한😀");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);
        fs::write(
            root.join("entry.ts"),
            "// 😀한\r\nconst name = 1;\r\nname;\r\n",
        )
        .unwrap();
        fs::write(root.join("target.ts"), "// target\n  const result = 1;\n").unwrap();
        Self {
            _temp: temp,
            root: root.canonicalize().unwrap(),
        }
    }
    fn config(&self, mode: &str) -> ServerConfig {
        ServerConfig {
            language: "typescript".into(),
            program: env!("CARGO_BIN_EXE_pokarak-mock-lsp").into(),
            args: vec![mode.into()],
        }
    }
    fn options(&self) -> NavigatorOptions {
        NavigatorOptions {
            config_file: None,
            initialize_timeout: Duration::from_millis(700),
            request_timeout: Duration::from_millis(700),
            shutdown_timeout: Duration::from_millis(100),
            ..Default::default()
        }
    }
    fn navigator(&self, mode: &str) -> Navigator {
        Navigator::with_options(self.root.clone(), vec![self.config(mode)], self.options())
    }
    fn log(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("protocol.jsonl"))
            .unwrap()
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}

#[test]
fn actual_stdio_lifecycle_links_requests_and_disk_updates() {
    for mode in ["normal", "full"] {
        let f = Fixture::new();
        let mut nav = f.navigator(mode);
        assert!(!f.root.join("mock.pid").exists());
        let result = nav
            .navigate(Path::new("entry.ts"), 0, 5, Definition)
            .unwrap();
        assert!(result.semantic);
        assert_eq!(result.locations[0].path, f.root.join("target.ts"));
        assert_eq!(
            (result.locations[0].line, result.locations[0].column),
            (1, 2)
        );
        assert!(result.locations[0].preview.contains("result"));
        let result = nav
            .navigate(Path::new("entry.ts"), 1, 6, References)
            .unwrap();
        assert_eq!(result.locations.len(), 1);
        let result = nav
            .navigate(Path::new("entry.ts"), 1, 6, Implementation)
            .unwrap();
        assert_eq!(result.locations[0].line, 1); // Selection range, not target range.
        fs::write(f.root.join("entry.ts"), "// 한😀 updated\nname;\n").unwrap();
        nav.navigate(Path::new("entry.ts"), 1, 2, Definition)
            .unwrap();
        nav.shutdown();
        nav.shutdown();
        assert!(
            nav.navigate(Path::new("entry.ts"), 1, 2, Definition)
                .is_err()
        );
        let log = f.log();
        let methods: Vec<_> = log.iter().filter_map(|m| m["method"].as_str()).collect();
        assert_eq!(
            &methods[..3],
            &[
                "initialize",
                "initialized",
                "workspace/didChangeConfiguration"
            ]
        );
        assert_eq!(
            methods
                .iter()
                .filter(|m| **m == "textDocument/didOpen")
                .count(),
            1
        );
        assert_eq!(
            methods
                .iter()
                .filter(|m| **m == "textDocument/didChange")
                .count(),
            1
        );
        assert!(methods.ends_with(&["shutdown", "exit"]));
        let init = &log[0]["params"];
        assert_eq!(init["rootUri"], path_to_uri(&f.root).unwrap());
        assert_eq!(
            init["initializationOptions"]["disableAutomaticTypingAcquisition"],
            true
        );
        assert_eq!(
            log.iter().find(|m| m["id"] == "config").unwrap()["result"],
            serde_json::json!([false, null])
        );
        assert_eq!(
            log.iter().find(|m| m["id"] == "edit").unwrap()["result"]["applied"],
            false
        );
        assert_eq!(
            log.iter().find(|m| m["id"] == "unknown").unwrap()["error"]["code"],
            -32601
        );
        assert_eq!(
            log.iter()
                .find(|m| m["method"] == "textDocument/definition")
                .unwrap()["params"]["position"]["character"],
            5
        );
        assert_process_gone(&f.root.join("mock.pid"));
    }
}

#[test]
fn refreshes_other_open_documents_and_closes_deleted_files() {
    let f = Fixture::new();
    let mut nav = f.navigator("normal");
    nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    nav.navigate(Path::new("target.ts"), 1, 3, Definition)
        .unwrap();
    fs::write(f.root.join("target.ts"), "changed 😀\n").unwrap();
    nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    fs::remove_file(f.root.join("target.ts")).unwrap();
    nav.refresh().unwrap();
    nav.shutdown();
    let log = f.log();
    assert!(log.iter().any(|m| m["method"] == "textDocument/didChange"
        && m["params"]["textDocument"]["uri"] == path_to_uri(&f.root.join("target.ts")).unwrap()));
    assert!(log.iter().any(|m| m["method"] == "textDocument/didClose"));
}

#[test]
fn errors_timeouts_malformed_frames_and_exit_are_honest_and_bounded() {
    for (mode, expected) in [
        ("exit", "UnsupportedClassVersionError"),
        ("request-exit", "server request crashed"),
        ("init-hang", "timed out"),
        ("hang", "timed out"),
        ("malformed", "body length"),
        ("error", "deliberate semantic failure"),
        ("utf8", "unsupported position encoding"),
        ("unsupported", "does not support"),
        ("stale-disk", "changed on disk"),
    ] {
        let f = Fixture::new();
        let mut nav = f.navigator(mode);
        let start = Instant::now();
        let error = nav
            .navigate(Path::new("entry.ts"), 1, 6, Implementation)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{mode}: {error}");
        assert!(nav.status().contains(expected), "{mode}");
        assert!(start.elapsed() < Duration::from_secs(4), "{mode}");
        assert_process_gone(&f.root.join("mock.pid"));
    }
}

#[test]
fn missing_server_invalid_position_and_config_do_not_fake_success() {
    let f = Fixture::new();
    let mut config = f.config("normal");
    config.program = "/no/such/lsp".into();
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], f.options());
    assert!(
        nav.navigate(Path::new("entry.ts"), 1, 0, Definition)
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    let mut nav = f.navigator("normal");
    assert!(
        nav.navigate(Path::new("entry.ts"), 0, 4, Definition)
            .unwrap_err()
            .to_string()
            .contains("surrogate")
    );
    assert!(!f.root.join("mock.pid").exists());
    let path = f.root.join("servers.json");
    fs::write(&path, "bad json").unwrap();
    let mut options = f.options();
    options.config_file = Some(path);
    let mut nav = Navigator::with_options(f.root.clone(), default_configs(), options);
    assert!(
        nav.navigate(Path::new("entry.ts"), 1, 0, Definition)
            .unwrap_err()
            .to_string()
            .contains("Invalid language-server config")
    );
}

#[test]
fn user_overrides_family_and_all_language_ids() {
    let f = Fixture::new();
    let config_path = f.root.join("servers.json");
    fs::write(
        &config_path,
        serde_json::json!({"servers": [f.config("normal")]}).to_string(),
    )
    .unwrap();
    let mut options = f.options();
    options.config_file = Some(config_path);
    let mut nav = Navigator::with_options(f.root.clone(), default_configs(), options);
    for extension in ["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"] {
        let filename = format!("view.{extension}");
        fs::write(f.root.join(&filename), "name\n").unwrap();
        nav.navigate(Path::new(&filename), 0, 1, Definition)
            .unwrap();
    }
    nav.shutdown();
    let log = f.log();
    assert_eq!(
        log.iter().filter(|m| m["method"] == "initialize").count(),
        1
    );
    let ids: Vec<_> = log
        .iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
        .map(|m| m["params"]["textDocument"]["languageId"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "typescript",
            "typescriptreact",
            "javascript",
            "javascriptreact",
            "typescript",
            "typescript",
            "javascript",
            "javascript"
        ]
    );
}

#[test]
fn external_dependencies_allowed_but_sibling_worktree_is_blocked() {
    let f = Fixture::new();
    let external = tempfile::tempdir().unwrap();
    let source = external.path().join("dependency #한.ts");
    fs::write(&source, "// dependency\n  name\n").unwrap();
    let mut config = f.config("normal");
    config.args.push(path_to_uri(&source).unwrap());
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], f.options());
    assert!(nav.navigate(&source, 1, 2, Definition).is_err());
    let target = nav
        .navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap()
        .locations
        .remove(0);
    assert_eq!(target.path, source.canonicalize().unwrap());
    nav.navigate(&target.path, 1, 2, Definition).unwrap();
    nav.shutdown();
    git(
        &f.root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    );
    let sibling = f._temp.path().join("sibling");
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "sibling",
            "--",
            sibling.to_str().unwrap(),
        ],
    );
    fs::write(sibling.join("target.ts"), "other checkout\nname\n").unwrap();
    let mut config = f.config("normal");
    config
        .args
        .push(path_to_uri(&sibling.join("target.ts")).unwrap());
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], f.options());
    let result = nav
        .navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    assert!(result.locations.is_empty());
    assert!(result.status.contains("another worktree"));
    assert!(
        nav.navigate(&sibling.join("target.ts"), 1, 2, Definition)
            .unwrap_err()
            .to_string()
            .contains("another worktree")
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(sibling.join("target.ts"), f.root.join("alias.ts")).unwrap();
        assert!(
            nav.navigate(Path::new("alias.ts"), 1, 2, Definition)
                .unwrap_err()
                .to_string()
                .contains("another worktree")
        );
    }
}

#[test]
fn independent_roots_and_namespace_directories() {
    let a = Fixture::new();
    let b = Fixture::new();
    let mut config = a.config("normal");
    config.args.push("{workspace}".into());
    let mut first = Navigator::with_options(a.root.clone(), vec![config.clone()], a.options());
    let mut second = Navigator::with_options(b.root.clone(), vec![config], b.options());
    assert_eq!(
        first
            .navigate(Path::new("entry.ts"), 1, 6, Definition)
            .unwrap()
            .locations[0]
            .path,
        a.root.join("target.ts")
    );
    assert_eq!(
        second
            .navigate(Path::new("entry.ts"), 1, 6, Definition)
            .unwrap()
            .locations[0]
            .path,
        b.root.join("target.ts")
    );
    let a_log = a.log();
    let b_log = b.log();
    let a_dir = a_log[0]["params"]["initializationOptions"]["tsserver"]["logDirectory"]
        .as_str()
        .unwrap();
    let b_dir = b_log[0]["params"]["initializationOptions"]["tsserver"]["logDirectory"]
        .as_str()
        .unwrap();
    assert_ne!(a_dir, b_dir);
    assert!(!Path::new(a_dir).starts_with(&a.root));
    first.shutdown();
    second.shutdown();
    assert!(!Path::new(a_dir).parent().unwrap().exists());
}

#[test]
fn null_and_unreadable_uri_are_reported_without_invented_locations() {
    let f = Fixture::new();
    let mut nav = f.navigator("null");
    let result = nav
        .navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    assert!(result.semantic && result.locations.is_empty());
    nav.shutdown();
    let mut config = f.config("normal");
    config.args.push("jdt://contents/Foo.class".into());
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], f.options());
    let result = nav
        .navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    assert!(result.locations.is_empty() && result.status.contains("Unsupported source URI"));
}

#[test]
fn shutdown_timeout_and_descendant_cleanup() {
    for mode in ["shutdown-hang", "descendants"] {
        let f = Fixture::new();
        let mut nav = f.navigator(mode);
        nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
            .unwrap();
        let start = Instant::now();
        drop(nav);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_process_gone(&f.root.join("mock.pid"));
        if mode == "descendants" {
            assert_process_gone(&f.root.join("descendant.pid"));
        }
    }
}

#[test]
fn java_runtime_errors_and_kotlin_import_gate_are_readable() {
    let f = Fixture::new();
    fs::write(f.root.join("Main.java"), "class Main {}\n").unwrap();
    let mut config = f.config("exit");
    config.language = "java".into();
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], f.options());
    let error = nav
        .navigate(Path::new("Main.java"), 0, 6, Definition)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Java 17") && error.contains("JDK runtime"));
    fs::write(f.root.join("Main.kt"), "class Main\n").unwrap();
    let mut config = f.config("normal");
    config.language = "kotlin".into();
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], f.options());
    assert!(
        nav.navigate(Path::new("Main.kt"), 0, 6, Definition)
            .unwrap_err()
            .to_string()
            .contains("allowProjectImports")
    );
}

fn assert_process_gone(path: &Path) {
    let pid: i32 = fs::read_to_string(path).unwrap().parse().unwrap();
    #[cfg(unix)]
    {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // SAFETY: signal 0 only checks process existence.
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            assert!(Instant::now() < deadline, "process {pid} survived shutdown");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn server_which_stops_reading_stdin_cannot_block_a_large_did_open() {
    let f = Fixture::new();
    fs::write(f.root.join("entry.ts"), "x".repeat(2 * 1024 * 1024)).unwrap();
    let mut nav = f.navigator("no-read-after-init");
    let start = Instant::now();
    let error = nav
        .navigate(Path::new("entry.ts"), 0, 1, Definition)
        .unwrap_err()
        .to_string();
    assert!(error.contains("write timed out"), "{error}");
    assert!(start.elapsed() < Duration::from_secs(4));
    assert_process_gone(&f.root.join("mock.pid"));
}

#[test]
fn stderr_flood_is_drained_and_retains_only_a_bounded_tail() {
    let f = Fixture::new();
    let mut nav = f.navigator("stderr-flood");
    nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    let stderr = nav.stderr();
    assert!(stderr.contains("stderr tail marker"));
    assert!(stderr.len() <= 16 * 1024);
}

#[test]
fn nested_and_symlinked_roots_resolve_to_the_correct_worktree() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("nested")).unwrap();
    let nav = Navigator::with_options(f.root.join("nested"), vec![], f.options());
    assert_eq!(nav.root(), f.root);
    #[cfg(unix)]
    {
        let alias = f._temp.path().join("alias");
        std::os::unix::fs::symlink(&f.root, &alias).unwrap();
        let nav = Navigator::with_options(alias, vec![], f.options());
        assert_eq!(nav.root(), f.root);
    }
}

#[cfg(unix)]
#[test]
fn an_open_file_replaced_by_a_symlink_invalidates_its_session() {
    let f = Fixture::new();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("foreign.ts"), "const other = 2;\n").unwrap();
    let mut nav = f.navigator("normal");
    nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    fs::remove_file(f.root.join("entry.ts")).unwrap();
    std::os::unix::fs::symlink(external.path().join("foreign.ts"), f.root.join("entry.ts"))
        .unwrap();
    let error = nav.refresh().unwrap_err().to_string();
    assert!(error.contains("filesystem target"), "{error}");
    assert_process_gone(&f.root.join("mock.pid"));
}

#[cfg(unix)]
#[test]
fn official_jvm_shares_one_session_and_reports_failed_imports() {
    for mode in ["import-ready", "import-failed"] {
        let f = Fixture::new();
        let launcher = f.root.join("intellij-server");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pokarak-mock-lsp"), &launcher).unwrap();
        let java = ServerConfig {
            language: "java".into(),
            program: "/usr/bin/env".into(),
            args: vec![
                "POKARAK_FIXTURE=1".into(),
                launcher.to_string_lossy().into_owned(),
                mode.into(),
            ],
        };
        let mut kotlin = java.clone();
        kotlin.language = "kotlin".into();
        fs::write(f.root.join("Main.java"), "class Main {}\n").unwrap();
        fs::write(f.root.join("Main.kt"), "class Main\n").unwrap();
        let mut options = f.options();
        options.allow_project_imports = true;
        let mut nav = Navigator::with_options(f.root.clone(), vec![java, kotlin], options);
        let result = nav.navigate(Path::new("Main.java"), 0, 6, Definition);
        if mode == "import-failed" {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("Project import failed")
            );
        } else {
            result.unwrap();
            nav.navigate(Path::new("Main.kt"), 0, 6, References)
                .unwrap();
        }
        nav.shutdown();
        let log = f.log();
        assert_eq!(
            log.iter()
                .filter(|entry| entry["method"] == "initialize")
                .count(),
            1
        );
        assert!(
            log[0]["params"]["initializationOptions"]["indexDir"]
                .as_str()
                .is_some()
        );
        if mode == "import-failed" {
            assert!(
                !log.iter()
                    .any(|entry| entry["method"] == "textDocument/definition")
            );
        }
    }
}

#[test]
fn installed_manifest_discovery_is_read_only_and_rejects_invalid_executables() {
    let f = Fixture::new();
    let path = f.root.join("installed.json");
    let config = f.config("normal");
    fs::write(
        &path,
        serde_json::json!({"schemaVersion":1,"servers":[config]}).to_string(),
    )
    .unwrap();
    let configs = pokarak_navigation::installed_configs(&f.root).unwrap();
    assert_eq!(configs.len(), 1);
    assert!(!f.root.join("mock.pid").exists());
    fs::write(&path,serde_json::json!({"schemaVersion":1,"servers":[{"language":"java","program":"relative/java","args":[]}]}).to_string()).unwrap();
    assert!(pokarak_navigation::installed_configs(&f.root).is_err());
}
