use super::*;
use tempfile::TempDir;
struct Fixture {
    dir: TempDir,
    ops: GitOperations,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let global = dir.path().join("global");
        fs::write(&global, "").unwrap();
        let mut ops = GitOperations {
            root,
            timeout: Duration::from_secs(10),
            global_config: Some(global),
            cancellation: OperationCancellation::default(),
        };
        // All fixture operations inherit only a private config, never global hooks/remotes.
        ops.checked(&["init", "-b", "main"]).unwrap();
        ops.checked(&["config", "--local", "user.name", "Fixture"])
            .unwrap();
        ops.checked(&["config", "--local", "user.email", "fixture@example.invalid"])
            .unwrap();
        ops.checked(&["config", "--local", "commit.gpgsign", "false"])
            .unwrap();
        ops.root = ops.root.canonicalize().unwrap();
        let f = Self { dir, ops };
        f.write("file.txt", "base\n");
        f.commit("base");
        f
    }
    fn write(&self, p: &str, s: &str) {
        fs::write(self.ops.root.join(p), s).unwrap();
    }
    fn git(&self, args: &[&str]) {
        self.ops.checked(args).unwrap();
    }
    fn commit(&self, message: &str) {
        self.git(&["add", "--", "."]);
        self.git(&["commit", "-m", message]);
    }
    fn conflict(&self, delete: bool) -> ConflictSnapshot {
        self.git(&["switch", "-c", "incoming"]);
        self.write("file.txt", "theirs\n");
        self.commit("incoming");
        self.git(&["switch", "main"]);
        if delete {
            self.git(&["rm", "--", "file.txt"]);
        } else {
            self.write("file.txt", "ours\n");
        }
        self.commit("ours");
        assert_ne!(self.ops.run(&["merge", "incoming"]).unwrap().code, Some(0));
        let file = self.ops.conflicts().unwrap().remove(0);
        self.ops.inspect_conflict(&file).unwrap()
    }
}
#[test]
fn stash_tracked_untracked_and_ignored() {
    let f = Fixture::new();
    f.write(".gitignore", "ignored\n");
    f.commit("ignore");
    f.write("file.txt", "tracked changes\n");
    f.write("new", "untracked\n");
    f.write("ignored", "private\n");
    f.ops.stash_save("tracked only", false).unwrap();
    assert!(f.ops.root.join("new").exists());
    let first = f.ops.stashes().unwrap().remove(0);
    assert_eq!(first.source_branch.as_deref(), Some("main"));
    assert!(first.message.contains("tracked only"));
    assert!(
        f.ops
            .stash_patch(&first)
            .unwrap()
            .stdout
            .contains("tracked changes")
    );
    assert_eq!(
        f.ops
            .stash_content(&first, Path::new("file.txt"), false)
            .unwrap()
            .text(),
        Some("tracked changes\n")
    );
    f.ops.stash_save("with untracked", true).unwrap();
    assert!(!f.ops.root.join("new").exists());
    assert!(f.ops.root.join("ignored").exists());
    let second = f.ops.stashes().unwrap().remove(0);
    assert_eq!(
        f.ops
            .stash_content(&second, Path::new("new"), true)
            .unwrap()
            .text(),
        Some("untracked\n")
    );
    f.ops.stash_pop(&second).unwrap();
    assert!(f.ops.root.join("new").exists());
    f.ops.stash_apply(&first).unwrap();
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file.txt")).unwrap(),
        "tracked changes\n"
    );
    f.ops.stash_drop(&first).unwrap();
    assert!(f.ops.stashes().unwrap().is_empty());
}
#[test]
fn stale_stash_cannot_drop_a_new_entry() {
    let f = Fixture::new();
    f.write("file.txt", "one");
    f.ops.stash_save("one", false).unwrap();
    let stale = f.ops.stashes().unwrap().remove(0);
    f.write("file.txt", "two");
    f.ops.stash_save("two", false).unwrap();
    assert!(f.ops.stash_drop(&stale).is_err());
    assert!(f.ops.stash_pop(&stale).is_err());
    assert_eq!(f.ops.stashes().unwrap().len(), 2);
    assert!(f.ops.stash_patch(&stale).unwrap().stdout.contains("one"));
}
#[test]
fn apply_and_pop_conflicts_retain_stash_even_after_resolution() {
    for pop in [false, true] {
        let f = Fixture::new();
        f.write("file.txt", "stashed\n");
        f.ops.stash_save("retain me", false).unwrap();
        let entry = f.ops.stashes().unwrap().remove(0);
        f.write("file.txt", "committed\n");
        f.commit("changed base");
        assert!(
            if pop {
                f.ops.stash_pop(&entry)
            } else {
                f.ops.stash_apply(&entry)
            }
            .is_err()
        );
        assert_eq!(f.ops.stashes().unwrap(), vec![entry.clone()]);
        assert!(f.ops.in_progress().unwrap().is_none());
        let snapshot = f
            .ops
            .inspect_conflict(&f.ops.conflicts().unwrap()[0])
            .unwrap();
        f.ops
            .resolve_conflict(&snapshot, &Resolution::Theirs)
            .unwrap();
        assert_eq!(f.ops.stashes().unwrap(), vec![entry]);
    }
}
#[test]
fn merge_stages_save_stage_continue_and_abort() {
    let f = Fixture::new();
    let snapshot = f.conflict(false);
    assert_eq!(
        snapshot
            .stages
            .iter()
            .map(|s| s.content.text().unwrap())
            .collect::<Vec<_>>(),
        vec!["base\n", "ours\n", "theirs\n"]
    );
    let progress = f.ops.in_progress().unwrap().unwrap();
    assert_eq!(progress.kind, ProgressKind::Merge);
    assert!(f.ops.finish_operation(&progress, false).is_err());
    f.ops
        .resolve_conflict(&snapshot, &Resolution::Edited("resolved\n".into()))
        .unwrap();
    assert!(f.ops.conflicts().unwrap().is_empty());
    assert_eq!(
        f.ops.metadata(&["show", ":file.txt"]).unwrap(),
        b"resolved\n"
    );
    f.ops.finish_operation(&progress, false).unwrap();
    assert!(f.ops.in_progress().unwrap().is_none());
    assert!(f.ops.finish_operation(&progress, true).is_err());
    let f = Fixture::new();
    f.conflict(false);
    let progress = f.ops.in_progress().unwrap().unwrap();
    f.ops.finish_operation(&progress, true).unwrap();
    assert!(f.ops.conflicts().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file.txt")).unwrap(),
        "ours\n"
    );
}
#[test]
fn external_edits_and_stale_index_are_rejected() {
    let f = Fixture::new();
    let snapshot = f.conflict(false);
    f.write("file.txt", "external edit\n");
    let err = f
        .ops
        .resolve_conflict(&snapshot, &Resolution::Ours)
        .unwrap_err();
    assert!(err.to_string().contains("externally"));
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file.txt")).unwrap(),
        "external edit\n"
    );
    let snapshot = f.ops.inspect_conflict(&snapshot.file).unwrap();
    f.git(&["add", "--", "file.txt"]);
    assert!(
        f.ops
            .resolve_conflict(&snapshot, &Resolution::Theirs)
            .unwrap_err()
            .to_string()
            .contains("stages changed")
    );
}
#[test]
fn delete_modify_missing_stage_is_an_explicit_deletion() {
    for side in [Resolution::Ours, Resolution::Theirs] {
        let f = Fixture::new();
        let snapshot = f.conflict(true);
        assert_eq!(
            snapshot
                .stages
                .iter()
                .map(|s| s.entry.stage)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        f.ops.resolve_conflict(&snapshot, &side).unwrap();
        assert!(f.ops.conflicts().unwrap().is_empty());
        assert_eq!(
            f.ops.root.join("file.txt").exists(),
            matches!(side, Resolution::Theirs)
        );
    }
}
#[test]
fn add_add_binary_and_large_files_are_honest() {
    let f = Fixture::new();
    f.git(&["switch", "-c", "incoming"]);
    f.write("added", "theirs\n");
    f.commit("add theirs");
    f.git(&["switch", "main"]);
    f.write("added", "ours\n");
    f.commit("add ours");
    f.ops.run(&["merge", "incoming"]).unwrap();
    let snapshot = f
        .ops
        .inspect_conflict(&f.ops.conflicts().unwrap()[0])
        .unwrap();
    assert_eq!(snapshot.stages.len(), 2);
    assert_eq!(snapshot.stages[0].entry.stage, 2);
    f.ops
        .resolve_conflict(&snapshot, &Resolution::Edited("combined\n".into()))
        .unwrap();
    assert!(matches!(content(vec![0, 1]), ConflictContent::Binary(_)));
    let f = Fixture::new();
    f.write("binary", "\0theirs");
    f.commit("binary");
    let oid = String::from_utf8(f.ops.metadata(&["rev-parse", "HEAD:binary"]).unwrap()).unwrap();
    assert!(matches!(
        f.ops.workflow_blob(oid.trim(), "100644").unwrap(),
        ConflictContent::Binary(_)
    ));
    f.write("large", &"x".repeat(MAX_RESOLUTION_BYTES + 1));
    f.commit("large");
    let oid = String::from_utf8(f.ops.metadata(&["rev-parse", "HEAD:large"]).unwrap()).unwrap();
    assert!(matches!(
        f.ops.workflow_blob(oid.trim(), "100644").unwrap(),
        ConflictContent::Unsupported(_)
    ));
}
#[test]
fn traversal_symlinks_and_partial_stage_failure() {
    let f = Fixture::new();
    let snapshot = f.conflict(false);
    assert!(f.ops.safe_working_path(Path::new("../escape")).is_err());
    assert!(f.ops.safe_working_path(Path::new(".git/config")).is_err());
    let external = f.dir.path().join("external");
    fs::write(&external, "outside").unwrap();
    fs::remove_file(f.ops.root.join("file.txt")).unwrap();
    std::os::unix::fs::symlink(&external, f.ops.root.join("file.txt")).unwrap();
    assert!(
        f.ops
            .resolve_conflict(&snapshot, &Resolution::Ours)
            .is_err()
    );
    assert_eq!(fs::read_to_string(&external).unwrap(), "outside");
    let f = Fixture::new();
    let snapshot = f.conflict(false);
    fs::write(f.ops.root.join(".git/index.lock"), "fixture lock").unwrap();
    let err = f
        .ops
        .resolve_conflict(&snapshot, &Resolution::Edited("saved result\n".into()))
        .unwrap_err();
    assert!(err.to_string().contains("Result saved, but staging failed"));
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file.txt")).unwrap(),
        "saved result\n"
    );
    fs::remove_file(f.ops.root.join(".git/index.lock")).unwrap();
    let retry = f.ops.inspect_conflict(&snapshot.file).unwrap();
    f.ops
        .resolve_conflict(&retry, &Resolution::Edited("saved result\n".into()))
        .unwrap();
}
#[test]
fn rebase_cherry_pick_revert_detection_and_abort() {
    for kind in [
        ProgressKind::Rebase,
        ProgressKind::CherryPick,
        ProgressKind::Revert,
    ] {
        let f = Fixture::new();
        f.git(&["switch", "-c", "incoming"]);
        f.write("file.txt", "incoming\n");
        f.commit("incoming");
        f.git(&["switch", "main"]);
        f.write("file.txt", "ours\n");
        f.commit("ours");
        let args = match kind {
            ProgressKind::Rebase => vec!["rebase", "incoming"],
            ProgressKind::CherryPick => vec!["cherry-pick", "incoming"],
            _ => vec!["revert", "incoming"],
        };
        assert_ne!(f.ops.run(&args).unwrap().code, Some(0));
        let progress = f.ops.in_progress().unwrap().unwrap();
        assert_eq!(progress.kind, kind);
        f.ops.finish_operation(&progress, true).unwrap();
        assert!(f.ops.in_progress().unwrap().is_none());
    }
}
#[test]
fn linked_worktree_sees_shared_stash() {
    let f = Fixture::new();
    let linked = f.dir.path().join("linked");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        "--",
        linked.to_str().unwrap(),
    ]);
    f.write("file.txt", "shared\n");
    f.ops.stash_save("shared repository stash", false).unwrap();
    let mut other = f.ops.clone();
    other.root = linked.canonicalize().unwrap();
    assert_eq!(f.ops.stashes().unwrap(), other.stashes().unwrap());
}

