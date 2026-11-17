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
