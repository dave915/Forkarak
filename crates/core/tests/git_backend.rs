use pokarak_core::{
    ChangedFile, CompareMode, DiffKind, FileStatus, MAX_TEXT_BYTES, diff, read_file,
    repository_root, search, snapshot,
};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    io::{Seek, SeekFrom, Write},
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

struct Repo {
    temp: TempDir,
    root: PathBuf,
}
impl Repo {
    fn new() -> Self {
        Self::with_format("sha1")
    }
    fn with_format(format: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repository 한 with spaces\n");
        fs::create_dir(&root).unwrap();
        let repo = Self { temp, root };
        repo.git(&["init", "-b", "main", &format!("--object-format={format}")]);
        repo.git(&["config", "user.name", "Fixture"]);
        repo.git(&["config", "user.email", "fixture@example.invalid"]);
        repo.git(&["config", "commit.gpgSign", "false"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        String::from_utf8(self.git_os(&args.iter().map(OsStr::new).collect::<Vec<_>>())).unwrap()
    }
    fn git_os(&self, args: &[&OsStr]) -> Vec<u8> {
        git_at(&self.root, args)
    }
    fn write(&self, path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn add(&self, path: impl AsRef<Path>) {
        self.git_os(&["add".as_ref(), "--".as_ref(), path.as_ref().as_os_str()]);
    }
    fn commit(&self) {
        self.git(&["commit", "-m", "fixture", "--no-verify"]);
    }
    fn seed(&self) {
        self.write("src/code.rs", "first\nsecond\nthird\n");
        self.add("src/code.rs");
        self.commit();
    }
    fn snap(&self) -> pokarak_core::Snapshot {
        snapshot(&self.root, &CompareMode::Uncommitted).unwrap()
    }
}

fn git_at(root: &Path, args: &[&OsStr]) -> Vec<u8> {
    let mut command = Command::new("git");
    // Isolate fixture configuration and inherited worktree routing, including
    // hooks/signing/ignore/filter settings from the developer's repositories.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["--literal-pathspecs", "-C"])
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn change(snapshot: &pokarak_core::Snapshot, path: impl AsRef<Path>) -> &ChangedFile {
    snapshot
        .changes
        .iter()
        .find(|f| f.path == path.as_ref())
        .unwrap_or_else(|| panic!("missing {:?}: {:?}", path.as_ref(), snapshot.changes))
}

#[test]
fn unborn_repository_root_and_inventory() {
    let repo = Repo::new();
    assert_eq!(
        repository_root(&repo.root).unwrap(),
        repo.root.canonicalize().unwrap()
    );
    assert!(repo.snap().changes.is_empty());
    repo.write("src/staged.rs", "staged\n");
    repo.add("src/staged.rs");
    repo.write("fresh.rs", "fresh\n");
    let snap = repo.snap();
    assert!(snap.worktree.head.is_empty());
    assert_eq!(snap.worktree.branch, "main");
    assert_eq!(snap.worktree.changed_files, 2);
    assert_eq!(change(&snap, "src/staged.rs").status, FileStatus::Added);
    assert!(change(&snap, "src/staged.rs").staged);
    assert!(!change(&snap, "src/staged.rs").unstaged);
    assert_eq!(change(&snap, "fresh.rs").status, FileStatus::Untracked);
    let doc = diff(&repo.root, change(&snap, "fresh.rs"), &snap.compare).unwrap();
    assert!(doc.old_text.is_empty());
    assert_eq!(doc.new_text, "fresh\n");
    assert_eq!(
        doc.lines
            .iter()
            .find(|l| l.kind == DiffKind::Addition)
            .unwrap()
            .new_line,
        Some(1)
    );
    assert_eq!(
        repository_root(&repo.root.join("src/staged.rs")).unwrap(),
        snap.worktree.path
    );
    assert_eq!(
        repository_root(&repo.root.join("src")).unwrap(),
        snap.worktree.path
    );
}

#[test]
fn combines_staged_and_unstaged_against_head() {
    let repo = Repo::new();
    repo.seed();
    repo.write("src/code.rs", "staged\nsecond\nthird\n");
    repo.add("src/code.rs");
    repo.write("src/code.rs", "staged\nworking\nthird\n");
    let snap = repo.snap();
    let file = change(&snap, "src/code.rs");
    assert!(file.staged && file.unstaged);
    assert_eq!((file.additions, file.deletions), (2, 2));
    let doc = diff(&repo.root, file, &snap.compare).unwrap();
    assert_eq!(doc.old_text, "first\nsecond\nthird\n");
    assert_eq!(doc.new_text, "staged\nworking\nthird\n");
    assert!(doc.patch.contains("@@ -1,3 +1,3 @@"));
    let deleted = doc
        .lines
        .iter()
        .filter(|l| l.kind == DiffKind::Deletion)
        .collect::<Vec<_>>();
    assert_eq!(
        deleted.iter().map(|l| l.old_line).collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
    let context = doc.lines.iter().find(|l| l.text == "third").unwrap();
    assert_eq!((context.old_line, context.new_line), (Some(3), Some(3)));
    assert_eq!(
        read_file(&repo.root, Path::new("src/code.rs")).unwrap(),
        doc.new_text
    );
}

#[test]
fn index_changes_remain_visible_when_working_bytes_return_to_head() {
    let repo = Repo::new();
    repo.seed();
    repo.write("src/code.rs", "staged\n");
    repo.add("src/code.rs");
    repo.write("src/code.rs", "first\nsecond\nthird\n");
    let snap = repo.snap();
    let file = change(&snap, "src/code.rs");
    assert!(file.staged && file.unstaged);
    assert_eq!((file.additions, file.deletions), (0, 0));
    repo.git(&["rm", "--cached", "-f", "--", "src/code.rs"]);
    let snap = repo.snap();
    assert!(change(&snap, "src/code.rs").staged && change(&snap, "src/code.rs").unstaged);
    assert_eq!(
        diff(&repo.root, change(&snap, "src/code.rs"), &snap.compare)
            .unwrap()
            .old_text,
        "first\nsecond\nthird\n"
    );
}

#[test]
fn branch_uses_merge_base_and_includes_committed_staged_and_working_changes() {
    let repo = Repo::new();
    repo.seed();
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("src/code.rs", "committed\nsecond\nthird\n");
    repo.add("src/code.rs");
    repo.commit();
    repo.git(&["checkout", "main"]);
    repo.write("src/code.rs", "base advanced\nsecond\nthird\n");
    repo.add("src/code.rs");
    repo.commit();
    repo.git(&["checkout", "feature"]);
    repo.write("src/code.rs", "committed\nstaged\nthird\n");
    repo.add("src/code.rs");
    repo.write("src/code.rs", "committed\nstaged\nworking\n");
    repo.write("new.rs", "untracked\n");
    let mode = CompareMode::Branch("main".into());
    let snap = snapshot(&repo.root, &mode).unwrap();
    let doc = diff(&repo.root, change(&snap, "src/code.rs"), &mode).unwrap();
    assert_eq!(doc.old_text, "first\nsecond\nthird\n");
    assert_eq!(doc.new_text, "committed\nstaged\nworking\n");
    assert_eq!(
        (
            change(&snap, "src/code.rs").additions,
            change(&snap, "src/code.rs").deletions
        ),
        (3, 3)
    );
    assert_eq!(change(&snap, "new.rs").status, FileStatus::Untracked);
    assert!(snap.branches.contains(&"main".into()) && snap.branches.contains(&"feature".into()));
}

#[test]
fn branch_counts_are_separate_from_dirty_worktree_counts() {
    let repo = Repo::new();
    repo.seed();
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("committed.rs", "new\n");
    repo.add("committed.rs");
    repo.commit();
    let snap = snapshot(&repo.root, &CompareMode::Branch("main".into())).unwrap();
    assert_eq!(snap.changes.len(), 1);
    assert_eq!(snap.worktree.changed_files, 0);
    let file = change(&snap, "committed.rs");
    assert_eq!(file.status, FileStatus::Added);
    assert!(!file.staged && !file.unstaged);
}

#[test]
fn graph_summary_distinguishes_tracked_changes_from_ignored_build_output() {
    let repo = Repo::new();
    repo.write(".gitignore", "**/build/\n.gradle/\n");
    repo.write("module/build/keep.kt", "tracked build input");
    repo.git(&["add", "-f", "--", "module/build/keep.kt"]);
    repo.add(".gitignore");
    repo.commit();
    let summary = || pokarak_core::worktree_change_summary(&repo.root).unwrap();
    let clean = summary();
    assert_eq!(clean.0, 0);
    repo.write("module/build/classes/Demo.class", "output");
    repo.write(".gradle/caches/item", "cache");
    assert_eq!(summary(), clean);
    repo.write("module/build/keep.kt", "changed once");
    let edited = summary();
    assert_eq!(edited.0, 1);
    repo.write("module/build/keep.kt", "changed twice");
    let edited_again = summary();
    assert_eq!(edited_again.0, 1);
    assert_ne!(edited_again.1, edited.1);
    repo.git(&["branch", "another"]);
    assert_ne!(summary().1, edited_again.1);
}

#[test]
fn paths_are_lossless_literal_and_nul_delimited() {
    let repo = Repo::new();
    let names = [
        "space name.rs",
        "한글😀.rs",
        "line\nbreak\t.rs",
        "-option.rs",
        ":(glob)*.rs",
        "quote\"slash\\.rs",
    ];
    for name in names {
        repo.write(name, "old\n");
        repo.add(name);
    }
    // macOS filesystems reject non-UTF-8 names. Exercise them on platforms that
    // support them, while keeping the required unicode/newline cases universal.
    let non_utf8 = if cfg!(target_os = "macos") {
        None
    } else {
        Some(PathBuf::from(OsString::from_vec(
            b"invalid-\xff.rs".to_vec(),
        )))
    };
    if let Some(path) = &non_utf8 {
        repo.write(path, "old\n");
        repo.add(path);
    }
    repo.commit();
    for name in names {
        repo.write(name, "new\n");
    }
    if let Some(path) = &non_utf8 {
        repo.write(path, "new\n");
    }
    let snap = repo.snap();
    assert_eq!(
        snap.changes.len(),
        names.len() + usize::from(non_utf8.is_some())
    );
    for name in names.iter().map(PathBuf::from).chain(non_utf8) {
        assert!(snap.files.contains(&name));
        let doc = diff(&repo.root, change(&snap, &name), &snap.compare).unwrap();
        assert_eq!(doc.old_text, "old\n");
        assert_eq!(doc.new_text, "new\n");
        assert_eq!(
            read_file(&repo.root, &repo.root.canonicalize().unwrap().join(&name)).unwrap(),
            "new\n"
        );
    }
}

#[test]
fn staged_rename_preserves_old_contents_after_unstaged_edits() {
    let repo = Repo::new();
    let old = "old\n한.rs";
    let new = "new space\n😀.rs";
    repo.write(old, "one\ntwo\nthree\n");
    repo.add(old);
    repo.commit();
    repo.git(&["mv", "--", old, new]);
    repo.write(new, "changed entirely\n");
    let snap = repo.snap();
    assert_eq!(snap.changes.len(), 1);
    let file = change(&snap, new);
    assert_eq!(file.status, FileStatus::Renamed);
    assert_eq!(file.old_path.as_deref(), Some(Path::new(old)));
    assert!(file.staged && file.unstaged);
    let doc = diff(&repo.root, file, &snap.compare).unwrap();
    assert_eq!(doc.old_text, "one\ntwo\nthree\n");
    assert_eq!(doc.new_text, "changed entirely\n");
}

#[test]
fn committed_edited_and_unstaged_renames_are_detected() {
    let repo = Repo::new();
    repo.seed();
    repo.git(&["checkout", "-b", "feature"]);
    repo.git(&["mv", "--", "src/code.rs", "renamed.rs"]);
    repo.write("renamed.rs", "first\nsecond\nTHIRD\n");
    repo.add("renamed.rs");
    repo.commit();
    let snap = snapshot(&repo.root, &CompareMode::Branch("main".into())).unwrap();
    assert_eq!(snap.changes.len(), 1);
    assert_eq!(change(&snap, "renamed.rs").status, FileStatus::Renamed);
    assert_eq!(
        diff(&repo.root, change(&snap, "renamed.rs"), &snap.compare)
            .unwrap()
            .old_text,
        "first\nsecond\nthird\n"
    );
    fs::rename(repo.root.join("renamed.rs"), repo.root.join("again.rs")).unwrap();
    let snap = repo.snap();
    let file = change(&snap, "again.rs");
    assert_eq!(file.status, FileStatus::Renamed);
    assert!(!file.staged && file.unstaged);
}

#[test]
fn deleted_untracked_binary_and_non_utf8_contents() {
    let repo = Repo::new();
    repo.seed();
    repo.write("binary.dat", b"\0old\xff");
    repo.add("binary.dat");
    repo.commit();
    fs::remove_file(repo.root.join("src/code.rs")).unwrap();
    repo.write("binary.dat", b"\0new\xff");
    repo.write("new-binary.dat", b"hello\0world");
    repo.write("legacy.dat", b"\xff\xfe");
    repo.write("fresh.rs", "no newline");
    let snap = repo.snap();
    let deleted = change(&snap, "src/code.rs");
    assert_eq!(deleted.status, FileStatus::Deleted);
    assert_eq!((deleted.additions, deleted.deletions), (0, 3));
    let doc = diff(&repo.root, deleted, &snap.compare).unwrap();
    assert_eq!(doc.old_text, "first\nsecond\nthird\n");
    assert!(doc.new_text.is_empty());
    assert!(!snap.files.contains(&PathBuf::from("src/code.rs")));
    for name in ["binary.dat", "new-binary.dat", "legacy.dat"] {
        let file = change(&snap, name);
        assert!(file.binary);
        let doc = diff(&repo.root, file, &snap.compare).unwrap();
        assert!(doc.binary && doc.old_text.is_empty() && doc.new_text.is_empty());
        assert!(read_file(&repo.root, Path::new(name)).is_err());
    }
    let doc = diff(&repo.root, change(&snap, "fresh.rs"), &snap.compare).unwrap();
    assert!(doc.patch.contains("\\ No newline at end of file"));
    assert_eq!(
        doc.lines
            .iter()
            .find(|l| l.kind == DiffKind::Addition)
            .unwrap()
            .new_line,
        Some(1)
    );
    fs::remove_file(repo.root.join("binary.dat")).unwrap();
    assert!(
        diff(
            &repo.root,
            change(&repo.snap(), "binary.dat"),
            &CompareMode::Uncommitted
        )
        .unwrap()
        .binary
    );
}

#[test]
fn lists_tracked_and_nonignored_files_without_extension_whitelists() {
    let repo = Repo::new();
    repo.seed();
    repo.write("tracked.ignore", "tracked\n");
    repo.add("tracked.ignore");
    repo.commit();
    repo.write(".gitignore", "*.ignore\nbuild/\n");
    repo.write("untracked.ignore", "ignored\n");
    repo.write("build/output.rs", "ignored\n");
    repo.write("nested/source.unknown", "visible\n");
    repo.write(".config", "visible\n");
    let snap = repo.snap();
    for name in [
        "tracked.ignore",
        "nested/source.unknown",
        ".config",
        ".gitignore",
        "src/code.rs",
    ] {
        assert!(snap.files.contains(&PathBuf::from(name)));
    }
    assert!(!snap.files.contains(&PathBuf::from("untracked.ignore")));
    assert!(
        !snap
            .files
            .iter()
            .any(|p| p.starts_with("build") || p.starts_with(".git"))
    );
}

#[test]
fn search_locations_are_absolute_zero_based_utf16_and_bounded() {
    let repo = Repo::new();
    repo.seed();
    repo.write("search.rs", "irrelevant\n한😀 needle needle\nNEEDLE\n");
    repo.write(".gitignore", "hidden.rs\n");
    repo.write("hidden.rs", "needle\n");
    repo.write("binary.dat", b"needle\0");
    symlink("search.rs", repo.root.join("link.rs")).unwrap();
    let hits = search(&repo.root, "needle", 20).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].path,
        repo.root.canonicalize().unwrap().join("search.rs")
    );
    assert_eq!((hits[0].line, hits[0].column), (1, 4));
    assert_eq!((hits[1].line, hits[1].column), (1, 11));
    assert!(hits[0].preview.contains("한😀 needle"));
    assert_eq!(search(&repo.root, "needle", 1).unwrap().len(), 1);
    assert!(search(&repo.root, "needle", 0).unwrap().is_empty());
    assert!(search(&repo.root, "", 10).unwrap().is_empty());
    assert!(search(&repo.root, "never found", 10).unwrap().is_empty());
}

#[test]
fn worktree_inventory_handles_linked_detached_locked_and_prunable() {
    let repo = Repo::new();
    repo.seed();
    let linked = repo.temp.path().join("linked\n작업 tree");
    repo.git_os(&[
        "worktree".as_ref(),
        "add".as_ref(),
        "-b".as_ref(),
        "linked".as_ref(),
        "--".as_ref(),
        linked.as_os_str(),
    ]);
    let detached = repo.temp.path().join("detached");
    repo.git_os(&[
        "worktree".as_ref(),
        "add".as_ref(),
        "--detach".as_ref(),
        "--".as_ref(),
        detached.as_os_str(),
        "HEAD".as_ref(),
    ]);
    repo.git_os(&[
        "worktree".as_ref(),
        "lock".as_ref(),
        "--reason".as_ref(),
        "test\nreason".as_ref(),
        "--".as_ref(),
        linked.as_os_str(),
    ]);
    fs::write(linked.join("untracked.rs"), "linked\n").unwrap();
    fs::write(detached.join("src/code.rs"), "detached\n").unwrap();
    let inventory = pokarak_core::worktree_inventory(&linked).unwrap();
    assert_eq!(inventory.len(), 3);
    assert!(
        inventory
            .iter()
            .any(|tree| tree.is_locked && tree.branch == "linked")
    );
    assert_eq!(pokarak_core::worktree_change_count(&repo.root).unwrap(), 0);
    assert_eq!(pokarak_core::worktree_change_count(&linked).unwrap(), 1);
    assert_eq!(pokarak_core::worktree_change_count(&detached).unwrap(), 1);
    let snap = snapshot(&linked, &CompareMode::Uncommitted).unwrap();
    assert_eq!(snap.worktrees.len(), 3);
    assert!(!snap.worktree.is_main);
    assert!(snap.worktree.is_locked);
    assert_eq!(snap.worktree.branch, "linked");
    assert_eq!(snap.worktree.changed_files, 1);
    assert_eq!(
        repository_root(&linked.join("src")).unwrap(),
        linked.canonicalize().unwrap()
    );
    assert!(
        snap.worktrees
            .iter()
            .any(|w| w.is_main && w.changed_files == 0)
    );
    assert!(
        snap.worktrees
            .iter()
            .any(|w| w.is_detached && w.changed_files == 1)
    );
    assert!(!repo.snap().files.contains(&PathBuf::from("untracked.rs")));
    let detached_snap = snapshot(&detached, &CompareMode::Uncommitted).unwrap();
    assert!(detached_snap.worktree.is_detached);
    assert_eq!(
        diff(
            &detached,
            change(&detached_snap, "src/code.rs"),
            &detached_snap.compare
        )
        .unwrap()
        .new_text,
        "detached\n"
    );
    let detached_canonical = detached.canonicalize().unwrap();
    fs::remove_dir_all(&detached).unwrap();
    assert!(
        repo.snap()
            .worktrees
            .iter()
            .any(|w| w.is_prunable && w.path == detached_canonical)
    );
}

#[test]
fn invalid_and_unrelated_bases_have_actionable_errors() {
    let repo = Repo::new();
    repo.seed();
    for base in ["missing", "--help", "main; touch injected", ""] {
        let error = snapshot(&repo.root, &CompareMode::Branch(base.into()))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("choose") || error.contains("Choose"),
            "{error}"
        );
    }
    repo.git(&["checkout", "--orphan", "unrelated"]);
    repo.git(&["rm", "-r", "-f", "--", "."]);
    repo.write("other.rs", "unrelated\n");
    repo.add("other.rs");
    repo.commit();
    let error = snapshot(&repo.root, &CompareMode::Branch("main".into()))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("No merge base") && error.contains("related"),
        "{error}"
    );
    let outside = tempfile::tempdir().unwrap();
    assert!(
        repository_root(outside.path())
            .unwrap_err()
            .to_string()
            .contains("select a repository")
    );
}

#[test]
fn symlinks_traversal_and_special_files_never_expose_targets() {
    let repo = Repo::new();
    repo.seed();
    let outside = repo.temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.rs"), "DO NOT READ\n").unwrap();
    symlink(outside.join("secret.rs"), repo.root.join("link.rs")).unwrap();
    symlink(&outside, repo.root.join("directory-link")).unwrap();
    symlink("missing-target", repo.root.join("broken-link")).unwrap();
    assert_eq!(
        read_file(&repo.root, Path::new("link.rs")).unwrap(),
        outside.join("secret.rs").to_str().unwrap()
    );
    assert_eq!(
        read_file(&repo.root, Path::new("broken-link")).unwrap(),
        "missing-target"
    );
    for path in [
        PathBuf::from("../outside/secret.rs"),
        outside.join("secret.rs"),
        PathBuf::from("directory-link/secret.rs"),
        PathBuf::from(".git/config"),
        PathBuf::from("src/../../outside/secret.rs"),
    ] {
        assert!(read_file(&repo.root, &path).is_err(), "{path:?}");
        assert!(
            diff(
                &repo.root,
                &ChangedFile {
                    path,
                    ..Default::default()
                },
                &CompareMode::Uncommitted
            )
            .is_err()
        );
    }
    let bad_old = ChangedFile {
        path: "src/code.rs".into(),
        old_path: Some("../secret".into()),
        ..Default::default()
    };
    assert!(diff(&repo.root, &bad_old, &CompareMode::Uncommitted).is_err());
    assert!(search(&repo.root, "DO NOT READ", 20).unwrap().is_empty());
    repo.add("link.rs");
    repo.commit();
    fs::remove_file(repo.root.join("link.rs")).unwrap();
    symlink("different-target", repo.root.join("link.rs")).unwrap();
    let snap = repo.snap();
    let doc = diff(&repo.root, change(&snap, "link.rs"), &snap.compare).unwrap();
    assert_eq!(doc.new_text, "different-target");
    assert!(!doc.old_text.contains("DO NOT READ"));
    fs::remove_dir_all(repo.root.join("src")).unwrap();
    symlink(&outside, repo.root.join("src")).unwrap();
    assert_eq!(
        change(&repo.snap(), "src/code.rs").status,
        FileStatus::Deleted
    );
    let fifo = repo.root.join("pipe");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: the NUL-terminated name is valid throughout this fixture-only call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(
        read_file(&repo.root, Path::new("pipe"))
            .unwrap_err()
            .to_string()
            .contains("special files")
    );
}

#[test]
fn unborn_branch_comparison_requests_uncommitted_mode() {
    let repo = Repo::new();
    repo.seed();
    repo.git(&["checkout", "--orphan", "not-yet-committed"]);
    assert!(repo.snap().worktree.head.is_empty());
    let error = snapshot(&repo.root, &CompareMode::Branch("main".into()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("choose Uncommitted"), "{error}");
}

#[test]
fn multiple_hunks_keep_correct_line_numbers_and_line_limit_is_bounded() {
    let repo = Repo::new();
    let original = (1..=30).map(|n| format!("line {n}\n")).collect::<String>();
    repo.write("many.rs", &original);
    repo.add("many.rs");
    repo.commit();
    repo.write(
        "many.rs",
        original
            .replace("line 2\n", "inserted\nline 2\n")
            .replace("line 25\n", ""),
    );
    let snap = repo.snap();
    let doc = diff(&repo.root, change(&snap, "many.rs"), &snap.compare).unwrap();
    assert_eq!(
        doc.lines
            .iter()
            .filter(|line| line.text.starts_with("@@ "))
            .count(),
        2
    );
    assert_eq!(
        doc.lines
            .iter()
            .find(|line| line.kind == DiffKind::Addition)
            .unwrap()
            .new_line,
        Some(2)
    );
    assert_eq!(
        doc.lines
            .iter()
            .find(|line| line.kind == DiffKind::Deletion)
            .unwrap()
            .old_line,
        Some(25)
    );
    repo.write("many.rs", "\n".repeat(50_001));
    let doc = diff(&repo.root, change(&snap, "many.rs"), &snap.compare).unwrap();
    assert!(doc.truncated && doc.lines.len() == 1);
}

#[test]
fn empty_added_then_deleted_and_empty_untracked_files_are_handled() {
    let repo = Repo::new();
    repo.seed();
    repo.write("empty.rs", "");
    repo.add("empty.rs");
    fs::remove_file(repo.root.join("empty.rs")).unwrap();
    let snap = repo.snap();
    let file = change(&snap, "empty.rs");
    assert!(file.staged && file.unstaged);
    let doc = diff(&repo.root, file, &snap.compare).unwrap();
    assert!(doc.old_text.is_empty() && doc.new_text.is_empty());
    repo.write("untracked.rs", "");
    let snap = repo.snap();
    assert_eq!(change(&snap, "untracked.rs").status, FileStatus::Untracked);
}

#[test]
fn large_files_are_bounded_and_fingerprint_hashes_past_preview() {
    let repo = Repo::new();
    let file_path = repo.root.join("huge.txt");
    let mut file = fs::File::create(&file_path).unwrap();
    for _ in 0..(MAX_TEXT_BYTES / 4096 + 1) {
        file.write_all(&[b'x'; 4096]).unwrap();
    }
    file.sync_all().unwrap();
    repo.add("huge.txt");
    repo.commit();
    file.seek(SeekFrom::Start(MAX_TEXT_BYTES as u64 + 7))
        .unwrap();
    file.write_all(b"y").unwrap();
    file.sync_all().unwrap();
    let first = repo.snap();
    assert!(
        read_file(&repo.root, Path::new("huge.txt"))
            .unwrap_err()
            .to_string()
            .contains("2 MiB")
    );
    let doc = diff(&repo.root, change(&first, "huge.txt"), &first.compare).unwrap();
    assert!(doc.truncated);
    assert!(doc.old_text.len() <= MAX_TEXT_BYTES && doc.new_text.len() <= MAX_TEXT_BYTES);
    assert!(doc.patch.contains("omitted"));
    assert!(search(&repo.root, "xxxx", 2).unwrap().is_empty());
    file.seek(SeekFrom::Start(MAX_TEXT_BYTES as u64 + 7))
        .unwrap();
    file.write_all(b"z").unwrap();
    file.sync_all().unwrap();
    assert_ne!(first.fingerprint, repo.snap().fingerprint);
}

#[test]
fn fingerprint_changes_for_same_length_edits_index_and_untracked_content() {
    let repo = Repo::new();
    repo.seed();
    let initial = repo.snap().fingerprint;
    assert_eq!(initial, repo.snap().fingerprint);
    repo.write("src/code.rs", "FIRST\nsecond\nthird\n");
    let dirty = repo.snap().fingerprint;
    assert_ne!(initial, dirty);
    repo.write("src/code.rs", "OTHER\nsecond\nthird\n");
    let edited = repo.snap().fingerprint;
    assert_ne!(dirty, edited);
    repo.add("src/code.rs");
    assert_ne!(edited, repo.snap().fingerprint);
    repo.write("new.rs", "one\n");
    let new = repo.snap().fingerprint;
    repo.write("new.rs", "two\n");
    assert_ne!(new, repo.snap().fingerprint);
}

#[test]
fn sha256_repository_and_executable_mode_changes() {
    let repo = Repo::with_format("sha256");
    repo.seed();
    assert!(repo.snap().changes.is_empty());
    let path = repo.root.join("src/code.rs");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let snap = repo.snap();
    assert_eq!(snap.changes.len(), 1);
    assert!(change(&snap, "src/code.rs").unstaged);
    repo.git(&["config", "core.filemode", "false"]);
    assert!(repo.snap().changes.is_empty());
    repo.write("src/code.rs", "modified\n");
    assert_eq!(repo.snap().changes.len(), 1);
}

#[test]
fn merge_conflicts_have_flags_and_readable_working_content() {
    let repo = Repo::new();
    repo.seed();
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("src/code.rs", "feature\n");
    repo.add("src/code.rs");
    repo.commit();
    repo.git(&["checkout", "main"]);
    repo.write("src/code.rs", "main\n");
    repo.add("src/code.rs");
    repo.commit();
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args(["merge", "--no-edit", "feature"])
        .output()
        .unwrap();
    assert!(!status.status.success());
    let snap = repo.snap();
    let file = change(&snap, "src/code.rs");
    assert_eq!(file.status, FileStatus::Conflicted);
    assert!(file.staged && file.unstaged);
    let doc = diff(&repo.root, file, &snap.compare).unwrap();
    assert_eq!(doc.old_text, "main\n");
    assert!(doc.new_text.contains("<<<<<<<"));
}

fn contents(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, result);
            } else if entry.file_type().unwrap().is_file() {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(directory, directory, &mut result);
    result
}

#[test]
fn backend_does_not_mutate_repository_or_execute_configured_filters() {
    let repo = Repo::new();
    repo.seed();
    repo.write(".gitattributes", "*.rs filter=forbidden diff=forbidden\n");
    repo.git(&["config", "filter.forbidden.clean", "false"]);
    repo.git(&["config", "filter.forbidden.required", "true"]);
    repo.git(&["config", "diff.forbidden.textconv", "false"]);
    repo.git(&["config", "diff.external", "false"]);
    repo.git(&["config", "core.fsmonitor", "false"]);
    repo.write("src/code.rs", "changed\n");
    let before = contents(&repo.root);
    let index_modified = fs::metadata(repo.root.join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    let snap = repo.snap();
    let doc = diff(&repo.root, change(&snap, "src/code.rs"), &snap.compare).unwrap();
    assert_eq!(doc.new_text, "changed\n");
    assert_eq!(
        read_file(&repo.root, Path::new("src/code.rs")).unwrap(),
        "changed\n"
    );
    assert_eq!(search(&repo.root, "changed", 10).unwrap().len(), 1);
    assert_eq!(before, contents(&repo.root));
    assert_eq!(
        index_modified,
        fs::metadata(repo.root.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap()
    );
}
