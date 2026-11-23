use super::super::tests::{data, fixture_git};
use super::{GitOperations, PushFormSource, PushRepository, PushTarget, ReviewSource, ReviewWrite};
use std::{env, fs, path::Path, process::Command};

#[test]
fn sidebar_branch_push_uses_the_clicked_source_and_only_its_known_upstream() {
    let (mut state, mut sync) = data();
    let mut team = state.remotes[0].clone();
    team.name = "team".into();
    state.remotes.push(team);
    let upstream = sync.upstream.as_mut().unwrap();
    upstream.remote = "team".into();
    upstream.reference = "refs/remotes/team/trunk".into();
    let (source, defaults) = PushFormSource::Selected("main").defaults(&state, Some(&sync));
    assert_eq!(source, Some("main"));
    assert_eq!(defaults.remote.as_deref(), Some("team"));
    assert_eq!(defaults.branch, "trunk");
    assert!(!defaults.set_upstream);

    let (source, defaults) = PushFormSource::Selected("unpublished").defaults(&state, Some(&sync));
    assert_eq!(source, Some("unpublished"));
    assert_eq!(defaults.remote.as_deref(), Some("origin"));
    assert_eq!(defaults.branch, "unpublished");
    assert!(!defaults.set_upstream);
    state.remotes[0].name = "fork".into();
    let (_, ambiguous) = PushFormSource::Selected("unpublished").defaults(&state, Some(&sync));
    assert_eq!(ambiguous.remote, None);
}

#[test]
fn publish_form_previews_then_tracks_the_selected_worktree_branch() {
    // Isolate the operations layer's child Git processes without changing the test runner's environment.
    const FIXTURE: &str = "FORKARAK_PUBLISH_FIXTURE";
    let Some(directory) = env::var_os(FIXTURE) else {
        let temp = tempfile::tempdir().unwrap();
        let output = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--nocapture",
            ])
            .env_clear()
            .env("PATH", env::var_os("PATH").unwrap())
            .env("HOME", temp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env(FIXTURE, temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let directory = Path::new(&directory);
    let root = directory.join("main");
    let linked = directory.join("linked");
    let bare = directory.join("team.git");
    let git = |root: &Path, args: &[&str]| fixture_git(root, Path::new("/dev/null"), args);
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "main", "--", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "seed",
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "feature/topic",
            "--",
            linked.to_str().unwrap(),
        ],
    );
    git(&root, &["init", "--bare", "--", bare.to_str().unwrap()]);
    git(
        &root,
        &["remote", "add", "--", "team", bare.to_str().unwrap()],
    );

    let ops = GitOperations::open(&linked).unwrap();
    let repo = PushRepository::read(&ops).unwrap();
    assert!(repo.validate_publish("main").is_err());
    repo.validate_publish("feature/topic").unwrap();
    let (local, defaults) = PushFormSource::Current.defaults(&repo.state, repo.sync.as_ref().ok());
    assert_eq!(local, Some("feature/topic"));
    assert_eq!(defaults.branch, "feature/topic");
    assert_eq!(defaults.remote.as_deref(), Some("team"));
    assert!(defaults.set_upstream);
    let target = PushTarget {
        local: local.unwrap().into(),
        remote: defaults.remote.unwrap(),
        branch: defaults.branch,
        set_upstream: defaults.set_upstream,
    };
    let review = target.review(&ops, &repo.state, &repo.starts).unwrap();
    assert!(git(&bare, &["for-each-ref"]).is_empty());
    assert!(ops.branch_sync().unwrap().upstream.is_none());
    let (ReviewSource::Operation(preview), Some(ReviewWrite::Push(confirmed))) =
        (review.source, review.write)
    else {
        panic!("expected the explicit Push confirmation");
    };
    ops.execute_push_preview(&preview, confirmed.set_upstream)
        .unwrap();
    assert_eq!(
        git(&bare, &["for-each-ref", "--format=%(refname)"]),
        "refs/heads/feature/topic"
    );
    let after = PushRepository::read(&ops).unwrap();
    assert!(after.validate_publish("feature/topic").is_err());
    assert_eq!(
        PushTarget::configured(&after.state, after.sync.as_ref().unwrap())
            .unwrap()
            .destination(),
        "team/feature/topic"
    );
    assert!(
        GitOperations::open(&root)
            .unwrap()
            .branch_sync()
            .unwrap()
            .upstream
            .is_none()
    );
}
