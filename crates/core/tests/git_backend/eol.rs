use super::*;

#[test]
fn clean_crlf_checkout_matches_index_and_head() {
    let repo = Repo::new();
    repo.write(".gitattributes", "*.txt text eol=crlf\n");
    repo.write("notes.txt", "first\nsecond\nthird\n");
    repo.add(".");
    repo.commit();
    fs::remove_file(repo.root.join("notes.txt")).unwrap();
    repo.git(&["checkout", "--", "notes.txt"]);
    let path = Path::new("notes.txt");
    assert_eq!(
        read_file(&repo.root, path).unwrap(),
        "first\r\nsecond\r\nthird\r\n"
    );
    assert_eq!(repo.git(&["status", "--porcelain", "--"]), "");
    let clean = repo.snap();
    assert!(clean.changes.is_empty(), "{:?}", clean.changes);
    assert_eq!(clean.worktree.changed_files, 0);
    assert!(
        pokarak_core::index::snapshot(&repo.root)
            .unwrap()
            .unstaged
            .is_empty()
    );
    let file = ChangedFile {
        path: path.into(),
        ..Default::default()
    };
    assert!(
        diff(&repo.root, &file, &clean.compare)
            .unwrap()
            .patch
            .is_empty()
    );
    repo.write(path, "first\r\nchanged\r\nthird\r\n");
    let edited = repo.snap();
    let file = change(&edited, path);
    assert!(file.unstaged && !file.staged);
    assert_eq!((file.additions, file.deletions), (1, 1));
    assert_eq!(
        diff(&repo.root, file, &edited.compare).unwrap().new_text,
        "first\nchanged\nthird\n"
    );
}

#[test]
fn autocrlf_and_attributes_match_git_for_text_and_binary_changes() {
    let repo = Repo::new();
    repo.write(".gitattributes", b"*.auto text=auto eol=crlf\n*.raw -text eol=crlf\n*.legacy crlf=input\n*.forced eol=lf\n*.bin binary\n");
    let cases: &[(&str, &[u8], bool)] = &[
        ("-plain file", b"one\ntwo\n", true),
        ("regular.auto", b"one\ntwo\n", true),
        ("nested/with\ttab\nand newline.auto", b"one\ntwo", true),
        ("eof.auto", b"one\n\x1a", true),
        ("nul.auto", b"one\n\0two", false),
        ("control.auto", b"one\n\x01two", false),
        ("lonecr.auto", b"one\ntwo\rthree", false),
        ("forced.forced", b"one\ntwo\rthree", true),
        ("explicit.raw", b"one\ntwo\n", false),
        ("legacy.legacy", b"one\ntwo\n", true),
        ("macro.bin", b"one\ntwo\n", false),
    ];
    for &(path, original, _) in cases {
        repo.write(path, original);
    }
    repo.add(".");
    repo.commit();
    for &(path, original, _) in cases {
        let mut crlf = Vec::new();
        for &b in original {
            if b == b'\n' {
                crlf.push(b'\r');
            }
            crlf.push(b);
        }
        repo.write(path, crlf);
    }
    for autocrlf in ["true", "input"] {
        repo.git(&["config", "core.autocrlf", autocrlf]);
        let changed = repo.git_os(&[
            "diff".as_ref(),
            "--name-only".as_ref(),
            "-z".as_ref(),
            "--".as_ref(),
        ]);
        let git_paths: Vec<_> = changed
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .collect();
        let snap = repo.snap();
        for &(name, _, clean) in cases {
            assert_eq!(
                git_paths.contains(&name.as_bytes()),
                !clean,
                "Git {autocrlf} {name:?}"
            );
            assert_eq!(
                snap.changes.iter().any(|f| f.path == Path::new(name)),
                !clean,
                "{autocrlf} {name:?}"
            );
        }
    }
}

#[test]
fn branch_diff_uses_current_index_eol_policy_and_immutable_baseline_blobs() {
    let repo = Repo::new();
    let p = Path::new("notes");
    repo.write(p, "one\r\ntwo\r\nthree\r\n");
    repo.add(p);
    repo.commit();
    repo.git(&["checkout", "-b", "topic"]);
    repo.write(".gitattributes", "notes text=auto\n");
    repo.add(".gitattributes");
    repo.commit();
    // Auto preserves CRLF already present in the index, including for an edit.
    repo.write(p, "one\r\nchanged\r\nthree\r\n");
    let mode = CompareMode::Branch("main".into());
    let snap = snapshot_current(&repo.root, &mode).unwrap();
    let file = change(&snap, p);
    assert_eq!((file.additions, file.deletions), (1, 1));
    let doc = diff(&repo.root, file, &mode).unwrap();
    assert_eq!(doc.new_text, "one\r\nchanged\r\nthree\r\n");

    // A staged renormalization changes the index policy while the branch
    // baseline still contains the original CRLF blob.
    repo.git(&["add", "--renormalize", "--", "notes"]);
    let snap = snapshot_current(&repo.root, &mode).unwrap();
    let file = change(&snap, p);
    assert!(file.staged && !file.unstaged);
    assert_eq!(snap.worktree.changed_files, 1);
    let doc = diff(&repo.root, file, &mode).unwrap();
    assert_eq!(doc.old_text, "one\r\ntwo\r\nthree\r\n");
    assert_eq!(doc.new_text, "one\nchanged\nthree\n");
    repo.commit();
    let committed = snapshot_current(&repo.root, &mode).unwrap();
    assert_eq!(committed.worktree.changed_files, 0);
    let file = change(&committed, p);
    assert!(!file.staged && !file.unstaged);
    repo.write(p, "one\r\nchanged\r\nworking\r\n");
    let snap = repo.snap();
    let file = change(&snap, p);
    assert!(file.unstaged && !file.staged);
    assert_eq!((file.additions, file.deletions), (1, 1));
}

