#[test]
fn explicit_init_and_local_bare_clone_do_not_reuse_destinations() {
    use super::super::repository::*;
    let f = Fixture::new();
    let init = f.temp.path().join("new init");
    let output = init_repository(&init, "initial/topic").unwrap();
    assert_eq!(output.root, init.canonicalize().unwrap());
    assert_eq!(f.at(&init, &["branch", "--show-current"]), "initial/topic");
    assert!(init_repository(&init, "main").is_err());
    assert!(init_repository(&f.temp.path().join("bad"), "--bad").is_err());
    assert!(!f.temp.path().join("bad").exists());
    let remote = f.add_origin();
    f.op().push_branch("origin", "main", "main", false).unwrap();
    f.at(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let destination = f.temp.path().join("new clone");
    let progress = OperationProgress::default();
    let output = clone_repository(remote.to_str().unwrap(), &destination, OperationCancellation::default(), progress.clone()).unwrap();
    assert_eq!(output.root, destination.canonicalize().unwrap());
    assert_eq!(fs::read(destination.join("tracked.txt")).unwrap(), b"initial\n");
    assert_eq!(f.at(&destination, &["remote"]), "origin");
    assert!(progress.drain().iter().all(|e| e.root == output.root));
    assert!(clone_repository(remote.to_str().unwrap(), &destination, OperationCancellation::default(), OperationProgress::default()).is_err());
    let token = OperationCancellation::default();
    token.cancel();
    let cancelled = f.temp.path().join("cancelled clone");
    assert!(clone_repository(remote.to_str().unwrap(), &cancelled, token, OperationProgress::default()).is_err());
    assert!(!cancelled.exists());
    assert!(init_repository(&f.root.join(".git/new"), "main").is_err());
}
