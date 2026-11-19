use super::*;
use std::{fs, os::unix::fs::symlink, process::Command, time::Duration};

const GRADLE: &str = "build/test-results/test";
const MAVEN: &str = "target/surefire-reports";
const JAVA: &str = "src/test/java/demo/OrderTest.java";
const PASS: &str = r#"<testsuite><testcase name="passes"/></testsuite>"#;

fn git(cwd: &Path, args: &[&str]) {
    let result = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn repository() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("checkout");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "--quiet", "--template=", "--", "."]);
    (temp, root)
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn nested_suites_count_cases_once_and_resolve_each_selected_module() {
    let (_temp, root) = repository();
    let gradle = root.join("services/gradle module");
    let maven = root.join("services/maven");
    let sibling = root.join("services/unselected");
    for module in [&root, &gradle, &maven, &sibling] {
        write(&module.join(JAVA), "package demo; class OrderTest {}");
    }
    write(&gradle.join("build.gradle.kts"), "");
    write(&maven.join("pom.xml"), "<project/>");
    write(&gradle.join("OrderTest.java"), "class OrderTest {}");
    let xml = r#"<j:testsuites xmlns:j="urn:junit" tests="4" failures="1" errors="1">
      <j:testsuite tests="4">
        <j:testcase classname="demo.OrderTest" name="ships[1 &amp; 2]">
          <j:failure message="expected &lt;2&gt; &amp; got &quot;0&quot;&#10;details"><![CDATA[
  assertion mentioned OrderTest.java:1
  at the following position (OrderTest.java:2)
  at org.example.Hidden.call(Unknown Source) referenced OrderTest.java:3
  at org.junit.Assert.fail(OrderTest.java:99)
  at app//demo.OrderTest$Nested.ships(OrderTest.java:9)
  at demo.OrderTest.ships(OrderTest.java:25)
          ]]></j:failure>
        </j:testcase>
        <j:testsuite tests="3"><j:testcase name="passes"/>
          <j:testcase name="disabled"><j:skipped/></j:testcase>
          <j:testcase name="errors"><j:error>
            Problem &amp; detail
            another line
            at java.base/java.lang.Thread.run(Thread.java:1)
          </j:error></j:testcase>
        </j:testsuite>
        <j:system-out><j:testcase name="printed XML"/></j:system-out>
      </j:testsuite>
    </j:testsuites>"#;
    let surefire = r#"<testsuite><testcase classname="demo.OrderTest" name="ships">
        <error type="FixtureError"><![CDATA[
  at checkout@1/demo.OrderTest.ships(OrderTest.java:31)
        ]]></error>
    </testcase></testsuite>"#;
    write(&gradle.join(GRADLE).join("suite.xml"), xml);
    write(&maven.join(MAVEN).join("TEST-order.xml"), surefire);
    write(&root.join(GRADLE).join("root.xml"), PASS);
    write(&sibling.join(MAVEN).join("TEST-sibling.xml"), PASS);
    let dirs = [maven.join(MAVEN), gradle.join(GRADLE), gradle.join(GRADLE)];
    let result = read_junit_reports(&root, &dirs, SystemTime::UNIX_EPOCH)
        .unwrap()
        .unwrap();
    assert_eq!(
        (result.total, result.skipped, result.failures.len()),
        (5, 1, 3)
    );
    let failed = &result.failures[0];
    assert_eq!(failed.name, "demo.OrderTest.ships[1 & 2]");
    assert_eq!(failed.message, r#"expected <2> & got "0""#);
    assert_eq!(
        failed.location,
        Some(StackLocation {
            path: gradle.join(JAVA),
            line: 8,
            column: 0
        })
    );
    assert_eq!(result.failures[1].message, "Problem & detail");
    assert!(result.failures[1].location.is_none());
    assert_eq!(
        result.failures[2].location,
        Some(StackLocation {
            path: maven.join(JAVA),
            line: 30,
            column: 0
        })
    );
}

#[test]
fn successive_runs_read_only_fresh_direct_matching_reports() {
    let (_temp, root) = repository();
    let reports = root.join(MAVEN);
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let later = start + Duration::from_secs(10);
    for (file, time) in [("TEST-old.xml", start), ("TEST-new.xml", later)] {
        let path = reports.join(file);
        write(&path, PASS);
        File::open(&path).unwrap().set_modified(time).unwrap();
    }
    for file in [
        "failsafe-summary.xml",
        "TEST-text.txt",
        "sub/TEST-extra.xml",
    ] {
        write(&reports.join(file), PASS);
    }
    write(&reports.join("TEST-malformed.xml"), "<testsuite><testcase>");
    write(
        &reports.join("TEST-other.xml"),
        "<config><testcase/></config>",
    );
    let dirs = [reports.clone()];
    assert_eq!(
        read_junit_reports(&root, &dirs, start)
            .unwrap()
            .unwrap()
            .total,
        2
    );
    assert_eq!(
        read_junit_reports(&root, &dirs, later)
            .unwrap()
            .unwrap()
            .total,
        1
    );
    assert!(
        read_junit_reports(&root, &dirs, later + Duration::from_secs(1))
            .unwrap()
            .is_none()
    );
    assert!(read_junit_reports(&root, &[], start).unwrap().is_none());
    let missing = root.join(GRADLE);
    assert!(
        read_junit_reports(&root, std::slice::from_ref(&missing), start)
            .unwrap()
            .is_none()
    );
    assert!(!missing.exists());
}

#[test]
fn report_access_excludes_other_checkouts_links_and_external_xml_entities() {
    let (temp, root) = repository();
    git(
        &root,
        &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
    );
    git(
        &root,
        &[
            "worktree", "add", "--quiet", "--detach", "--", "nested", "HEAD",
        ],
    );
    let nested = root.join("nested");
    let outside = temp.path().canonicalize().unwrap().join("outside");
    for dir in [&nested, &outside, &root.join(".git")] {
        write(&dir.join(GRADLE).join("other.xml"), PASS);
    }
    let reports = root.join(MAVEN);
    let secret = outside.join("private.txt");
    write(&secret, "outside content");
    let dtd = format!(
        r#"<!DOCTYPE testsuite [<!ENTITY outside SYSTEM "file://{}">]>
        <testsuite><testcase name="external"><error>&outside;</error></testcase></testsuite>"#,
        secret.display()
    );
    write(&reports.join("TEST-dtd.xml"), &dtd);
    write(
        &reports.join("TEST-dtdless.xml"),
        r#"<testsuite xmlns:xi="http://www.w3.org/2001/XInclude">
        <xi:include href="side-effect.xml"/><testcase name="pass"/></testsuite>"#,
    );
    symlink(&outside, root.join("linked")).unwrap();
    symlink(
        outside.join(GRADLE).join("other.xml"),
        reports.join("TEST-linked.xml"),
    )
    .unwrap();
    let before = fs::read(reports.join("TEST-dtd.xml")).unwrap();
    let dirs = [
        reports.clone(),
        outside.join(GRADLE),
        root.join("linked").join(GRADLE),
        nested.join(GRADLE),
        root.join(".git").join(GRADLE),
        root.join("nested/../nested").join(GRADLE),
    ];
    let result = read_junit_reports(&root, &dirs, SystemTime::UNIX_EPOCH)
        .unwrap()
        .unwrap();
    assert_eq!(result.total, 1);
    assert!(result.failures.is_empty());
    assert_eq!(fs::read(reports.join("TEST-dtd.xml")).unwrap(), before);
    assert_eq!(fs::read_to_string(secret).unwrap(), "outside content");
    assert!(!reports.join("side-effect.xml").exists());
}

#[test]
fn root_report_skips_source_links_into_nested_or_external_checkouts() {
    let (temp, root) = repository();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "--quiet", "--template=", "--", "."]);
    let external = temp.path().join("external");
    let module = root.join("child-module");
    write(&module.join("build.gradle"), "");
    for (name, target) in [("Outside", external), ("Nested", nested), ("Child", module)] {
        let source = format!("src/test/java/demo/{name}.java");
        let target = target.join(&source);
        write(&target, "// fixture");
        fs::create_dir_all(root.join("src/test/java/demo")).unwrap();
        symlink(target, root.join(source)).unwrap();
    }
    write(&root.join(JAVA), "// fixture");
    let xml = r#"<testsuite><testcase name="root"><failure>Stack:
        at demo.Outside.go(Outside.java:2)
        at demo.Nested.go(Nested.java:3)
        at demo.Child.go(Child.java:4)
        at demo.OrderTest.go(OrderTest.java:8)
    </failure></testcase></testsuite>"#;
    write(&root.join(GRADLE).join("root.xml"), xml);
    let result = read_junit_reports(&root, &[root.join(GRADLE)], SystemTime::UNIX_EPOCH)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.failures[0].location,
        Some(StackLocation {
            path: root.join(JAVA),
            line: 7,
            column: 0
        })
    );
}

#[test]
fn opened_report_directory_stays_bound_to_the_selected_build() {
    let (temp, root) = repository();
    let reports = root.join(GRADLE);
    let outside = temp.path().join("unrelated");
    let name = OsStr::new("TEST-report.xml");
    write(&reports.join(name), PASS);
    write(&outside.join(name), "<testsuite/>");
    let root_fd = open(&root, dir_flags(), Mode::empty()).unwrap();
    let directory = open_report_directory(&root_fd, Path::new(GRADLE)).unwrap();
    fs::rename(&reports, reports.with_file_name("previous")).unwrap();
    symlink(outside, &reports).unwrap();
    assert_eq!(
        read_fresh_xml(&directory, name, SystemTime::UNIX_EPOCH).as_deref(),
        Some(PASS)
    );
    assert!(
        read_junit_reports(&root, &[reports], SystemTime::UNIX_EPOCH)
            .unwrap()
            .is_none()
    );
}
