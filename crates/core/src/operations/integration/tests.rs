use super::super::{
    integration::{BranchIntegration, IntegrationKind},
    workflow::ProgressKind,
};

fn choose(op: &GitOperations, reference: &str) -> BranchIntegration {
    op.integration_choices()
        .unwrap()
        .into_iter()
        .find(|choice| choice.reference == reference)
        .unwrap()
}

#[test]
fn merge_fast_forward_then_merge_diverged_branch_with_both_parents() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["switch", "-c", "topic"]);
    f.commit("topic.txt", "topic one");
    let tip = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "main"]);
    op.integrate_branch(IntegrationKind::Merge, &choose(&op, "refs/heads/topic"))
        .unwrap();
    assert_eq!(f.git(&["rev-parse", "HEAD"]), tip);
    f.commit("main.txt", "main changes");
    let main = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "topic"]);
    f.commit("topic.txt", "topic two");
    let tip = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "main"]);
    f.git(&["tag", "main"]);
    f.git(&["config", "branch.main.mergeOptions", "--squash"]);
    op.integrate_branch(IntegrationKind::Merge, &choose(&op, "refs/heads/topic"))
        .unwrap();
    assert_eq!(
        f.git(&["show", "-s", "--format=%P", "HEAD"]),
        format!("{main} {tip}")
    );
    assert!(op.in_progress().unwrap().is_none());
}

#[test]
fn rebase_onto_remote_tracking_commit_preserves_other_branch_refs() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["switch", "-c", "work"]);
    f.commit("work-one", "one");
    let other = f.git(&["rev-parse", "HEAD"]);
    f.git(&["branch", "other"]);
    f.commit("work-two", "two");
    f.git(&["switch", "main"]);
    f.commit("upstream", "new");
    let base = f.git(&["rev-parse", "HEAD"]);
    f.git(&["update-ref", "refs/remotes/origin/main", &base]);
    f.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    f.git(&["switch", "work"]);
    f.git(&["config", "rebase.updateRefs", "true"]);
    assert!(
        op.integration_choices()
            .unwrap()
            .iter()
            .all(|t| !t.reference.ends_with("/HEAD"))
    );
    let target = choose(&op, "refs/remotes/origin/main");
    op.integrate_branch(IntegrationKind::Rebase, &target)
        .unwrap();
    assert_eq!(f.git(&["rev-parse", "HEAD~2"]), base);
    assert_eq!(f.git(&["rev-parse", "refs/heads/other"]), other);
    assert_eq!(f.git(&["symbolic-ref", "HEAD"]), "refs/heads/work");
    assert!(f.root.join("work-two").exists());
}

#[test]
fn guards_stale_target_current_head_dirty_untracked_and_checked_out_branch() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["branch", "topic"]);
    let old = choose(&op, "refs/heads/topic");
    f.commit("main", "updated");
    let main = f.git(&["rev-parse", "HEAD"]);
    assert!(op.integrate_branch(IntegrationKind::Merge, &old).is_err());
    let old = choose(&op, "refs/heads/topic");
    f.git(&["branch", "-f", "topic", &main]);
    assert!(op.integrate_branch(IntegrationKind::Rebase, &old).is_err());
    let current = choose(&op, "refs/heads/topic");
    fs::write(f.root.join("local"), "private").unwrap();
    for kind in [IntegrationKind::Merge, IntegrationKind::Rebase] {
        assert!(op.integrate_branch(kind, &current).is_err());
    }
    f.git(&["add", "--", "local"]);
    assert!(
        op.integrate_branch(IntegrationKind::Rebase, &current)
            .is_err()
    );
    assert_eq!(fs::read_to_string(f.root.join("local")).unwrap(), "private");
    f.git(&["commit", "-m", "local"]);
    f.git(&["switch", "--detach"]);
    assert!(op.integration_choices().is_err());
}

