use super::ctl::response;
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    before: PathBuf,
    after: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo '한글 space");
        fs::create_dir(&root).unwrap();
        let before = dir.path().join("before");
        let after = dir.path().join("after");
        let this = Self {
            _dir: dir,
            root,
            before,
            after,
        };
        this.git(&["init", "-b", "main", "--template="]);
        for (key, value) in [
            ("user.name", "CLI Fixture"),
            ("user.email", "cli-fixture@example.invalid"),
            ("commit.gpgSign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            this.git(&["config", "--local", key, value]);
        }
        fs::write(this.root.join("file.txt"), "one\nhello world\n").unwrap();
        fs::write(this.root.join("-option.txt"), "literal\n").unwrap();
        this.git(&["add", "--", "file.txt", "-option.txt"]);
        this.git(&["commit", "-m", "initial", "--"]);
        this
    }
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
    fn call(&self, command: &str, rest: &[&str]) -> (Value, i32) {
        let mut args = vec![
            OsString::from(command),
            OsString::from("--repo"),
            self.root.as_os_str().into(),
        ];
        args.extend(rest.iter().map(OsString::from));
        response(&args)
    }
    fn ok(&self, command: &str, rest: &[&str]) -> Value {
        let (value, code) = self.call(command, rest);
        assert_eq!(code, 0, "{value}");
        value["result"].clone()
    }
    fn fail(&self, command: &str, rest: &[&str]) -> Value {
        let (value, code) = self.call(command, rest);
        assert_ne!(code, 0, "{value}");
        assert_eq!(value["ok"], false);
        value
    }
    fn save_args(&self) -> Vec<OsString> {
        vec![
            "write".into(),
            "--repo".into(),
            self.root.clone().into_os_string(),
            "--path".into(),
            "file.txt".into(),
            "--expected-file".into(),
            self.before.clone().into_os_string(),
            "--content-file".into(),
            self.after.clone().into_os_string(),
            "--yes".into(),
        ]
    }
}

#[test]
fn partial_inspection_lists_readable_changes_and_preserves_the_index() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let locked = f.root.join("-option.txt");
    fs::write(f.root.join("file.txt"), "changed and searchable").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&locked).is_ok() {
        return; // Privileged runners can read mode-000 files.
    }
    let index = fs::read(f.root.join(".git/index")).unwrap();
    for (command, flags) in [
        ("status", &[][..]),
        ("files", &["--changed"][..]),
        ("files", &["--changed", "--name-only"][..]),
        ("search", &["--query", "searchable"][..]),
    ] {
        let result = f.ok(command, flags);
        assert_eq!(result["skipped_paths"], serde_json::json!(["-option.txt"]));
        assert!(
            !result
                .get("changes")
                .or_else(|| result.get("files"))
                .or_else(|| result.get("matches"))
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        f.ok("diff", &["--path", "file.txt"])["patch"]
            .as_str()
            .unwrap()
            .contains("+changed and searchable")
    );
    assert!(
        f.fail("diff", &["--path", "-option.txt"])["error"]["message"]
            .as_str()
            .unwrap()
            .contains("permission denied")
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        f.ok("files", &["--changed"])["skipped_paths"],
        serde_json::json!([])
    );
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
}

