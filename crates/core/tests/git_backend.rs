use pokarak_core::{
    ChangedFile, CompareMode, DiffKind, FileStatus, MAX_TEXT_BYTES, diff, read_file,
    repository_root, repository_watch_paths, search, search_with_exclusions, snapshot,
    snapshot_current, snapshot_current_inventory,
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

#[path = "git_backend/eol.rs"]
mod eol;

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

fn excluded_search(repo: &Repo, patterns: &[&str]) -> Vec<PathBuf> {
    search_with_exclusions(
        &repo.root,
        "needle",
        100,
        &patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
    .unwrap()
    .into_iter()
    .map(|hit| {
        hit.path
            .strip_prefix(repo.root.canonicalize().unwrap())
            .unwrap()
            .to_path_buf()
    })
    .collect()
}

#[test]
fn search_exclusions_do_not_spend_the_300_or_10000_hit_budget() {
    let repo = Repo::new();
    // Each excluded candidate sorts before the one included file. More than
    // 300 files and more than 10,000 hits guard both former post-filter caps.
    for number in 0..301 {
        repo.write(format!("a-excluded/{number:04}.txt"), "needle\n");
    }
    repo.write("a-excluded/many.txt", "needle\n".repeat(10_001));
    repo.write("z-included.txt", "needle\n");
    let patterns = vec!["a-excluded/".to_string()];
    let hits = search_with_exclusions(&repo.root, "needle", 1, &patterns).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].path,
        repo.root.canonicalize().unwrap().join("z-included.txt")
    );
    // Also guard a single excluded candidate producing each excess hit count.
    for count in [301, 10_001] {
        repo.write("a-excluded/many.txt", "needle\n".repeat(count));
        let hits = search_with_exclusions(&repo.root, "needle", 1, &patterns).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("z-included.txt"));
    }
}

#[test]
fn search_exclusions_basenames_and_directories_match_at_any_depth() {
    let repo = Repo::new();
    for path in [
        "root.tmp",
        "src/nested.tmp",
        "src/deep/nested.tmp",
        "cache/root.rs",
        "src/cache/nested.rs",
        "src/deep/cache/more.rs",
        "src/keep.rs",
        "cache-file",
    ] {
        repo.write(path, "needle\n");
    }
    assert_eq!(
        excluded_search(&repo, &["*.tmp", "cache/"]),
        vec![PathBuf::from("cache-file"), PathBuf::from("src/keep.rs")],
    );
}

#[test]
fn search_exclusions_directory_only_rules_do_not_exclude_regular_files() {
    let repo = Repo::new();
    repo.write("cache", "needle\n");
    repo.write("src/cache/entry", "needle\n");
    assert_eq!(
        excluded_search(&repo, &["cache/"]),
        vec![PathBuf::from("cache")]
    );
    assert!(excluded_search(&repo, &["cache"]).is_empty());
}

#[test]
fn search_exclusions_slashes_anchor_at_the_worktree_root() {
    let repo = Repo::new();
    for path in ["top.rs", "src/top.rs", "src/file.rs", "nested/src/file.rs"] {
        repo.write(path, "needle\n");
    }
    assert_eq!(
        excluded_search(&repo, &["/top.rs", "src/*.rs"]),
        vec![PathBuf::from("nested/src/file.rs")],
    );
    assert_eq!(
        excluded_search(&repo, &["/src/"]),
        vec![PathBuf::from("nested/src/file.rs"), PathBuf::from("top.rs")],
    );
}

#[test]
fn search_exclusions_double_star_supports_zero_or_many_directories() {
    let repo = Repo::new();
    for path in [
        "src/gen.rs",
        "src/nested/gen.rs",
        "src/deep/nested/gen.rs",
        "generated/a.rs",
        "nested/generated/b.rs",
        "keep.rs",
    ] {
        repo.write(path, "needle\n");
    }
    assert_eq!(
        excluded_search(&repo, &["src/**/gen.rs", "**/generated/**"]),
        vec![PathBuf::from("keep.rs")],
    );
}

