use super::*;

#[test]
fn independent_process_close_keeps_both_worktree_drafts() {
    const CHILD_STORE: &str = "FORKARAK_DRAFT_TEST_STORE";
    let path = Path::new("src/main.rs");
    let first = Path::new("/fixture/first");
    let second = Path::new("/fixture/second");
    if let Some(storage) = std::env::var_os(CHILD_STORE) {
        let mut child = CodeEdits::load_from(storage.into());
        child.loaded(second, path, "disk".into(), true, true);
        edit(&mut child, second, path, "child edit");
        child.persist().unwrap();
        return;
    }
    let (_temp, mut state) = state();
    state.loaded(first, path, "disk".into(), true, true);
    edit(&mut state, first, path, "parent edit");
    let other = Path::new("notes.md");
    state.loaded(second, other, "notes".into(), true, true);
    edit(&mut state, second, other, "parent notes");
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", std::thread::current().name().unwrap()])
        .env(CHILD_STORE, &state.storage.path)
        .output()
        .unwrap();
    assert!(
        result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed")
    );
    state.persist().unwrap(); // The original window flushes after the child quits.
    let reopened = CodeEdits::load_from(state.storage.path.clone());
    assert_eq!(
        reopened.analysis_drafts(first),
        [(path.into(), "parent edit".into())]
    );
    let mut second_drafts = reopened.analysis_drafts(second);
    second_drafts.sort();
    assert_eq!(
        second_drafts,
        [
            (other.into(), "parent notes".into()),
            (path.into(), "child edit".into()),
        ]
    );
}

#[test]
fn simultaneous_checkpoints_expose_every_same_file_version_after_restart() {
    let (_temp, seed) = state();
    let storage = seed.storage.path.clone();
    let root = Path::new("/fixture/parallel");
    let path = Path::new("draft.txt");
    let key = FileKey::new(root, path).unwrap();
    let barrier = std::sync::Barrier::new(6);
    std::thread::scope(|scope| {
        for n in 0..6 {
            let (storage, barrier) = (&storage, &barrier);
            scope.spawn(move || {
                let mut window = CodeEdits::load_from(storage.clone());
                window.loaded(root, path, format!("original {n}"), true, true);
                edit(&mut window, root, path, &format!("edit {n}"));
                barrier.wait();
                window.persist().unwrap();
            });
        }
    });

    let mut reopened = CodeEdits::load_from(storage);
    let mut versions = vec![reopened.buffer(root, path).unwrap().draft.clone()];
    let choices: Vec<_> = reopened
        .choices(root, path)
        .unwrap()
        .iter()
        .map(|c| c.revision)
        .collect();
    for revision in choices {
        reopened.recover(&key, revision).unwrap();
        assert!(!reopened.can_edit(root, path)); // The reader must check the disk again.
        versions.push(reopened.buffer(root, path).unwrap().draft.clone());
    }
    versions.sort_by(|a, b| a.text.cmp(&b.text));
    assert_eq!(versions.len(), 6);
    for (n, version) in versions.iter().enumerate() {
        assert_eq!(version.text, format!("edit {n}"));
        assert_eq!(version.expected, format!("original {n}"));
    }
}

#[test]
fn legacy_draft_forks_and_recovery_preserves_pending_input_and_disk_baselines() {
    let (_temp, seed, root) = comparison_fixture();
    let path = Path::new("main.rs");
    let key = FileKey::new(&root, path).unwrap();
    let storage = &seed.storage.path;
    fs::create_dir_all(storage.parent().unwrap()).unwrap();
    fs::write(storage, serde_json::to_vec(&seed.drafts()).unwrap()).unwrap();
    let mut left = CodeEdits::load_from(storage.clone());
    let mut right = CodeEdits::load_from(storage.clone());
    assert_eq!(
        right.buffer(&root, path).unwrap().draft.text,
        "my draft 😀\r\n"
    );
    edit(&mut left, &root, path, "left 😀\r\n");
    left.persist().unwrap();
    fs::write(root.join(path), "new disk\r\n").unwrap();
    let comparison = DraftComparison::read(
        root.clone(),
        path.into(),
        "original 😀\r\n".into(),
        "my draft 😀\r\n".into(),
    );
    right
        .reconcile(&comparison, "right\n".into(), "new disk\r\n")
        .unwrap();
    let mut reopened = CodeEdits::load_from(storage.clone());
    assert_eq!(
        reopened.loaded(&root, path, "new disk\r\n".into(), true, true),
        "right\n"
    );
    let left_revision = reopened.choices(&root, path).unwrap()[0].revision;
    edit(&mut reopened, &root, path, "pending typing on right");
    reopened.recover(&key, left_revision).unwrap();
    assert_eq!(
        reopened.loaded(&root, path, "new disk\r\n".into(), true, true),
        "left 😀\r\n"
    );
    assert!(reopened.stale(&root, path));
    let choices = reopened.choices(&root, path).unwrap();
    assert_eq!(choices.len(), 1);
    let right_revision = choices[0].revision;
    reopened.recover(&key, right_revision).unwrap();
    assert_eq!(
        reopened.loaded(&root, path, "new disk\r\n".into(), true, true),
        "pending typing on right"
    );
    assert!(!reopened.stale(&root, path));
    assert_eq!(fs::read(root.join(path)).unwrap(), b"new disk\r\n");
}