#[test]
fn conflicts_support_continue_and_abort_for_merge_and_rebase() {
    for kind in [IntegrationKind::Merge, IntegrationKind::Rebase] {
        for abort in [false, true] {
            let f = Fixture::new();
            let op = f.op();
            f.git(&["switch", "-c", "incoming"]);
            f.commit("tracked.txt", "incoming\n");
            f.git(&["switch", "main"]);
            f.commit("tracked.txt", "on main\n");
            let main = f.git(&["rev-parse", "HEAD"]);
            let incoming = choose(&op, "refs/heads/incoming");
            assert!(op.integrate_branch(kind, &incoming).is_err());
            let progress = op.in_progress().unwrap().unwrap();
            assert_eq!(
                progress.kind,
                if kind == IntegrationKind::Merge {
                    ProgressKind::Merge
                } else {
                    ProgressKind::Rebase
                }
            );
            assert!(!op.conflicts().unwrap().is_empty());
            if !abort {
                fs::write(f.root.join("tracked.txt"), "reconciled\n").unwrap();
                f.git(&["add", "--", "tracked.txt"]);
            }
            op.finish_operation(&progress, abort).unwrap();
            assert!(op.in_progress().unwrap().is_none());
            assert_eq!(f.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
            if abort {
                assert_eq!(main, f.git(&["rev-parse", "HEAD"]));
                assert_eq!(
                    fs::read_to_string(f.root.join("tracked.txt")).unwrap(),
                    "on main\n"
                );
            } else {
                assert_eq!(
                    fs::read_to_string(f.root.join("tracked.txt")).unwrap(),
                    "reconciled\n"
                );
            }
        }
    }
}

#[test]
fn ignored_changes_survive_both_integrations_including_transient_replay_paths() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["branch", "upstream"]);
    f.commit("temporary", "tracked on one replayed commit");
    f.git(&["rm", "--", "temporary"]);
    f.git(&["commit", "-m", "remove temporary"]);
    f.git(&["switch", "upstream"]);
    f.commit("private-file", "upstream content");
    f.git(&["switch", "main"]);
    fs::write(
        f.root.join(".git/info/exclude"),
        "private-file\ntemporary\nbuild/\n",
    )
    .unwrap();
    let target = choose(&op, "refs/heads/upstream");
    for file in ["private-file", "temporary"] {
        fs::write(f.root.join(file), "local content").unwrap();
        if file == "private-file" {
            assert!(
                op.integrate_branch(IntegrationKind::Merge, &target)
                    .is_err()
            );
            assert_eq!(
                fs::read_to_string(f.root.join(file)).unwrap(),
                "local content"
            );
        }
        assert!(
            op.integrate_branch(IntegrationKind::Rebase, &target)
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(f.root.join(file)).unwrap(),
            "local content"
        );
        assert_eq!(f.git(&["rev-parse", "HEAD"]), target.current.head);
        fs::remove_file(f.root.join(file)).unwrap();
    }
    fs::create_dir(f.root.join("build")).unwrap();
    fs::write(f.root.join("build/cache"), "unrelated").unwrap();
    op.integrate_branch(IntegrationKind::Rebase, &target)
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("build/cache")).unwrap(),
        "unrelated"
    );
}

#[test]
fn inferred_directory_rename_cannot_replace_an_ignored_file() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["config", "merge.directoryRenames", "true"]);
    fs::create_dir(f.root.join("old")).unwrap();
    f.commit("old/tracked", "moved anchor");
    f.git(&["switch", "-c", "incoming"]);
    f.commit("old/new", "incoming file");
    f.git(&["switch", "main"]);
    f.git(&["mv", "--", "old", "renamed"]);
    f.git(&["commit", "-m", "move directory"]);
    fs::write(f.root.join(".git/info/exclude"), "renamed/new\n").unwrap();
    fs::write(f.root.join("renamed/new"), "private bytes").unwrap();
    let chosen = choose(&op, "refs/heads/incoming");
    for kind in [IntegrationKind::Merge, IntegrationKind::Rebase] {
        assert!(
            op.integrate_branch(kind, &chosen)
                .unwrap_err()
                .to_string()
                .contains("renamed")
        );
        assert_eq!(
            fs::read(f.root.join("renamed/new")).unwrap(),
            b"private bytes"
        );
        assert_eq!(f.git(&["rev-parse", "HEAD"]), chosen.current.head);
    }
}
