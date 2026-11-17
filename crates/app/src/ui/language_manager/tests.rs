use super::*;
use pokarak_navigation::{DocumentSnapshot, InstalledTool, ToolArtifact};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    manager: LanguageManager,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("앱 언어 도구");
        let tools = ManagedTools::new(root.clone(), None).unwrap();
        let mut manager = LanguageManager::default();
        manager.tools = Ok(tools);
        manager.refresh();
        Self {
            _temp: temp,
            root,
            manager,
        }
    }
    fn install_fixture(&mut self) -> InstalledTool {
        fs::create_dir_all(&self.root).unwrap();
        let server = self.root.join("authored-server");
        let marker = self.root.join("started");
        fs::write(
            &server,
            format!("#!/bin/sh\n/usr/bin/touch '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
        let tool = InstalledTool {
            package: "java".into(),
            version: "fixture-1".into(),
            distribution: ToolDistribution::FirstUseDownload,
            artifacts: vec![ToolArtifact {
                version: "fixture-runtime-21".into(),
                source_url: "https://example.invalid/authored-test".into(),
                checksum: "sha256:fixture".into(),
                metadata_url: String::new(),
            }],
            servers: vec![ServerConfig {
                language: "java".into(),
                program: server.to_str().unwrap().into(),
                args: vec![],
            }],
            formatters: vec![],
            notices: vec![],
        };
        fs::write(
            self.root.join("managed-tools.json"),
            serde_json::to_vec(&json!({"schemaVersion":1,"packages":{"java":tool}})).unwrap(),
        )
        .unwrap();
        self.manager.refresh();
        tool
    }
    fn pending(&self) -> PreparedRequest {
        let source = self._temp.path().join("authored repo");
        fs::create_dir_all(&source).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "--"])
                .current_dir(&source)
                .status()
                .unwrap()
                .success()
        );
        fs::write(source.join("Main.java"), "class Main {}\n").unwrap();
        let root = source.canonicalize().unwrap();
        PreparedRequest {
            package: "java".into(),
            tool: language_tools::Tool::Navigate(NavigationKind::Definition),
            position: None,
            identity: language_tools::RequestIdentity {
                snapshot: DocumentSnapshot::new(
                    root.clone(),
                    "Main.java".into(),
                    "class Main { int dirty; }\n".into(),
                    1,
                )
                .unwrap(),
                key: code_edit::FileKey {
                    root,
                    path: "Main.java".into(),
                },
                editor: EntityId::from(1),
                selection: 10..10,
                cursor: 10,
                settings: json!({}),
                view: 1,
            },
        }
    }
}

#[test]
fn first_activation_discovery_and_viewing_never_install_or_start_tools() {
    let mut f = Fixture::new();
    assert!(!f.root.exists());
    assert!(!f.manager.ready("java"));
    assert!(f.manager.running.is_empty());
    assert!(
        f.manager
            .tools
            .as_ref()
            .unwrap()
            .server_configs()
            .unwrap()
            .is_empty()
    );
    assert!(!f.root.exists());
    f.install_fixture();
    assert!(f.manager.ready("java"));
    assert!(
        Path::new(&f.manager.tools.as_ref().unwrap().server_configs().unwrap()[0].program)
            .is_absolute()
    );
    assert!(!f.root.join("started").exists());
}

#[test]
fn cancelled_and_failed_update_keep_old_version_and_offer_explicit_retry() {
    for phase in [InstallPhase::Cancelled, InstallPhase::Failed] {
        let mut f = Fixture::new();
        let old = f.install_fixture();
        fs::create_dir(f.root.join("attempts")).unwrap();
        let progress = InstallProgress {
            package: "java".into(),
            phase: phase.clone(),
            message: "authored interrupted update".into(),
            downloaded_bytes: 12,
            total_bytes: None,
            error: Some("authored failure".into()),
        };
        fs::write(
            f.root.join("attempts/java.json"),
            serde_json::to_vec(&progress).unwrap(),
        )
        .unwrap();
        f.manager.refresh();
        assert_eq!(f.manager.progress("java").unwrap().phase, phase);
        assert!(f.manager.ready("java"));
        assert_eq!(
            f.manager.statuses["java"]
                .installed
                .as_ref()
                .unwrap()
                .version,
            old.version
        );
        assert!(
            f.manager.running.is_empty(),
            "retry must require an explicit action"
        );
        assert!(!f.root.join("started").exists());
    }
}

#[test]
fn custom_paths_and_language_disable_prevent_managed_preparation_override() {
    assert!(custom_override(
        Some(&json!({"java":{"path":"/custom/java","args":[]}})),
        "java"
    ));
    assert!(custom_override(
        Some(&json!({"typescript":{"path":"/custom/ts","args":[]}})),
        "typescriptreact"
    ));
    assert!(custom_override(Some(&json!("invalid-json")), "java"));
    assert!(!custom_override(
        Some(&json!({"python":{"path":"/custom/py"}})),
        "java"
    ));
    assert!(!enabled(Some(&json!({"kotlin":false})), "kotlin"));
    assert!(!enabled(
        Some(&json!({"typescript":false})),
        "typescriptreact"
    ));
    assert!(enabled(
        Some(&json!({"typescript":false,"typescriptreact":true})),
        "typescriptreact"
    ));
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("language-servers.json");
    fs::write(
        &config,
        r#"{"servers":[{"language":"kt","program":"/custom/kotlin","args":[]}]}"#,
    )
    .unwrap();
    assert!(legacy_override_at(&config, "kotlin"));
    assert!(!legacy_override_at(&config, "java"));
}

#[test]
fn late_package_completion_cannot_change_origin_or_resume_another_worktree() {
    let mut f = Fixture::new();
    f.manager.pending = Some(f.pending());
    let origin = f.manager.pending.as_ref().unwrap().identity.clone();
    // Real discovery observes an authored installed executable; it never executes
    // an action or rewrites the retained originating request.
    f.install_fixture();
    assert!(origin.matches(&f.manager.pending.as_ref().unwrap().identity, true));
    let mut switched = origin.clone();
    switched.key.root = f._temp.path().join("other worktree");
    assert!(!origin.matches(&switched, true));
    switched = origin.clone();
    switched.view += 1;
    assert!(!origin.matches(&switched, true));
    origin.snapshot.invalidate();
    assert!(!origin.matches(&f.manager.pending.as_ref().unwrap().identity, true));
    f.manager.cancel_request(); // stop analysis/trust revocation uses this path
    assert!(f.manager.pending.is_none());
    f.manager.refresh();
    assert!(
        f.manager.pending.is_none(),
        "completion never recreates a revoked request"
    );
    assert!(!f.root.join("started").exists());
}

#[test]
fn progress_uses_only_actual_bytes_and_handles_missing_http_totals() {
    let mut p = InstallProgress {
        package: "java".into(),
        phase: InstallPhase::Downloading,
        message: String::new(),
        downloaded_bytes: 120,
        total_bytes: None,
        error: None,
    };
    assert_eq!(bytes_label(&p), "120 bytes · 전체 크기 미제공");
    p.total_bytes = Some(240);
    assert_eq!(bytes_label(&p), "120 / 240 bytes · 50%");
    p.total_bytes = Some(0);
    assert!(!bytes_label(&p).contains('%'));
    assert!(absolute_directory("relative".into()).is_err());
}
