#[test]
fn previews_are_local_immutable_and_execution_rejects_changed_sources() {
    let f = Fixture::new();
    let remote = f.add_origin();
    let op = f.op();
    let before = f.at(&remote, &["for-each-ref"]);
    let preview = op.preview_push("origin", "main", "main").unwrap();
    assert!(!preview.remote_state_known);
    assert!(!preview.commits.is_empty());
    assert_eq!(f.at(&remote, &["for-each-ref"]), before);
    f.commit("later", "later\n");
    assert!(op.execute_push_preview(&preview, false).is_err());
    let preview = op.preview_push("origin", "main", "main").unwrap();
    op.execute_push_preview(&preview, true).unwrap();
    let preview = op.preview_push("origin", "main", "main").unwrap();
    assert!(preview.remote_state_known);
    assert!(preview.commits.is_empty());
    f.git(&["branch", "incoming"]);
    f.git(&["switch", "incoming"]);
    f.commit("incoming.txt", "incoming\n");
    f.git(&["switch", "main"]);
    let choice = op.integration_choices().unwrap().into_iter().find(|c| c.reference == "refs/heads/incoming").unwrap();
    let preview = op.preview_integration(super::super::integration::IntegrationKind::Merge, &choice).unwrap();
    assert_eq!(preview.commits.len(), 1);
    assert!(preview.files.iter().any(|f| f.path == Path::new("incoming.txt")));
    op.execute_integration_preview(&preview).unwrap();
    assert!(f.root.join("incoming.txt").exists());
}
#[test]
fn push_preview_honors_custom_tracking_mapping_and_rejects_remote_changes() {
    let f = Fixture::new();
    let remote = f.add_origin();
    let op = f.op();
    op.push_branch("origin", "main", "main", false).unwrap();
    let head = f.git(&["rev-parse", "HEAD"]);
    f.git(&["config", "--local", "--replace-all", "remote.origin.fetch", "+refs/heads/*:refs/known/*"]);
    f.git(&["update-ref", "refs/known/main", &head]);
    f.commit("local", "local\n");
    let preview = op.preview_push("origin", "main", "main").unwrap();
    assert_eq!(preview.base_oid.as_deref(), Some(head.as_str()));
    assert_eq!(preview.commits.len(), 1);
    f.git(&["config", "--local", "remote.origin.pushurl", "/nonexistent/other.git"]);
    assert!(op.execute_push_preview(&preview, false).is_err());
    assert_eq!(f.at(&remote, &["rev-parse", "refs/heads/main"]), head);
    let unknown = op.preview_push("origin", "main", "main").unwrap();
    assert!(!unknown.remote_state_known);
}
#[test]
fn retained_rebase_preview_replays_only_reviewed_branch_and_rejects_stale_worktrees() {
    let f = Fixture::new();
    f.git(&["branch", "destination"]);
    f.commit("local", "local\n");
    f.git(&["switch", "destination"]);
    f.commit("upstream", "upstream\n");
    f.git(&["switch", "main"]);
    let op = f.op();
    let choice = op.integration_choices().unwrap().into_iter().find(|c| c.reference == "refs/heads/destination").unwrap();
    let preview = op.preview_integration(super::super::integration::IntegrationKind::Rebase, &choice).unwrap();
    assert_eq!(preview.commits.len(), 1);
    assert_eq!(preview.commits[0].oid, choice.current.head);
    let linked = f.temp.path().join("linked preview");
    f.git(&["worktree", "add", "--detach", linked.to_str().unwrap(), "HEAD"]);
    assert!(GitOperations::open(&linked).unwrap().execute_integration_preview(&preview).is_err());
    op.execute_integration_preview(&preview).unwrap();
    assert!(f.root.join("local").exists());
    assert!(f.root.join("upstream").exists());
    assert!(op.execute_integration_preview(&preview).is_err());
}
