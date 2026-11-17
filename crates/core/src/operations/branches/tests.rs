use super::super::branches::BranchStart;

fn start(op: &GitOperations, reference: &str) -> BranchStart {
    op.branch_starts()
        .unwrap()
        .into_iter()
        .find(|s| s.reference == reference)
        .unwrap()
}

#[test]
fn choices_are_full_refs_excluding_symbolic_remote_head() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["update-ref", "refs/remotes/origin/topic/deep", "HEAD"]);
    f.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/topic/deep",
    ]);
    assert_eq!(
        op.branch_starts()
            .unwrap()
            .iter()
            .map(|s| s.reference.as_str())
            .collect::<Vec<_>>(),
        ["HEAD", "refs/heads/main", "refs/remotes/origin/topic/deep"]
    );
    f.git(&["switch", "--detach"]);
    assert_eq!(start(&op, "HEAD").oid, f.git(&["rev-parse", "HEAD"]));
    f.git(&["switch", "--orphan", "unborn"]);
    assert!(
        op.branch_starts()
            .unwrap()
            .iter()
            .all(|s| s.reference != "HEAD")
    );
}

#[test]
fn selected_ref_and_explicit_commit_create_without_switch_or_automatic_tracking() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["branch", "old"]);
    let older = start(&op, "refs/heads/old");
    f.commit("new", "later");
    f.git(&["tag", "old"]);
    let head = start(&op, "HEAD").oid;
    f.git(&["config", "branch.autoSetupMerge", "always"]);
    op.create_branch_from("from/old", &older, false, false, &head, Some("main"))
        .unwrap();
    let explicit = BranchStart {
        reference: older.oid.clone(),
        oid: older.oid.clone(),
    };
    op.create_branch_from("from/commit", &explicit, false, false, &head, Some("main"))
        .unwrap();
    for branch in ["from/old", "from/commit"] {
        assert_eq!(
            f.git(&["rev-parse", &format!("refs/heads/{branch}")]),
            older.oid
        );
        assert!(
            op.config_value(&format!("branch.{branch}.remote"), true)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(f.git(&["branch", "--show-current"]), "main");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn remote_tracking_create_and_switch_uses_custom_mapping() {
    let f = Fixture::new();
    f.add_origin();
    let op = f.op();
    f.git(&[
        "config",
        "remote.origin.fetch",
        "+refs/heads/team/*:refs/remotes/review/*",
    ]);
    f.git(&["branch", "topic"]);
    f.git(&["switch", "topic"]);
    f.commit("topic-file", "selected remote content");
    let selected = f.git(&["rev-parse", "HEAD"]);
    f.git(&["update-ref", "refs/remotes/review/deep/topic", &selected]);
    f.git(&["switch", "main"]);
    let head = start(&op, "HEAD").oid;
    let remote = start(&op, "refs/remotes/review/deep/topic");
    op.create_branch_from("work/deep", &remote, true, true, &head, Some("main"))
        .unwrap();
    assert_eq!(f.git(&["branch", "--show-current"]), "work/deep");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), selected);
    assert_eq!(
        fs::read_to_string(f.root.join("topic-file")).unwrap(),
        "selected remote content"
    );
    assert_eq!(
        op.config_value("branch.work/deep.remote", true)
            .unwrap()
            .as_deref(),
        Some("origin")
    );
    assert_eq!(
        op.config_value("branch.work/deep.merge", true)
            .unwrap()
            .as_deref(),
        Some("refs/heads/team/deep/topic")
    );
    assert_eq!(
        op.branch_sync().unwrap().upstream.unwrap().reference,
        remote.reference
    );
}

#[test]
fn stale_source_head_branch_and_occupied_destinations_are_rejected() {
    let f = Fixture::new();
    let op = f.op();
    f.git(&["branch", "selected"]);
    let old_head = start(&op, "HEAD").oid;
    let source = start(&op, "refs/heads/selected");
    f.commit("advanced", "new head");
    assert!(
        op.create_branch_from("new", &source, false, false, &old_head, Some("main"))
            .is_err()
    );
    let head = start(&op, "HEAD").oid;
    f.git(&["branch", "same-head"]);
    f.git(&["switch", "same-head"]);
    assert!(
        op.create_branch_from("new", &source, true, false, &head, Some("main"))
            .is_err()
    );
    f.git(&["switch", "main"]);
    f.git(&["update-ref", "refs/heads/selected", &head]);
    assert!(
        op.create_branch_from("new", &source, false, false, &head, Some("main"))
            .is_err()
    );
    f.git(&["branch", "-D", "selected"]);
    assert!(
        op.create_branch_from("new", &source, false, false, &head, Some("main"))
            .is_err()
    );
    let source = start(&op, "HEAD");
    for name in ["main", "same-head", "--bad", "HEAD", "bad..name"] {
        assert!(
            op.create_branch_from(name, &source, true, false, &head, Some("main"))
                .is_err()
        );
    }
    // An unborn branch can be occupied without a local branch ref.
    let linked = f.temp.path().join("linked");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        "--",
        linked.to_str().unwrap(),
    ]);
    f.at(&linked, &["switch", "--orphan", "unborn-occupied"]);
    assert!(
        op.create_branch_from(
            "unborn-occupied",
            &source,
            false,
            false,
            &head,
            Some("main")
        )
        .is_err()
    );
    assert!(op.branch_commit("refs/heads/new").unwrap().is_none());
}