#[test]
fn saved_revision_stays_acknowledged_through_stale_refresh_close_restore_and_discard() {
    let (_temp, mut writer, root) = comparison_fixture();
    let path = Path::new("main.rs");
    writer.persist().unwrap();
    let storage = writer.storage.path.clone();
    let mut stale = CodeEdits::load_from(storage.clone());
    let mut discard = CodeEdits::load_from(storage.clone());
    let initial = writer.buffer(&root, path).unwrap().draft.clone();
    pokarak_core::operations::save_file(&root, path, &initial.expected, &initial.text).unwrap();
    writer
        .buffers
        .get_mut(&initial.key)
        .unwrap()
        .saved(&initial.text);
    writer.persist().unwrap();
    stale.loaded(&root, path, initial.text.clone(), true, true);
    stale.persist().unwrap();
    assert!(stale.buffer(&root, path).unwrap().retired);
    assert!(CodeEdits::load_from(storage.clone()).buffers.is_empty());

    // Recreate even the exact same expected/text pair as a NEW edit.
    fs::write(root.join(path), &initial.expected).unwrap();
    writer.loaded(&root, path, initial.expected.clone(), true, true);
    edit(&mut writer, &root, path, &initial.text);
    writer.persist().unwrap();
    edit(&mut stale, &root, path, &initial.expected); // Undo to original bytes.
    stale.persist().unwrap();
    discard.discard(&initial).unwrap(); // Stale confirmation against the old revision.
    let reopened = CodeEdits::load_from(storage);
    assert_eq!(
        reopened.analysis_drafts(&root),
        [(path.into(), initial.text)]
    );
    assert!(reopened.choices(&root, path).is_none());
}

#[test]
fn stale_recovery_menu_cannot_restore_a_discarded_or_replaced_revision() {
    let (_temp, mut left, root) = comparison_fixture();
    let path = Path::new("main.rs");
    let key = FileKey::new(&root, path).unwrap();
    let storage = left.storage.path.clone();
    let mut right = CodeEdits::load_from(storage.clone());
    left.persist().unwrap();
    right.loaded(&root, path, "original 😀\r\n".into(), true, true);
    edit(&mut right, &root, path, "right");
    right.persist().unwrap();
    let retired = right.choices(&root, path).unwrap()[0].revision;
    let old = left.buffer(&root, path).unwrap().draft.clone();
    left.discard(&old).unwrap();
    left.loaded(&root, path, old.expected.clone(), true, true);
    edit(&mut left, &root, path, &old.text);
    left.persist().unwrap();

    edit(&mut right, &root, path, "typing before stale menu click");
    assert!(right.recover(&key, retired).is_err());
    assert_eq!(
        right.buffer(&root, path).unwrap().draft.text,
        "typing before stale menu click"
    );
    let replacement = right.choices(&root, path).unwrap()[0].revision;
    assert_ne!(replacement, retired);
    right.recover(&key, replacement).unwrap();
    right.discard(&old).unwrap();
    let reopened = CodeEdits::load_from(storage);
    assert_eq!(
        reopened.analysis_drafts(&root),
        [(path.into(), "typing before stale menu click".into())]
    );
    assert!(reopened.choices(&root, path).is_none());
}

#[test]
fn failed_or_obsolete_discard_keeps_current_input_and_peer_drafts() {
    let (_temp, mut local, root) = comparison_fixture();
    let path = Path::new("main.rs");
    let storage = local.storage.path.clone();
    let mut peer = CodeEdits::load_from(storage.clone());
    peer.loaded(&root, path, "original 😀\r\n".into(), true, true);
    edit(&mut peer, &root, path, "peer");
    peer.persist().unwrap();
    local.persist().unwrap();
    let prompted = local.buffer(&root, path).unwrap().draft.clone();
    edit(&mut local, &root, path, "newer edit");
    assert!(local.discard(&prompted).is_err());
    let current = local.buffer(&root, path).unwrap().draft.clone();
    let bytes = fs::read(&storage).unwrap();
    fs::write(&storage, b"damaged checkpoint").unwrap();
    assert!(local.discard(&current).is_err());
    assert_eq!(fs::read(&storage).unwrap(), b"damaged checkpoint");
    assert_eq!(local.buffer(&root, path).unwrap().draft.text, "newer edit");
    fs::write(&storage, bytes).unwrap();
    local.discard(&current).unwrap();
    let reopened = CodeEdits::load_from(storage);
    assert_eq!(
        reopened.analysis_drafts(&root),
        [(path.into(), "peer".into())]
    );
}