#[test]
fn search_exclusions_last_matching_rule_controls_files_and_parent_directories() {
    let repo = Repo::new();
    for path in [
        "generated/keep.rs",
        "generated/drop.rs",
        "generated/deep/keep.rs",
        "generated/deep/drop.rs",
        "outside.rs",
    ] {
        repo.write(path, "needle\n");
    }
    assert_eq!(
        excluded_search(&repo, &["generated/", "!generated/**/keep.rs"]),
        vec![
            PathBuf::from("generated/deep/keep.rs"),
            PathBuf::from("generated/keep.rs"),
            PathBuf::from("outside.rs")
        ],
    );
    // A later parent rule must override an earlier, more specific file rule.
    assert_eq!(
        excluded_search(
            &repo,
            &["generated/", "!generated/**/keep.rs", "generated/"]
        ),
        vec![PathBuf::from("outside.rs")],
    );
    assert_eq!(
        excluded_search(&repo, &["*.rs", "!generated/", "generated/deep/"]),
        vec![
            PathBuf::from("generated/drop.rs"),
            PathBuf::from("generated/keep.rs")
        ],
    );
    assert!(excluded_search(&repo, &["!*.rs", "*.rs"]).is_empty());
    assert_eq!(
        excluded_search(&repo, &["generated/", "!generated/", "generated/"]),
        vec![PathBuf::from("outside.rs")]
    );
}

#[test]
fn search_exclusions_invalid_patterns_fail_before_any_repository_access() {
    let temp = tempfile::tempdir().unwrap();
    let missing_root = temp.path().join("not-a-repository");
    for invalid in [
        "[unterminated",
        "[z-a]",
        "{unclosed",
        "abc\0def",
        "abc\ndef",
        "abc\rdef",
    ] {
        let patterns = vec!["valid/**".to_string(), invalid.to_string()];
        // Validation is not bypassed by empty query, multiline query or limit=0.
        for (query, limit) in [("needle", 1), ("", 1), ("needle", 0), ("a\nb", 1)] {
            let error = search_with_exclusions(&missing_root, query, limit, &patterns).unwrap_err();
            let error = format!("{error:#}");
            assert!(
                error.contains("pattern #2") && error.contains("search.exclude"),
                "{error}"
            );
            assert!(error.contains(&format!("{invalid:?}")), "{error}");
            assert!(!error.contains("select a repository"), "{error}");
        }
    }
}

#[test]
fn search_exclusions_apply_before_opening_an_excluded_candidate() {
    let repo = Repo::new();
    repo.write("a-unreadable", "needle\n");
    repo.add("a-unreadable");
    fs::remove_file(repo.root.join("a-unreadable")).unwrap();
    // A FIFO is rejected by filesystem::open before reading. Unlike permissions
    // this check is reliable even with elevated privileges; O_NONBLOCK prevents
    // a failed regression from blocking on a pipe reader.
    let fifo = std::ffi::CString::new(
        repo.root
            .join("a-unreadable")
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    // SAFETY: fifo is a valid NUL-terminated path within this authored fixture.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    repo.write("z-readable", "needle\n");
    assert!(search(&repo.root, "needle", 1).is_err());
    assert_eq!(
        excluded_search(&repo, &["a-unreadable"]),
        vec![PathBuf::from("z-readable")]
    );
    assert!(
        search_with_exclusions(
            &repo.root,
            "needle",
            1,
            &["a-*".into(), "!a-unreadable".into()]
        )
        .is_err()
    );
}

#[test]
fn search_exclusions_preserve_literal_paths_and_utf16_coordinates() {
    let repo = Repo::new();
    let literal = "nested 한/:(glob)*[x]?\\ quoted\t\"\n.rs";
    repo.write(literal, "irrelevant\n한😀 needle needle\nNEEDLE\n");
    for path in ["#literal", "!literal", "space ", "Case.TMP", "lower.tmp"] {
        repo.write(path, "needle\n");
    }
    let rules = [
        "# comment",
        "",
        "   ",
        "\\#literal",
        "\\!literal",
        "space\\ ",
        "*.tmp",
    ];
    let relative = excluded_search(&repo, &rules);
    assert_eq!(
        relative,
        vec![
            PathBuf::from("Case.TMP"),
            PathBuf::from(literal),
            PathBuf::from(literal)
        ]
    );
    let hits = search_with_exclusions(
        &repo.root,
        "needle",
        20,
        &["*".into(), "!nested 한/**".into()],
    )
    .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].path,
        repo.root.canonicalize().unwrap().join(literal)
    );
    assert_eq!((hits[0].line, hits[0].column), (1, 4));
    assert_eq!((hits[1].line, hits[1].column), (1, 11));
    assert!(hits[0].preview.contains("한😀 needle"));
}

