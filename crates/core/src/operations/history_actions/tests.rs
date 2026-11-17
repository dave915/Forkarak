use super::*;
use std::fs;
struct Fixture {
    _dir: tempfile::TempDir,
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
            cancellation: OperationCancellation::default(),
            progress: None,
            global_config: Some(global),
        };
        ops.checked(&["init", "-b", "main"]).unwrap();
        ops.checked(&["config", "user.name", "History Fixture"])
            .unwrap();
        ops.checked(&["config", "user.email", "history@example.invalid"])
            .unwrap();
        ops.checked(&["config", "commit.gpgsign", "false"]).unwrap();
        ops.root = ops.root.canonicalize().unwrap();
        let f = Self { _dir: dir, ops };
        f.commit("file", "base\n");
        f
    }
    fn git(&self, args: &[&str]) -> String {
        String::from_utf8(self.ops.metadata(args).unwrap())
            .unwrap()
            .trim_end_matches('\n')
            .to_owned()
    }
    fn write(&self, path: &str, text: &str) {
        fs::write(self.ops.root.join(path), text).unwrap();
    }
    fn commit(&self, path: &str, text: &str) -> String {
        self.write(path, text);
        self.git(&["add", "--", "."]);
        self.git(&["commit", "-m", text]);
        self.git(&["rev-parse", "HEAD"])
    }
    fn source(&self, text: &str) -> String {
        self.git(&["switch", "-c", "source"]);
        let source = self.commit("file", text);
        self.git(&["switch", "main"]);
        source
    }
}
#[test]
fn history_actions_pick_and_revert_one_commit() {
    let f = Fixture::new();
    let source = f.source("picked\n");
    f.ops
        .history_action(
            HistoryAction::CherryPick,
            &source,
            &f.ops.history_target().unwrap(),
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file")).unwrap(),
        "picked\n"
    );
    let picked = f.git(&["rev-parse", "HEAD"]);
    f.ops
        .history_action(
            HistoryAction::Revert,
            &picked,
            &f.ops.history_target().unwrap(),
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file")).unwrap(),
        "base\n"
    );
    assert!(f.ops.in_progress().unwrap().is_none());
}
#[test]
fn history_actions_conflicts_remain_for_continue_or_abort() {
    for action in [HistoryAction::CherryPick, HistoryAction::Revert] {
        let f = Fixture::new();
        let source = f.source("incoming\n");
        if action == HistoryAction::Revert {
            f.ops
                .history_action(
                    HistoryAction::CherryPick,
                    &source,
                    &f.ops.history_target().unwrap(),
                )
                .unwrap();
        }
        let head = f.commit("file", "target divergence\n");
        let target = f.ops.history_target().unwrap();
        assert!(f.ops.history_action(action, &source, &target).is_err());
        assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
        assert!(!f.ops.conflicts().unwrap().is_empty());
        let progress = f.ops.in_progress().unwrap().unwrap();
        assert_eq!(progress.kind.label(), action.label());
        assert!(
            f.ops
                .history_action(action, &source, &target)
                .unwrap_err()
                .to_string()
                .contains("already in progress")
        );
        f.ops.finish_operation(&progress, true).unwrap();
        assert_eq!(
            fs::read_to_string(f.ops.root.join("file")).unwrap(),
            "target divergence\n"
        );
    }
}
#[test]
fn history_actions_preserve_dirty_index_untracked_and_ignored() {
    for mode in ["working", "staged", "untracked", "ignored"] {
        let f = Fixture::new();
        let source = if mode == "ignored" {
            f.git(&["switch", "-c", "source"]);
            let oid = f.commit("ignored", "incoming\n");
            f.git(&["switch", "main"]);
            oid
        } else {
            f.source("incoming\n")
        };
        let target = f.ops.history_target().unwrap();
        match mode {
            "working" => f.write("file", "private working\n"),
            "staged" => {
                f.write("file", "private index\n");
                f.git(&["add", "--", "file"]);
                f.write("file", "private working\n");
            }
            "untracked" => f.write("untracked", "private\n"),
            _ => {
                f.write(".git/info/exclude", "ignored\n");
                f.write("ignored", "private\n");
            }
        }
        let before = fs::read(f.ops.root.join(".git/index")).unwrap();
        let working = fs::read(f.ops.root.join("file")).unwrap();
        assert!(
            f.ops
                .history_action(HistoryAction::CherryPick, &source, &target)
                .is_err()
        );
        assert_eq!(before, fs::read(f.ops.root.join(".git/index")).unwrap());
        assert_eq!(working, fs::read(f.ops.root.join("file")).unwrap());
        assert_eq!(f.git(&["rev-parse", "HEAD"]), target.head);
        assert!(f.ops.stashes().unwrap().is_empty());
    }
}
#[test]
fn history_actions_reject_stale_target_merge_invalid_and_cancelled() {
    let f = Fixture::new();
    let source = f.source("incoming\n");
    let stale = f.ops.history_target().unwrap();
    f.commit("different", "new head");
    assert!(
        f.ops
            .history_action(HistoryAction::CherryPick, &source, &stale)
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    let target = f.ops.history_target().unwrap();
    assert!(
        f.ops
            .history_action(HistoryAction::CherryPick, "--help", &target)
            .is_err()
    );
    f.git(&["merge", "source", "-m", "merge"]);
    let merge = f.git(&["rev-parse", "HEAD"]);
    let target = f.ops.history_target().unwrap();
    assert!(
        f.ops
            .history_action(HistoryAction::Revert, &merge, &target)
            .unwrap_err()
            .to_string()
            .contains("mainline")
    );
    let cancellation = OperationCancellation::default();
    cancellation.cancel();
    let cancelled = f.ops.clone().with_cancellation(cancellation);
    assert!(
        cancelled
            .history_action(HistoryAction::Revert, &source, &target)
            .is_err()
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), merge);
    f.git(&["switch", "--detach", "HEAD"]);
    assert!(f.ops.history_target().is_err());
}
#[test]
fn history_actions_cancel_owned_hook_and_retain_repository_state() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let source = f.source("incoming\n");
    let hooks = f.ops.root.join(".git/hooks");
    let hook = hooks.join("prepare-commit-msg");
    fs::write(&hook, "#!/bin/sh\ntouch hook-started\nsleep 30\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let target = f.ops.history_target().unwrap();
    let cancellation = OperationCancellation::default();
    let ops = f.ops.clone().with_cancellation(cancellation.clone());
    let worker =
        thread::spawn(move || ops.history_action(HistoryAction::CherryPick, &source, &target));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.ops.root.join("hook-started").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    cancellation.cancel();
    assert!(worker.join().unwrap().is_err());
    assert!(f.ops.in_progress().unwrap().is_some());
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file")).unwrap(),
        "incoming\n"
    );
}

