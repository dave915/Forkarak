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
    let fork = &page.entries[0].graph;
    assert_eq!(fork.outgoing_tracks[0], fork.track);
    assert_ne!(fork.outgoing_tracks[0], fork.outgoing_tracks[1]);
    let all_before = first_page(&f.root, 200).unwrap();
    for rows in all_before.entries.windows(2) {
        assert_eq!(rows[0].graph.outgoing_tracks, rows[1].graph.incoming_tracks);
    }
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

#[test]
fn file_history_follows_renames_with_literal_paths_and_a_fixed_start() {
    use crate::file_history::{file_blame, file_history, revision_diff};
    let f = Fixture::new();
    let original = ":(glob)*\told\nfile";
    let renamed = "--new path";
    let base = f.commit(original, "base\nsecond\n");
    f.git(&["mv", "--", original, renamed]);
    f.git(&["commit", "-m", "rename"]);
    let rename = f.git(&["rev-parse", "HEAD"]);
    let changed = f.commit(renamed, "base\nedited\n");
    f.git(&["mv", "--", renamed, "staged name"]);
    fs::write(f.root.join("staged name"), "extra\nbase\nedited\n").unwrap();

    let index = fs::read(f.root.join(".git/index")).unwrap();
    let working = file_history(&f.root, Path::new("staged name"), None).unwrap();
    assert!(!working.truncated);
    assert_eq!(
        working
            .entries
            .iter()
            .map(|e| (&e.oid, e.path.as_path()))
            .collect::<Vec<_>>(),
        [
            (&changed, Path::new(renamed)),
            (&rename, Path::new(renamed)),
            (&base, Path::new(original))
        ]
    );
    let renamed_revision = &working.entries[1];
    assert_eq!(renamed_revision.parent.as_ref(), Some(&base));
    let pure = revision_diff(&f.root, renamed_revision).unwrap();
    assert_eq!(pure.old_path.as_deref(), Some(Path::new(original)));
    assert_eq!(pure.old_text, pure.new_text);
    assert!(pure.patch.contains("rename from"));
    assert!(!pure.patch.contains("@@"));
    let initial = &working.entries[2];
    assert!(initial.parent.is_none());
    let first = revision_diff(&f.root, initial).unwrap();
    assert_eq!(first.old_text, "");
    assert_eq!(first.new_text, "base\nsecond\n");
    let current_blame = file_blame(&f.root, Path::new("staged name"), None).unwrap();
    assert_eq!(
        current_blame
            .iter()
            .map(|l| (l.line, l.oid.as_ref()))
            .collect::<Vec<_>>(),
        [(1, None), (2, Some(&base)), (3, Some(&changed))]
    );
    let before = file_history(&f.root, Path::new(renamed), Some(&rename)).unwrap();
    assert_eq!(before.entries.len(), 2);
    let previous = file_blame(&f.root, Path::new(renamed), Some(&rename)).unwrap();
    assert_eq!(previous[1].oid.as_ref(), Some(&base));
    assert_eq!(previous[1].text, "second");
    assert_eq!(previous[1].author, "History Fixture");
    assert_eq!(previous[1].date, &before.entries[1].date[..10]);
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    f.git(&["commit", "-m", "staged rename"]);
    f.git(&["rm", "-f", "--", "staged name"]);
    f.git(&["commit", "-m", "delete"]);
    let removed = file_history(&f.root, Path::new("staged name"), None).unwrap();
    assert!(removed.entries[0].deleted);
    assert_eq!(removed.entries.len(), 5);
    let deletion = revision_diff(&f.root, &removed.entries[0]).unwrap();
    assert_eq!(deletion.old_text, "base\nedited\n");
    assert_eq!(deletion.new_text, "");
}

#[test]
fn file_blame_uses_raw_contents_and_handles_untracked_binary_and_unsafe_paths() {
    use crate::file_history::{file_blame, file_history};
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let path = Path::new("new file");
    fs::write(f.root.join(path), "first\r\n\nlast").unwrap();
    assert!(
        file_history(&f.root, path, None)
            .unwrap()
            .entries
            .is_empty()
    );
    let untracked = file_blame(&f.root, path, None).unwrap();
    assert_eq!(
        untracked
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>(),
        ["first", "", "last"]
    );
    assert!(untracked.iter().all(|l| l.oid.is_none()));
    fs::write(
        f.root.join(".gitattributes"),
        "* -text diff=tripwire filter=tripwire\n",
    )
    .unwrap();
    f.git(&["add", "--", "."]);
    f.git(&["commit", "-m", "baseline"]);
    let head = f.git(&["rev-parse", "HEAD"]);
    f.git(&[
        "config",
        "diff.tripwire.textconv",
        "touch textconv-fired && cat",
    ]);
    f.git(&[
        "config",
        "filter.tripwire.clean",
        "touch filter-fired && cat",
    ]);
    fs::write(f.root.join(path), "working\n\nlast").unwrap();
    assert_eq!(file_blame(&f.root, path, None).unwrap()[0].oid, None);
    assert_eq!(
        file_blame(&f.root, path, Some(&head)).unwrap()[0].text,
        "first"
    );
    assert_eq!(
        file_history(&f.root, path, None).unwrap().entries[0].oid,
        head
    );
    assert!(!f.root.join("filter-fired").exists());
    assert!(!f.root.join("textconv-fired").exists());
    fs::write(f.root.join(path), [0, 255]).unwrap();
    assert!(file_blame(&f.root, path, None).is_err());
    fs::write(f.root.join(path), "\n".repeat(50_001)).unwrap();
    assert!(file_blame(&f.root, path, None).is_err());
    assert!(file_blame(&f.root, path, Some("--all")).is_err());
    assert!(file_history(&f.root, Path::new("../x"), Some(&head)).is_err());
    let outside = f._dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("private"), "outside content").unwrap();
    symlink(&outside, f.root.join("link")).unwrap();
    assert!(file_blame(&f.root, Path::new("link/private"), None).is_err());
}

