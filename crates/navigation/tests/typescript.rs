//! Opt-in real-server test. Installs nothing; supply POKARAK_TS_SERVER explicitly.
use pokarak_navigation::{NavigationKind::*, Navigator, NavigatorOptions, ServerConfig};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Project {
    fn new(marker: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("TS fixture #한😀");
        fs::create_dir(&root).unwrap();
        let output = Command::new("git")
            .args(["init", "-q", "--"])
            .arg(&root)
            .output()
            .unwrap();
        assert!(output.status.success());
        fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{"target":"ES2022","module":"CommonJS","strict":true,"allowJs":true,"checkJs":true,"jsx":"preserve"},"include":["*.ts","*.tsx","*.js","*.jsx"]}"#).unwrap();
        fs::write(root.join("model.ts"), format!("export interface Greeter {{ greet(): string; }}\nexport class Hello implements Greeter {{ greet() {{ return '{marker}'; }} }}\nexport function greet() {{ return '{marker}'; }}\n")).unwrap();
        fs::write(root.join("entry.ts"), "import { greet, Greeter, Hello } from './model';\nconst emoji = '😀'; greet();\nconst x: Greeter = new Hello();\nx.greet();\n").unwrap();
        fs::write(root.join("view.tsx"), "import { greet } from './model';\nconst title = greet();\nconst view = <span>{title}</span>;\n").unwrap();
        fs::write(
            root.join("util.js"),
            "export function ping() { return 'pong'; }\n",
        )
        .unwrap();
        fs::write(root.join("view.jsx"), "import { ping } from './util';\nconst text = ping();\nconst view = <span>{text}</span>;\n").unwrap();
        Self {
            _temp: temp,
            root: root.canonicalize().unwrap(),
        }
    }
    fn navigator(&self, server: &str) -> Navigator {
        Navigator::with_options(
            self.root.clone(),
            vec![ServerConfig {
                language: "typescript".into(),
                program: server.into(),
                args: vec!["--stdio".into()],
            }],
            NavigatorOptions {
                config_file: None,
                initialize_timeout: Duration::from_secs(20),
                request_timeout: Duration::from_secs(15),
                shutdown_timeout: Duration::from_secs(2),
                ..Default::default()
            },
        )
    }
}

#[test]
#[ignore = "requires explicitly installed POKARAK_TS_SERVER outside the project"]
fn real_typescript_navigation_and_disk_updates() {
    let server = std::env::var("POKARAK_TS_SERVER")
        .expect("Set POKARAK_TS_SERVER to an installed typescript-language-server executable");
    let a = Project::new("first");
    let b = Project::new("second");
    let mut nav = a.navigator(&server);
    // The emoji consumes two UTF-16 units, but four UTF-8 bytes.
    let definition = nav
        .navigate(Path::new("entry.ts"), 1, 20, Definition)
        .unwrap();
    assert!(definition.semantic, "{}", definition.status);
    assert!(
        definition
            .locations
            .iter()
            .any(|hit| hit.path == a.root.join("model.ts") && hit.line == 2),
        "{:?}",
        definition.locations
    );
    let references = nav
        .navigate(Path::new("model.ts"), 2, 17, References)
        .unwrap();
    assert!(
        references
            .locations
            .iter()
            .any(|hit| hit.path == a.root.join("entry.ts") && hit.line == 1),
        "{:?}",
        references.locations
    );
    let implementations = nav
        .navigate(Path::new("model.ts"), 0, 18, Implementation)
        .unwrap();
    assert!(
        implementations
            .locations
            .iter()
            .any(|hit| hit.path == a.root.join("model.ts") && hit.line == 1),
        "{:?}",
        implementations.locations
    );
    let tsx = nav
        .navigate(Path::new("view.tsx"), 1, 16, Definition)
        .unwrap();
    assert!(
        tsx.locations
            .iter()
            .any(|hit| hit.path == a.root.join("model.ts"))
    );
    let jsx = nav
        .navigate(Path::new("view.jsx"), 1, 14, Definition)
        .unwrap();
    assert!(
        jsx.locations
            .iter()
            .any(|hit| hit.path == a.root.join("util.js"))
    );
    // model.ts is now open in the server. Refresh its in-memory copy from disk.
    let model = fs::read_to_string(a.root.join("model.ts")).unwrap();
    fs::write(
        a.root.join("model.ts"),
        format!("// inserted externally\n{model}"),
    )
    .unwrap();
    let updated = nav
        .navigate(Path::new("entry.ts"), 1, 20, Definition)
        .unwrap();
    assert!(
        updated
            .locations
            .iter()
            .any(|hit| hit.path == a.root.join("model.ts") && hit.line == 3),
        "{:?}",
        updated.locations
    );
    let mut other = b.navigator(&server);
    let other_result = other
        .navigate(Path::new("entry.ts"), 1, 20, Definition)
        .unwrap();
    assert!(
        other_result
            .locations
            .iter()
            .all(|hit| hit.path.starts_with(&b.root))
    );
    assert!(other_result.locations.iter().any(|hit| hit.line == 2));
    nav.shutdown();
    other.shutdown();
    // No language-server logs, package installs, or compiler output in fixtures.
    for project in [&a, &b] {
        assert!(!project.root.join("node_modules").exists());
        assert!(!project.root.join(".log").exists());
        assert!(!project.root.join("model.js").exists());
    }
}
