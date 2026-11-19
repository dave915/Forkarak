use super::super::{ExecutionUi, LogSource, TreeConfig};
use super::{Launch, LogOptions, Request, RunId, Selection, TestRuns, TestSummary, TestTarget};
use crate::task_runtime::TaskProfile;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn repository(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    fs::create_dir(&root).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    root.canonicalize().unwrap()
}

fn target(name: &str, command: &[&str]) -> TestTarget {
    TestTarget {
        label: name.to_owned(),
        profile: TaskProfile {
            name: format!("테스트 · {name}"),
            command: command.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: ".".into(),
            env: Default::default(),
        },
        report_dirs: vec![],
    }
}

fn request(runs: &mut TestRuns, root: &Path) -> Request {
    Request {
        root: root.to_owned(),
        project: root.to_owned(),
        generation: runs.next_request(),
        workspace: 1,
        navigation: 1,
        origin: None,
    }
}

fn launch(request: Request, target: TestTarget) -> Launch {
    Launch {
        request,
        target,
        require_clean: true,
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "test process failed to settle");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn untrusted_request_never_starts_or_stops_a_process() {
    let temp = tempfile::tempdir().unwrap();
    let root = repository(temp.path(), "untrusted");
    let mut tests = TestRuns::default();
    let marker = target("marker", &["/usr/bin/touch", "started"]);
    let req = request(&mut tests, &root);
    tests.awaiting_trust = Some((req.clone(), Selection::Previous(marker.clone())));
    assert!(tests.runtime.runs(&root).is_empty());
    assert!(
        tests
            .submit(
                launch(req.clone(), marker.clone()),
                false,
                LogOptions::default()
            )
            .is_err()
    );
    assert!(!root.join("started").exists());
    assert!(tests.results.is_empty());
    assert!(tests.after_stop.is_empty());

    let long = target("active", &["/bin/sleep", "60"]);
    let id = tests
        .submit(launch(req.clone(), long), true, LogOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        tests
            .submit(launch(req, marker), false, LogOptions::default())
            .is_err()
    );
    assert!(tests.run(&root, id).unwrap().process.status().active());
    assert!(!tests.results[&root].cancelled);
    tests.shutdown();
    assert!(tests.runtime.runs(&root).is_empty());
    assert!(tests.awaiting_trust.is_none());
}

#[test]
fn accepted_replacement_survives_another_worktree_and_saved_profile_with_same_name() {
    let temp = tempfile::tempdir().unwrap();
    let a = repository(temp.path(), "worktree a");
    let b = repository(temp.path(), "worktree b");
    let options = LogOptions::default();
    let config_path = temp.path().join("session/execution.json");
    let extensions = temp.path().join("extensions");
    let mut ui = ExecutionUi::load(config_path.clone(), extensions.clone());
    let long = target("same", &["/bin/sleep", "60"]);
    ui.config.worktrees.insert(
        a.clone(),
        TreeConfig {
            profiles: Some(vec![long.profile.clone()]),
            ..Default::default()
        },
    );
    ui.save().unwrap();
    let before = fs::read(&config_path).unwrap();
    let saved = &mut ui.tasks;
    let tests = &mut ui.tests;
    let saved_id = saved
        .start(&a, long.profile.clone(), true, options)
        .unwrap();
    let a_req = request(tests, &a);
    let old = tests
        .submit(launch(a_req, long.clone()), true, options)
        .unwrap()
        .unwrap();
    assert!(!std::sync::Arc::ptr_eq(
        &saved.run(&a, &long.profile.name).unwrap().logs,
        &tests.logs(&a, old).unwrap().buffer
    ));

    let replace = target("finished", &["/usr/bin/touch", "replacement"]);
    let a_req = request(tests, &a);
    assert!(
        tests
            .submit(launch(a_req, replace.clone()), true, options)
            .unwrap()
            .is_none()
    );
    let b_req = request(tests, &b);
    let sibling = tests
        .submit(launch(b_req, long.clone()), true, options)
        .unwrap()
        .unwrap();
    assert!(tests.after_stop.contains_key(&a));
    assert!(!tests.finish_report(
        &a,
        old,
        Ok(Some(TestSummary::default())),
        Default::default()
    ));
    wait_until(|| !tests.run(&a, old).unwrap().process.status().active());
    let (pending, options) = tests.ready_replacements().pop().unwrap();
    assert_eq!(pending.request.root, a);
    let winner = tests.submit(pending, true, options).unwrap().unwrap();
    wait_until(|| !tests.run(&a, winner).unwrap().process.status().active());
    assert!(a.join("replacement").is_file());
    assert!(!b.join("replacement").exists());
    assert!(tests.logs(&a, old).is_none());
    assert!(!tests.stop(&a, old));
    assert!(!tests.finish_report(
        &a,
        old,
        Ok(Some(TestSummary::default())),
        Default::default()
    ));
    assert!(!tests.finish_report(
        &b,
        winner,
        Ok(Some(TestSummary::default())),
        Default::default()
    ));
    assert!(tests.finish_report(
        &a,
        winner,
        Ok(Some(TestSummary {
            total: 3,
            ..Default::default()
        })),
        Default::default()
    ));
    assert_eq!(tests.results[&a].summary.as_ref().unwrap().total, 3);
    assert!(tests.results[&b].summary.is_none());

    let previous = tests.results[&a].target.clone();
    assert_eq!(previous.profile.command, replace.profile.command);
    fs::remove_file(a.join("replacement")).unwrap();
    let req = request(tests, &a);
    let rerun = tests
        .submit(launch(req, previous), true, options)
        .unwrap()
        .unwrap();
    assert_ne!(winner, rerun);
    wait_until(|| a.join("replacement").exists());
    assert!(tests.run(&b, sibling).unwrap().process.status().active());
    tests.shutdown_root(&b);
    assert!(tests.logs(&b, sibling).is_none());
    assert!(tests.results.contains_key(&a));
    assert!(
        saved
            .run(&a, &long.profile.name)
            .unwrap()
            .process
            .status()
            .active()
    );

    tests.shutdown();
    let req = request(tests, &a);
    let next = tests
        .submit(launch(req, long.clone()), true, options)
        .unwrap()
        .unwrap();
    assert!(next.0 > rerun.0);
    assert!(!tests.finish_report(&a, rerun, Ok(None), Default::default()));
    let host = crate::extension_host::ExtensionHost::default();
    assert!(
        LogSource::Task(long.profile.name.clone(), saved_id)
            .logs(&a, saved, tests, &host)
            .is_some()
    );
    assert!(
        LogSource::Test(RunId(0))
            .logs(&a, saved, tests, &host)
            .is_none()
    );
    assert!(tests.stop(&a, next));
    wait_until(|| !tests.run(&a, next).unwrap().process.status().active());
    assert!(
        saved
            .run(&a, &long.profile.name)
            .unwrap()
            .process
            .status()
            .active()
    );
    ui.save().unwrap();
    assert_eq!(fs::read(&config_path).unwrap(), before);
    let restored = ExecutionUi::load(config_path, extensions);
    assert!(restored.tests.results.is_empty());
    let profiles = restored.config.worktrees[&a].profiles.as_ref().unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].command, long.profile.command);
}

