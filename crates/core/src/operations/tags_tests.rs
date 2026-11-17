#[test]
fn create_and_push_only_the_explicit_tag_without_overwrite() {
    let f = Fixture::new();
    let remote = f.add_origin();
    let op = f.op();
    op.create_tag("fixture/light", "HEAD", None).unwrap();
    op.create_tag("fixture/annotated", "HEAD", Some("Fixture tag\n")).unwrap();
    assert_eq!(f.git(&["cat-file", "-t", "refs/tags/fixture/annotated"]), "tag");
    assert!(op.create_tag("fixture/light", "HEAD", None).is_err());
    assert!(op.create_tag("--all", "HEAD", None).is_err());
    op.push_tag("origin", "fixture/annotated").unwrap();
    assert_eq!(f.at(&remote, &["for-each-ref", "--format=%(refname)"]), "refs/tags/fixture/annotated");
}
