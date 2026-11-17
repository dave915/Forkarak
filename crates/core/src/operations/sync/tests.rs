use super::super::sync::Upstream;

#[test]
fn first_push_sets_only_selected_upstream_and_remote_branch() {
    let f = Fixture::new();
    let remote = f.add_origin();
    let op = f.op();
    f.git(&["branch", "other"]);
    f.git(&["config", "remote.origin.mirror", "true"]);
    f.git(&["config", "remote.origin.push", "+refs/heads/*:refs/heads/*"]);
    f.git(&["config", "push.followTags", "true"]);
    f.git(&["config", "push.autoSetupRemote", "true"]);
    f.git(&["tag", "-a", "private-tag", "-m", "local"]);
    op.push_branch("origin", "other", "published/other-name", false)
        .unwrap();
    assert_eq!(op.config_value("branch.other.remote", true).unwrap(), None);
    assert_eq!(op.config_value("branch.other.merge", true).unwrap(), None);
    op.push_branch("origin", "main", "published/main-name", true)
        .unwrap();
    assert_eq!(
        op.config_value("branch.main.remote", true)
            .unwrap()
            .as_deref(),
        Some("origin")
    );
    assert_eq!(
        op.config_value("branch.main.merge", true)
            .unwrap()
            .as_deref(),
        Some("refs/heads/published/main-name")
    );
    assert_eq!(
        f.at(&remote, &["for-each-ref", "--format=%(refname)"]),
        "refs/heads/published/main-name\nrefs/heads/published/other-name"
    );
    let sync = op.branch_sync().unwrap();
    assert_eq!(
        sync.upstream,
        Some(Upstream {
            reference: "refs/remotes/origin/published/main-name".into(),
            remote: "origin".into(),
            branch: "refs/heads/published/main-name".into(),
            missing: false
        })
    );
    assert_eq!((sync.ahead, sync.behind), (Some(0), Some(0)));
    assert!(sync.last_fetch.is_none());
    for (local, destination) in [("HEAD", "main"), ("main", "--all"), ("main", "bad:ref")] {
        assert!(op.push_branch("origin", local, destination, true).is_err());
    }
}

#[test]
fn explicit_push_rejects_non_fast_forward_without_setting_tracking() {
    let f = Fixture::new();
    let remote = f.add_origin();
    let op = f.op();
    f.git(&["branch", "old"]);
    f.commit("advanced", "new");
    op.push_branch("origin", "main", "published", false)
        .unwrap();
    let published = f.at(&remote, &["rev-parse", "refs/heads/published"]);
    assert!(op.push_branch("origin", "old", "published", true).is_err());
    assert_eq!(
        f.at(&remote, &["rev-parse", "refs/heads/published"]),
        published
    );
    assert!(
        op.config_value("branch.old.remote", true)
            .unwrap()
            .is_none()
    );
}

#[test]
fn tracking_counts_ahead_behind_missing_and_local_upstream() {
    let f = Fixture::new();
    f.add_origin();
    let op = f.op();
    op.push_branch("origin", "main", "topic/with/slashes", true)
        .unwrap();
    let initial = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "-c", "peer"]);
    f.commit("peer", "peer commit");
    let peer = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "main"]);
    f.commit("local", "local commit");
    let ahead = op.branch_sync().unwrap();
    assert_eq!((ahead.ahead, ahead.behind), (Some(1), Some(0)));
    f.git(&[
        "update-ref",
        "refs/remotes/origin/topic/with/slashes",
        &peer,
    ]);
    let diverged = op.branch_sync().unwrap();
    assert_eq!((diverged.ahead, diverged.behind), (Some(1), Some(1)));
    f.git(&["update-ref", "-d", "refs/remotes/origin/topic/with/slashes"]);
    let missing = op.branch_sync().unwrap();
    let upstream = missing.upstream.unwrap();
    assert_eq!(upstream.remote, "origin");
    assert_eq!(upstream.branch, "refs/heads/topic/with/slashes");
    assert!(upstream.missing);
    assert_eq!((missing.ahead, missing.behind), (None, None));
    f.git(&["config", "branch.main.remote", "."]);
    f.git(&["config", "branch.main.merge", "refs/heads/peer"]);
    let local = op.branch_sync().unwrap();
    assert_eq!(
        local.upstream,
        Some(Upstream {
            reference: "refs/heads/peer".into(),
            remote: ".".into(),
            branch: "refs/heads/peer".into(),
            missing: false
        })
    );
    assert_eq!((local.ahead, local.behind), (Some(1), Some(1)));
    f.git(&["branch", "-D", "peer"]);
    let local_missing = op.branch_sync().unwrap();
    assert!(local_missing.upstream.unwrap().missing);
    assert_eq!((local_missing.ahead, local_missing.behind), (None, None));
    assert_ne!(f.git(&["rev-parse", "HEAD"]), initial);
}

