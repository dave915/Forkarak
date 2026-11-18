use super::*;
use pokarak_core::{search_with_exclusions_report, worktree_change_summary_report};

fn deny_read(path: &Path) -> bool {
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    match fs::File::open(path) {
        Err(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            true
        }
        // Privileged runners can read mode 000. Do not substitute a different
        // kind of node: only permission failures on regular files can be skipped.
        Ok(_) => false,
    }
}

#[test]
fn search_continues_after_denied_files_and_reports_only_attempted_reads() {
    let repo = Repo::new();
    repo.write("a/tracked", "needle");
    repo.add("a/tracked");
    repo.commit();
    repo.write("b/hello", "한 needle");
    repo.write("z/untracked", "needle");
    if !deny_read(&repo.root.join("a/tracked")) || !deny_read(&repo.root.join("z/untracked")) {
        return;
    }

    let search_report = |limit, rules: &[String]| {
        search_with_exclusions_report(&repo.root.join("a"), "needle", limit, rules).unwrap()
    };
    let all = search_report(10, &[]);
    assert_eq!(
        all.skipped_paths,
        [Path::new("a/tracked"), Path::new("z/untracked")]
    );
    assert_eq!(all.matches.len(), 1);
    assert_eq!(
        all.matches[0].path,
        repo.root.canonicalize().unwrap().join("b/hello")
    );
    assert_eq!(all.matches[0].column, 2);
    assert_eq!(
        search_report(1, &[]).skipped_paths,
        [Path::new("a/tracked")]
    );
    assert_eq!(
        search_report(10, &["a/".into()]).skipped_paths,
        [Path::new("z/untracked")]
    );
    assert!(search_report(0, &[]).skipped_paths.is_empty());
    assert_eq!(search(&repo.root, "needle", 10).unwrap().len(), 1);
}

#[test]
fn partial_counts_fingerprints_and_strict_guards_follow_selected_worktree() {
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
    fs::write(linked.join("new"), "changed\n").unwrap();
    // Same bytes as the tracked file: a skipped tracked path must never become
    // a deletion or a rename source for this addition.
    fs::write(linked.join("copy"), "first\nsecond\nthird\n").unwrap();
    fs::write(linked.join("private"), "untracked").unwrap();
    let mode = CompareMode::Uncommitted;
    let complete = snapshot_current(&linked, &mode).unwrap();
    let complete_summary = worktree_change_summary_report(&linked).unwrap();
    assert_eq!(complete.worktree.changed_files, 3);
    assert!(complete.skipped_paths.is_empty());
    let index_path = repository_watch_paths(&linked).unwrap()[1].join("index");
    let original_index = fs::read(&index_path).unwrap();
    let tracked = linked.join("src/code.rs");
    let untracked = linked.join("private");
    if !deny_read(&tracked) || !deny_read(&untracked) {
        return;
    }

    let expected = [Path::new("private"), Path::new("src/code.rs")];
    let partial = snapshot_current(&linked, &mode).unwrap();
    let inventory = snapshot_current_inventory(&linked, &mode).unwrap();
    assert_eq!(partial.fingerprint, inventory.fingerprint);
    assert_ne!(partial.fingerprint, complete.fingerprint);
    for snap in [
        partial,
        inventory,
        snapshot(&linked, &mode).unwrap(),
        snapshot_current(&linked, &CompareMode::Branch("main".into())).unwrap(),
    ] {
        assert_eq!(snap.skipped_paths, expected);
        assert_eq!(snap.files, complete.files);
        assert_eq!(snap.changes.len(), 2);
        assert_eq!(change(&snap, "copy").status, FileStatus::Untracked);
        assert_eq!(change(&snap, "new").status, FileStatus::Untracked);
        assert_eq!(snap.worktree.changed_files, 2);
        assert!(!snap.worktree.changed_files_available);
    }
    let summary = worktree_change_summary_report(&linked).unwrap();
    assert_eq!(summary.changed_files, 2);
    assert_eq!(summary.skipped_paths, expected);
    assert_ne!(summary.fingerprint, complete_summary.fingerprint);
    // The worktree manager and the staging dialog require a complete read.
    assert!(pokarak_core::worktree_change_count(&linked).is_err());
    assert!(pokarak_core::worktree_change_summary(&linked).is_err());
    assert!(pokarak_core::index::snapshot(&linked).is_err());
    assert!(read_file(&linked, Path::new("src/code.rs")).is_err());
    let main = repo.snap();
    assert!(main.skipped_paths.is_empty() && main.worktree.changed_files_available);
    let sibling = main
        .worktrees
        .iter()
        .find(|w| w.branch == "linked")
        .unwrap();
    assert!(!sibling.changed_files_available && sibling.inspection_error.is_some());

    fs::set_permissions(&tracked, fs::Permissions::from_mode(0o644)).unwrap();
    let one_skipped = worktree_change_summary_report(&linked).unwrap();
    assert_eq!(one_skipped.changed_files, summary.changed_files);
    assert_eq!(one_skipped.skipped_paths, [Path::new("private")]);
    assert_ne!(one_skipped.fingerprint, summary.fingerprint);
    fs::set_permissions(untracked, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        snapshot_current(&linked, &mode).unwrap().fingerprint,
        complete.fingerprint
    );
    assert_eq!(
        worktree_change_summary_report(&linked).unwrap().fingerprint,
        complete_summary.fingerprint
    );
    assert_eq!(fs::read(index_path).unwrap(), original_index);
}

#[test]
fn skipping_the_only_file_is_distinct_from_clean_and_deleted() {
    let repo = Repo::new();
    repo.seed();
    let clean = repo.snap();
    let path = repo.root.join("src/code.rs");
    if !deny_read(&path) {
        return;
    }
    let partial = repo.snap();
    assert_eq!(partial.skipped_paths, [Path::new("src/code.rs")]);
    assert!(partial.changes.is_empty());
    assert_eq!(partial.worktree.changed_files, 0);
    assert!(!partial.worktree.changed_files_available);
    assert_ne!(partial.fingerprint, clean.fingerprint);
    assert_eq!(partial.fingerprint, repo.snap().fingerprint);

    fs::remove_file(&path).unwrap();
    let missing = repo.snap();
    assert!(missing.skipped_paths.is_empty());
    assert_eq!(change(&missing, "src/code.rs").status, FileStatus::Deleted);
    assert_ne!(missing.fingerprint, partial.fingerprint);
}

#[test]
fn broken_repository_metadata_is_fatal_even_with_a_denied_working_file() {
    let repo = Repo::new();
    repo.seed();
    fs::set_permissions(
        repo.root.join("src/code.rs"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    fs::write(repo.root.join(".git/index"), b"invalid index").unwrap();
    assert!(search_with_exclusions_report(&repo.root, "needle", 10, &[]).is_err());
    assert!(worktree_change_summary_report(&repo.root).is_err());
    assert!(snapshot_current_inventory(&repo.root, &CompareMode::Uncommitted).is_err());
    assert!(snapshot(&repo.root, &CompareMode::Uncommitted).is_err());
}
