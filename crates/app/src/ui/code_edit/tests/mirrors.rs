use super::{CodeEdits, EditorBinding, FileKey};
use crate::ui::code_edit::tests::comparison_fixture;
use gpui_kit::component::input::Rope;
use gpui_kit::{EntityId, Subscription};
use std::path::Path;

fn bind(edits: &mut CodeEdits, key: &FileKey, id: EntityId) {
    let observed = Rope::from(edits.buffers[key].draft.text.as_str());
    edits.bindings.entry(key.clone()).or_default().insert(
        id,
        EditorBinding {
            _subscription: Subscription::new(|| {}),
            observed,
            pending_mirrors: Default::default(),
        },
    );
}

#[test]
fn split_capture_ignores_unchanged_peer_and_obsolete_or_foreign_editor_ids() {
    let (temp, mut edits, root) = comparison_fixture();
    let path = Path::new("main.rs");
    let key = FileKey::new(&root, path).unwrap();
    let left = EntityId::from(1);
    let right = EntityId::from(2);
    bind(&mut edits, &key, left);
    bind(&mut edits, &key, right);
    let old = edits.binding(&key, left).unwrap().observed.clone();
    let typed = Rope::from("from right 😀\r\n");
    edits.capture_editor(&key, right, &typed);
    // Cmd-S or a close checkpoint runs before the subscription's mirror.
    edits.capture_editor(&key, left, &old);
    let alias = FileKey::new(&root, &root.join(path)).unwrap();
    assert_eq!(edits.buffers[&alias].draft.text, typed.to_string());
    assert_eq!(edits.dirty_count(&root), 1);

    let sibling = temp.path().join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    let other_file = Path::new("other.rs");
    for (dir, file) in [(&root, other_file), (&sibling, path)] {
        std::fs::write(dir.join(file), "untouched").unwrap();
        edits.loaded(dir, file, "untouched".into(), true, true);
        let other = FileKey::new(dir, file).unwrap();
        bind(&mut edits, &other, EntityId::from(3));
        assert!(!edits.accepts_input(&other, right, &typed, &typed));
        edits.capture_editor(&other, right, &typed);
        assert!(!edits.dirty(dir, file));
    }

    // The old entity can still be held by a pending formatter.
    edits.forget_editor(&key, right);
    bind(&mut edits, &key, EntityId::from(4));
    assert!(!edits.accepts_input(&key, right, &old, &old));
    edits.capture_editor(&key, right, &old);
    assert_eq!(edits.buffers[&key].draft.text, typed.to_string());
    assert_eq!(edits.bindings[&key].len(), 2);
}

#[test]
fn queued_workspace_and_undo_mirrors_do_not_advance_the_shared_revision() {
    let (_temp, mut edits, root) = comparison_fixture();
    let key = FileKey::new(&root, Path::new("main.rs")).unwrap();
    let left = EntityId::from(1);
    let right = EntityId::from(2);
    bind(&mut edits, &key, left);
    bind(&mut edits, &key, right);
    let before = edits.buffers[&key].draft.text.clone();
    let after = "renamed 😀\r\n";
    let change = pokarak_navigation::WorkspaceEdit {
        files: vec![pokarak_navigation::FileEdits {
            path: root.join(&key.path),
            original: before.clone(),
            edits: vec![pokarak_navigation::TextEdit {
                range: pokarak_navigation::TextRange {
                    start: Default::default(),
                    end: pokarak_navigation::Position {
                        line: 0,
                        character: 8,
                    },
                },
                new_text: "renamed".into(),
            }],
        }],
    };
    let replacements = edits.prepare_workspace_edit(&root, &change).unwrap();
    edits.install_workspace_edit(&root, "rename", replacements);
    assert_eq!(edits.buffers[&key].draft.text, after);
    let (before, after) = (Rope::from(before), Rope::from(after));
    let first_mirrors = [left, right].map(|id| edits.binding(&key, id).unwrap().mirrored(&after));
    edits.undo_transaction(&root).unwrap();
    let version = edits.buffers[&key].revision;
    for (id, first_delivered) in [left, right].into_iter().zip(first_mirrors) {
        let undo_delivered = edits.binding(&key, id).unwrap().mirrored(&before);
        // A stale callback from before the workspace edit may even carry the
        // same bytes as the restored draft. Neither it nor the two mirrors
        // may revise the new transaction.
        assert!(!edits.accepts_input(&key, id, &Rope::from("older input"), &before));
        assert!(!edits.accepts_input(&key, id, &after, &before));
        first_delivered();
        assert!(!edits.accepts_input(&key, id, &before, &before));
        undo_delivered();
    }
    assert_eq!(edits.buffers[&key].revision, version);

    // The same sequence from a user really did change the revision, even if
    // their Undo restored the bytes before GPUI dispatched the first event.
    assert!(!edits.accepts_input(&key, right, &after, &before));
    assert_ne!(edits.buffers[&key].revision, version);
    assert!(edits.accepts_input(&key, right, &before, &before));
}

#[test]
fn format_save_and_native_undo_keep_one_baseline_for_both_editors() {
    let (_temp, mut edits, root) = comparison_fixture();
    let key = FileKey::new(&root, Path::new("main.rs")).unwrap();
    let left = EntityId::from(1);
    let right = EntityId::from(2);
    bind(&mut edits, &key, left);
    bind(&mut edits, &key, right);
    let before = edits.buffers[&key].draft.text.clone();
    let old_target = edits.dirty_targets(&root).pop().unwrap();
    let newer = Rope::from("queued edit");
    edits.capture_editor(&key, right, &newer);
    edits.capture_editor(&key, left, &Rope::from(before.as_str()));
    assert!(
        edits
            .begin_batch_write(&old_target, Some("stale format".into()))
            .is_err()
    );

    let target = edits.dirty_targets(&root).pop().unwrap();
    let formatted = "formatted\r\n";
    let submission = edits
        .begin_batch_write(&target, Some(formatted.into()))
        .unwrap();
    assert!(!edits.can_edit(&root, &key.path));
    let result = submission.write_working_file();
    edits
        .finish_batch_write(&target, formatted, result)
        .unwrap();
    assert!(edits.can_edit(&root, &key.path));
    let revision = edits.buffers[&key].revision;
    for id in [left, right] {
        let after = Rope::from(formatted);
        let delivered = edits.binding(&key, id).unwrap().mirrored(&after);
        assert!(!edits.accepts_input(&key, id, &after, &after));
        delivered();
    }
    assert_eq!(edits.buffers[&key].revision, revision);
    assert!(!edits.dirty(&root, &key.path));
    // Native Undo of the format from either side edits the same retained draft.
    assert!(edits.accepts_input(&key, left, &newer, &newer));
    edits.capture_editor(&key, left, &newer);
    edits.capture_editor(&key, right, &Rope::from(formatted));
    assert_eq!(edits.dirty_count(&root), 1);
    assert_eq!(edits.buffers[&key].draft.text, "queued edit");
    assert_eq!(
        std::fs::read_to_string(root.join(&key.path)).unwrap(),
        formatted
    );
}