#[test]
fn routes_reads_without_changing_index_and_scopes_worktrees() {
    let f = Fixture::new();
    let index = fs::read(f.root.join(".git/index")).unwrap();
    fs::write(f.root.join("file.txt"), "two\nhello world\n").unwrap();
    assert_eq!(f.ok("status", &[])["changes"][0]["path"], "file.txt");
    assert_eq!(
        f.ok("read", &["--path", "file.txt"])["text"],
        "two\nhello world\n"
    );
    assert!(
        f.ok("diff", &["--path", "file.txt"])["patch"]
            .as_str()
            .unwrap()
            .contains("+two")
    );
    let changed = f.ok("files", &["--changed"]);
    let changes = changed["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["additions"], 1);
    assert_eq!(changes[0]["deletions"], 1);
    let names = f.ok("files", &["--changed", "--name-only"]);
    assert_eq!(names["files"], serde_json::json!(["file.txt"]));
    assert!(names.get("changes").is_none());
    assert_eq!(f.ok("files", &[])["files"].as_array().unwrap().len(), 2);
    assert!(
        f.ok("branches", &[])["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "main")
    );
    let search = f.ok("search", &["--query", "hello", "--limit", "1"]);
    assert_eq!(search["kind"], "literal_text");
    assert_eq!(search["matches"][0]["line"], 1);
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    let linked = f._dir.path().join("linked");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        linked.to_str().unwrap(),
        "main",
    ]);
    fs::write(linked.join("file.txt"), "linked only\n").unwrap();
    assert_eq!(
        f.ok("worktrees", &[])["worktrees"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (value, code) = response(&[
        "read".into(),
        "--repo".into(),
        linked.into_os_string(),
        "--path".into(),
        "file.txt".into(),
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["result"]["text"], "linked only\n");
    assert_eq!(
        f.ok("read", &["--path", "file.txt"])["text"],
        "two\nhello world\n"
    );
    // A relative --repo is resolved against the CLI's cwd, never the selected GUI.
    let cwd = std::env::current_dir().unwrap();
    let mut relative = PathBuf::new();
    for _ in cwd
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
    {
        relative.push("..");
    }
    relative.push(f.root.strip_prefix(Path::new("/")).unwrap());
    let (value, code) = response(&["status".into(), "--repo".into(), relative.into_os_string()]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        value["result"]["repo"],
        f.root.canonicalize().unwrap().to_str().unwrap()
    );
}

#[test]
fn write_requires_confirmation_exact_expected_and_safe_path() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(&f.before, "one\nhello world\n").unwrap();
    fs::write(&f.after, "replacement\n").unwrap();
    let args = f.save_args();
    let (_, code) = response(&args[..args.len() - 1]);
    assert_ne!(code, 0);
    assert_eq!(
        fs::read_to_string(f.root.join("file.txt")).unwrap(),
        "one\nhello world\n"
    );
    let (value, code) = response(&args);
    assert_eq!(code, 0, "{value}");
    let (value, code) = response(&args);
    assert_ne!(code, 0, "{value}");
    assert_eq!(
        fs::read_to_string(f.root.join("file.txt")).unwrap(),
        "replacement\n"
    );
    for path in ["../before", ".git/config"] {
        let mut bad = args.clone();
        bad[4] = path.into();
        let (value, code) = response(&bad);
        assert_ne!(code, 0, "{value}");
    }
    symlink(&f.before, f.root.join("link.txt")).unwrap();
    let mut bad = args;
    bad[4] = "link.txt".into();
    let (_, code) = response(&bad);
    assert_ne!(code, 0);
    assert_eq!(fs::read_to_string(&f.before).unwrap(), "one\nhello world\n");
    assert!(f.git(&["diff", "--cached", "--name-only", "--"]).is_empty());
}

#[test]
fn named_stage_unstage_commit_and_clean_merge_use_core_guards() {
    let f = Fixture::new();
    f.git(&["branch", "feature"]);
    fs::write(f.root.join("file.txt"), "changed\n").unwrap();
    fs::write(f.root.join("-option.txt"), "other\n").unwrap();
    for (command, args) in [
        ("stage", vec!["--path", "file.txt"]),
        ("unstage", vec!["--path", "file.txt"]),
        ("commit", vec!["--message", "no"]),
        ("merge", vec!["--branch", "feature"]),
    ] {
        f.fail(command, &args);
    }
    f.fail("stage", &["--yes"]);
    f.fail("stage", &["--path", ".", "--yes"]);
    f.ok("stage", &["--path", "file.txt", "--yes"]);
    assert_eq!(
        f.git(&["diff", "--cached", "--name-only", "--"]),
        b"file.txt\n"
    );
    f.ok("unstage", &["--path", "file.txt", "--yes"]);
    assert!(f.git(&["diff", "--cached", "--name-only", "--"]).is_empty());
    assert_eq!(
        fs::read_to_string(f.root.join("file.txt")).unwrap(),
        "changed\n"
    );
    f.ok("stage", &["--path", "file.txt", "--yes"]);
    f.ok("commit", &["--message", "selected only", "--yes"]);
    assert_eq!(
        f.git(&["log", "-1", "--format=%s", "--"]),
        b"selected only\n"
    );
    assert_eq!(f.git(&["show", "HEAD:-option.txt"]), b"literal\n");
    f.fail("merge", &["--branch", "feature", "--yes"]); // dirty worktree
    f.ok("stage", &["--path", "-option.txt", "--yes"]);
    f.ok("commit", &["--message", "other", "--yes"]);
    f.git(&["switch", "feature"]);
    fs::write(f.root.join("feature.txt"), "feature\n").unwrap();
    f.git(&["add", "--", "feature.txt"]);
    f.git(&["commit", "-m", "feature", "--"]);
    f.git(&["switch", "main"]);
    f.ok("merge", &["--branch", "feature", "--yes"]);
    assert_eq!(
        fs::read_to_string(f.root.join("feature.txt")).unwrap(),
        "feature\n"
    );
    assert_eq!(
        f.git(&["rev-list", "--parents", "-1", "HEAD", "--"])
            .split(|b| *b == b' ')
            .count(),
        3
    );
}

#[test]
fn json_help_guides_and_failures_are_unambiguous() {
    for args in [
        vec![],
        vec!["help"],
        vec!["instructions", "--agent", "claude"],
        vec!["instructions", "--agent", "codex"],
    ] {
        let (value, code) = response(&args.into_iter().map(OsString::from).collect::<Vec<_>>());
        assert_eq!(code, 0, "{value}");
        assert_eq!(value["ok"], true);
    }
    for args in [
        vec!["unknown"],
        vec!["status"],
        vec!["instructions", "--agent", "other"],
        vec!["status", "--repo", "/tmp", "--yes"],
        vec!["read", "--repo", "/tmp", "--repo", "/tmp"],
    ] {
        let (value, code) = response(&args.into_iter().map(OsString::from).collect::<Vec<_>>());
        assert_ne!(code, 0, "{value}");
        assert_eq!(value["ok"], false);
        assert!(value["error"]["message"].is_string());
    }
}