#[test]
fn no_upstream_detached_and_unborn_counts_are_unknown() {
    let f = Fixture::new();
    let op = f.op();
    let plain = op.branch_sync().unwrap();
    assert_eq!(plain.branch.as_deref(), Some("main"));
    assert_eq!(
        (plain.upstream, plain.ahead, plain.behind),
        (None, None, None)
    );
    f.git(&["switch", "--detach"]);
    let detached = op.branch_sync().unwrap();
    assert_eq!(
        (
            detached.branch,
            detached.upstream,
            detached.ahead,
            detached.behind
        ),
        (None, None, None, None)
    );
    f.git(&["switch", "--orphan", "unborn"]);
    f.git(&["config", "branch.unborn.remote", "origin"]);
    f.git(&["config", "branch.unborn.merge", "refs/heads/main"]);
    let unborn = op.branch_sync().unwrap();
    assert_eq!(unborn.branch.as_deref(), Some("unborn"));
    assert_eq!(
        (unborn.upstream, unborn.ahead, unborn.behind),
        (None, None, None)
    );
}

#[test]
fn custom_fetch_mapping_and_multiple_remotes_are_read_exactly_without_network() {
    let f = Fixture::new();
    f.add_origin();
    let op = f.op();
    f.git(&[
        "remote",
        "add",
        "company",
        "https://invalid.example.test/never-contact",
    ]);
    f.git(&[
        "config",
        "remote.company.fetch",
        "+refs/heads/team/*:refs/remotes/review/*",
    ]);
    f.git(&["config", "branch.main.remote", "company"]);
    f.git(&["config", "branch.main.merge", "refs/heads/team/deep/topic"]);
    f.git(&["update-ref", "refs/remotes/review/deep/topic", "HEAD"]);
    let sync = op.branch_sync().unwrap();
    assert_eq!(
        sync.upstream,
        Some(Upstream {
            reference: "refs/remotes/review/deep/topic".into(),
            remote: "company".into(),
            branch: "refs/heads/team/deep/topic".into(),
            missing: false
        })
    );
    assert_eq!((sync.ahead, sync.behind), (Some(0), Some(0)));
    // An unmappable upstream must not invent a tracking ref or counts.
    f.git(&["config", "branch.main.merge", "refs/heads/outside/mapping"]);
    let unsupported = op.branch_sync().unwrap();
    assert_eq!(
        (unsupported.upstream, unsupported.ahead, unsupported.behind),
        (None, None, None)
    );
    assert!(!f.root.join(".git/FETCH_HEAD").exists());
}

#[test]
fn fetch_time_belongs_to_selected_worktree() {
    let f = Fixture::new();
    let op = f.op();
    let linked = f.temp.path().join("linked");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        "--",
        linked.to_str().unwrap(),
    ]);
    let linked_op = GitOperations {
        root: linked.canonicalize().unwrap(),
        ..op.clone()
    };
    fs::write(f.root.join(".git/FETCH_HEAD"), "main fetch metadata").unwrap();
    assert!(linked_op.branch_sync().unwrap().last_fetch.is_none());
    let path = f.at(
        &linked,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "FETCH_HEAD",
        ],
    );
    fs::write(&path, "linked fetch metadata").unwrap();
    let earlier = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(earlier))
        .unwrap();
    assert_eq!(linked_op.branch_sync().unwrap().last_fetch, Some(earlier));
    assert_eq!(
        op.branch_sync().unwrap().last_fetch,
        Some(
            fs::metadata(f.root.join(".git/FETCH_HEAD"))
                .unwrap()
                .modified()
                .unwrap()
        )
    );
    assert_ne!(op.branch_sync().unwrap().last_fetch, Some(earlier));
}