#[test]
fn search_exclusions_escaped_globs_are_literal_and_non_utf8_paths_stay_distinct() {
    let repo = Repo::new();
    repo.write("[literal]*?.rs", "needle\n");
    repo.write("ordinary.rs", "needle\n");
    // macOS filesystems reject non-UTF-8 names; retain the literal-glob test
    // everywhere and exercise raw byte paths where the filesystem supports it.
    let raw = (!cfg!(target_os = "macos"))
        .then(|| PathBuf::from(OsString::from_vec(b"raw-\xff.rs".to_vec())));
    if let Some(raw) = &raw {
        repo.write(raw, "needle\n");
    }
    assert_eq!(
        excluded_search(&repo, &[r"\[literal\]\*\?.rs", "raw-*.rs"]),
        vec![PathBuf::from("ordinary.rs")],
    );
    let hits =
        search_with_exclusions(&repo.root, "needle", 10, &[r"\[literal\]\*\?.rs".into()]).unwrap();
    assert_eq!(hits.len(), 1 + usize::from(raw.is_some()));
    if let Some(raw) = raw {
        assert!(
            hits.iter()
                .any(|hit| hit.path == repo.root.canonicalize().unwrap().join(&raw))
        );
    }
}

#[test]
fn search_exclusions_linked_worktrees_keep_roots_and_rules_isolated() {
    let repo = Repo::new();
    repo.seed();
    repo.write("same.rs", "needle main\n");
    repo.add("same.rs");
    repo.commit();
    let linked = repo.temp.path().join("generated linked 한");
    repo.git_os(&[
        "worktree".as_ref(),
        "add".as_ref(),
        "-b".as_ref(),
        "linked".as_ref(),
        "--".as_ref(),
        linked.as_os_str(),
    ]);
    fs::write(linked.join("same.rs"), "한😀 needle linked\n").unwrap();
    fs::write(linked.join("linked-only.rs"), "needle linked only\n").unwrap();
    repo.write("main-only.rs", "needle main only\n");
    // The absolute parent name matches neither this basename nor anchored rule.
    let rules = vec!["generated*".into(), "/main-only.rs".into()];
    let linked_hits = search_with_exclusions(&linked.join("src"), "needle", 10, &rules).unwrap();
    assert_eq!(linked_hits.len(), 2);
    assert!(
        linked_hits
            .iter()
            .all(|hit| hit.path.starts_with(linked.canonicalize().unwrap()))
    );
    let same = linked_hits
        .iter()
        .find(|hit| hit.path.ends_with("same.rs"))
        .unwrap();
    assert_eq!((same.line, same.column), (0, 4));
    assert!(same.preview.contains("linked"));
    let main_hits = search_with_exclusions(&repo.root, "needle", 10, &rules).unwrap();
    assert_eq!(main_hits.len(), 1);
    assert!(main_hits[0].preview.contains("main"));
    assert!(excluded_search(&repo, &["*.rs"]).is_empty());
    assert_eq!(
        search_with_exclusions(&linked, "needle", 10, &[])
            .unwrap()
            .len(),
        2
    );
    assert_eq!(search(&repo.root, "needle", 10).unwrap().len(), 2);
}

