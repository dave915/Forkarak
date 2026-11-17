use super::*;
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    process::{Command, Output},
};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_git_dir(false)
    }

    fn with_git_dir(separate: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repository 한 with spaces\n");
        fs::create_dir(&root).unwrap();
        let fixture = Self {
            temp,
            root: root.canonicalize().unwrap(),
        };
        if separate {
            fixture.git(&["init", "-b", "main", "--separate-git-dir=private metadata"]);
        } else {
            fixture.git(&["init", "-b", "main"]);
        }
        fixture.git(&["config", "--local", "user.name", "Save fixture"]);
        fixture.git(&["config", "--local", "user.email", "save@example.invalid"]);
        fixture.git(&["config", "--local", "commit.gpgsign", "false"]);
        fixture.git(&["config", "--local", "core.autocrlf", "false"]);
        fixture.write("src/code.rs", "original\n");
        fixture.git(&["add", "--", "src/code.rs"]);
        fixture.git(&["commit", "-m", "Authored fixture"]);
        fixture
    }

    fn run(&self, args: &[&OsStr]) -> Output {
        let mut command = Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.as_bytes().starts_with(b"GIT_") {
                command.env_remove(name);
            }
        }
        command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "--literal-pathspecs",
                "-c",
                "core.hooksPath=/dev/null",
                "-C",
            ])
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap()
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = self.run(&args.iter().map(OsStr::new).collect::<Vec<_>>());
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.root.join(path)).unwrap()
    }

    fn save(&self, path: &str, expected: &str, replacement: &str) -> Result<()> {
        // Exercise the exported public operations API, not just private helpers.
        crate::operations::save_file(&self.root, Path::new(path), expected, replacement)
    }

    fn prepare(&self) -> PreparedSave {
        PreparedSave::new(
            &self.root,
            Path::new("src/code.rs"),
            "original\n",
            "saved\n",
        )
        .unwrap()
    }

    fn assert_no_temporary_files(&self) {
        for entry in fs::read_dir(self.root.join("src")).unwrap() {
            assert!(
                !entry
                    .unwrap()
                    .file_name()
                    .as_bytes()
                    .starts_with(b".pokarak-save-")
            );
        }
    }
}

#[test]
fn success_is_atomic_and_leaves_inspection_index_head_and_hard_links_untouched() {
    let f = Fixture::new();
    let index = fs::read(f.root.join(".git/index")).unwrap();
    let head = f.git(&["rev-parse", "HEAD"]);
    let path = f.root.join("src/code.rs");
    let mut old_reader = File::open(&path).unwrap();
    let original_inode = old_reader.metadata().unwrap().ino();
    let hard_link = f.temp.path().join("another hard link");
    fs::hard_link(&path, &hard_link).unwrap();
    f.save("src/code.rs", "original\n", "fn saved() {}\n")
        .unwrap();
    assert_eq!(f.read("src/code.rs"), b"fn saved() {}\n");
    assert_eq!(
        crate::read_file(&f.root, Path::new("src/code.rs")).unwrap(),
        "fn saved() {}\n"
    );
    let mut original_bytes = Vec::new();
    old_reader.read_to_end(&mut original_bytes).unwrap();
    assert_eq!(original_bytes, b"original\n");
    assert_eq!(fs::read(hard_link).unwrap(), b"original\n");
    assert_ne!(fs::metadata(path).unwrap().ino(), original_inode);
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    assert!(f.git(&["diff", "--cached", "--exit-code", "--"]).is_empty());
    f.assert_no_temporary_files();
}

#[test]
fn unicode_paths_bytes_newlines_empty_and_noop_are_preserved_exactly() {
    let f = Fixture::new();
    let path = "src/한글 🦀 file\n.rs";
    for (before, after) in [
        ("한글 🦀\r\nsecond\r\n", "수정 😀\r\nnext\nlast"),
        ("no final newline", "final newline\n"),
        ("\u{feff}BOM\r\n", "\u{feff}보존\r\n"),
        ("", "새 파일 내용"),
        ("nonempty", ""),
        ("same\n", "same\n"),
    ] {
        f.write(path, before);
        f.save(path, before, after).unwrap();
        assert_eq!(f.read(path), after.as_bytes());
    }
    f.assert_no_temporary_files();
}

