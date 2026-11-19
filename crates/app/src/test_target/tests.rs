use super::*;
use pokarak_navigation::{Position, TextRange};
use serde_json::json;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

struct Repo(TempDir, PathBuf);
impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("work tree");
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        Self(temp, root)
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.1.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    fn run(
        &self,
        path: &str,
        line: u32,
        file_scope: bool,
        symbols: &[DocumentSymbol],
        settings: &Value,
    ) -> Result<Option<TestTarget>> {
        let source = fs::read_to_string(self.1.join(path))?;
        detect(
            &self.1,
            Path::new(path),
            &source,
            line,
            file_scope,
            symbols,
            settings,
        )
    }
    fn target(&self, path: &str, line: u32, file_scope: bool) -> TestTarget {
        self.run(path, line, file_scope, &[], &Value::Null)
            .unwrap()
            .unwrap()
    }
}

const JAVA: &str = r#"package shop.api;
class OrdersTest {
  // @Test void thisIsAComment() {}
  @org.junit.jupiter.params.ParameterizedTest
  @DisplayName("braces } and @Test")
  public void shipsFree(
      int value) {
    if (value > 0) { check(value); }
  }
  void helper() { }
  @Nested class Region {
    @Test void localOnly() {}
  }
}
class SecondTest {
    void helper() { }
}
"#;

#[test]
fn gradle_nested_build_uses_its_own_wrapper_selector_and_report_directory() {
    let repo = Repo::new();
    for path in [
        "settings.gradle",
        "build.gradle",
        "gradlew",
        "sibling/gradlew",
        "services/checkout/gradlew",
        "services/checkout/order modules/api/build.gradle.kts",
    ] {
        repo.write(path, "");
    }
    repo.write(
        "services/checkout/settings.gradle.kts",
        "include(\":order modules:api\")",
    );
    let path = "services/checkout/order modules/api/src/test/java/shop/api/OrdersTest.java";
    repo.write(path, JAVA);
    let target = repo.target(path, 7, false);
    assert_eq!(target.profile.cwd, Path::new("services/checkout"));
    assert_eq!(
        target.profile.validate(&repo.1).unwrap(),
        repo.1.join("services/checkout")
    );
    assert_eq!(
        target.profile.command,
        [
            repo.1.join("services/checkout/gradlew").to_str().unwrap(),
            ":order modules:api:test",
            "--tests",
            "shop.api.OrdersTest.shipsFree"
        ]
    );
    assert_eq!(target.profile.name, "테스트 · OrdersTest.shipsFree");
    assert_eq!(
        target.report_dirs,
        [repo
            .1
            .join("services/checkout/order modules/api/build/test-results/test")]
    );
    assert_eq!(repo.target(path, 3, false).label, "OrdersTest.shipsFree");
    assert_eq!(
        repo.target(path, 11, false).label,
        "OrdersTest$Region.localOnly"
    );
    for line in [2, 9] {
        assert_eq!(repo.target(path, line, false).label, "OrdersTest");
    }
    assert_eq!(repo.target(path, 15, false).label, "SecondTest");
    assert_eq!(repo.target(path, 11, true).label, "OrdersTest");
}

