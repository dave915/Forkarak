//! Opt-in real JVM server checks, using authored temporary repositories only.
use pokarak_navigation::{
    NavigationKind, NavigationKind::*, Navigator, NavigatorOptions, ServerConfig, installed_configs,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

fn tools() -> PathBuf {
    std::env::var_os("POKARAK_TOOLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share/pokarak/tools")
        })
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("JVM fixture 한");
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
            _temp: temp,
            root: root.canonicalize().unwrap(),
        }
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn nav(&self, configs: Vec<ServerConfig>) -> Navigator {
        Navigator::with_options(
            self.root.clone(),
            configs,
            NavigatorOptions {
                config_file: None,
                allow_project_imports: true,
                initialize_timeout: Duration::from_secs(120),
                request_timeout: Duration::from_secs(120),
                shutdown_timeout: Duration::from_secs(3),
            },
        )
    }
    fn position(&self, path: &str, needle: &str) -> (u32, u32) {
        let text = fs::read_to_string(self.root.join(path)).unwrap();
        let index = text.find(needle).unwrap();
        let prefix = &text[..index];
        (
            prefix.bytes().filter(|b| *b == b'\n').count() as u32,
            prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
        )
    }
    fn check(
        &self,
        nav: &mut Navigator,
        path: &str,
        needle: &str,
        kind: NavigationKind,
        target: &str,
        target_needle: &str,
    ) -> pokarak_navigation::NavigationResult {
        let (line, col) = self.position(path, needle);
        let expected = self.position(target, target_needle);
        let result = nav
            .navigate(Path::new(path), line, col, kind.clone())
            .unwrap_or_else(|error| panic!("{kind:?} {path}:{line}:{col}: {error:#}"));
        eprintln!(
            "{kind:?} {path}:{line}:{col} -> {:?} ({})",
            result.locations, result.status
        );
        assert!(result.semantic);
        assert!(
            result
                .locations
                .iter()
                .any(|hit| hit.path == self.root.join(target) && hit.line == expected.0),
            "Expected {target}:{}; {}\n{}\n{}",
            expected.0,
            result.status,
            nav.server_messages(),
            nav.stderr()
        );
        result
    }
}

