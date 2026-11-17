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
    assert!(error.contains("UnsupportedClassVersionError"));
    assert!(!error.contains("Java 17 may be too old"));
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
        fs::write(f.root.join("pom.xml"), "<project/>").unwrap();
        fs::write(f.root.join("Main.java"), "class Main {}\n").unwrap();
        fs::write(f.root.join("Main.kt"), "class Main\n").unwrap();
        let mut options = f.options();
        options.allow_project_imports = true;
        let mut nav = Navigator::with_options(f.root.clone(), vec![java, kotlin], options);
        let result = nav.navigate(Path::new("Main.java"), 0, 6, Definition);
        if mode == "import-failed" {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("Project import failed"));
            assert!(!error.contains("JDK runtime"));
            assert!(
                nav.server_messages()
                    .contains("Maven import failed deliberately")
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

struct CancelOnDrop(pokarak_navigation::CancellationHandle);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let _ = self.0.cancel();
    }
}

fn wait_for_marker(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "mock did not reach {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
#[test]
fn cancel_during_locked_navigation_kills_owned_group_and_leaves_other_roots_alive() {
    use std::sync::{Arc, Mutex, mpsc};
    fn send_sync_clone<T: Send + Sync + Clone>() {}
    send_sync_clone::<pokarak_navigation::CancellationHandle>();

    let f = Fixture::new();
    let other = Fixture::new();
    let mut survivor = other.navigator("normal");
    survivor
        .navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    let mut options = f.options();
    options.request_timeout = Duration::from_secs(30);
    options.shutdown_timeout = Duration::from_secs(30);
    let nav = Navigator::with_options(
        f.root.clone(),
        vec![f.config("cancel-descendants")],
        options,
    );
    let handle = nav.cancellation_handle();
    let _cleanup = CancelOnDrop(handle.clone());
    let shared = Arc::new(Mutex::new(nav));
    let worker_nav = Arc::clone(&shared);
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = worker_nav
            .lock()
            .unwrap()
            .navigate(Path::new("entry.ts"), 1, 6, Definition);
        finished_tx
            .send(result.map_err(|error| error.to_string()))
            .unwrap();
    });
    wait_for_marker(&f.root.join("request.started"));
    assert!(
        shared.try_lock().is_err(),
        "navigate must own the application mutex"
    );
    let start = Instant::now();
    handle.cancel().unwrap();
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "cancel waited on navigation or process exit"
    );
    assert!(handle.is_cancelled());
    let error = finished_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    worker.join().unwrap();
    assert_process_gone(&f.root.join("mock.pid"));
    assert_process_gone(&f.root.join("descendant.pid"));
    assert!(start.elapsed() < Duration::from_secs(3));
    {
        let mut nav = shared.lock().unwrap();
        assert!(nav.status().contains("cancelled"));
        assert!(
            nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(nav.refresh().is_err());
    }
    drop(shared);
    // Retained handles cannot touch a new session, even at the identical root.
    let mut replacement = f.navigator("normal");
    replacement
        .navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    let callbacks: Vec<_> = (0..4)
        .map(|_| {
            let handle = handle.clone();
            std::thread::spawn(move || handle.cancel().unwrap())
        })
        .collect();
    for callback in callbacks {
        callback.join().unwrap();
    }
    replacement
        .navigate(Path::new("entry.ts"), 1, 6, References)
        .unwrap();
    survivor
        .navigate(Path::new("entry.ts"), 1, 6, References)
        .unwrap();
}

#[test]
fn cancellation_interrupts_initialization_and_blocked_writes() {
    use std::sync::mpsc;
    for (mode, marker) in [
        ("init-hang", "initialize.started"),
        ("no-read-after-init", "stdin.blocked"),
    ] {
        let f = Fixture::new();
        fs::write(f.root.join("entry.ts"), "x".repeat(2 * 1024 * 1024)).unwrap();
        let mut options = f.options();
        options.initialize_timeout = Duration::from_secs(30);
        options.request_timeout = Duration::from_secs(30);
        let mut nav = Navigator::with_options(f.root.clone(), vec![f.config(mode)], options);
        let handle = nav.cancellation_handle();
        let _cleanup = CancelOnDrop(handle.clone());
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = nav.navigate(Path::new("entry.ts"), 0, 1, Definition);
            tx.send(result.map_err(|error| error.to_string())).unwrap();
        });
        wait_for_marker(&f.root.join(marker));
        let start = Instant::now();
        handle.cancel().unwrap();
        assert!(
            rx.recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err()
                .contains("cancelled")
        );
        worker.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_process_gone(&f.root.join("mock.pid"));
    }
}

