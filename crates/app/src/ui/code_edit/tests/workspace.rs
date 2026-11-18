use super::super::{SaveAllReport, save_all::format_with_navigator, workspace::changed_range};
use super::{CodeEdits, FileKey, edit, git, state};
use pokarak_navigation::{
    DocumentSnapshot, FileEdits, FormatterConfig, FormattingOptions, Navigator, Position, TextEdit,
    TextRange, WorkspaceEdit,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn fixture() -> (tempfile::TempDir, CodeEdits, PathBuf) {
    let (temp, state) = state();
    let root = temp.path().join("workspace 편집");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    let root = root.canonicalize().unwrap();
    (temp, state, root)
}

fn change(root: &Path, path: &str, original: &str, new: &str) -> FileEdits {
    FileEdits {
        path: root.join(path),
        original: original.into(),
        edits: vec![TextEdit {
            range: TextRange {
                start: Position::default(),
                end: Position {
                    line: 0,
                    character: 3,
                },
            },
            new_text: new.into(),
        }],
    }
}

fn apply(state: &mut CodeEdits, root: &Path, files: Vec<FileEdits>) -> Result<(), String> {
    let replacements = state.prepare_workspace_edit(root, &WorkspaceEdit { files })?;
    state.install_workspace_edit(root, "이름 바꾸기", replacements);
    Ok(())
}

fn text<'a>(state: &'a CodeEdits, root: &Path, path: &str) -> &'a str {
    &state.buffer(root, Path::new(path)).unwrap().draft.text
}

#[test]
fn atomic_workspace_edit_creates_unopened_draft_and_undo_restores_preexisting_draft() {
    let (temp, mut state, root) = fixture();
    let a = Path::new("a.ts");
    fs::write(root.join(a), "old\r\n").unwrap();
    fs::write(root.join("b.ts"), "old 😀\r\n").unwrap();
    state.loaded(&root, a, "old\r\n".into(), true, true);
    edit(&mut state, &root, a, "old // mine\r\n");
    apply(
        &mut state,
        &root,
        vec![
            change(&root, "b.ts", "old 😀\r\n", "new"),
            change(&root, "a.ts", "old // mine\r\n", "new"),
        ],
    )
    .unwrap();
    assert_eq!(text(&state, &root, "a.ts"), "new // mine\r\n");
    assert_eq!(text(&state, &root, "b.ts"), "new 😀\r\n");
    assert_eq!(state.dirty_count(&root), 2);
    assert_eq!(fs::read(root.join(a)).unwrap(), b"old\r\n");
    assert_eq!(fs::read_to_string(root.join("b.ts")).unwrap(), "old 😀\r\n");
    state.persist().unwrap();
    let restored = CodeEdits::load_from(temp.path().join("session/code-drafts.json"));
    assert_eq!(text(&restored, &root, "b.ts"), "new 😀\r\n");
    state.undo_transaction(&root).unwrap();
    assert_eq!(state.dirty_count(&root), 1);
    assert_eq!(text(&state, &root, "a.ts"), "old // mine\r\n");
    assert_eq!(text(&state, &root, "b.ts"), "old 😀\r\n");
    assert!(!state.dirty(&root, Path::new("b.ts")));
}

#[test]
fn stale_draft_disk_or_invalid_last_range_rejects_every_file() {
    let (_temp, mut state, root) = fixture();
    fs::write(root.join("a.ts"), "old").unwrap();
    fs::write(root.join("b.ts"), "old").unwrap();
    let a = change(&root, "a.ts", "old", "new");
    let b = change(&root, "b.ts", "old", "new");
    let mut overlapping = b.clone();
    overlapping.edits.push(overlapping.edits[0].clone());
    let mut duplicate = b.clone();
    duplicate.path = a.path.clone();
    for invalid in [overlapping, duplicate] {
        assert!(apply(&mut state, &root, vec![a.clone(), invalid]).is_err());
        assert!(state.buffers.is_empty());
    }
    state.loaded(&root, Path::new("b.ts"), "old".into(), true, true);
    fs::write(root.join("b.ts"), "disk after request").unwrap();
    assert!(
        apply(&mut state, &root, vec![a.clone(), b.clone()])
            .unwrap_err()
            .contains("내용이 바뀌어")
    );
    edit(&mut state, &root, Path::new("b.ts"), "my newer draft");
    assert!(
        apply(&mut state, &root, vec![a, b])
            .unwrap_err()
            .contains("내용이 바뀌어")
    );
    assert!(state.buffer(&root, Path::new("a.ts")).is_none());
    assert_eq!(text(&state, &root, "b.ts"), "my newer draft");
    assert_eq!(fs::read_to_string(root.join("a.ts")).unwrap(), "old");
    assert!(!state.transactions.contains_key(&root));
}

