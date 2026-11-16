use super::*;
use std::{
    fs,
    io::Write,
    os::unix::ffi::OsStringExt,
    process::{Command, Stdio},
};
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    global: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repository with spaces");
        fs::create_dir(&root).unwrap();
        let global = dir.path().join("config");
        fs::write(&global, "").unwrap();
        let f = Self {
            _dir: dir,
            root,
            global,
        };
        f.git(&["init", "-b", "main"]);
        f.git(&["config", "user.name", "History Fixture"]);
        f.git(&["config", "user.email", "history@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        f
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .trim_end_matches('\n')
            .to_string()
    }
    fn input(&self, args: &[&str], input: &[u8]) -> String {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .trim_end_matches('\n')
            .to_owned()
    }
    fn commit(&self, path: &str, body: &str) -> String {
        fs::write(self.root.join(path), body).unwrap();
        self.git(&["add", "--", "."]);
        self.git(&["commit", "-m", body]);
        self.git(&["rev-parse", "HEAD"])
    }
}
#[test]
fn history_root_merge_refs_and_stable_pagination() {
    let f = Fixture::new();
    assert!(first_page(&f.root, 20).unwrap().entries.is_empty());
    let root = f.commit("root", "root");
    f.git(&["branch", "side"]);
    let main = f.commit("main", "main");
    f.git(&["switch", "side"]);
    let side = f.commit("side", "side");
    f.git(&["switch", "main"]);
    f.git(&["merge", "--no-ff", "side", "-m", "merge"]);
    let merge = f.git(&["rev-parse", "HEAD"]);
    f.git(&["tag", "-a", "release", "-m", "release"]);
    let page = first_page(&f.root, 1).unwrap();
    assert_eq!(page.entries[0].commit.oid, merge);
    assert_eq!(page.entries[0].commit.parents, [main, side]);
    assert!(
        page.entries[0]
            .commit
            .refs
            .contains(&"refs/tags/release".into())
    );
    assert_eq!(page.entries[0].graph.edges, [(0, 0), (0, 1)]);
    let all_before = first_page(&f.root, 200).unwrap();
    f.commit("later", "later"); // Pinned cursor must not shift when HEAD moves.
    let mut rows = page.entries.clone();
    let mut next = page.next;
    while let Some(cursor) = next {
        let p = next_page(&f.root, &cursor, 1).unwrap();
        rows.extend(p.entries);
        next = p.next;
    }
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter().map(|e| &e.commit.oid).collect::<Vec<_>>(),
        all_before
            .entries
            .iter()
            .map(|e| &e.commit.oid)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rows.iter().map(|e| &e.graph).collect::<Vec<_>>(),
        all_before
            .entries
            .iter()
            .map(|e| &e.graph)
            .collect::<Vec<_>>()
    );
    assert_eq!(rows.last().unwrap().commit.oid, root);
    assert!(rows.last().unwrap().graph.edges.is_empty());
    let d = commit_detail(&f.root, &merge, 0).unwrap();
    assert_eq!(d.files[0].path, Path::new("side"));
    let d = commit_detail(&f.root, &merge, 1).unwrap();
    assert_eq!(d.files[0].path, Path::new("main"));
    assert!(commit_detail(&f.root, &merge, 2).is_err());
    let d = commit_detail(&f.root, &root, 0).unwrap();
    assert!(d.parent.is_none());
    assert_eq!(
        historical_diff(&f.root, &d, Path::new("root"))
            .unwrap()
            .new_text,
        "root"
    );
}
#[test]
fn history_literal_paths_and_working_index_are_untouched() {
    let f = Fixture::new();
    let paths = [
        PathBuf::from(":(glob)*.rs"),
        PathBuf::from("-option\tline\nname"),
        PathBuf::from("한글 공백.rs"),
    ];
    for p in &paths {
        fs::write(f.root.join(p), "old\n").unwrap();
    }
    f.git(&["add", "--", "."]);
    f.git(&["commit", "-m", "unusual paths"]);
    let oid = f.git(&["rev-parse", "HEAD"]);
    for p in &paths {
        fs::write(f.root.join(p), "working only\n").unwrap();
    }
    f.git(&["add", "--", "."]);
    let index = fs::read(f.root.join(".git/index")).unwrap();
    let detail = commit_detail(&f.root, &oid, 0).unwrap();
    assert_eq!(detail.files.len(), 3);
    for path in &paths {
        let diff = historical_diff(&f.root, &detail, path).unwrap();
        assert_eq!(diff.new_text, "old\n");
        assert!(diff.old_text.is_empty());
    }
    assert!(historical_diff(&f.root, &detail, Path::new("../x")).is_err());
    assert!(commit_detail(&f.root, "--all", 0).is_err());
    first_page(&f.root, 20).unwrap();
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    for p in paths {
        assert_eq!(
            fs::read_to_string(f.root.join(p)).unwrap(),
            "working only\n"
        );
    }
    let other = Fixture::new();
    assert!(historical_diff(&other.root, &detail, Path::new(":(glob)*.rs")).is_err());
}
#[test]
fn history_limits_and_wrong_worktree_cursor() {
    let f = Fixture::new();
    f.commit("file", "base");
    f.commit("file", "second");
    let p = first_page(&f.root, 0).unwrap();
    assert_eq!(p.entries.len(), 1);
    let other = Fixture::new();
    assert!(next_page(&other.root, p.next.as_ref().unwrap(), 1).is_err());
    let mut cursor = p.next.unwrap();
    // Reach the cap with a one-row page while retaining a real second row.
    cursor.offset = 0;
    let p = next_page(&f.root, &cursor, usize::MAX).unwrap();
    assert_eq!(p.entries.len(), 2);
    cursor.offset = HISTORY_LIMIT;
    assert!(next_page(&f.root, &cursor, 1).unwrap().entries.is_empty());
}
#[test]
fn history_binary_large_and_mode_changes_are_explicit() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let base = f.commit("script", "hello\n");
    fs::set_permissions(f.root.join("script"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(f.root.join("binary"), [0, 255]).unwrap();
    fs::write(f.root.join("large"), vec![b'a'; crate::MAX_TEXT_BYTES + 10]).unwrap();
    f.git(&["add", "--", "."]);
    f.git(&["commit", "-m", "binary and mode"]);
    let d = commit_detail(&f.root, &f.git(&["rev-parse", "HEAD"]), 0).unwrap();
    assert_eq!(d.parent.as_ref(), Some(&base));
    assert!(
        historical_diff(&f.root, &d, Path::new("binary"))
            .unwrap()
            .binary
    );
    assert!(
        historical_diff(&f.root, &d, Path::new("large"))
            .unwrap()
            .truncated
    );
    assert!(
        historical_diff(&f.root, &d, Path::new("script"))
            .unwrap()
            .patch
            .contains("100755")
    );
}

#[test]
fn history_reads_non_utf8_tree_names_without_materializing_them() {
    let f = Fixture::new();
    let blob = f.input(&["hash-object", "-w", "--stdin"], b"object only\n");
    let mut entry = format!("100644 blob {blob}\t").into_bytes();
    entry.extend_from_slice(b"non-utf8-\xff\0");
    let tree = f.input(&["mktree", "-z"], &entry);
    let commit = f.input(&["commit-tree", &tree], b"object fixture\n");
    f.git(&["update-ref", "refs/heads/main", &commit]);
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"non-utf8-\xff".to_vec()));
    let detail = commit_detail(&f.root, &commit, 0).unwrap();
    assert_eq!(detail.files[0].path, path);
    assert_eq!(
        historical_diff(&f.root, &detail, &path).unwrap().new_text,
        "object only\n"
    );
    assert!(!f.root.join(".git/index").exists());
}
#[test]
fn history_real_fixture_enforces_page_and_total_limits() {
    let f = Fixture::new();
    let mut input = String::new();
    for i in 0..=HISTORY_LIMIT {
        input.push_str(&format!("commit refs/heads/main\ncommitter Fixture <fixture@example.invalid> {} +0000\ndata 1\nx\n\n",i+1));
    }
    f.input(&["fast-import", "--quiet"], input.as_bytes());
    let mut page = first_page(&f.root, usize::MAX).unwrap();
    assert_eq!(page.entries.len(), PAGE_LIMIT);
    let mut count = page.entries.len();
    while let Some(cursor) = page.next {
        page = next_page(&f.root, &cursor, usize::MAX).unwrap();
        assert!(page.entries.len() <= PAGE_LIMIT);
        count += page.entries.len();
    }
    assert_eq!(count, HISTORY_LIMIT);
    assert!(page.limit_reached);
    assert_eq!(page.loaded, HISTORY_LIMIT);
}