#[test]
fn continue_rebase_and_cherry_pick_without_an_editor() {
    for kind in [
        ProgressKind::Rebase,
        ProgressKind::CherryPick,
        ProgressKind::Revert,
    ] {
        let f = Fixture::new();
        f.git(&["switch", "-c", "incoming"]);
        f.write("file.txt", "incoming\n");
        f.commit("incoming");
        f.git(&["switch", "main"]);
        f.write("file.txt", "ours\n");
        f.commit("ours");
        f.ops.run(&[kind.label(), "incoming"]).unwrap();
        let progress = f.ops.in_progress().unwrap().unwrap();
        let snapshot = f
            .ops
            .inspect_conflict(&f.ops.conflicts().unwrap()[0])
            .unwrap();
        f.ops
            .resolve_conflict(&snapshot, &Resolution::Edited("resolved\n".into()))
            .unwrap();
        f.ops.finish_operation(&progress, false).unwrap();
        assert!(f.ops.in_progress().unwrap().is_none());
    }
}
#[test]
fn replaced_parent_symlink_is_rejected() {
    let f = Fixture::new();
    let mut snapshot = f.conflict(false);
    // Public path input is rejected before any write even for a crafted selection.
    let external = f.dir.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("file.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(&external, f.ops.root.join("linked")).unwrap();
    snapshot.file.path = PathBuf::from("linked/file.txt");
    assert!(f.ops.safe_working_path(&snapshot.file.path).is_err());
    assert!(
        f.ops
            .resolve_conflict(&snapshot, &Resolution::Ours)
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(external.join("file.txt")).unwrap(),
        "outside"
    );
}

#[test]
fn binary_conflict_can_choose_exact_side_but_not_edit_text() {
    let f = Fixture::new();
    f.write("file.txt", "\0base");
    f.commit("binary base");
    f.git(&["switch", "-c", "incoming"]);
    f.write("file.txt", "\0theirs");
    f.commit("binary theirs");
    f.git(&["switch", "main"]);
    f.write("file.txt", "\0ours");
    f.commit("binary ours");
    f.ops.run(&["merge", "incoming"]).unwrap();
    let snapshot = f
        .ops
        .inspect_conflict(&f.ops.conflicts().unwrap()[0])
        .unwrap();
    assert!(
        f.ops
            .resolve_conflict(&snapshot, &Resolution::Edited("not binary".into()))
            .is_err()
    );
    f.ops
        .resolve_conflict(&snapshot, &Resolution::Theirs)
        .unwrap();
    assert_eq!(fs::read(f.ops.root.join("file.txt")).unwrap(), b"\0theirs");
    assert!(f.ops.conflicts().unwrap().is_empty());
}
#[test]
fn replaced_stage_oid_cannot_save_stale_result() {
    let f = Fixture::new();
    let snapshot = f.conflict(false);
    let oid = &snapshot.file.stages[0].oid;
    let mut command = f.ops.command(&["update-index", "--index-info"]);
    let mut child = command.stdin(Stdio::piped()).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("100644 {oid} 2\tfile.txt\n").as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(f.ops.conflicts().unwrap().len(), 1);
    assert!(
        f.ops
            .resolve_conflict(&snapshot, &Resolution::Theirs)
            .unwrap_err()
            .to_string()
            .contains("stages changed")
    );
    assert_eq!(
        fs::read(f.ops.root.join("file.txt")).unwrap(),
        snapshot.disk.unwrap().bytes
    );
}