#[test]
fn raw_file_save_and_fingerprints_detect_equivalent_eol_rewrite() {
    let repo = Repo::new();
    repo.write(".gitattributes", "file text eol=crlf\n");
    repo.write("file", "one\r\ntwo");
    repo.add(".");
    repo.commit();
    let path = Path::new("file");
    let first = repo.snap();
    let raw = read_file(&repo.root, path).unwrap();
    repo.write(path, "one\ntwo");
    let second = repo.snap();
    assert!(first.changes.is_empty() && second.changes.is_empty());
    assert_ne!(first.fingerprint, second.fingerprint);
    assert!(pokarak_core::operations::save_file(&repo.root, path, &raw, "late edit").is_err());
    let new = "one\r\ncurrent edit";
    pokarak_core::operations::save_file(&repo.root, path, "one\ntwo", new).unwrap();
    assert_eq!(read_file(&repo.root, path).unwrap(), new);
    assert_eq!(repo.git(&["show", ":file"]), "one\ntwo");
}

#[test]
fn streamed_sha256_crlf_pairs_at_buffer_boundaries_and_after_preview() {
    let repo = Repo::with_format("sha256");
    repo.write(".gitattributes", "*.data text\n");
    let mut bytes = vec![b'x'; MAX_TEXT_BYTES + 80];
    for end in [64 * 1024, 2 * 64 * 1024, MAX_TEXT_BYTES + 22] {
        bytes[end - 2..end + 1].copy_from_slice(b"\r\r\n");
    }
    *bytes.last_mut().unwrap() = b'\r';
    repo.write("large.data", &bytes);
    repo.add(".");
    repo.commit();
    assert!(repo.snap().changes.is_empty());
    *bytes.last_mut().unwrap() = b'z';
    repo.write("large.data", bytes);
    let snap = repo.snap();
    let large = change(&snap, "large.data");
    assert!(large.unstaged);
    assert!(diff(&repo.root, large, &snap.compare).unwrap().truncated);
}

#[test]
fn current_index_and_head_keep_distinct_staged_and_unstaged_changes() {
    let repo = Repo::new();
    let path = Path::new("file");
    repo.write(".gitattributes", "file text eol=crlf\n");
    repo.write(path, "first\r\nsecond\r\n");
    repo.add(".");
    repo.commit();
    repo.write(path, "staged\r\nsecond\r\n");
    repo.add(path);
    let staged = pokarak_core::index::snapshot(&repo.root).unwrap();
    assert_eq!(staged.staged.len(), 1);
    assert!(staged.unstaged.is_empty());
    assert_eq!(
        pokarak_core::index::diff(&staged, path).unwrap().new_text,
        "staged\nsecond\n"
    );
    repo.write(path, "first\r\nsecond\r\n");
    let snap = repo.snap();
    let file = change(&snap, path);
    assert!(file.staged && file.unstaged);
    assert_eq!((file.additions, file.deletions), (0, 0));
    assert!(
        diff(&repo.root, file, &snap.compare)
            .unwrap()
            .patch
            .is_empty()
    );
    let index = pokarak_core::index::snapshot(&repo.root).unwrap();
    assert_eq!((index.staged.len(), index.unstaged.len()), (1, 1));
    assert!(
        pokarak_core::index::partial::preview(
            &index,
            path,
            pokarak_core::index::partial::Side::Unstaged
        )
        .is_err()
    );
}

#[test]
fn batch_eol_lookup_respects_nested_attributes_and_literal_native_paths() {
    let repo = Repo::new();
    repo.write(".gitattributes", "*.txt text=auto\n");
    repo.write("nested/.gitattributes", "except.txt -text\n");
    let count = 160;
    for i in 0..count {
        let name = format!("nested/-{i} [literal]\tline\n.txt");
        repo.write(name, "first\r\nsecond\r\n");
    }
    repo.write("nested/except.txt", "first\nsecond\n");
    repo.add(".");
    repo.commit();
    repo.write("nested/except.txt", "first\r\nsecond\r\n");
    let snap = snapshot_current(&repo.root, &CompareMode::Uncommitted).unwrap();
    assert_eq!(snap.changes.len(), 1);
    let raw = change(&snap, "nested/except.txt");
    assert_eq!((raw.additions, raw.deletions), (2, 2));
}