#[test]
fn replacing_a_preview_target_with_an_external_symlink_cancels_entire_transaction() {
    let (temp, mut state, root) = fixture();
    let outside = temp.path().join("outside.ts");
    fs::write(&outside, "old").unwrap();
    fs::write(root.join("first.ts"), "old").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("alias.ts")).unwrap();
    assert!(
        apply(
            &mut state,
            &root,
            vec![
                change(&root, "first.ts", "old", "new"),
                change(&root, "alias.ts", "old", "new"),
            ]
        )
        .is_err()
    );
    assert!(state.buffers.is_empty());
    assert_eq!(fs::read_to_string(outside).unwrap(), "old");
}

#[test]
fn edited_then_reverted_target_invalidates_batch_undo_without_changing_other_files() {
    let (_temp, mut state, root) = fixture();
    for name in ["a.ts", "b.ts"] {
        fs::write(root.join(name), "old").unwrap();
    }
    apply(
        &mut state,
        &root,
        vec![
            change(&root, "a.ts", "old", "new"),
            change(&root, "b.ts", "old", "new"),
        ],
    )
    .unwrap();
    edit(&mut state, &root, Path::new("b.ts"), "newer");
    edit(&mut state, &root, Path::new("b.ts"), "new");
    assert!(state.undo_transaction(&root).is_err());
    assert_eq!(text(&state, &root, "a.ts"), "new");
    assert_eq!(text(&state, &root, "b.ts"), "new");
    assert_eq!(fs::read_to_string(root.join("b.ts")).unwrap(), "old");
}

#[test]
fn transaction_and_dirty_save_targets_are_scoped_to_the_worktree() {
    let (temp, mut state, root) = fixture();
    let other = temp.path().join("other");
    fs::create_dir(&other).unwrap();
    git(&other, &["init", "-b", "main"]);
    let other = other.canonicalize().unwrap();
    for dir in [&root, &other] {
        fs::write(dir.join("a.ts"), "old").unwrap();
        apply(&mut state, dir, vec![change(dir, "a.ts", "old", "new")]).unwrap();
    }
    assert_eq!(state.dirty_targets(&root).len(), 1);
    assert_eq!(state.dirty_targets(&root)[0].key.root, root);
    assert!(
        apply(
            &mut state,
            &root,
            vec![change(&other, "a.ts", "new", "bad")]
        )
        .is_err()
    );
    state.undo_transaction(&root).unwrap();
    assert_eq!(state.dirty_count(&root), 0);
    assert_eq!(state.dirty_count(&other), 1);
    assert_eq!(text(&state, &other, "a.ts"), "new");
    state.undo_transaction(&other).unwrap();
    assert_eq!(state.dirty_count(&other), 0);
}

#[test]
fn native_range_replacement_preserves_utf8_boundaries_for_one_undo_step() {
    for (old, new, middle) in [
        ("앞 old 😀 old 끝", "앞 new 😀 new 끝", "new 😀 new"),
        ("😀", "😁", "😁"),
        ("한글", "한글!", "!"),
        ("가나다", "가다", ""),
        ("\r\n", "\n", ""),
    ] {
        let (range, replacement) = changed_range(old, new);
        assert_eq!(replacement, middle);
        let mut actual = old.to_owned();
        actual.replace_range(range, replacement);
        assert_eq!(actual, new);
    }
}