#[test]
fn history_actions_allow_unrelated_ignored_build_output() {
    let f = Fixture::new();
    let source = f.source("incoming\n");
    fs::create_dir(f.ops.root.join("target")).unwrap();
    f.write(".git/info/exclude", "target/\n.DS_Store\n");
    f.write("target/cache", "valuable build output");
    f.write(".DS_Store", "desktop metadata");
    f.ops
        .history_action(
            HistoryAction::CherryPick,
            &source,
            &f.ops.history_target().unwrap(),
        )
        .unwrap();
    let picked = f.git(&["rev-parse", "HEAD"]);
    f.ops
        .history_action(
            HistoryAction::Revert,
            &picked,
            &f.ops.history_target().unwrap(),
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.ops.root.join("target/cache")).unwrap(),
        "valuable build output"
    );
    assert_eq!(
        fs::read_to_string(f.ops.root.join(".DS_Store")).unwrap(),
        "desktop metadata"
    );
}
#[test]
fn history_actions_preserve_ignored_file_directory_and_rename_collisions() {
    for mode in ["file_to_directory", "directory_to_file", "rename"] {
        let f = Fixture::new();
        f.git(&["switch", "-c", "source"]);
        if mode == "file_to_directory" {
            fs::create_dir(f.ops.root.join("collision")).unwrap();
            f.commit("collision/child", "incoming");
        } else if mode == "directory_to_file" {
            f.commit("collision", "incoming");
        } else {
            f.git(&["mv", "--", "file", "collision"]);
            f.git(&["commit", "-m", "rename"]);
        }
        let source = f.git(&["rev-parse", "HEAD"]);
        f.git(&["switch", "main"]);
        f.write(".git/info/exclude", "collision\n");
        let ignored = if mode == "directory_to_file" {
            fs::create_dir(f.ops.root.join("collision")).unwrap();
            "collision/local"
        } else {
            "collision"
        };
        f.write(ignored, "must survive");
        let target = f.ops.history_target().unwrap();
        let index = fs::read(f.ops.root.join(".git/index")).unwrap();
        let error = f
            .ops
            .history_action(HistoryAction::CherryPick, &source, &target)
            .unwrap_err();
        assert!(error.to_string().contains("Ignored path"), "{error:#}");
        assert_eq!(
            fs::read_to_string(f.ops.root.join(ignored)).unwrap(),
            "must survive"
        );
        assert_eq!(fs::read(f.ops.root.join(".git/index")).unwrap(), index);
        assert_eq!(f.git(&["rev-parse", "HEAD"]), target.head);
    }
}

#[test]
fn history_actions_root_commit_and_linked_worktree_target() {
    let f = Fixture::new();
    let root_commit = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "--orphan", "destination"]);
    let destination = f.commit("destination", "target base");
    let target = f.ops.history_target().unwrap();
    f.ops
        .history_action(HistoryAction::CherryPick, &root_commit, &target)
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.ops.root.join("file")).unwrap(),
        "base\n"
    );
    f.ops
        .history_action(
            HistoryAction::Revert,
            &root_commit,
            &f.ops.history_target().unwrap(),
        )
        .unwrap();
    assert!(!f.ops.root.join("file").exists());
    assert_eq!(
        fs::read_to_string(f.ops.root.join("destination")).unwrap(),
        "target base"
    );
    let linked_path = f._dir.path().join("linked worktree");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        linked_path.to_str().unwrap(),
        &destination,
    ]);
    let mut linked = f.ops.clone();
    linked.root = linked_path.canonicalize().unwrap();
    assert!(
        linked
            .history_action(HistoryAction::CherryPick, &root_commit, &target)
            .is_err()
    );
    let main_head = f.git(&["rev-parse", "HEAD"]);
    linked
        .history_action(
            HistoryAction::CherryPick,
            &root_commit,
            &linked.history_target().unwrap(),
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(linked.root.join("file")).unwrap(),
        "base\n"
    );
    assert!(!f.ops.root.join("file").exists());
    assert_eq!(f.git(&["rev-parse", "HEAD"]), main_head);
}