#[test]
fn save_all_launch_survives_language_view_invalidation_but_rejects_navigation() {
    use super::{CaretSource, RequestIdentity};
    use crate::ui::code_edit::FileKey;
    use gpui_kit::EntityId;
    use pokarak_navigation::DocumentSnapshot;

    let temp = tempfile::tempdir().unwrap();
    let root = repository(temp.path(), "save and run");
    let path = Path::new("checkout.test.js");
    fs::write(root.join(path), "disk").unwrap();
    let mut tests = TestRuns::default();
    let mut req = request(&mut tests, &root);
    let editor = EntityId::from(71);
    let snapshot = DocumentSnapshot::new(root.clone(), path.to_owned(), "draft".into(), 4).unwrap();
    req.origin = Some(RequestIdentity {
        key: FileKey::new(&root, path).unwrap(),
        editor,
        selection: 0..0,
        cursor: 0,
        settings: Default::default(),
        view: 7,
        snapshot: snapshot.clone(),
    });
    assert!(req.matches_reader(
        path,
        editor,
        1,
        Some(CaretSource {
            view: 7,
            cursor: 0,
            text: "draft"
        })
    ));
    pokarak_core::operations::save_file(&root, path, "disk", "draft").unwrap();
    snapshot.invalidate();
    // Save All temporarily excluded the read-only editor from language analysis (view 7 -> 9).
    assert!(!req.matches_reader(
        path,
        editor,
        1,
        Some(CaretSource {
            view: 9,
            cursor: 0,
            text: "draft"
        })
    ));
    assert!(req.matches_reader(path, editor, 1, None));
    assert!(!req.matches_reader(path, editor, 2, None));
    assert!(!req.matches_reader(Path::new("other.test.js"), editor, 1, None));
    assert!(!req.matches_reader(path, EntityId::from(72), 1, None));
    let target = target("checkout", &["/bin/cp", "checkout.test.js", "observed"]);
    let id = tests
        .submit(launch(req, target), true, LogOptions::default())
        .unwrap()
        .unwrap();
    wait_until(|| !tests.run(&root, id).unwrap().process.status().active());
    assert_eq!(fs::read(root.join("observed")).unwrap(), b"draft");
}

#[test]
fn stop_cancels_an_accepted_replacement_and_late_stop_does_not_cancel_its_successor() {
    let temp = tempfile::tempdir().unwrap();
    let root = repository(temp.path(), "cancel");
    let mut tests = TestRuns::default();
    let req = request(&mut tests, &root);
    let previous = target("long", &["/bin/sleep", "60"]);
    let active = tests
        .submit(
            launch(req.clone(), previous.clone()),
            true,
            LogOptions::default(),
        )
        .unwrap()
        .unwrap();
    let replacement = target("touch", &["/usr/bin/touch", "started"]);
    assert!(
        tests
            .submit(
                launch(req.clone(), replacement),
                true,
                LogOptions::default()
            )
            .unwrap()
            .is_none()
    );
    tests.cancel(&root, active);
    wait_until(|| !tests.run(&root, active).unwrap().process.status().active());
    assert!(tests.ready_replacements().is_empty());
    assert!(!root.join("started").exists());
    let next = tests
        .submit(launch(req, previous), true, LogOptions::default())
        .unwrap()
        .unwrap();
    tests.cancel(&root, active);
    assert!(tests.run(&root, next).unwrap().process.status().active());
    assert!(!tests.results[&root].cancelled);
}