#[test]
fn save_all_waits_for_unopened_format_and_reports_each_conflict_or_unwritable_target() {
    let (_temp, mut state, root) = fixture();
    for name in ["c.ts", "a.ts", "b.ts"] {
        fs::write(root.join(name), "old\r\n").unwrap();
    }
    apply(
        &mut state,
        &root,
        ["c.ts", "b.ts", "a.ts"]
            .map(|name| change(&root, name, "old\r\n", "draft"))
            .to_vec(),
    )
    .unwrap();
    assert!(state.bindings.is_empty());
    fs::write(root.join("b.ts"), "external").unwrap();
    fs::remove_file(root.join("c.ts")).unwrap();
    fs::create_dir(root.join("c.ts")).unwrap();
    let mut nav = Navigator::with_options(
        root.clone(),
        vec![],
        pokarak_navigation::NavigatorOptions {
            config_file: None,
            ..Default::default()
        },
    );
    nav.set_formatter_configs(vec![FormatterConfig {
        language: "typescript".into(),
        program: "/usr/bin/tr".into(),
        args: vec!["a-z".into(), "A-Z".into()],
    }])
    .unwrap();
    let targets = state.dirty_targets(&root);
    let mut visited = vec![];
    let (start, gate) = smol::channel::bounded::<()>(1);
    let (out, completed) = smol::channel::bounded(1);
    smol::block_on(async {
        let collect = async {
            let report = SaveAllReport::collect(targets, async |target| {
                if visited.is_empty() {
                    gate.recv().await.unwrap();
                }
                visited.push(target.key.path.clone());
                let source = DocumentSnapshot::new(
                    root.clone(),
                    target.key.path.clone(),
                    state.buffers[&target.key].draft.text.clone(),
                    1,
                )
                .unwrap();
                let formatted =
                    format_with_navigator(&mut nav, &source, &[], &FormattingOptions::default())?;
                let submission = state.begin_batch_write(target, Some(formatted))?;
                let result = submission.write_working_file();
                state.finish_batch_write(target, &submission.text, result)
            })
            .await;
            out.send(report).await.unwrap();
        };
        let check = async {
            assert!(completed.try_recv().is_err());
            assert_eq!(fs::read_to_string(root.join("a.ts")).unwrap(), "old\r\n");
            start.send(()).await.unwrap();
            let report = completed.recv().await.unwrap();
            assert_eq!(report.saved, [PathBuf::from("a.ts")]);
            assert_eq!(
                report
                    .failures
                    .iter()
                    .map(|(p, _)| p.clone())
                    .collect::<Vec<_>>(),
                ["b.ts", "c.ts"].map(PathBuf::from)
            );
            assert!(report.failures[0].1.contains("changed externally"));
            assert!(report.failures[1].1.contains("regular files"));
            assert_eq!(fs::read_to_string(root.join("a.ts")).unwrap(), "DRAFT\r\n");
        };
        smol::future::zip(collect, check).await;
    });
    assert_eq!(visited, ["a.ts", "b.ts", "c.ts"].map(PathBuf::from));
    assert_eq!(state.dirty_count(&root), 2);
    assert_eq!(text(&state, &root, "a.ts"), "DRAFT\r\n");
    assert_eq!(text(&state, &root, "b.ts"), "draft\r\n");
    assert_eq!(text(&state, &root, "c.ts"), "draft\r\n");
    assert_eq!(fs::read_to_string(root.join("b.ts")).unwrap(), "external");
    assert!(root.join("c.ts").is_dir());
}