#[test]
#[ignore = "requires scripts/setup-language-servers.sh; real JDT LS / isolated JDK25"]
fn real_jdt_java17_definitions_and_references() {
    let f = Fixture::new();
    f.write(".project", r#"<projectDescription><name>pokarak-java17</name><comment/><projects/><buildSpec><buildCommand><name>org.eclipse.jdt.core.javabuilder</name><arguments/></buildCommand></buildSpec><natures><nature>org.eclipse.jdt.core.javanature</nature></natures></projectDescription>"#);
    f.write(".classpath", r#"<classpath><classpathentry kind="src" path="src"/><classpathentry kind="con" path="org.eclipse.jdt.launching.JRE_CONTAINER/org.eclipse.jdt.internal.debug.ui.launcher.StandardVMType/JavaSE-17"/><classpathentry kind="output" path="bin"/></classpath>"#);
    f.write(".settings/org.eclipse.jdt.core.prefs", "eclipse.preferences.version=1\norg.eclipse.jdt.core.compiler.codegen.targetPlatform=17\norg.eclipse.jdt.core.compiler.compliance=17\norg.eclipse.jdt.core.compiler.source=17\n");
    f.write("src/demo/Greeter.java", "package demo;\npublic class Greeter {\n    public String greet() { return \"hello\"; }\n}\n");
    f.write(
        "src/demo/Caller.java",
        "package demo;\nclass Caller {\n    String call() { return new Greeter().greet(); }\n}\n",
    );
    let configs = installed_configs(&tools())
        .unwrap()
        .into_iter()
        .filter(|c| c.language == "java")
        .collect();
    let mut nav = f.nav(configs);
    f.check(
        &mut nav,
        "src/demo/Caller.java",
        "greet()",
        Definition,
        "src/demo/Greeter.java",
        "greet()",
    );
    let references = f.check(
        &mut nav,
        "src/demo/Greeter.java",
        "greet()",
        References,
        "src/demo/Caller.java",
        "greet()",
    );
    assert!(references.status.contains("Kotlin callers may be missing"));
    nav.shutdown();
}

fn mixed_fixture() -> Fixture {
    let f = Fixture::new();
    f.write("pom.xml", r#"<project xmlns="http://maven.apache.org/POM/4.0.0"><modelVersion>4.0.0</modelVersion><groupId>dev.pokarak.fixture</groupId><artifactId>mixed</artifactId><version>1.0</version><properties><maven.compiler.release>17</maven.compiler.release><kotlin.version>2.2.21</kotlin.version><project.build.sourceEncoding>UTF-8</project.build.sourceEncoding></properties><dependencies><dependency><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-stdlib</artifactId><version>${kotlin.version}</version></dependency></dependencies><build><plugins><plugin><groupId>org.jetbrains.kotlin</groupId><artifactId>kotlin-maven-plugin</artifactId><version>${kotlin.version}</version><executions><execution><id>kotlin-compile</id><phase>compile</phase><goals><goal>compile</goal></goals></execution></executions><configuration><jvmTarget>17</jvmTarget><sourceDirs><source>src/main/kotlin</source><source>src/main/java</source></sourceDirs></configuration></plugin><plugin><groupId>org.apache.maven.plugins</groupId><artifactId>maven-compiler-plugin</artifactId><version>3.14.1</version><configuration><release>17</release></configuration></plugin></plugins></build></project>"#);
    f.write("src/main/java/demo/JavaGreeter.java", "package demo;\npublic class JavaGreeter {\n    public String javaGreeting() { return \"hello\"; }\n}\n");
    f.write("src/main/kotlin/demo/KotlinGreeter.kt", "package demo\nclass KotlinGreeter {\n    fun kotlinGreeting(): String = JavaGreeter().javaGreeting()\n}\n");
    f.write(
        "src/main/kotlin/demo/KotlinCaller.kt",
        "package demo\nfun kotlinCall(): String = KotlinGreeter().kotlinGreeting()\n",
    );
    f.write("src/main/java/demo/JavaCaller.java", "package demo;\nclass JavaCaller {\n    String javaCall() { return new KotlinGreeter().kotlinGreeting(); }\n}\n");
    f
}

#[test]
#[ignore = "requires official JetBrains Kotlin LSP; imports authored Maven fixture with source level 17"]
fn real_official_kotlin_mixed_java17_definitions_and_references() {
    let f = mixed_fixture();
    let kotlin = installed_configs(&tools())
        .unwrap()
        .into_iter()
        .find(|c| c.language == "kotlin")
        .unwrap();
    let mut java = kotlin.clone();
    java.language = "java".into();
    let mut nav = f.nav(vec![kotlin, java]);
    f.check(
        &mut nav,
        "src/main/kotlin/demo/KotlinCaller.kt",
        "kotlinGreeting()",
        Definition,
        "src/main/kotlin/demo/KotlinGreeter.kt",
        "kotlinGreeting()",
    );
    f.check(
        &mut nav,
        "src/main/kotlin/demo/KotlinGreeter.kt",
        "kotlinGreeting()",
        References,
        "src/main/kotlin/demo/KotlinCaller.kt",
        "kotlinGreeting()",
    );
    f.check(
        &mut nav,
        "src/main/kotlin/demo/KotlinGreeter.kt",
        "javaGreeting()",
        Definition,
        "src/main/java/demo/JavaGreeter.java",
        "javaGreeting()",
    );
    for (path, needle, kind) in [
        (
            "src/main/java/demo/JavaCaller.java",
            "kotlinGreeting()",
            Definition,
        ),
        (
            "src/main/java/demo/JavaGreeter.java",
            "javaGreeting()",
            References,
        ),
    ] {
        let (line, column) = f.position(path, needle);
        let result = nav
            .navigate(Path::new(path), line, column, kind.clone())
            .unwrap();
        eprintln!(
            "JAVA-SOURCE CAPABILITY PROBE {kind:?} {path}: {:?}; current official Kotlin-only bundle may return no locations",
            result.locations
        );
        // Do not claim Java-source support based on observing Java files in an import.
    }
    f.check(
        &mut nav,
        "src/main/kotlin/demo/KotlinGreeter.kt",
        "kotlinGreeting()",
        References,
        "src/main/java/demo/JavaCaller.java",
        "kotlinGreeting()",
    );
    nav.shutdown();
}

#[test]
#[ignore = "requires official JetBrains Kotlin LSP and authored Kotlin-only Maven import"]
fn real_official_kotlin_only_definitions_and_references() {
    let f = mixed_fixture();
    fs::remove_dir_all(f.root.join("src/main/java")).unwrap();
    f.write(
        "src/main/kotlin/demo/KotlinGreeter.kt",
        "package demo\nclass KotlinGreeter {\n    fun kotlinGreeting(): String = \"hello\"\n}\n",
    );
    let kotlin = installed_configs(&tools())
        .unwrap()
        .into_iter()
        .find(|c| c.language == "kotlin")
        .unwrap();
    let mut nav = f.nav(vec![kotlin]);
    f.check(
        &mut nav,
        "src/main/kotlin/demo/KotlinCaller.kt",
        "kotlinGreeting()",
        Definition,
        "src/main/kotlin/demo/KotlinGreeter.kt",
        "kotlinGreeting()",
    );
    f.check(
        &mut nav,
        "src/main/kotlin/demo/KotlinGreeter.kt",
        "kotlinGreeting()",
        References,
        "src/main/kotlin/demo/KotlinCaller.kt",
        "kotlinGreeting()",
    );
    nav.shutdown();
}