#[test]
fn maven_reactor_follows_declared_modules_and_uses_actual_settings() {
    let repo = Repo::new();
    repo.write(
        "pom.xml",
        "<project><modules><module>sibling</module></modules></project>",
    );
    repo.write("mvnw", "unrelated");
    repo.write("inner root/pom.xml", r#"<project xmlns="http://maven.apache.org/POM/4.0.0"><modules><module>group</module></modules></project>"#);
    repo.write(
        "inner root/group/pom.xml",
        "<project><modules><module>order api</module></modules></project>",
    );
    repo.write("inner root/group/order api/pom.xml", "<project/>");
    repo.write("inner root/mvnw", "fixture only");
    repo.write("tools/jdk 21/bin/java", "fixture only");
    let path = "inner root/group/order api/src/test/java/OrdersTest.java";
    repo.write(path, JAVA);
    let target = repo.target(path, 7, false);
    assert_eq!(target.profile.cwd, Path::new("inner root"));
    assert_eq!(
        target.profile.command,
        [
            repo.1.join("inner root/mvnw").to_str().unwrap(),
            "-pl",
            "group/order api",
            "-Dtest=shop.api.OrdersTest#shipsFree",
            "test"
        ]
    );
    assert_eq!(
        target.report_dirs,
        [repo
            .1
            .join("inner root/group/order api/target/surefire-reports")]
    );
    let settings = json!({"spring.maven":"/Tools/Maven Wrapper/mvn", "spring.jdk":"tools/jdk 21", "spring.env":{"PATH":"/configured/bin"}});
    let target = repo.run(path, 7, true, &[], &settings).unwrap().unwrap();
    assert_eq!(target.profile.command[0], "/Tools/Maven Wrapper/mvn");
    assert_eq!(target.profile.command[3], "-Dtest=shop.api.OrdersTest");
    assert_eq!(
        target.profile.env["JAVA_HOME"],
        repo.1.join("tools/jdk 21").to_str().unwrap()
    );
    assert_eq!(
        target.profile.env["PATH"],
        format!("{}/tools/jdk 21/bin:/configured/bin", repo.1.display())
    );
}

#[test]
fn standalone_maven_and_gradle_do_not_select_sibling_or_enclosing_unrelated_wrappers() {
    let repo = Repo::new();
    repo.write("mvnw", "unrelated");
    repo.write("pom.xml", "<project/>");
    repo.write("independent/pom.xml", "<project/>");
    let path = "independent/OrdersTest.java";
    repo.write(path, JAVA);
    let target = repo.target(path, 0, true);
    assert_eq!(target.profile.cwd, Path::new("independent"));
    assert_eq!(
        target.profile.command,
        ["mvn", "-Dtest=shop.api.OrdersTest", "test"]
    );
    repo.write("other/gradlew", "unrelated");
    repo.write("a/b/build.gradle", "");
    repo.write("a/b/OrdersTest.java", JAVA);
    let target = repo.target("a/b/OrdersTest.java", 0, true);
    assert_eq!(target.profile.cwd, Path::new("a/b"));
    assert_eq!(
        target.profile.command,
        ["gradle", ":test", "--tests", "shop.api.OrdersTest"]
    );
}

fn symbol(
    name: &str,
    kind: u32,
    start: u32,
    end: u32,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    let range = TextRange {
        start: Position {
            line: start,
            character: 0,
        },
        end: Position {
            line: end,
            character: 20,
        },
    };
    DocumentSymbol {
        name: name.into(),
        detail: None,
        kind,
        range,
        selection_range: range,
        children,
    }
}

#[test]
fn active_jdt_symbol_can_identify_a_method_with_a_custom_test_annotation() {
    let repo = Repo::new();
    repo.write("build.gradle", "");
    let path = "src/test/java/ContractTest.java";
    repo.write(path, "package fixture;\nclass ContractTest {\n  @Contract\n  void verifies(int n) {\n    check(n);\n  }\n}\n");
    let symbols = [symbol(
        "ContractTest",
        5,
        1,
        6,
        vec![symbol("verifies(int)", 6, 2, 5, vec![])],
    )];
    assert_eq!(
        repo.run(path, 4, false, &symbols, &Value::Null)
            .unwrap()
            .unwrap()
            .profile
            .command[3],
        "fixture.ContractTest.verifies"
    );
    assert_eq!(repo.target(path, 4, false).label, "ContractTest");
    assert_eq!(
        repo.run(path, 4, true, &symbols, &Value::Null)
            .unwrap()
            .unwrap()
            .label,
        "ContractTest"
    );
}

#[test]
fn kotlin_annotated_functions_use_declared_class_and_literal_backtick_names() {
    let repo = Repo::new();
    repo.write("build.gradle.kts", "");
    let path = "src/test/kotlin/Shipping.kt";
    repo.write(path, "package shop.shipping\n\nclass DeliveryTest {\n    // @Test fun fake() {}\n    @kotlin.test.Test\n    fun `ships for free`() {\n        check(true)\n    }\n    fun helper() {}\n}\n");
    let target = repo.target(path, 6, false);
    assert_eq!(
        target.profile.command,
        [
            "gradle",
            ":test",
            "--tests",
            "shop.shipping.DeliveryTest.ships for free"
        ]
    );
    assert_eq!(repo.target(path, 8, false).label, "DeliveryTest");
    assert_eq!(repo.target(path, 3, false).label, "DeliveryTest");
}

#[test]
fn cargo_workspace_selects_owning_crate_and_exact_inline_or_external_module_test() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers=['crates/checkout']\n");
    repo.write(
        "crates/checkout/Cargo.toml",
        "[package]\nname='orders-core'\nversion='0.1.0'\n",
    );
    repo.write("crates/checkout/src/main.rs", "mod regional;");
    repo.write(
        "crates/checkout/src/lib.rs",
        "pub mod policy;\npub mod storage;",
    );
    repo.write("crates/checkout/src/storage/mod.rs", "mod policy;");
    let path = "crates/checkout/src/policy.rs";
    let source = r####"#[cfg(test)]
mod tests {
    // #[test] fn fake() {}
    #[test]
    fn same() { let _ = r#" } #[test] "#; }
    mod fast {
        #[test] fn same() {}
    }
}
"####;
    repo.write(path, source);
    repo.write("crates/checkout/src/storage/policy.rs", source);
    let target = repo.target(path, 4, false);
    assert_eq!(
        target.profile.command,
        [
            "cargo",
            "test",
            "-p",
            "orders-core",
            "--lib",
            "policy::tests::same",
            "--",
            "--exact"
        ]
    );
    assert_eq!(target.profile.cwd, Path::new("crates/checkout"));
    assert!(target.report_dirs.is_empty());
    assert_eq!(
        repo.target(path, 6, false).label,
        "policy::tests::fast::same"
    );
    assert_eq!(
        repo.target(path, 3, true).profile.command[5..],
        [
            "policy::tests::same",
            "--",
            "--exact",
            "policy::tests::fast::same"
        ]
    );
    assert_eq!(
        repo.target("crates/checkout/src/storage/policy.rs", 6, false)
            .label,
        "storage::policy::tests::fast::same"
    );
    repo.write("crates/checkout/src/regional.rs", "#[test] fn same() {}");
    assert_eq!(
        repo.target("crates/checkout/src/regional.rs", 0, false)
            .profile
            .command[4..],
        ["--bin", "orders-core", "regional::same", "--", "--exact"]
    );
}

#[test]
fn cargo_integration_file_and_same_named_method_stay_in_the_selected_target() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[package]\nname='suite'\nversion='0.1.0'\n[[test]]\nname='special'\npath='custom/check suite.rs'");
    for (path, target_name) in [
        ("tests/first.rs", "first"),
        ("tests/second/main.rs", "second"),
        ("custom/check suite.rs", "special"),
    ] {
        repo.write(path, "#[test]\nfn duplicate() {}\n");
        assert_eq!(
            repo.target(path, 1, false).profile.command,
            [
                "cargo",
                "test",
                "-p",
                "suite",
                "--test",
                target_name,
                "duplicate",
                "--",
                "--exact"
            ]
        );
        assert_eq!(
            repo.target(path, 1, true).profile.command,
            ["cargo", "test", "-p", "suite", "--test", target_name]
        );
    }
}