#[test]
fn switching_protects_tracked_untracked_and_ignored_files_without_leaving_branch() {
    for collision in ["tracked", "untracked", "ignored", "ignored-directory"] {
        let f = Fixture::new();
        let op = f.op();
        f.git(&["switch", "-c", "source"]);
        f.commit("tracked.txt", "source version");
        fs::create_dir(f.root.join("directory")).unwrap();
        f.commit("directory/file", "incoming file");
        f.commit("collision", "source version");
        f.git(&["switch", "main"]);
        let path = match collision {
            "tracked" => "tracked.txt",
            "ignored-directory" => "directory",
            _ => "collision",
        };
        fs::write(f.root.join(path), "private local content").unwrap();
        if collision.starts_with("ignored") {
            fs::write(f.root.join(".git/info/exclude"), format!("{path}\n")).unwrap();
        }
        let source = start(&op, "refs/heads/source");
        let head = start(&op, "HEAD").oid;
        f.git(&["config", "merge.autostash", "true"]);
        assert!(
            op.create_branch_from("new", &source, true, false, &head, Some("main"))
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(f.root.join(path)).unwrap(),
            "private local content",
            "{collision}"
        );
        assert_eq!(f.git(&["branch", "--show-current"]), "main");
        assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
        assert!(op.branch_commit("refs/heads/new").unwrap().is_none());
        assert!(op.metadata(&["stash", "list"]).unwrap().is_empty());
        // Creating only a branch leaves these files and the worktree untouched.
        op.create_branch_from("branch-only", &source, false, false, &head, Some("main"))
            .unwrap();
        assert_eq!(
            fs::read_to_string(f.root.join(path)).unwrap(),
            "private local content"
        );
    }
}

#[test]
fn tracking_rejects_local_or_ambiguous_unmapped_and_excluded_refs() {
    let f = Fixture::new();
    let remote = f.add_origin();
    let op = f.op();
    let head = start(&op, "HEAD");
    assert!(
        op.create_branch_from("invalid", &head, false, true, &head.oid, Some("main"))
            .is_err()
    );
    f.git(&["update-ref", "refs/remotes/origin/topic", "HEAD"]);
    let source = start(&op, "refs/remotes/origin/topic");
    f.git(&[
        "config",
        "--add",
        "remote.origin.fetch",
        "^refs/heads/topic",
    ]);
    assert!(
        op.create_branch_from("excluded", &source, false, true, &head.oid, Some("main"))
            .is_err()
    );
    f.git(&["config", "--unset-all", "remote.origin.fetch"]);
    assert!(
        op.create_branch_from("unmapped", &source, false, true, &head.oid, Some("main"))
            .is_err()
    );
    f.git(&[
        "config",
        "remote.origin.fetch",
        "+refs/heads/*:refs/remotes/origin/*",
    ]);
    f.git(&["remote", "add", "duplicate", remote.to_str().unwrap()]);
    f.git(&[
        "config",
        "remote.duplicate.fetch",
        "+refs/heads/*:refs/remotes/origin/*",
    ]);
    assert!(
        op.create_branch_from("ambiguous", &source, false, true, &head.oid, Some("main"))
            .is_err()
    );
    let short = BranchStart {
        reference: "origin/topic".into(),
        oid: head.oid.clone(),
    };
    assert!(
        op.create_branch_from("short-ref", &short, false, false, &head.oid, Some("main"))
            .is_err()
    );
    assert!(op.branch_commit("refs/heads/invalid").unwrap().is_none());
}

#[test]
fn explicit_commit_start_supports_detached_and_unborn_head_guards() {
    let f = Fixture::new();
    let op = f.op();
    let head = start(&op, "HEAD").oid;
    let explicit = BranchStart {
        reference: head.clone(),
        oid: head.clone(),
    };
    f.git(&["switch", "--detach"]);
    op.create_branch_from("detached-start", &explicit, false, false, &head, None)
        .unwrap();
    f.git(&["switch", "--orphan", "unborn"]);
    assert!(
        op.create_branch_from("stale", &explicit, false, false, &head, None)
            .is_err()
    );
    op.create_branch_from("unborn-start", &explicit, true, false, "", Some("unborn"))
        .unwrap();
    assert_eq!(f.git(&["branch", "--show-current"]), "unborn-start");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    let blob = f.git(&["hash-object", "--", "tracked.txt"]);
    let invalid = BranchStart {
        reference: blob.clone(),
        oid: blob,
    };
    assert!(
        op.create_branch_from("blob", &invalid, false, false, &head, Some("unborn-start"))
            .is_err()
    );
}

#[test]
fn rename_preserves_tracking_and_rejects_stale_occupied_or_existing_target() {
    let f = Fixture::new();
    f.add_origin();
    let op = f.op();
    op.push_branch("origin", "main", "published/main", true)
        .unwrap();
    let old = start(&op, "HEAD").oid;
    f.commit("advance", "new");
    assert!(op.rename_branch("main", "new", &old).is_err());
    let current = start(&op, "HEAD").oid;
    f.git(&["branch", "exists"]);
    assert!(op.rename_branch("main", "exists", &current).is_err());
    let linked = f.temp.path().join("linked");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        "--",
        linked.to_str().unwrap(),
    ]);
    assert!(op.rename_branch("linked", "nope", &current).is_err());
    op.rename_branch("main", "renamed/deep", &current).unwrap();
    assert_eq!(f.git(&["branch", "--show-current"]), "renamed/deep");
    assert_eq!(
        op.config_value("branch.renamed/deep.remote", true)
            .unwrap()
            .as_deref(),
        Some("origin")
    );
    assert_eq!(
        op.config_value("branch.renamed/deep.merge", true)
            .unwrap()
            .as_deref(),
        Some("refs/heads/published/main")
    );
    assert!(op.branch_commit("refs/heads/main").unwrap().is_none());
}