#[test]
fn file_history_and_blame_are_isolated_between_linked_worktrees() {
    use crate::file_history::{file_blame, file_history};
    let f = Fixture::new();
    let base = f.commit("tracked", "base\n");
    let linked = f._dir.path().join("linked");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "side",
        "--",
        linked.to_str().unwrap(),
        "HEAD",
    ]);
    let main = f.commit("tracked", "main\n");
    fs::write(linked.join("tracked"), "base\nside\n").unwrap();
    let path = Path::new("tracked");
    assert_eq!(
        file_history(&linked, path, None).unwrap().entries[0].oid,
        base
    );
    assert_eq!(
        file_history(&f.root, path, None).unwrap().entries[0].oid,
        main
    );
    let side = file_blame(&linked, path, None).unwrap();
    assert_eq!(side[0].oid.as_ref(), Some(&base));
    assert_eq!(side[1].oid, None);
    assert_eq!(
        file_blame(&f.root, path, None).unwrap()[0].oid.as_ref(),
        Some(&main)
    );
}

#[test]
fn file_blame_keeps_git_line_boundaries_with_embedded_cr() {
    use crate::file_history::file_blame;
    let f = Fixture::new();
    let base = f.commit("file", "first\rcontinued\nsecond\n");
    fs::write(f.root.join("file"), "first\rcontinued\ninserted\nsecond\n").unwrap();
    let lines = file_blame(&f.root, Path::new("file"), None).unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].text, "first\rcontinued");
    assert_eq!(lines[1].oid, None);
    assert_eq!(lines[2].oid.as_ref(), Some(&base));
}

#[test]
fn file_revision_uses_both_rename_paths_and_the_actual_parent_for_mode_changes() {
    use crate::file_history::{file_history, revision_diff};
    use std::os::unix::fs::PermissionsExt;

    let f = Fixture::new();
    let old = "script\told";
    let new = "source/:(literal)script-new";
    let before = "one retained line\nanother retained line\nold ending\n";
    let after = "one retained line\nanother retained line\nnew ending\n";
    f.commit(old, before);
    let parent = f.commit("unrelated", "intervening commit");
    fs::create_dir(f.root.join("source")).unwrap();
    f.git(&["mv", "--", old, new]);
    f.commit(new, after);
    fs::set_permissions(f.root.join(new), fs::Permissions::from_mode(0o755)).unwrap();
    f.git(&["add", "--", new]);
    f.git(&["commit", "-m", "executable"]);
    fs::write(f.root.join(new), "other worktree contents").unwrap();
    let list = file_history(&f.root, Path::new(new), None).unwrap();
    let mode = revision_diff(&f.root, &list.entries[0]).unwrap();
    assert!(mode.patch.contains("100644 → 100755"));
    assert_eq!(mode.lines[0].text, mode.patch.trim_end());
    let renamed = &list.entries[1];
    assert_eq!(renamed.parent.as_ref(), Some(&parent));
    let change = revision_diff(&f.root, renamed).unwrap();
    assert_eq!(change.old_path.as_deref(), Some(Path::new(old)));
    assert_eq!(
        (change.old_text.as_str(), change.new_text.as_str()),
        (before, after)
    );
    assert!(change.patch.contains("-old ending\n+new ending"));
    let different = Fixture::new();
    assert!(revision_diff(&different.root, renamed).is_err());
}

#[test]
fn exact_copy_in_followed_history_identifies_the_copy_source() {
    let f = Fixture::new();
    f.commit("original", "identical lines\n");
    f.commit("copy", "identical lines\n");
    let list = crate::file_history::file_history(&f.root, Path::new("copy"), None).unwrap();
    let patch = crate::file_history::revision_diff(&f.root, &list.entries[0]).unwrap();
    assert_eq!(patch.patch, "copy from \"original\"\ncopy to \"copy\"\n");
    assert_eq!(patch.old_text, patch.new_text);
}
