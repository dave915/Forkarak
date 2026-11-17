#[test]
fn exact_comparison_is_distinct_from_merge_base_and_survives_ref_movement() {
    let f = Fixture::new();
    let base = f.git(&["rev-parse", "HEAD"]);
    f.git(&["branch", "other"]);
    f.commit("left.txt", "left\n");
    let left = f.git(&["rev-parse", "HEAD"]);
    f.git(&["switch", "other"]);
    f.commit("right.txt", "right\n");
    let comparison = crate::comparison::compare_refs(&f.root, &left, "other").unwrap();
    assert_eq!(comparison.left_oid, left);
    assert_ne!(comparison.left_oid, base);
    assert_eq!(comparison.files.len(), 2);
    let patch = crate::comparison::diff(&f.root, &comparison, Path::new("left.txt")).unwrap();
    assert!(patch.patch.contains("-left"));
    f.commit("right.txt", "moved\n");
    let patch = crate::comparison::diff(&f.root, &comparison, Path::new("right.txt")).unwrap();
    assert!(patch.patch.contains("+right"));
    assert!(!patch.patch.contains("moved"));
    assert!(crate::comparison::compare_refs(&f.root, "--all", "HEAD").is_err());
    let linked = f.temp.path().join("linked");
    f.git(&["worktree", "add", "--detach", linked.to_str().unwrap(), "HEAD"]);
    assert!(crate::comparison::diff(&linked, &comparison, Path::new("right.txt")).is_err());
}