#[test]
fn search_exclusions_preserve_search_safety_limits_and_empty_rules_compatibility() {
    let repo = Repo::new();
    repo.seed();
    repo.write("search.rs", "한😀 needle needle\n");
    repo.write("binary.dat", b"needle\0");
    repo.write("invalid-utf8.dat", b"needle\xff");
    repo.write("huge.txt", "needle\n".repeat(MAX_TEXT_BYTES / 7 + 1));
    repo.write(".gitignore", "ignored.rs\n");
    repo.write("ignored.rs", "needle\n");
    repo.write("tracked-ignored.rs", "needle\n");
    repo.add("tracked-ignored.rs");
    repo.write(".gitignore", "ignored.rs\ntracked-ignored.rs\n");
    let outside = repo.temp.path().join("outside.rs");
    fs::write(&outside, "needle\n").unwrap();
    symlink(&outside, repo.root.join("link.rs")).unwrap();
    symlink(repo.temp.path(), repo.root.join("outside-link")).unwrap();
    for limit in [0, 1, 10, 10_001] {
        for query in ["needle", "", "never found", "NEEDLE", "a\nb", "a\rb"] {
            let old = search(&repo.root, query, limit).unwrap();
            let new = search_with_exclusions(&repo.root, query, limit, &[]).unwrap();
            assert_eq!(
                old.iter()
                    .map(|h| (&h.path, h.line, h.column, &h.preview))
                    .collect::<Vec<_>>(),
                new.iter()
                    .map(|h| (&h.path, h.line, h.column, &h.preview))
                    .collect::<Vec<_>>()
            );
        }
    }
    // Reinclusion does not bypass binary/size/symlink/Git-ignore safety.
    let hits =
        search_with_exclusions(&repo.root, "needle", 20, &["*".into(), "!*".into()]).unwrap();
    assert_eq!(hits.len(), 3);
    assert!(
        hits.iter()
            .all(|hit| hit.path.ends_with("search.rs") || hit.path.ends_with("tracked-ignored.rs"))
    );
    assert!(
        search_with_exclusions(
            &repo.root,
            "needle",
            20,
            &["*".into(), "!ignored.rs".into()]
        )
        .unwrap()
        .is_empty()
    );
    repo.write("many.rs", "needle\n".repeat(10_001));
    assert_eq!(
        search_with_exclusions(
            &repo.root,
            "needle",
            usize::MAX,
            &["search.rs".into(), "tracked-ignored.rs".into()]
        )
        .unwrap()
        .len(),
        10_000
    );
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
fn watch_paths_include_real_linked_metadata_and_preserve_native_newlines() {
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
    let paths = repository_watch_paths(&linked.join("src")).unwrap();
    let expected_git_dir = git_at(
        &linked,
        &[
            "rev-parse".as_ref(),
            "--path-format=absolute".as_ref(),
            "--git-dir".as_ref(),
        ],
    );
    let expected_git_dir = PathBuf::from(OsString::from_vec(
        expected_git_dir.strip_suffix(b"\n").unwrap().to_vec(),
    ))
    .canonicalize()
    .unwrap();
    assert_eq!(paths[0], linked.canonicalize().unwrap());
    assert_eq!(paths.len(), 3);
    assert!(paths.contains(&expected_git_dir));
    assert!(paths.contains(&repo.root.join(".git").canonicalize().unwrap()));
    assert!(!expected_git_dir.starts_with(&paths[0]));
    assert!(expected_git_dir.join("HEAD").is_file());
    assert!(expected_git_dir.join("index").is_file());
    let main = repository_watch_paths(&repo.root).unwrap();
    assert_eq!(main.len(), 2); // Main Git dir and common Git dir deduplicate.
    assert_eq!(main[0], repo.root.canonicalize().unwrap());
    assert_eq!(main[1], repo.root.join(".git").canonicalize().unwrap());
}

#[test]
fn sibling_inspection_failure_does_not_block_healthy_selected_worktree() {
    let repo = Repo::new();
    repo.seed();
    let bad = repo.temp.path().join("unreadable sibling");
    let good = repo.temp.path().join("healthy sibling");
    for (branch, path) in [("bad", &bad), ("good", &good)] {
        repo.git_os(&[
            "worktree".as_ref(),
            "add".as_ref(),
            "-b".as_ref(),
            branch.as_ref(),
            "--".as_ref(),
            path.as_os_str(),
        ]);
    }
    fs::write(good.join("new.rs"), "healthy change\n").unwrap();
    repo.write("src/code.rs", "selected change\n");
    let blocked = bad.join("src/code.rs");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    // Elevated runners can still read chmod(000). A FIFO is a deterministic
    // unsupported node instead; inspection must reject it without blocking.
    if fs::File::open(&blocked).is_ok() {
        fs::remove_file(&blocked).unwrap();
        let name = std::ffi::CString::new(blocked.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    }
    let snap = repo.snap();
    assert_eq!(snap.worktree.changed_files, 1);
    assert!(snap.worktree.changed_files_available);
    assert!(snap.worktree.inspection_error.is_none());
    assert_eq!(change(&snap, "src/code.rs").status, FileStatus::Modified);
    let bad_row = snap.worktrees.iter().find(|w| w.branch == "bad").unwrap();
    assert!(!bad_row.changed_files_available);
    assert!(
        bad_row
            .inspection_error
            .as_deref()
            .unwrap()
            .contains("Cannot inspect worktree")
    );
    let good_row = snap.worktrees.iter().find(|w| w.branch == "good").unwrap();
    assert!(good_row.changed_files_available);
    assert_eq!(good_row.changed_files, 1);
    assert!(good_row.inspection_error.is_none());
    // A failure in the actual selected worktree must still be visible.
    assert!(snapshot(&bad, &CompareMode::Uncommitted).is_err());
    let local = snapshot_current(&repo.root, &CompareMode::Uncommitted).unwrap();
    assert!(local.worktree.changed_files_available);
    assert!(
        local
            .worktrees
            .iter()
            .filter(|w| w.path != local.worktree.path)
            .all(|w| !w.changed_files_available && w.inspection_error.is_none())
    );
}

#[test]
fn current_snapshot_skips_sibling_contents_and_preserves_branch_comparison() {
    let repo = Repo::new();
    repo.seed();
    let linked = repo.temp.path().join("linked");
    repo.git_os(&[
        "worktree".as_ref(),
        "add".as_ref(),
        "-b".as_ref(),
        "linked".as_ref(),
        "--".as_ref(),
        linked.as_os_str(),
    ]);
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("committed.rs", "committed on feature\n");
    repo.add("committed.rs");
    repo.commit();
    let mode = CompareMode::Branch("main".into());
    let first = snapshot_current(&repo.root, &mode).unwrap();
    assert_eq!(first.changes.len(), 1);
    assert_eq!(first.worktree.changed_files, 0);
    assert!(first.worktree.changed_files_available);
    assert_eq!(first.compare, mode);
    let sibling = first
        .worktrees
        .iter()
        .find(|w| w.branch == "linked")
        .unwrap();
    assert!(!sibling.changed_files_available);
    assert!(sibling.inspection_error.is_none());
    let full_before = snapshot(&repo.root, &mode).unwrap();
    fs::write(linked.join("src/code.rs"), "sibling changed\n").unwrap();
    assert_eq!(
        first.fingerprint,
        snapshot_current(&repo.root, &mode).unwrap().fingerprint
    );
    assert_ne!(
        full_before.fingerprint,
        snapshot(&repo.root, &mode).unwrap().fingerprint
    );
    repo.write("src/code.rs", "selected changed\n");
    let next = snapshot_current(&repo.root, &mode).unwrap();
    assert_ne!(first.fingerprint, next.fingerprint);
    assert_eq!(next.worktree.changed_files, 1);
    let listing = snapshot_current_inventory(&repo.root, &mode).unwrap();
    assert_eq!(listing.fingerprint, next.fingerprint);
    assert_eq!(listing.worktree.changed_files, 1);
    assert_eq!(listing.changes.len(), 2);
    assert!(
        !listing
            .worktrees
            .iter()
            .find(|w| w.branch == "linked")
            .unwrap()
            .changed_files_available
    );
}

#[test]
fn inventory_defers_baseline_blobs_until_a_file_is_selected() {
    let repo = Repo::new();
    repo.seed();
    repo.write("missing.bin", b"\0old");
    repo.add("missing.bin");
    repo.commit();
    repo.git(&["mv", "--", "src/code.rs", "renamed.rs"]);
    repo.write("renamed.rs", "first\nchanged\nthird\n");
    repo.write("missing.bin", "now text");
    let mode = CompareMode::Uncommitted;
    let measured = snapshot_current(&repo.root, &mode).unwrap();
    assert!(change(&measured, "missing.bin").binary);

    // Move one authored loose blob outside the fixture's object directory.
    // Tree/index metadata and the selected rename's baseline are still readable.
    let id = repo.git(&["rev-parse", "--verify", "HEAD:missing.bin"]);
    let id = id.trim_end();
    fs::rename(
        repo.root.join(".git/objects").join(&id[..2]).join(&id[2..]),
        repo.temp.path().join("held-blob"),
    )
    .unwrap();

    let listing = snapshot_current_inventory(&repo.root, &mode).unwrap();
    assert_eq!(listing.fingerprint, measured.fingerprint);
    assert_eq!(listing.files, measured.files);
    assert_eq!(listing.changes.len(), 2);
    let renamed = change(&listing, "renamed.rs");
    assert_eq!(renamed.old_path.as_deref(), Some(Path::new("src/code.rs")));
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert!(renamed.staged && renamed.unstaged);
    let doc = diff(&repo.root, renamed, &mode).unwrap();
    assert!(doc.patch.contains("-second\n+changed\n"));
    assert!(diff(&repo.root, change(&listing, "missing.bin"), &mode).is_err());
    assert!(snapshot_current(&repo.root, &mode).is_err());
}

#[test]
fn batched_statistics_match_selected_previews_across_blob_boundaries() {
    let repo = Repo::new();
    repo.write("a-empty", "");
    repo.write("b-original-binary", b"\xff\n");
    repo.write("c-big", vec![b'x'; MAX_TEXT_BYTES + 1]);
    repo.write("d-mixed", b"one\rtwo\nthree\r");
    repo.write("e-lines", "\n".repeat(50_001));
    repo.write("f-last", "no newline");
    repo.add(".");
    repo.commit();
    for name in [
        "a-empty",
        "b-original-binary",
        "c-big",
        "d-mixed",
        "e-lines",
        "f-last",
    ] {
        repo.write(name, "replacement\nlast line");
    }
    let snap = repo.snap();
    assert_eq!(snap.changes.len(), 6);
    for row in &snap.changes {
        let preview = diff(&repo.root, row, &snap.compare).unwrap();
        assert_eq!(row.binary, preview.binary, "{:?}", row.path);
        let count = |kind| {
            preview
                .lines
                .iter()
                .filter(|line| line.kind == kind)
                .count()
        };
        assert_eq!(
            (row.additions, row.deletions),
            (count(DiffKind::Addition), count(DiffKind::Deletion)),
            "{:?}",
            row.path
        );
    }
    assert!(change(&snap, "b-original-binary").binary);
    let last = change(&snap, "f-last");
    assert_eq!((last.additions, last.deletions), (2, 1));
}

#[test]
fn gitlink_statistics_and_lazy_preview_use_opaque_commit_ids() {
    let repo = Repo::new();
    repo.seed();
    let old = "1".repeat(40);
    let new = "2".repeat(40);
    fs::create_dir(repo.root.join("module")).unwrap();
    let set_gitlink = |oid| {
        repo.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{oid},module"),
            "--",
        ]);
    };
    set_gitlink(&old);
    repo.commit();
    set_gitlink(&new);
    let snap = repo.snap();
    let file = change(&snap, "module");
    assert_eq!((file.additions, file.deletions, file.binary), (1, 1, false));
    let listing = snapshot_current_inventory(&repo.root, &snap.compare).unwrap();
    let doc = diff(&repo.root, change(&listing, "module"), &snap.compare).unwrap();
    assert_eq!(doc.old_text, format!("Subproject commit {old}\n"));
    assert_eq!(doc.new_text, format!("Subproject commit {new}\n"));
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
    let working_diff = || {
        let snap = repo.snap();
        diff(
            &repo.root,
            change(&snap, "src/code.rs"),
            &CompareMode::Uncommitted,
        )
        .unwrap()
    };
    assert_eq!(
        working_diff().file_mode_header(),
        Some("File mode: 100644 → 100755")
    );
    repo.git(&["config", "core.filemode", "false"]);
    assert!(repo.snap().changes.is_empty());
    repo.write("src/code.rs", "modified\n");
    assert_eq!(repo.snap().changes.len(), 1);
    assert!(working_diff().file_mode_header().is_none());
    repo.git(&["update-index", "--chmod=+x", "--", "src/code.rs"]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let staged_mode = working_diff();
    assert_eq!(
        staged_mode.file_mode_header(),
        Some("File mode: 100644 → 100755")
    );
    assert!(staged_mode.patch.contains("+modified"));
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
    repo.write(
        ".gitattributes",
        "*.rs text=auto filter=forbidden diff=forbidden\n",
    );
    repo.git(&["config", "filter.forbidden.clean", "false"]);
    repo.git(&["config", "filter.forbidden.required", "true"]);
    repo.git(&["config", "diff.forbidden.textconv", "false"]);
    repo.git(&["config", "diff.external", "false"]);
    repo.git(&["config", "core.fsmonitor", "false"]);
    repo.write("src/code.rs", "changed\r\n");
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
        "changed\r\n"
    );
    assert_eq!(search(&repo.root, "changed", 10).unwrap().len(), 1);
    let current = snapshot_current(&repo.root, &CompareMode::Uncommitted).unwrap();
    assert_eq!(current.changes.len(), snap.changes.len());
    assert_eq!(repository_watch_paths(&repo.root).unwrap().len(), 2);
    assert_eq!(before, contents(&repo.root));
    assert_eq!(
        index_modified,
        fs::metadata(repo.root.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap()
    );
}
