#[test]
fn ignore_preview_uses_real_git_rules_without_writes_and_apply_is_guarded() {
    use super::super::ignore::*;
    let f = Fixture::new();
    f.git(&["config", "--local", "core.excludesFile", ""]);
    fs::write(f.root.join(".gitignore"), "*.log\n").unwrap();
    fs::write(f.root.join("old.log"), "old").unwrap();
    fs::write(f.root.join("new.tmp"), "new").unwrap();
    fs::write(f.root.join(":(glob)*.tmp"), "literal").unwrap();
    fs::write(f.root.join("line\nbreak.tmp"), "newline").unwrap();
    fs::write(f.root.join("keep.tmp"), "keep").unwrap();
    fs::create_dir(f.root.join("nested")).unwrap();
    fs::write(f.root.join("nested/.gitignore"), "!keep.tmp\n").unwrap();
    fs::write(f.root.join("nested/keep.tmp"), "nested keep").unwrap();
    let index = fs::read(f.root.join(".git/index")).unwrap();
    let preview = preview_gitignore(&f.root, Path::new(".gitignore"), Some("*.log\n"), "*.tmp\ntracked.txt\n").unwrap();
    assert!(preview.newly_ignored.contains(&PathBuf::from("new.tmp")));
    assert!(preview.newly_ignored.contains(&PathBuf::from(":(glob)*.tmp")));
    assert!(preview.newly_ignored.contains(&PathBuf::from("line\nbreak.tmp")));
    assert!(preview.newly_unignored.contains(&PathBuf::from("old.log")));
    assert!(!preview.newly_ignored.contains(&PathBuf::from("tracked.txt")));
    assert!(!preview.newly_ignored.contains(&PathBuf::from("nested/keep.tmp")));
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read_to_string(f.root.join(".gitignore")).unwrap(), "*.log\n");
    apply_gitignore(&preview).unwrap();
    assert_eq!(fs::read_to_string(f.root.join(".gitignore")).unwrap(), "*.tmp\ntracked.txt\n");
    assert!(apply_gitignore(&preview).is_err());
    let preview = preview_gitignore(&f.root, Path::new("nested/.gitignore"), Some("!keep.tmp\n"), "*.tmp\n").unwrap();
    fs::write(f.root.join("nested/added.tmp"), "external").unwrap();
    assert!(apply_gitignore(&preview).is_err());
    let new = preview_gitignore(&f.root, Path::new("other/.gitignore"), None, "*.tmp\n");
    assert!(new.is_err()); // no implicit directory creation
    fs::create_dir(f.root.join("other")).unwrap();
    let new = preview_gitignore(&f.root, Path::new("other/.gitignore"), None, "*.tmp\n").unwrap();
    apply_gitignore(&new).unwrap();
    assert_eq!(fs::read_to_string(f.root.join("other/.gitignore")).unwrap(), "*.tmp\n");
}
#[test]
fn ignore_preview_rejects_symlinks_and_honors_directory_patterns() {
    use super::super::ignore::*;
    let f = Fixture::new();
    f.git(&["config", "--local", "core.excludesFile", ""]);
    fs::create_dir(f.root.join("build")).unwrap();
    fs::write(f.root.join("build/generated"), b"generated").unwrap();
    let preview = preview_gitignore(&f.root, Path::new(".gitignore"), None, "/build/\n").unwrap();
    assert!(preview.newly_ignored.contains(&PathBuf::from("build/generated")));
    f.git(&["init", "--", "nested"]);
    let nested = preview_gitignore(&f.root, Path::new(".gitignore"), None, "/nested/\n").unwrap();
    assert!(nested.newly_ignored.contains(&PathBuf::from("nested")));
    std::os::unix::fs::symlink(f.root.join("tracked.txt"), f.root.join(".gitignore")).unwrap();
    assert!(preview_gitignore(&f.root, Path::new(".gitignore"), Some("initial\n"), "*.tmp\n").is_err());
    assert!(apply_gitignore(&preview).is_err());
}
#[test]
fn ignore_preview_preserves_info_global_and_nested_precedence() {
    use super::super::ignore::*;
    let f = Fixture::new();
    let global = f.temp.path().join("global-ignore");
    fs::write(&global, "*.hidden\n").unwrap();
    f.git(&["config", "--local", "core.excludesFile", global.to_str().unwrap()]);
    fs::write(f.root.join(".git/info/exclude"), "*.info\n").unwrap();
    fs::write(f.root.join("global.hidden"), "global").unwrap();
    fs::write(f.root.join("private.info"), "info").unwrap();
    let preview = preview_gitignore(&f.root, Path::new(".gitignore"), None, "!*.hidden\n!*.info\n").unwrap();
    assert!(preview.newly_unignored.contains(&PathBuf::from("global.hidden")));
    assert!(preview.newly_unignored.contains(&PathBuf::from("private.info")));
    fs::write(&global, "different\n").unwrap();
    assert!(apply_gitignore(&preview).is_err());
    assert!(!f.root.join(".gitignore").exists());
}
