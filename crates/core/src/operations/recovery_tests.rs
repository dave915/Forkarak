#[test]
fn reflog_recovery_and_undo_preserve_dirty_index_and_working_bytes() {
    let f = Fixture::new();
    let op = f.op();
    assert!(op.undo_last_commit_target().is_err());
    f.commit("tracked.txt", "second\n");
    let undo = op.undo_last_commit_target().unwrap();
    fs::write(f.root.join("tracked.txt"), "staged\n").unwrap();
    f.git(&["add", "--", "tracked.txt"]);
    fs::write(f.root.join("tracked.txt"), "unstaged\n").unwrap();
    let index = fs::read(f.root.join(".git/index")).unwrap();
    op.undo_last_commit(&undo).unwrap();
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read_to_string(f.root.join("tracked.txt")).unwrap(), "unstaged\n");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), undo.parent_oid);
    assert!(op.undo_last_commit(&undo).is_err());
    let entries = op.reflog(20).unwrap();
    let entry = entries.iter().find(|e| e.oid == undo.current.head).unwrap();
    assert!(!entry.timestamp.is_empty());
    op.create_recovery_branch("recovered", entry).unwrap();
    assert_eq!(f.git(&["rev-parse", "refs/heads/recovered"]), undo.current.head);
    assert_eq!(f.git(&["branch", "--show-current"]), "main");
    assert!(op.create_recovery_branch("recovered", entry).is_err());
}
