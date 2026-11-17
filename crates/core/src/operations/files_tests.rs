#[test]
fn guarded_file_actions_no_overwrite_traversal_symlink_or_stale_source() {
    use super::super::files::*;
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    create_directory(&f.root, Path::new("folder")).unwrap();
    create_file(&f.root, Path::new("folder/new.txt"), b"new\n").unwrap();
    let source = inspect_path(&f.root, Path::new("folder/new.txt")).unwrap();
    assert!(create_file(&f.root, &source.path, b"overwrite").is_err());
    move_path(&f.root, &source, Path::new("folder/renamed.txt")).unwrap();
    assert!(!f.root.join("folder/new.txt").exists());
    assert_eq!(fs::read(f.root.join("folder/renamed.txt")).unwrap(), b"new\n");
    let source = inspect_path(&f.root, Path::new("folder/renamed.txt")).unwrap();
    assert!(move_path(&f.root, &source, Path::new("tracked.txt")).is_err());
    fs::write(f.root.join(&source.path), b"changed").unwrap();
    assert!(move_path(&f.root, &source, Path::new("folder/stale.txt")).is_err());
    let outside = f.temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, f.root.join("link")).unwrap();
    for path in ["../escape", ".git/config", "link/escape", "folder/../../escape"] {
        assert!(create_file(&f.root, Path::new(path), b"bad").is_err(), "{path}");
        assert!(create_directory(&f.root, Path::new(path)).is_err(), "{path}");
    }
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    f.git(&["init", "--", "nested"]);
    assert!(create_file(&f.root, Path::new("nested/a"), b"bad").is_err());
    assert!(inspect_path(&f.root, Path::new("nested")).is_err());
    let dir = inspect_path(&f.root, Path::new("folder")).unwrap();
    move_path(&f.root, &dir, Path::new("moved-folder")).unwrap();
    assert!(f.root.join("moved-folder/renamed.txt").exists());
}