#[test]
fn older_format_result_is_rejected_and_successful_write_keeps_queued_input_dirty() {
    let (_temp, mut state, root) = fixture();
    let path = Path::new("a.ts");
    fs::write(root.join(path), "old").unwrap();
    apply(&mut state, &root, vec![change(&root, "a.ts", "old", "new")]).unwrap();
    let target = state.dirty_targets(&root).pop().unwrap();
    edit(&mut state, &root, path, "changed");
    edit(&mut state, &root, path, "new");
    assert!(
        state
            .begin_batch_write(&target, Some("OLD RESULT".into()))
            .is_err()
    );
    assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "old");
    let target = state.dirty_targets(&root).pop().unwrap();
    let submission = state
        .begin_batch_write(&target, Some("NEW".into()))
        .unwrap();
    edit(&mut state, &root, path, "typed during save");
    let result = submission.write_working_file();
    state
        .finish_batch_write(&target, &submission.text, result)
        .unwrap();
    assert_eq!(
        state.buffers[&FileKey::new(&root, path).unwrap()]
            .draft
            .expected,
        "NEW"
    );
    assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "NEW");
    assert_eq!(text(&state, &root, "a.ts"), "typed during save");
    assert_eq!(state.dirty_count(&root), 1);
}

#[test]
fn newly_nested_checkout_rejects_save_and_external_disk_change_rejects_undo() {
    let (_temp, mut state, root) = fixture();
    let path = Path::new("nested/main.ts");
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join(path), "old").unwrap();
    apply(
        &mut state,
        &root,
        vec![change(&root, "nested/main.ts", "old", "new")],
    )
    .unwrap();
    let target = state.dirty_targets(&root).pop().unwrap();
    git(&root.join("nested"), &["init", "-b", "other"]);
    let submission = state.begin_batch_write(&target, None).unwrap();
    let result = submission.write_working_file();
    assert!(
        state
            .finish_batch_write(&target, &submission.text, result)
            .unwrap_err()
            .contains("다른 워크트리")
    );
    assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "old");
    fs::write(root.join(path), "external").unwrap();
    assert!(state.undo_transaction(&root).is_err());
    assert_eq!(text(&state, &root, "nested/main.ts"), "new");
}

#[test]
fn failed_format_leaves_original_draft_and_allows_later_file_to_save() {
    let (_temp, mut state, root) = fixture();
    for name in ["a.ts", "b.java"] {
        fs::write(root.join(name), "old").unwrap();
    }
    apply(
        &mut state,
        &root,
        vec![
            change(&root, "a.ts", "old", "new"),
            change(&root, "b.java", "old", "new"),
        ],
    )
    .unwrap();
    let mut nav = Navigator::with_options(
        root.clone(),
        vec![],
        pokarak_navigation::NavigatorOptions {
            config_file: None,
            ..Default::default()
        },
    );
    nav.set_formatter_configs(vec![
        FormatterConfig {
            language: "typescript".into(),
            program: "/usr/bin/false".into(),
            args: vec![],
        },
        FormatterConfig {
            language: "java".into(),
            program: "/usr/bin/tr".into(),
            args: vec!["a-z".into(), "A-Z".into()],
        },
    ])
    .unwrap();
    let report = smol::block_on(SaveAllReport::collect(
        state.dirty_targets(&root),
        async |target| {
            let source = DocumentSnapshot::new(
                root.clone(),
                target.key.path.clone(),
                state.buffers[&target.key].draft.text.clone(),
                1,
            )
            .unwrap();
            let formatted =
                format_with_navigator(&mut nav, &source, &[], &FormattingOptions::default())?;
            let submission = state.begin_batch_write(target, Some(formatted))?;
            let result = submission.write_working_file();
            state.finish_batch_write(target, &submission.text, result)
        },
    ));
    assert_eq!(report.saved, [PathBuf::from("b.java")]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].0, PathBuf::from("a.ts"));
    assert!(!report.failures[0].1.is_empty());
    assert_eq!(state.dirty_count(&root), 1);
    assert_eq!(text(&state, &root, "a.ts"), "new");
    assert_eq!(fs::read_to_string(root.join("a.ts")).unwrap(), "old");
    assert_eq!(text(&state, &root, "b.java"), "NEW");
    assert_eq!(fs::read_to_string(root.join("b.java")).unwrap(), "NEW");
}