#[cfg(unix)]
#[test]
fn exited_group_leader_is_not_reaped_before_descendant_cancellation() {
    let f = Fixture::new();
    let mut nav = f.navigator("exit-descendants");
    let handle = nav.cancellation_handle();
    let _cleanup = CancelOnDrop(handle.clone());
    nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !nav.status().contains("exit code 0") {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let pid = fs::read_to_string(f.root.join("mock.pid"))
        .unwrap()
        .parse::<libc::id_t>()
        .unwrap();
    // The child remains waitable after status(): its PID cannot be recycled yet.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    assert_eq!(result, 0);
    assert_eq!(unsafe { info.si_pid() }, pid as libc::pid_t);
    handle.cancel().unwrap();
    // Reaping is independent of Navigator shutdown, even for an idle instance.
    assert_process_gone(&f.root.join("mock.pid"));
    assert_process_gone(&f.root.join("descendant.pid"));
    nav.shutdown();
    handle.cancel().unwrap();
}

#[test]
fn cancellation_before_first_request_prevents_server_start() {
    let f = Fixture::new();
    let mut nav = f.navigator("normal");
    let handle = nav.cancellation_handle();
    handle.cancel().unwrap();
    assert!(
        nav.navigate(Path::new("entry.ts"), 1, 6, Definition)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert!(!f.root.join("mock.pid").exists());
    nav.shutdown();
    drop(nav);
    handle.cancel().unwrap();
}

#[cfg(unix)]
#[test]
fn java_gui_discovery_starts_only_on_navigation_and_preserves_user_argv() {
    const CHILD: &str = "POKARAK_TEST_JAVA_GUI_LAUNCHER";
    const TEST: &str = "java_gui_discovery_starts_only_on_navigation_and_preserves_user_argv";
    if let Some(launcher) = std::env::var_os(CHILD) {
        // Run in a separate process so PATH/HOME changes cannot race other tests.
        let root = PathBuf::from(std::env::var_os("POKARAK_TEST_JAVA_GUI_ROOT").unwrap());
        let configs = default_configs();
        let java = configs.iter().find(|c| c.language == "java").unwrap();
        assert_eq!(Path::new(&java.program), Path::new(&launcher));
        assert_eq!(
            java.args,
            [
                "-data",
                "{workspace}",
                "-configuration",
                "{workspace}/configuration"
            ]
        );
        let args = ["normal", "argument with spaces #한", "literal-$HOME-;"];
        let config_file = root.parent().unwrap().join("language-servers.json");
        fs::write(
            &config_file,
            serde_json::to_vec(&serde_json::json!({
                "servers": [{"language":"java", "program":"jdtls", "args":args}]
            }))
            .unwrap(),
        )
        .unwrap();
        let options = NavigatorOptions {
            config_file: Some(config_file),
            initialize_timeout: Duration::from_secs(3),
            request_timeout: Duration::from_secs(3),
            shutdown_timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let mut cancelled = Navigator::with_options(root.clone(), configs.clone(), options.clone());
        assert!(!root.join("mock.pid").exists());
        cancelled.cancellation_handle().cancel().unwrap();
        assert!(
            cancelled
                .navigate(Path::new("Main.java"), 0, 6, Definition)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(!root.join("mock.pid").exists());
        let mut nav = Navigator::with_options(root.clone(), configs, options);
        assert!(!root.join("mock.pid").exists());
        assert!(
            nav.navigate(Path::new("Main.java"), 0, 6, Definition)
                .unwrap()
                .semantic
        );
        nav.shutdown();
        let launched: Vec<String> =
            serde_json::from_str(&fs::read_to_string(root.join("launch.json")).unwrap()).unwrap();
        assert_eq!(Path::new(&launched[0]), Path::new(&launcher));
        assert_eq!(&launched[1..4], &args);
        assert_eq!(launched[4], "-data");
        assert!(!Path::new(&launched[5]).starts_with(&root));
        assert_process_gone(&root.join("mock.pid"));
        return;
    }

    for location in ["brew", "home", "JDTLS_HOME", "JDTLS_DIR"] {
        let f = Fixture::new();
        fs::write(f.root.join("Main.java"), "class Main {}\n").unwrap();
        fs::write(f.root.join("pom.xml"), "<project/>\n").unwrap();
        let home = f._temp.path().join("isolated home");
        let brew = f._temp.path().join("isolated brew");
        let dist = f._temp.path().join("user JDT LS #한");
        let launcher = match location {
            "brew" => brew.join("opt/jdtls/bin/jdtls"),
            "home" => home.join(".local/share/jdtls/bin/jdtls"),
            _ => dist.join("bin/jdtls"),
        };
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pokarak-mock-lsp"), &launcher).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, &launcher)
            .env("POKARAK_TEST_JAVA_GUI_ROOT", &f.root)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("HOMEBREW_PREFIX", &brew)
            .env_remove("JDTLS_HOME")
            .env_remove("JDTLS_DIR");
        if location.starts_with("JDTLS_") {
            child.env(location, &dist);
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{location}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn missing_java_override_is_actionable_before_build_discovery_and_can_be_retried() {
    let f = Fixture::new();
    let source = Path::new("Main.java");
    fs::write(f.root.join(source), "class Main {}\n").unwrap();
    let launcher = f._temp.path().join("custom Java install/jdtls");
    let config_file = f._temp.path().join("language-servers.json");
    fs::write(
        &config_file,
        serde_json::to_vec(&serde_json::json!({
            "servers": [{"language":"java", "program":launcher, "args":["normal"]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut options = f.options();
    options.config_file = Some(config_file);
    // Even a real installed JDT LS must never replace a broken explicit override.
    let mut nav = Navigator::with_options(f.root.clone(), default_configs(), options);
    assert!(!f.root.join("mock.pid").exists());
    let error = nav
        .navigate(source, 0, 6, Definition)
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("Language server unavailable for java"));
    assert!(error.contains(launcher.to_str().unwrap()));
    assert!(error.contains("실행 파일을 찾을 수 없거나 실행 권한이 없습니다"));
    assert!(error.contains("brew install jdtls"));
    assert!(error.contains("scripts/setup-language-servers.sh"));
    assert!(error.contains("~/.config/pokarak/language-servers.json"));
    assert_eq!(
        nav.navigate(source, 0, 6, References)
            .unwrap_err()
            .to_string(),
        error
    );
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pokarak-mock-lsp"), &launcher).unwrap();
    assert!(
        nav.navigate(source, 0, 6, Definition)
            .unwrap_err()
            .to_string()
            .contains("noBuildSystemFound")
    );
    assert!(!f.root.join("mock.pid").exists());
    fs::write(f.root.join("pom.xml"), "<project/>\n").unwrap();
    assert!(nav.navigate(source, 0, 6, Definition).unwrap().semantic);
    nav.shutdown();
    assert_process_gone(&f.root.join("mock.pid"));
}

#[cfg(unix)]
#[test]
fn unavailable_kotlin_honors_override_and_trust_before_project_discovery() {
    use std::os::unix::fs::symlink;

    let f = Fixture::new();
    let source = Path::new("Main.kt");
    fs::write(f.root.join(source), "class Main\n").unwrap();
    let launcher = f._temp.path().join("custom Kotlin launcher");
    let config_path = f._temp.path().join("language-servers.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&serde_json::json!({
            "servers": [{"language":"kotlin", "program": launcher, "args": ["--stdio"]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut options = f.options();
    options.config_file = Some(config_path);
    // The real installed server, if any, must never replace an explicit broken override.
    let mut nav = Navigator::with_options(f.root.clone(), default_configs(), options.clone());
    assert!(
        nav.navigate(source, 0, 6, Definition)
            .unwrap_err()
            .to_string()
            .contains("allowProjectImports")
    );
    options.allow_project_imports = true;
    let mut nav = Navigator::with_options(f.root.clone(), default_configs(), options);
    let missing = nav
        .navigate(source, 0, 6, Definition)
        .unwrap_err()
        .to_string();
    assert!(missing.starts_with("Language server unavailable for kotlin"));
    assert!(missing.contains(launcher.to_str().unwrap()));
    assert!(missing.contains("brew install JetBrains/utils/kotlin-lsp"));
    assert_eq!(
        nav.navigate(source, 0, 6, References)
            .unwrap_err()
            .to_string(),
        missing
    );

    // Fixing the executable allows a retry to progress to build metadata discovery.
    symlink(env!("CARGO_BIN_EXE_pokarak-mock-lsp"), &launcher).unwrap();
    assert!(
        nav.navigate(source, 0, 6, Definition)
            .unwrap_err()
            .to_string()
            .contains("noBuildSystemFound")
    );
    assert!(!f.root.join("mock.pid").exists());
    nav.cancellation_handle().cancel().unwrap();
    assert!(
        nav.navigate(source, 0, 6, Definition)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}

#[test]
fn missing_kotlin_build_is_actionable_and_does_not_launch_a_server() {
    let f = Fixture::new();
    fs::write(f.root.join("Main.kt"), "class Main\n").unwrap();
    let mut config = f.config("normal");
    config.language = "kotlin".into();
    let mut options = f.options();
    options.allow_project_imports = true;
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], options);
    let error = nav
        .navigate(Path::new("Main.kt"), 0, 6, Definition)
        .unwrap_err()
        .to_string();
    assert!(error.contains("noBuildSystemFound") && error.contains("pom.xml"));
    assert!(!error.contains("JDK"));
    assert!(!f.root.join("mock.pid").exists());
}

#[cfg(unix)]
#[test]
fn nested_import_roots_keep_worktree_tokens_and_dependency_clients_isolated() {
    let f = Fixture::new();
    let launcher = f.root.join("intellij-server");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pokarak-mock-lsp"), &launcher).unwrap();
    let deps = tempfile::tempdir().unwrap();
    let deps = deps.path().canonicalize().unwrap();
    let config = ServerConfig {
        language: "kotlin".into(),
        program: launcher.to_string_lossy().into_owned(),
        args: vec!["dependency".into(), "{root}".into()],
    };
    for name in ["a", "b"] {
        fs::create_dir(f.root.join(name)).unwrap();
        fs::write(f.root.join(name).join("pom.xml"), "<project/>").unwrap();
        fs::write(f.root.join(name).join("Main.kt"), "class Main\n").unwrap();
        let dependency = deps.join(format!("{name}.java"));
        fs::write(&dependency, "// dependency\n  class Source {}\n").unwrap();
        fs::write(
            f.root.join(name).join("dependency-uri"),
            path_to_uri(&dependency).unwrap(),
        )
        .unwrap();
    }
    let mut options = f.options();
    options.allow_project_imports = true;
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], options);
    assert_eq!(nav.root(), f.root);
    let a = nav
        .navigate(Path::new("a/Main.kt"), 0, 6, Definition)
        .unwrap()
        .locations
        .remove(0);
    nav.navigate(Path::new("b/Main.kt"), 0, 6, Definition)
        .unwrap();
    // No Java configuration exists. This must return to a's Kotlin client, not
    // infer a new import root / JDT client from the external Java file.
    nav.navigate(&a.path, 1, 2, References).unwrap();
    nav.shutdown();
    let mut index_dirs = vec![];
    for name in ["a", "b"] {
        let project = f.root.join(name);
        let log: Vec<Value> = fs::read_to_string(project.join("protocol.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let init = &log[0]["params"];
        assert_eq!(init["rootUri"], path_to_uri(&project).unwrap());
        assert_eq!(init["workspaceFolders"][0]["uri"], init["rootUri"]);
        index_dirs.push(
            init["initializationOptions"]["indexDir"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        assert_eq!(
            log.iter().filter(|m| m["method"] == "initialize").count(),
            1
        );
        let args: Vec<String> =
            serde_json::from_str(&fs::read_to_string(project.join("launch.json")).unwrap())
                .unwrap();
        assert!(
            args.contains(&f.root.to_string_lossy().into_owned()),
            "public root token changed"
        );
        assert_eq!(
            log.iter().any(|m| m["method"] == "textDocument/references"),
            name == "a"
        );
        assert_process_gone(&project.join("mock.pid"));
    }
    assert_ne!(index_dirs[0], index_dirs[1]);
    assert!(!Path::new(&index_dirs[0]).starts_with(&f.root));
}

#[cfg(unix)]
#[test]
fn jdt_uses_nested_build_root_with_imports_disabled_until_opt_in() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("backend")).unwrap();
    fs::write(f.root.join("backend/pom.xml"), "<project/>").unwrap();
    fs::write(f.root.join("backend/Main.java"), "class Main {}\n").unwrap();
    let launcher = f.root.join("jdtls");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pokarak-mock-lsp"), &launcher).unwrap();
    let config = ServerConfig {
        language: "java".into(),
        program: launcher.to_string_lossy().into_owned(),
        args: vec!["normal".into()],
    };
    let mut nav = Navigator::with_options(f.root.clone(), vec![config], f.options());
    nav.navigate(Path::new("backend/Main.java"), 0, 6, Definition)
        .unwrap();
    nav.shutdown();
    let log: Vec<Value> = fs::read_to_string(f.root.join("backend/protocol.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let init = &log[0]["params"];
    assert_eq!(
        init["rootUri"],
        path_to_uri(&f.root.join("backend")).unwrap()
    );
    assert_eq!(
        init["initializationOptions"]["settings"]["java"]["import"]["maven"]["enabled"],
        false
    );
    assert_eq!(
        init["initializationOptions"]["settings"]["java"]["autobuild"]["enabled"],
        false
    );
    assert_eq!(nav.root(), f.root);
}
