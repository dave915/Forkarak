//! Explicit opt-in tests: real installed tools, authored temporary projects, no installation.
use anyhow::{Context, Result, bail};
use pokarak_navigation::{
    DocumentSnapshot, FormattingOptions, ManagedTools, NavigationKind, Navigator, NavigatorOptions,
};
use std::{fs, path::PathBuf, process::Command, time::Duration};
fn tools() -> ManagedTools {
    ManagedTools::new(
        PathBuf::from(
            std::env::var_os("FORKARAK_MANAGED_TOOLS_DIR").expect("set FORKARAK_MANAGED_TOOLS_DIR"),
        ),
        None,
    )
    .unwrap()
}
struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Project {
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("managed fixture 한");
        fs::create_dir(&root).unwrap();
        assert!(
            Command::new("/usr/bin/git")
                .args(["init", "-q", "--"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        Self {
            _temp: t,
            root: root.canonicalize().unwrap(),
        }
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    fn nav(&self, package: &str) -> Result<Navigator> {
        let status = tools().status(package)?;
        if let Some(error) = status.problem {
            bail!("{error}");
        }
        let installed = status
            .installed
            .context("Package must be explicitly installed before this test")?;
        Ok(Navigator::with_options(
            self.root.clone(),
            installed.servers,
            NavigatorOptions {
                config_file: None,
                allow_project_imports: true,
                initialize_timeout: Duration::from_secs(120),
                request_timeout: Duration::from_secs(40),
                shutdown_timeout: Duration::from_secs(3),
            },
        ))
    }
    fn snapshot(&self, path: &str, source: &str, version: i64) -> DocumentSnapshot {
        DocumentSnapshot::new(self.root.clone(), path.into(), source.into(), version).unwrap()
    }
}

#[test]
#[ignore = "requires explicitly installed Node-based servers; real language responses, short PATH supported"]
fn real_managed_static_language_matrix() -> Result<()> {
    // A marker is removed before sending. Completion is required for selected cases;
    // symbols for data/config languages prove actual structural analysis.
    let cases = [
        ("web", "test.html", "<div><spa|</div>", true),
        ("web", "test.css", ".item { col| }", true),
        (
            "web",
            "test.scss",
            "$primary: red;\n.item { color: $primary; }|",
            false,
        ),
        ("web", "test.json", "{\"server\": {\"port\": 8080}}|", false),
        (
            "web",
            "test.jsonc",
            "// comment\n{\"server\": {\"port\": 8080}}|",
            false,
        ),
        ("yaml", "test.yaml", "server:\n  port: 8080\n|", false),
        ("toml", "test.toml", "[server]\nport = 8080\n|", false),
        ("dockerfile", "Dockerfile", "FROM alpine\nEN|", true),
        (
            "shell",
            "test.sh",
            "#!/bin/bash\nhello() { echo hi; }\nhello\n|",
            false,
        ),
        ("sql", "test.sql", "SEL|", true),
        (
            "svelte",
            "test.svelte",
            "<script>let name = 'world';</script>\n<h|",
            true,
        ),
        (
            "vue",
            "test.vue",
            "<script setup lang=\"ts\">const name = 'world';</script>\n<h|",
            true,
        ),
        (
            "xml",
            "test.xml",
            "<server><port>8080</port></server>|",
            false,
        ),
        (
            "groovy",
            "test.groovy",
            "class Greeter { String greeting(String name) { return name } }\n|",
            false,
        ),
        (
            "go",
            "test.go",
            "package fixture\nfunc Greeting(name string) string { return name }\n|",
            false,
        ),
        (
            "makefile",
            "Makefile",
            "GREETING := hello\nall: build\nbuild:\n\t@echo $(GREETING)\n|",
            false,
        ),
        (
            "markdown",
            "README.md",
            "# Title\n\n## Section\nText\n|",
            false,
        ),
        (
            "rust",
            "main.rs",
            "fn greeting(value: i32) -> i32 { value + 1 }\nfn main() { greeting(1); }\n|",
            false,
        ),
    ];
    let mut errors = Vec::new();
    for (package, path, source, complete) in cases {
        let run = (|| -> Result<()> {
            let p = Project::new();
            let index = source.find('|').unwrap();
            let text = source.replace('|', "");
            p.write(path, "// disk differs from displayed working draft\n");
            if package == "go" {
                p.write("go.mod", "module fixture\n\ngo 1.27\n");
            }
            if package == "rust" {
                p.write("rust-project.json",&format!("{{\"crates\":[{{\"root_module\":\"{}\",\"edition\":\"2021\",\"deps\":[],\"cfg\":[]}}]}}",p.root.join(path).display()));
            }
            let mut nav = p.nav(package)?;
            let snapshot = p.snapshot(path, &text, 7);
            let caps = nav.capabilities_buffer(&snapshot)?.value;
            eprintln!("CAPABILITIES {package}/{path}: {caps}");
            if complete {
                let prefix = &source[..index];
                let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
                let col = prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
                let values = nav.complete_buffer(&snapshot, line, col)?.value.items;
                eprintln!(
                    "COMPLETIONS {path}: {:?}",
                    values.iter().take(12).map(|v| &v.label).collect::<Vec<_>>()
                );
                if values.is_empty() {
                    bail!("No actual completions returned");
                }
            } else {
                let mut symbols = nav.document_symbols(&snapshot)?.value;
                if package == "shell" {
                    for _ in 0..30 {
                        if !symbols.is_empty() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                        symbols = nav.document_symbols(&snapshot)?.value;
                    }
                }
                eprintln!(
                    "SYMBOLS {path}: {:?}",
                    symbols.iter().map(|v| &v.name).collect::<Vec<_>>()
                );
                if symbols.is_empty() {
                    bail!("No actual symbols returned");
                }
            }
            snapshot.invalidate();
            assert!(nav.capabilities_buffer(&snapshot).is_err());
            assert!(fs::read_to_string(p.root.join(path))?.starts_with("// disk differs"));
            nav.shutdown();
            Ok(())
        })();
        if let Err(error) = run {
            errors.push(format!("{package}/{path}: {error:#}"));
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    Ok(())
}

fn offset(text: &str, needle: &str) -> (u32, u32) {
    let i = text.find(needle).unwrap();
    let p = &text[..i];
    (
        p.bytes().filter(|b| *b == b'\n').count() as u32,
        p.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
    )
}
#[test]
#[ignore = "requires managed Pyright; checks definitions, hover, signatures and dirty snapshots"]
fn real_managed_python_dirty_semantics() -> Result<()> {
    let p = Project::new();
    let text = "def greeting(name: str) -> str:\n    return name\n\nresult = greeting(\"world\")\n";
    p.write("test.py", "# disk only\n");
    let mut nav = p.nav("python")?;
    let s = p.snapshot("test.py", text, 11);
    let (l, c) = offset(text, "greeting(\"world");
    let result = nav
        .navigate_buffer(&s, l, c, NavigationKind::Definition)?
        .value;
    assert!(
        result.semantic
            && result
                .locations
                .iter()
                .any(|h| h.path == s.path() && h.line == 0)
    );
    let hover = nav
        .hover_buffer(&s, l, c)?
        .value
        .context("Expected Python hover")?;
    assert!(hover.contents.iter().any(|s| s.value.contains("greeting")));
    let signature = nav
        .signature_help_buffer(&s, l, c + 9)?
        .value
        .context("Expected Python signature help")?;
    assert!(
        signature
            .signatures
            .iter()
            .any(|s| s.label.contains("name"))
    );
    let refs = nav
        .navigate_buffer(&s, 0, 5, NavigationKind::References)?
        .value;
    assert!(refs.locations.iter().any(|h| h.line == 3));
    s.invalidate();
    assert!(nav.hover_buffer(&s, l, c).is_err());
    assert_eq!(fs::read_to_string(p.root.join("test.py"))?, "# disk only\n");
    nav.shutdown();
    Ok(())
}

#[test]
#[ignore = "requires managed Java/Kotlin; dirty completion/navigation/hover, not initialization only"]
fn real_managed_jvm_dirty_features() -> Result<()> {
    let mut errors = Vec::new();
    for language in ["java", "kotlin"] {
        let run = (|| -> Result<()> {
            let p = Project::new();
            let (path, text, needle, complete, signature) = if language == "java" {
                p.write(".project",r#"<projectDescription><name>managed</name><projects/><buildSpec><buildCommand><name>org.eclipse.jdt.core.javabuilder</name><arguments/></buildCommand></buildSpec><natures><nature>org.eclipse.jdt.core.javanature</nature></natures></projectDescription>"#);
                p.write(".classpath",r#"<classpath><classpathentry kind="src" path="src"/><classpathentry kind="con" path="org.eclipse.jdt.launching.JRE_CONTAINER"/><classpathentry kind="output" path="bin"/></classpath>"#);
                (
                    "src/Draft.java",
                    "class Draft {\n String greeting(String name) { return name; }\n String call() { return greeting(\"world\"); }\n}\n",
                    "greeting(\"world",
                    "greet",
                    9,
                )
            } else {
                p.write("pom.xml",r#"<project xmlns="http://maven.apache.org/POM/4.0.0"><modelVersion>4.0.0</modelVersion><groupId>dev.fixture</groupId><artifactId>managed-kotlin</artifactId><version>1.0</version><properties><kotlin.version>2.2.21</kotlin.version></properties><dependencies><dependency><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-stdlib</artifactId><version>${kotlin.version}</version></dependency></dependencies><build><plugins><plugin><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-maven-plugin</artifactId><version>${kotlin.version}</version><executions><execution><id>kotlin-compile</id><goals><goal>compile</goal></goals></execution></executions><configuration><sourceDirs><source>src/main/kotlin</source></sourceDirs></configuration></plugin></plugins></build></project>"#);
                (
                    "src/main/kotlin/Draft.kt",
                    "fun greeting(name: String): String = name\nfun call(): String = greeting(\"world\")\n",
                    "greeting(\"world",
                    "greet",
                    9,
                )
            };
            // Existing project source establishes the import; the symbol is introduced only in memory.
            p.write(
                path,
                if language == "java" {
                    "class Draft {}\n"
                } else {
                    "fun initial() = 1\n"
                },
            );
            let mut nav = p.nav(language)?;
            let s = p.snapshot(path, text, 17);
            let (l, c) = offset(text, needle);
            let result = nav
                .navigate_buffer(&s, l, c, NavigationKind::Definition)?
                .value;
            assert!(
                result.semantic
                    && result
                        .locations
                        .iter()
                        .any(|h| h.path == s.path() && h.line <= 1),
                "{language}: {:?}",
                result.locations
            );
            let h = nav
                .hover_buffer(&s, l, c)?
                .value
                .context("Expected real JVM hover")?;
            assert!(h.contents.iter().any(|v| v.value.contains("greeting")));
            let sig = nav
                .signature_help_buffer(&s, l, c + signature + 1)?
                .value
                .context("Expected real JVM signature")?;
            eprintln!("{language} signatures {:?}", sig.signatures);
            assert!(
                sig.signatures.iter().any(|v| v.label.contains("name")),
                "{:?}",
                sig.signatures
            );
            let candidates = nav
                .complete_buffer(&s, l, c + complete.len() as u32)?
                .value
                .items;
            assert!(
                candidates.iter().any(|v| v.label.contains("greeting")),
                "{language}: {:?}",
                candidates.iter().map(|v| &v.label).collect::<Vec<_>>()
            );
            let symbols = nav.document_symbols(&s)?.value;
            assert!(!symbols.is_empty());
            let _ = nav.format_buffer(&s, &FormattingOptions::default())?;
            s.invalidate();
            assert!(nav.complete_buffer(&s, l, c).is_err());
            nav.shutdown();
            Ok(())
        })();
        if let Err(e) = run {
            errors.push(format!("{language}: {e:#}"));
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    Ok(())
}

#[test]
#[ignore = "requires installed managed Rust/Go/Groovy; actual cross-call semantics under short PATH"]
fn real_managed_compiled_language_semantics() -> Result<()> {
    for package in ["rust", "go", "groovy"] {
        let p = Project::new();
        let (path, text, needle, definition) = match package {
            "rust" => {
                p.write(
                    "Cargo.toml",
                    "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
                );
                (
                    "src/main.rs",
                    "fn greeting(value: i32) -> i32 { value + 1 }\nfn main() { let result = greeting(1); }\n",
                    "greeting(1)",
                    0,
                )
            }
            "go" => {
                p.write("go.mod", "module fixture\n\ngo 1.27\n");
                (
                    "main.go",
                    "package fixture\nfunc Greeting(name string) string { return name }\nfunc Use() string { return Greeting(\"world\") }\n",
                    "Greeting(\"world",
                    1,
                )
            }
            _ => (
                "Main.groovy",
                "class Main {\n String greeting(String name) { return name }\n String call() { return greeting(\"world\") }\n}\n",
                "greeting(\"world",
                1,
            ),
        };
        p.write(path, text);
        let mut nav = p.nav(package)?;
        let snapshot = p.snapshot(path, text, 3);
        let (line, column) = offset(text, needle);
        let column = column + 2;
        let mut hits = vec![];
        for _ in 0..40 {
            hits = nav
                .navigate_buffer(&snapshot, line, column, NavigationKind::Definition)?
                .value
                .locations;
            if !hits.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            hits.iter()
                .any(|hit| hit.path == snapshot.path() && hit.line == definition),
            "{package}: {hits:?}\n{}\n{}",
            nav.server_messages(),
            nav.stderr()
        );
        let hover = nav
            .hover_buffer(&snapshot, line, column)?
            .value
            .context(format!("{package} returned no hover"))?;
        assert!(
            hover
                .contents
                .iter()
                .any(|v| v.value.to_lowercase().contains("greeting")),
            "{package}: {:?}",
            hover.contents
        );
        let refs = nav
            .navigate_buffer(
                &snapshot,
                definition,
                if package == "rust" {
                    4
                } else if package == "go" {
                    5
                } else {
                    9
                },
                NavigationKind::References,
            )?
            .value;
        assert!(
            refs.locations
                .iter()
                .any(|h| h.path == snapshot.path() && h.line == line),
            "{package}: {:?}",
            refs.locations
        );
        if package != "groovy" {
            let edits = nav
                .format_buffer(&snapshot, &FormattingOptions::default())?
                .value;
            let _ = pokarak_navigation::TextEdit::apply_all(snapshot.text(), &edits)?;
        }
        snapshot.invalidate();
        assert!(nav.hover_buffer(&snapshot, line, column).is_err());
        nav.shutdown();
    }
    Ok(())
}

#[test]
#[ignore = "requires managed Pyright and VS Code JSON server; real dirty-buffer diagnostics"]
fn real_managed_versioned_diagnostics() -> Result<()> {
    for (package, path, text) in [
        ("python", "test.py", "value: int = \"wrong\"\n"),
        ("web", "test.json", "{\"name\": }\n"),
    ] {
        let p = Project::new();
        p.write(path, "// disk differs\n");
        let mut nav = p.nav(package)?;
        let snapshot = p.snapshot(path, text, 23);
        let diagnostics = nav.diagnostics(&snapshot)?.value;
        eprintln!(
            "DIAGNOSTICS {package}: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(!diagnostics.is_empty(), "{package}");
        assert!(
            diagnostics.iter().any(|d| d.severity == Some(1)),
            "{package}"
        );
        snapshot.invalidate();
        assert!(nav.diagnostics(&snapshot).is_err());
        nav.shutdown();
    }
    Ok(())
}

#[test]
fn expanded_import_gates_and_stale_capabilities_precede_startup() {
    for (language, path, text) in [
        ("rust", "main.rs", "fn main() {}"),
        ("go", "main.go", "package main"),
        ("groovy", "build.gradle", "plugins {}"),
        ("svelte", "App.svelte", "<p>hello</p>"),
        ("sql", "test.sql", "SELECT 1"),
        ("kotlin", "Main.kt", "fun main() {}"),
    ] {
        let p = Project::new();
        p.write(path, text);
        let mut nav = Navigator::with_options(
            p.root.clone(),
            vec![pokarak_navigation::ServerConfig {
                language: language.into(),
                program: "/definitely/not/installed".into(),
                args: vec![],
            }],
            NavigatorOptions {
                config_file: None,
                ..Default::default()
            },
        );
        let snapshot = p.snapshot(path, text, 1);
        let error = nav.capabilities_buffer(&snapshot).unwrap_err().to_string();
        assert!(error.contains("allowProjectImports"), "{language}: {error}");
        snapshot.invalidate();
        assert!(
            nav.capabilities_buffer(&snapshot)
                .unwrap_err()
                .to_string()
                .contains("Stale")
        );
    }
}

#[test]
#[ignore = "requires packaging worker Resources; actual bundled JDT navigation with managed data path"]
fn real_bundled_java_resource_manifest() -> Result<()> {
    let resources = PathBuf::from(
        std::env::var_os("FORKARAK_BUNDLED_RESOURCES").context("set FORKARAK_BUNDLED_RESOURCES")?,
    )
    .canonicalize()?;
    let p = Project::new();
    let state = p.root.parent().unwrap().join("managed-state");
    let tools = ManagedTools::new(state.clone(), Some(resources.clone()))?;
    let status = tools.status("java")?;
    assert!(status.problem.is_none(), "{:?}", status.problem);
    let java = status.installed.context("Packaged Java missing")?;
    assert_eq!(
        java.distribution,
        pokarak_navigation::ToolDistribution::Bundled
    );
    assert!(!state.exists());
    p.write(".project",r#"<projectDescription><name>bundled-java</name><projects/><buildSpec><buildCommand><name>org.eclipse.jdt.core.javabuilder</name><arguments/></buildCommand></buildSpec><natures><nature>org.eclipse.jdt.core.javanature</nature></natures></projectDescription>"#);
    p.write(".classpath",r#"<classpath><classpathentry kind="src" path="src"/><classpathentry kind="con" path="org.eclipse.jdt.launching.JRE_CONTAINER"/><classpathentry kind="output" path="bin"/></classpath>"#);
    p.write("src/Bundled.java", "class Bundled {}\n");
    let text = "class Bundled {\n String greeting(String name) { return name; }\n String call() { return greeting(\"world\"); }\n}\n";
    let mut nav = Navigator::with_options(
        p.root.clone(),
        java.servers,
        NavigatorOptions {
            config_file: None,
            initialize_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    );
    let snapshot = p.snapshot("src/Bundled.java", text, 9);
    let (line, col) = offset(text, "greeting(\"world");
    let definitions = nav
        .navigate_buffer(&snapshot, line, col + 2, NavigationKind::Definition)?
        .value;
    assert!(
        definitions.semantic
            && definitions
                .locations
                .iter()
                .any(|h| h.path == snapshot.path() && h.line == 1)
    );
    let hover = nav
        .hover_buffer(&snapshot, line, col + 2)?
        .value
        .context("Packaged JDT hover missing")?;
    assert!(hover.contents.iter().any(|v| v.value.contains("greeting")));
    let signature = nav
        .signature_help_buffer(&snapshot, line, col + 10)?
        .value
        .context("Packaged JDT signature missing")?;
    assert!(!signature.signatures.is_empty());
    let completions = nav.complete_buffer(&snapshot, line, col + 5)?.value.items;
    assert!(completions.iter().any(|i| i.label.contains("greeting")));
    nav.shutdown();
    assert_eq!(
        fs::read_to_string(p.root.join("src/Bundled.java"))?,
        "class Bundled {}\n"
    );
    assert!(!state.exists());
    assert!(resources.join("language-tools/manifest.json").is_file());
    Ok(())
}

fn assert_method_rename(
    nav: &mut Navigator,
    source: &DocumentSnapshot,
    caller: &std::path::Path,
) -> Result<()> {
    let (line, column) = offset(source.text(), "greet(");
    let (_, old) = nav
        .prepare_rename_buffer(source, line, column + 1)?
        .value
        .context("prepareRename returned null")?;
    assert_eq!(old, "greet");
    let edit = nav
        .rename_buffer(source, line, column + 1, "welcome")?
        .value;
    let paths: Vec<_> = edit.files.iter().map(|f| f.path.as_path()).collect();
    assert!(
        paths.contains(&source.path()) && paths.contains(&caller),
        "{paths:?}"
    );
    assert_eq!(paths.len(), 2);
    for f in edit.files {
        assert_eq!(f.edits.len(), 1);
        assert!(f.original.contains("greet("));
        assert_eq!(
            pokarak_navigation::TextEdit::apply_all(&f.original, &f.edits)?,
            f.original.replace("greet(", "welcome(")
        );
    }
    Ok(())
}

fn assert_missing_import(
    nav: &mut Navigator,
    source: &DocumentSnapshot,
    needle: &str,
    expected: &str,
) -> Result<()> {
    let diagnostics = nav.diagnostics(source)?.value;
    let (line, column) = offset(source.text(), needle);
    let range = diagnostics
        .iter()
        .find(|d| {
            d.range.start.line == line
                && d.range.start.character <= column
                && d.range.end.character >= column
        })
        .with_context(|| format!("Missing diagnostic for {needle}: {diagnostics:?}"))?
        .range;
    let actions = nav.code_actions_buffer(source, range, &diagnostics)?.value;
    let mut summary = Vec::new();
    for action in actions {
        summary.push((action.title.clone(), action.disabled.clone()));
        if action.disabled.is_some()
            || !action
                .kind
                .as_deref()
                .is_some_and(|kind| kind.starts_with("quickfix"))
            || !action.title.to_lowercase().contains("import")
        {
            continue;
        }
        let resolved = nav.resolve_code_action_buffer(source, &action)?.value;
        if let Some(edit) = resolved.edit {
            for file in edit.files {
                if file.path == source.path() {
                    assert_eq!(file.original, source.text());
                    let text =
                        pokarak_navigation::TextEdit::apply_all(&file.original, &file.edits)?;
                    if text.replace('\"', "'").contains(expected) && text.contains(needle) {
                        return Ok(());
                    }
                }
            }
        }
    }
    bail!("No quick fix added {expected}: {summary:?}")
}

#[test]
#[ignore = "requires managed TypeScript; cross-file method rename and sibling import edits"]
fn real_workspace_edits_typescript() -> Result<()> {
    let p = Project::new();
    p.write("tsconfig.json",r#"{"compilerOptions":{"strict":true,"target":"es2020","module":"esnext"},"include":["*.ts"]}"#);
    let service = "export class Greeter { greet() { return 1; } }\n";
    let caller = "import { Greeter } from './service';\nnew Greeter().greet();\n";
    let missing = "export const n = helperValue();\n";
    p.write("service.ts", service);
    p.write("caller.ts", caller);
    p.write(
        "helper.ts",
        "export function helperValue() { return 42; }\n",
    );
    p.write("missing.ts", "export const n = 0;\n");
    let mut nav = p.nav("typescript")?;
    assert_method_rename(
        &mut nav,
        &p.snapshot("service.ts", service, 8),
        &p.root.join("caller.ts"),
    )?;
    assert_missing_import(
        &mut nav,
        &p.snapshot("missing.ts", missing, 13),
        "helperValue",
        "import { helperValue } from './helper'",
    )?;
    assert_eq!(
        fs::read_to_string(p.root.join("missing.ts"))?,
        "export const n = 0;\n"
    );
    assert_eq!(fs::read_to_string(p.root.join("service.ts"))?, service);
    assert_eq!(fs::read_to_string(p.root.join("caller.ts"))?, caller);
    nav.shutdown();
    Ok(())
}

#[test]
#[ignore = "requires bundled JDT LS; cross-file method rename and missing-type import edits"]
fn real_workspace_edits_bundled_jdt() -> Result<()> {
    let p = Project::new();
    let resources = PathBuf::from(
        std::env::var_os("FORKARAK_BUNDLED_RESOURCES").context("set FORKARAK_BUNDLED_RESOURCES")?,
    );
    let tools = ManagedTools::new(
        p.root.parent().unwrap().join("managed-state"),
        Some(resources),
    )?;
    let java = tools
        .status("java")?
        .installed
        .context("Packaged Java missing")?;
    assert_eq!(
        java.distribution,
        pokarak_navigation::ToolDistribution::Bundled
    );
    p.write(".project",r#"<projectDescription><name>workspace-edits</name><projects/><buildSpec><buildCommand><name>org.eclipse.jdt.core.javabuilder</name><arguments/></buildCommand></buildSpec><natures><nature>org.eclipse.jdt.core.javanature</nature></natures></projectDescription>"#);
    p.write(".classpath",r#"<classpath><classpathentry kind="src" path="src"/><classpathentry kind="con" path="org.eclipse.jdt.launching.JRE_CONTAINER"/><classpathentry kind="output" path="bin"/></classpath>"#);
    let service = "package demo;\npublic class Greeter { public int greet() { return 1; } }\n";
    let caller = "package demo;\nclass Caller { int call() { return new Greeter().greet(); } }\n";
    let missing = "package demo;\nclass Missing { UniqueHelper field; }\n";
    p.write("src/demo/Greeter.java", service);
    p.write("src/demo/Caller.java", caller);
    p.write(
        "src/tools/UniqueHelper.java",
        "package tools;\npublic class UniqueHelper {}\n",
    );
    p.write("src/demo/Missing.java", "package demo;\nclass Missing {}\n");
    let mut nav = Navigator::with_options(
        p.root.clone(),
        java.servers,
        NavigatorOptions {
            config_file: None,
            initialize_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(25),
            ..Default::default()
        },
    );
    assert_method_rename(
        &mut nav,
        &p.snapshot("src/demo/Greeter.java", service, 8),
        &p.root.join("src/demo/Caller.java"),
    )?;
    assert_missing_import(
        &mut nav,
        &p.snapshot("src/demo/Missing.java", missing, 13),
        "UniqueHelper",
        "import tools.UniqueHelper;",
    )?;
    let repaired = p.snapshot(
        "src/demo/Missing.java",
        "package demo;\nclass Missing { tools.UniqueHelper field; }\n",
        14,
    );
    let diagnostics = nav.diagnostics(&repaired)?.value;
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.message.contains("cannot be resolved")),
        "{diagnostics:?}"
    );
    assert_eq!(
        fs::read_to_string(p.root.join("src/demo/Missing.java"))?,
        "package demo;\nclass Missing {}\n"
    );
    assert_eq!(
        fs::read_to_string(p.root.join("src/demo/Greeter.java"))?,
        service
    );
    assert_eq!(
        fs::read_to_string(p.root.join("src/demo/Caller.java"))?,
        caller
    );
    nav.shutdown();
    Ok(())
}