#[test]
fn executable_restricted_and_readonly_permission_bits_survive() {
    let f = Fixture::new();
    let path = f.root.join("src/code.rs");
    for mode in [0o755, 0o640, 0o600, 0o444, 0o4755] {
        // Recreate after each readonly save so the test works without root.
        fs::remove_file(&path).unwrap();
        fs::write(&path, "before").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let actual = fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        f.save("src/code.rs", "before", "after").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            actual
        );
        assert_eq!(f.read("src/code.rs"), b"after");
    }
    f.assert_no_temporary_files();
}

#[test]
fn stale_same_length_changes_and_newline_mismatches_are_rejected() {
    let f = Fixture::new();
    f.write("src/code.rs", "external\n");
    let error = f.save("src/code.rs", "original\n", "mine\n").unwrap_err();
    assert!(error.to_string().contains("changed externally"));
    assert_eq!(f.read("src/code.rs"), b"external\n");
    f.write("src/code.rs", "a\r\n");
    assert!(f.save("src/code.rs", "a\n", "mine").is_err());
    assert_eq!(f.read("src/code.rs"), b"a\r\n");
    f.assert_no_temporary_files();
}

#[test]
fn external_writes_replacements_deletions_and_chmod_during_save_are_rejected() {
    for change in ["write", "replace", "delete", "chmod", "symlink"] {
        let f = Fixture::new();
        let save = f.prepare();
        let path = f.root.join("src/code.rs");
        match change {
            "write" => fs::write(&path, "external\n").unwrap(),
            "replace" => {
                f.write("other", "original\n");
                fs::rename(f.root.join("other"), &path).unwrap();
            }
            "delete" => fs::remove_file(&path).unwrap(),
            "chmod" => fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap(),
            "symlink" => {
                f.write("other", "original\n");
                fs::remove_file(&path).unwrap();
                symlink("../other", &path).unwrap();
            }
            _ => unreachable!(),
        }
        // Fixture umasks can already be 0600; force an actual permission change.
        if change == "chmod" && fs::metadata(&path).unwrap().mode() == save.original.metadata.mode()
        {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        assert!(save.commit().is_err(), "accepted external {change}");
        if change == "write" {
            assert_eq!(f.read("src/code.rs"), b"external\n");
        } else if change == "delete" {
            assert!(!path.exists());
        } else {
            assert_eq!(f.read("src/code.rs"), b"original\n");
        }
        f.assert_no_temporary_files();
    }
}

#[test]
fn unsafe_relative_absolute_and_git_metadata_paths_are_rejected() {
    let f = Fixture::new();
    let outside = f.temp.path().join("outside");
    fs::write(&outside, "original\n").unwrap();
    let config = fs::read(f.root.join(".git/config")).unwrap();
    for path in [
        Path::new(""),
        Path::new("."),
        Path::new(".."),
        Path::new("../outside"),
        Path::new("src/../../outside"),
        Path::new("src/../src/code.rs"),
        Path::new("./src/code.rs"),
        Path::new(".git/config"),
        Path::new(".GIT/HEAD"),
        Path::new("src/.git/config"),
        Path::new("src/file\0name"),
        &f.root.join("src/code.rs"),
        &outside,
    ] {
        assert!(
            crate::operations::save_file(&f.root, path, "original\n", "bad").is_err(),
            "{}",
            path.display()
        );
    }
    assert_eq!(fs::read(outside).unwrap(), b"original\n");
    assert_eq!(f.read("src/code.rs"), b"original\n");
    assert_eq!(fs::read(f.root.join(".git/config")).unwrap(), config);
    f.assert_no_temporary_files();
}

#[test]
fn separate_git_metadata_and_linked_worktree_administration_are_protected() {
    let f = Fixture::with_git_dir(true);
    let config = f.read("private metadata/config");
    let text = std::str::from_utf8(&config).unwrap();
    assert!(f.save("private metadata/config", text, "bad").is_err());
    assert_eq!(f.read("private metadata/config"), config);
    f.save("src/code.rs", "original\n", "main saved\n").unwrap();
    let linked = f.temp.path().join("linked worktree");
    let output = f.run(&[
        "worktree".as_ref(),
        "add".as_ref(),
        "--detach".as_ref(),
        "--".as_ref(),
        linked.as_os_str(),
        "HEAD".as_ref(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let gitfile = fs::read_to_string(linked.join(".git")).unwrap();
    assert!(crate::operations::save_file(&linked, Path::new(".git"), &gitfile, "bad").is_err());
    crate::operations::save_file(
        &linked,
        Path::new("src/code.rs"),
        "original\n",
        "linked saved\n",
    )
    .unwrap();
    assert_eq!(f.read("src/code.rs"), b"main saved\n");
    assert_eq!(
        fs::read(linked.join("src/code.rs")).unwrap(),
        b"linked saved\n"
    );
}

#[test]
fn symlink_leaves_and_parents_are_rejected_even_when_the_target_is_inside() {
    let f = Fixture::new();
    let outside = f.temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("code.rs"), "original\n").unwrap();
    symlink(outside.join("code.rs"), f.root.join("outside-link")).unwrap();
    symlink("src/code.rs", f.root.join("inside-link")).unwrap();
    symlink("src", f.root.join("inside-dir")).unwrap();
    symlink(&outside, f.root.join("outside-dir")).unwrap();
    symlink("missing", f.root.join("dangling")).unwrap();
    symlink(".git", f.root.join("metadata-alias")).unwrap();
    for path in [
        "outside-link",
        "inside-link",
        "inside-dir/code.rs",
        "outside-dir/code.rs",
        "dangling",
        "metadata-alias/config",
    ] {
        assert!(f.save(path, "original\n", "bad").is_err(), "{path}");
    }
    assert_eq!(fs::read(outside.join("code.rs")).unwrap(), b"original\n");
    assert_eq!(f.read("src/code.rs"), b"original\n");
    assert!(
        fs::symlink_metadata(f.root.join("inside-link"))
            .unwrap()
            .is_symlink()
    );
    f.assert_no_temporary_files();
}

#[test]
fn parent_swaps_during_save_do_not_follow_symlinks_or_write_to_detached_directories() {
    for use_symlink in [false, true] {
        let f = Fixture::new();
        let save = f.prepare();
        let moved = f.temp.path().join("moved outside the worktree");
        fs::rename(f.root.join("src"), &moved).unwrap();
        if use_symlink {
            symlink(&moved, f.root.join("src")).unwrap();
        } else {
            f.write("src/code.rs", "replacement directory\n");
        }
        assert!(save.commit().is_err());
        assert_eq!(fs::read(moved.join("code.rs")).unwrap(), b"original\n");
        assert_eq!(
            fs::read_dir(moved).unwrap().count(),
            1,
            "temporary file leaked"
        );
        if !use_symlink {
            assert_eq!(f.read("src/code.rs"), b"replacement directory\n");
        }
    }
}

#[test]
fn root_swaps_and_invalid_roots_are_rejected() {
    let f = Fixture::new();
    assert!(
        crate::operations::save_file(
            &f.root.join("src"),
            Path::new("code.rs"),
            "original\n",
            "bad"
        )
        .is_err()
    );
    assert!(crate::operations::save_file(f.temp.path(), Path::new("outside"), "", "bad").is_err());
    let save = f.prepare();
    let moved = f.temp.path().join("moved root");
    fs::rename(&f.root, &moved).unwrap();
    symlink(&moved, &f.root).unwrap();
    assert!(save.commit().is_err());
    assert_eq!(fs::read(moved.join("src/code.rs")).unwrap(), b"original\n");
    assert_eq!(fs::read_dir(moved.join("src")).unwrap().count(), 1);
}

#[test]
fn missing_directory_fifo_binary_non_utf8_and_oversized_inputs_are_rejected() {
    let f = Fixture::new();
    assert!(f.save("missing", "", "bad").is_err());
    assert!(!f.root.join("missing").exists());
    assert!(f.save("src", "", "bad").is_err());
    let fifo = CString::new(f.root.join("fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: terminated path points into this owned temporary fixture only.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(f.save("fifo", "", "bad").is_err());
    for bytes in [b"binary\0data".as_slice(), b"invalid\xffutf8".as_slice()] {
        f.write("src/code.rs", bytes);
        assert!(f.save("src/code.rs", "", "bad").is_err());
        assert_eq!(f.read("src/code.rs"), bytes);
    }
    f.write("src/code.rs", "original\n");
    assert!(f.save("src/code.rs", "original\n", "bad\0text").is_err());
    assert!(f.save("src/code.rs", "original\0", "bad").is_err());
    let huge = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(f.save("src/code.rs", "original\n", &huge).is_err());
    f.write("src/code.rs", &huge);
    assert!(f.save("src/code.rs", &huge, "bad").is_err());
    assert!(f.save("src/code.rs", "truncated preview", "bad").is_err());
    assert_eq!(f.read("src/code.rs"), huge.as_bytes());
    let largest = "x".repeat(MAX_TEXT_BYTES);
    f.write("src/code.rs", &largest);
    f.save("src/code.rs", &largest, "within the limit\n")
        .unwrap();
    f.assert_no_temporary_files();
}

#[test]
fn preparing_or_failing_a_save_never_changes_the_original_or_leaks_temporary_files() {
    let f = Fixture::new();
    let save = f.prepare();
    assert_eq!(f.read("src/code.rs"), b"original\n");
    drop(save);
    f.assert_no_temporary_files();
    let save = f.prepare();
    let temporary = f
        .root
        .join("src")
        .join(OsStr::from_bytes(save.temporary.as_bytes()));
    fs::write(&temporary, "tampered temporary contents").unwrap();
    assert!(save.commit().is_err());
    assert_eq!(f.read("src/code.rs"), b"original\n");
    f.assert_no_temporary_files();
}

#[test]
fn saving_conflict_text_does_not_stage_resolve_or_run_git_filters() {
    let f = Fixture::new();
    f.git(&["switch", "-c", "incoming"]);
    f.write("src/code.rs", "incoming\n");
    f.git(&["add", "--", "src/code.rs"]);
    f.git(&["commit", "-m", "Incoming"]);
    f.git(&["switch", "main"]);
    f.write("src/code.rs", "ours\n");
    f.git(&["add", "--", "src/code.rs"]);
    f.git(&["commit", "-m", "Ours"]);
    let merge = f.run(&["merge".as_ref(), "incoming".as_ref()]);
    assert!(!merge.status.success());
    let conflicts = f.git(&["ls-files", "--unmerged", "--", "src/code.rs"]);
    assert!(!conflicts.is_empty());
    f.write(".gitattributes", "*.rs filter=save-fixture\n");
    f.git(&[
        "config",
        "--local",
        "filter.save-fixture.clean",
        "touch filter-ran; cat",
    ]);
    f.git(&["config", "--local", "filter.save-fixture.required", "true"]);
    let index = f.read(".git/index");
    let merge_head = f.read(".git/MERGE_HEAD");
    let contents = String::from_utf8(f.read("src/code.rs")).unwrap();
    f.save("src/code.rs", &contents, "user's edited resolution\n")
        .unwrap();
    assert_eq!(f.read(".git/index"), index);
    assert_eq!(f.read(".git/MERGE_HEAD"), merge_head);
    assert_eq!(
        f.git(&["ls-files", "--unmerged", "--", "src/code.rs"]),
        conflicts
    );
    assert_eq!(f.read("src/code.rs"), b"user's edited resolution\n");
    assert!(!f.root.join("filter-ran").exists());
    f.assert_no_temporary_files();
}