#[test]
fn npm_uses_nearest_packages_real_script_and_one_relative_filename_argument() {
    let repo = Repo::new();
    repo.write("package.json", r#"{"scripts":{"test":"outer"}}"#);
    repo.write(
        "web/package.json",
        r#"{"scripts":{"test":"runner --some-options && echo never-evaluated"}}"#,
    );
    let path = "web/spec/order $(check) test.ts";
    repo.write(path, "");
    let target = repo.target(path, 0, false);
    assert_eq!(target.profile.cwd, Path::new("web"));
    assert_eq!(
        target.profile.command,
        ["npm", "test", "--", "spec/order $(check) test.ts"]
    );
    assert!(target.report_dirs.is_empty());
    for manifest in [
        r#"{"test":"elsewhere"}"#,
        r#"{"scripts":{"test":false}}"#,
        r#"{"scripts":{"test":" "}}"#,
    ] {
        repo.write("web/package.json", manifest);
        assert!(
            repo.run(path, 0, true, &[], &Value::Null)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn detection_rejects_worktree_escapes_in_source_manifest_wrapper_and_report_prefixes() {
    let repo = Repo::new();
    let outside = repo.0.path().join("sibling");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("OrdersTest.java"), JAVA).unwrap();
    fs::write(outside.join("pom.xml"), "<project/>").unwrap();
    fs::write(outside.join("mvnw"), "fixture").unwrap();
    for path in [
        Path::new("../sibling/OrdersTest.java"),
        &outside.join("OrdersTest.java"),
    ] {
        assert!(detect(&repo.1, path, JAVA, 0, true, &[], &Value::Null).is_err());
    }
    symlink(&outside, repo.1.join("linked")).unwrap();
    assert!(
        detect(
            &repo.1,
            Path::new("linked/OrdersTest.java"),
            JAVA,
            0,
            true,
            &[],
            &Value::Null
        )
        .is_err()
    );
    repo.write("OrdersTest.java", JAVA);
    assert!(
        repo.run("OrdersTest.java", 0, true, &[], &Value::Null)
            .unwrap()
            .is_none()
    );
    symlink(outside.join("pom.xml"), repo.1.join("pom.xml")).unwrap();
    assert!(
        repo.run("OrdersTest.java", 0, true, &[], &Value::Null)
            .is_err()
    );
    fs::remove_file(repo.1.join("pom.xml")).unwrap();
    repo.write("pom.xml", "<project/>");
    symlink(outside.join("mvnw"), repo.1.join("mvnw")).unwrap();
    assert!(
        repo.run("OrdersTest.java", 0, true, &[], &Value::Null)
            .is_err()
    );
    fs::remove_file(repo.1.join("mvnw")).unwrap();
    symlink(&outside, repo.1.join("target")).unwrap();
    assert!(
        repo.run("OrdersTest.java", 0, true, &[], &Value::Null)
            .is_err()
    );
}

#[test]
fn gradle_literal_project_directory_assignments_keep_the_declared_task_path() {
    for (file, assignment) in [
        (
            "settings.gradle",
            "project(':orders').projectDir = new File(settingsDir, 'warehouse/order service')",
        ),
        (
            "settings.gradle.kts",
            "project(\":orders\").projectDir = file(\n  \"warehouse/order service\"\n)",
        ),
    ] {
        let repo = Repo::new();
        repo.write(file, &format!("include(\":orders:api\")\n{assignment}\n/* project(':fake').projectDir = file('warehouse/order service/api') */"));
        repo.write("warehouse/order service/api/build.gradle", "");
        let path = "warehouse/order service/api/OrdersTest.java";
        repo.write(path, JAVA);
        assert_eq!(
            repo.target(path, 7, false).profile.command,
            [
                "gradle",
                ":orders:api:test",
                "--tests",
                "shop.api.OrdersTest.shipsFree"
            ]
        );
    }
}

#[test]
fn enclosing_worktree_does_not_supply_builds_or_trust_to_nested_git_checkouts() {
    let repo = Repo::new();
    repo.write("build.gradle", "");
    repo.write("checkout/.git", "gitdir: /some/other/worktree");
    let path = "checkout/OrdersTest.java";
    repo.write(path, JAVA);
    assert!(
        repo.run(path, 0, true, &[], &Value::Null)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cargo_rejects_a_manifest_that_selects_a_workspace_in_another_checkout() {
    let repo = Repo::new();
    fs::create_dir(repo.0.path().join("outside")).unwrap();
    repo.write(
        "Cargo.toml",
        "[package]\nname='suite'\nworkspace='../outside'\nversion='0.1.0'",
    );
    repo.write("src/lib.rs", "#[test] fn check() {}");
    assert!(repo.run("src/lib.rs", 0, false, &[], &Value::Null).is_err());
}
