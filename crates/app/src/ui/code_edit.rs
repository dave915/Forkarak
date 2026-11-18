//! Direct working-file editor. Drafts outlive reader entities
//! and project sessions; the core operation owns the actual filesystem write.
mod storage;
use storage::{Alternatives, Checkpoint, DraftStorage};

use super::draft_reconciliation::DraftComparison;
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;
use serde::{Deserialize, Serialize};
use std::{io, path::Component};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct FileKey {
    pub(super) root: PathBuf,
    pub(super) path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::{CodeEdits, FileKey};
    use crate::ui::draft_reconciliation::DraftComparison;
    use std::{fs, path::Path, process::Command};

    mod concurrency;

    fn state() -> (tempfile::TempDir, CodeEdits) {
        let temp = tempfile::tempdir().unwrap();
        let state = CodeEdits::load_from(temp.path().join("session/code-drafts.json"));
        (temp, state)
    }

    fn edit(state: &mut CodeEdits, root: &Path, path: &Path, text: &str) {
        let buffer = state
            .buffers
            .get_mut(&FileKey::new(root, path).unwrap())
            .unwrap();
        buffer.changed(text.into());
    }

    fn comparison_fixture() -> (tempfile::TempDir, CodeEdits, std::path::PathBuf) {
        let (temp, mut state) = state();
        let root = temp.path().join("authored comparison 한글");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        let root = root.canonicalize().unwrap();
        fs::write(root.join("main.rs"), "original 😀\r\n").unwrap();
        state.loaded(
            &root,
            Path::new("main.rs"),
            "original 😀\r\n".into(),
            true,
            true,
        );
        edit(&mut state, &root, Path::new("main.rs"), "my draft 😀\r\n");
        (temp, state, root)
    }

    #[test]
    fn draft_reconciliation_rebases_expected_bytes_without_saving_disk() {
        let (_temp, mut state, root) = comparison_fixture();
        let path = Path::new("main.rs");
        fs::write(root.join(path), "external 😀\r\n").unwrap();
        state.loaded(&root, path, "external 😀\r\n".into(), true, true);
        let comparison = DraftComparison::read(
            root.clone(),
            path.into(),
            "original 😀\r\n".into(),
            "my draft 😀\r\n".into(),
        );
        let fresh = pokarak_core::read_file(&root, path).unwrap();
        let merged = "external + my draft 😀\r\n";
        state.reconcile(&comparison, merged.into(), &fresh).unwrap();
        let buffer = state.buffer(&root, path).unwrap();
        assert_eq!(buffer.draft.expected, fresh);
        assert_eq!(buffer.draft.text, merged);
        assert!(buffer.dirty() && buffer.editing && !buffer.stale);
        assert_eq!(fs::read(root.join(path)).unwrap(), fresh.as_bytes());
        assert!(git(&root, &["ls-files", "--stage", "--", "main.rs"]).is_empty());
        let restored = CodeEdits::load_from(state.storage.path.clone());
        let restored = restored.buffer(&root, path).unwrap();
        assert_eq!(restored.draft.expected, fresh);
        assert_eq!(restored.draft.text, merged);
        // Only the later explicit Save may change the working file.
        pokarak_core::operations::save_file(&root, path, &fresh, merged).unwrap();
        assert_eq!(fs::read(root.join(path)).unwrap(), merged.as_bytes());
    }

    #[test]
    fn draft_reconciliation_rejects_second_disk_change_and_keeps_original_draft() {
        let (_temp, mut state, root) = comparison_fixture();
        let path = Path::new("main.rs");
        fs::write(root.join(path), "external first\r\n").unwrap();
        let comparison = DraftComparison::read(
            root.clone(),
            path.into(),
            "original 😀\r\n".into(),
            "my draft 😀\r\n".into(),
        );
        fs::write(root.join(path), "external second\r\n").unwrap();
        let fresh = pokarak_core::read_file(&root, path).unwrap();
        let error = state
            .reconcile(&comparison, "retained result\r\n".into(), &fresh)
            .unwrap_err();
        assert!(error.contains("다시 변경"));
        let buffer = state.buffer(&root, path).unwrap();
        assert_eq!(buffer.draft.expected, "original 😀\r\n");
        assert_eq!(buffer.draft.text, "my draft 😀\r\n");
        assert_eq!(fs::read(root.join(path)).unwrap(), fresh.as_bytes());
        // An explicit reread may adopt the new disk version after review.
        let reloaded = DraftComparison::read(
            root.clone(),
            path.into(),
            comparison.original,
            comparison.draft,
        );
        state
            .reconcile(&reloaded, "retained result\r\n".into(), &fresh)
            .unwrap();
        assert_eq!(
            state.buffer(&root, path).unwrap().draft.text,
            "retained result\r\n"
        );
    }

    #[test]
    fn draft_reconciliation_rejects_newer_input_and_isolates_other_worktree() {
        let (temp, mut state, root) = comparison_fixture();
        let path = Path::new("main.rs");
        let other = temp.path().join("another worktree");
        fs::create_dir(&other).unwrap();
        git(&other, &["init", "-b", "main"]);
        let other = other.canonicalize().unwrap();
        fs::write(other.join(path), "other original\r\n").unwrap();
        state.loaded(&other, path, "other original\r\n".into(), true, true);
        edit(&mut state, &other, path, "other draft\r\n");
        let comparison = DraftComparison::read(
            root.clone(),
            path.into(),
            "original 😀\r\n".into(),
            "my draft 😀\r\n".into(),
        );
        edit(&mut state, &root, path, "newer queued input\r\n");
        assert!(
            state
                .reconcile(&comparison, "obsolete result\r\n".into(), "original 😀\r\n")
                .is_err()
        );
        assert_eq!(
            state.buffer(&root, path).unwrap().draft.text,
            "newer queued input\r\n"
        );
        assert_eq!(
            state.buffer(&other, path).unwrap().draft.text,
            "other draft\r\n"
        );
        assert_eq!(
            state.buffer(&other, path).unwrap().draft.expected,
            "other original\r\n"
        );
    }

    #[test]
    fn draft_reconciliation_storage_or_missing_disk_failure_keeps_baseline() {
        let (_temp, mut state, root) = comparison_fixture();
        let path = Path::new("main.rs");
        fs::write(root.join(path), "external\r\n").unwrap();
        let comparison = DraftComparison::read(
            root.clone(),
            path.into(),
            "original 😀\r\n".into(),
            "my draft 😀\r\n".into(),
        );
        fs::create_dir_all(&state.storage.path).unwrap(); // Block atomic checkpoint.
        assert!(
            state
                .reconcile(&comparison, "merged\r\n".into(), "external\r\n")
                .is_err()
        );
        assert_eq!(
            state.buffer(&root, path).unwrap().draft.expected,
            "original 😀\r\n"
        );
        assert_eq!(
            state.buffer(&root, path).unwrap().draft.text,
            "my draft 😀\r\n"
        );
        fs::remove_file(root.join(path)).unwrap();
        let missing = DraftComparison::read(
            root.clone(),
            path.into(),
            comparison.original,
            comparison.draft,
        );
        assert!(state.reconcile(&missing, "merged\r\n".into(), "").is_err());
        assert_eq!(
            state.buffer(&root, path).unwrap().draft.text,
            "my draft 😀\r\n"
        );
    }

    #[test]
    fn drafts_are_isolated_by_worktree_and_path() {
        let (_temp, mut state) = state();
        let first = Path::new("/fixture/worktree-one");
        let second = Path::new("/fixture/worktree-two");
        let path = Path::new("src/Main.kt");
        state.loaded(first, path, "one".into(), true, true);
        state.loaded(second, path, "two".into(), true, true);
        state.loaded(first, Path::new("other.kt"), "other".into(), true, true);
        edit(&mut state, first, path, "draft");
        assert!(state.dirty(first, path));
        assert!(!state.dirty(second, path));
        assert!(!state.dirty(first, Path::new("other.kt")));
        assert_eq!(state.loaded(first, path, "one".into(), true, true), "draft");
        assert_eq!(state.loaded(second, path, "two".into(), true, true), "two");
    }

    #[test]
    fn refresh_that_started_before_typing_does_not_replace_dirty_buffer() {
        let (_temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        state.loaded(root, path, "disk before refresh".into(), true, true);
        assert!(state.editing(root, path));
        // Keystroke arrives while the load is in flight.
        edit(&mut state, root, path, "unsaved 😀\r\n");
        assert_eq!(
            state.loaded(root, path, "external edit".into(), true, true),
            "unsaved 😀\r\n"
        );
        let buffer = state.buffer(root, path).unwrap();
        assert_eq!(buffer.draft.expected, "disk before refresh");
        assert!(buffer.stale && buffer.editing && buffer.dirty());
        // Repeated refreshes must not reset the expected version to new disk.
        assert_eq!(
            state.loaded(root, path, "another external edit".into(), true, true),
            "unsaved 😀\r\n"
        );
        assert_eq!(
            state.buffer(root, path).unwrap().draft.expected,
            "disk before refresh"
        );
    }

    #[test]
    fn clean_direct_editor_adopts_external_change_without_a_phantom_conflict() {
        let (_temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        state.loaded(root, path, "original".into(), true, true);
        edit(&mut state, root, path, "original");
        assert_eq!(
            state.loaded(root, path, "external".into(), true, true),
            "external"
        );
        assert!(!state.buffer(root, path).unwrap().stale);
        assert!(state.editing(root, path));
        assert!(!state.dirty(root, path));
    }

    #[test]
    fn restoring_original_bytes_cleans_all_draft_consumers_and_late_checkpoints() {
        let (_temp, mut state, root) = comparison_fixture();
        let path = Path::new("main.rs");
        // Exercise exact BOM, Unicode, CRLF/mixed newlines and final-newline policy.
        for original in [
            "\u{feff}한글 😀\r\nnext\n",
            "no final newline 😀",
            "",
            "\r\n",
        ] {
            state.buffers.clear();
            fs::write(root.join(path), original).unwrap();
            let disk = pokarak_core::read_file(&root, path).unwrap();
            state.loaded(&root, path, disk, true, true);
            assert!(state.can_edit(&root, path)); // No activation action.
            let index_before = git(&root, &["ls-files", "--stage", "--", "main.rs"]);
            edit(&mut state, &root, path, &format!("{original}typed"));
            assert!(state.dirty(&root, path));
            assert_eq!(state.buffer(&root, path).unwrap().save_label(), "● 저장");
            state.persist().unwrap();
            let older = state.drafts();
            let old_generation = state.generation;
            // Undo and a manual full-byte restoration both reach this Change path.
            edit(&mut state, &root, path, original);
            assert!(!state.dirty(&root, path));
            assert!(state.can_edit(&root, path)); // Clean still accepts typing.
            assert!(state.retained_text(&root, path).is_none());
            assert!(state.analysis_drafts(&root).is_empty());
            assert!(!state.has_draft_under(&root, path));
            assert!(state.drafts().is_empty());
            let buffer = state.buffer(&root, path).unwrap();
            assert!(!buffer.stale && buffer.error.is_none());
            assert_eq!(buffer.save_label(), "저장");
            assert_eq!(buffer.status(), "변경 없음");
            state.persist().unwrap();
            state
                .storage
                .checkpoint(old_generation, &older, None)
                .unwrap();
            assert!(
                CodeEdits::load_from(state.storage.path.clone())
                    .buffers
                    .is_empty()
            );
            assert_eq!(fs::read(root.join(path)).unwrap(), original.as_bytes());
            assert_eq!(
                git(&root, &["ls-files", "--stage", "--", "main.rs"]),
                index_before
            );
        }
    }

    #[test]
    fn restoring_original_does_not_hide_a_real_external_conflict() {
        let (_temp, mut state, root) = comparison_fixture();
        let path = Path::new("main.rs");
        fs::write(root.join(path), "external\r\n").unwrap();
        state.loaded(&root, path, "external\r\n".into(), true, true);
        edit(&mut state, &root, path, "original 😀\r\n");
        assert!(!state.dirty(&root, path));
        assert!(state.stale(&root, path));
        assert!(state.drafts().is_empty());
        assert_eq!(
            state.loaded(&root, path, "external\r\n".into(), true, true),
            "external\r\n"
        );
        assert!(!state.stale(&root, path));
        assert!(state.can_edit(&root, path));
    }

    #[test]
    fn unavailable_binary_and_historical_sources_cannot_reactivate_retained_input() {
        let (_temp, mut state, root) = comparison_fixture();
        let path = Path::new("main.rs");
        // Deleted/unreadable retains actual edits, but never allows new typing.
        state.loaded(&root, path, "original 😀\r\n".into(), false, true);
        assert!(state.dirty(&root, path));
        assert!(!state.can_edit(&root, path));
        state.loaded(&root, path, "binary\0data".into(), true, true);
        assert!(!state.can_edit(&root, path));
        state.loaded(&root, path, "historic\n".into(), true, false);
        assert!(state.dirty(&root, path));
        assert!(!state.can_edit(&root, path));
        state.loaded(&root, path, "original 😀\r\n".into(), true, true);
        assert!(state.can_edit(&root, path));
        assert_eq!(
            state.buffer(&root, path).unwrap().draft.text,
            "my draft 😀\r\n"
        );
    }

    #[test]
    fn legacy_clean_checkpoint_is_not_restored_as_a_draft() {
        let (_temp, state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        let draft = super::Draft {
            key: FileKey::new(root, path).unwrap(),
            expected: "unchanged\r\n".into(),
            text: "unchanged\r\n".into(),
        };
        fs::create_dir_all(state.storage.path.parent().unwrap()).unwrap();
        fs::write(&state.storage.path, serde_json::to_vec(&[draft]).unwrap()).unwrap();
        let mut restored = CodeEdits::load_from(state.storage.path.clone());
        assert!(restored.buffers.is_empty());
        restored.loaded(root, path, "unchanged\r\n".into(), true, true);
        assert!(restored.can_edit(root, path));
        assert!(!restored.dirty(root, path));
    }

    #[test]
    fn externally_deleted_or_unreadable_file_keeps_its_draft() {
        let (_temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        state.loaded(root, path, "original".into(), true, true);
        edit(&mut state, root, path, "unsaved");
        let fallback = state.retained_text(root, path).unwrap();
        assert_eq!(state.loaded(root, path, fallback, false, true), "unsaved");
        assert!(state.buffer(root, path).unwrap().stale);
        assert!(state.dirty(root, path));
    }

    #[test]
    fn historical_deleted_binary_and_external_previews_do_not_enable_editing() {
        let (_temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        assert_eq!(
            state.loaded(root, path, "old commit".into(), true, false),
            "old commit"
        );
        assert!(state.buffer(root, path).is_none());
        state.loaded(root, path, "working file".into(), true, true);
        edit(&mut state, root, path, "working file");
        // Previously live, now a historical deletion preview.
        state.loaded(root, path, "old commit".into(), false, false);
        assert!(!state.editing(root, path));
        assert!(state.buffer(root, path).is_none());
        state.loaded(
            root,
            Path::new("/outside/dependency.rs"),
            "dependency".into(),
            true,
            true,
        );
        assert!(
            state
                .buffer(root, Path::new("/outside/dependency.rs"))
                .is_none()
        );
        assert!(FileKey::new(root, Path::new("../escape")).is_none());
    }

    #[test]
    fn restarting_restores_exact_drafts_and_reenables_input_after_disk_validation() {
        let (_temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("src/한글.kt");
        state.loaded(root, path, "class A {}\r\n".into(), true, true);
        state.loaded(root, Path::new("clean.rs"), "clean".into(), true, true);
        edit(&mut state, root, path, "class 한글 { val x = \"😀\" }\r\n");
        state.persist().unwrap();
        let storage = state.storage.path.clone();
        drop(state); // Reader entities and all project sessions can be gone.
        let mut restored = CodeEdits::load_from(storage);
        assert_eq!(restored.buffers.len(), 1);
        assert!(!restored.editing(root, path)); // Revalidate the working file first.
        assert_eq!(
            restored.loaded(root, path, "class A {}\r\n".into(), true, true),
            "class 한글 { val x = \"😀\" }\r\n"
        );
        assert_eq!(
            restored.buffer(root, path).unwrap().draft.expected,
            "class A {}\r\n"
        );
        assert!(restored.editing(root, path)); // No Edit activation, even after restart.
        // A restored draft must also survive an external refresh.
        assert_eq!(
            restored.loaded(root, path, "external".into(), true, true),
            "class 한글 { val x = \"😀\" }\r\n"
        );
        assert!(restored.buffer(root, path).unwrap().stale);
    }

    #[test]
    fn successful_save_keeps_any_newer_input_dirty() {
        let (_temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        state.loaded(root, path, "original".into(), true, true);
        edit(&mut state, root, path, "submitted");
        state
            .buffers
            .get_mut(&FileKey::new(root, path).unwrap())
            .unwrap()
            .saving = true;
        edit(&mut state, root, path, "newer queued input");
        let buffer = state
            .buffers
            .get_mut(&FileKey::new(root, path).unwrap())
            .unwrap();
        buffer.saved("submitted");
        assert_eq!(buffer.draft.text, "newer queued input");
        assert_eq!(buffer.draft.expected, "submitted");
        assert!(buffer.dirty());
        buffer.saved("newer queued input");
        state.persist().unwrap();
        assert!(
            CodeEdits::load_from(state.storage.path.clone())
                .buffers
                .is_empty()
        );
    }

    #[test]
    fn old_background_checkpoint_cannot_overwrite_close_flush() {
        let (_temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        state.loaded(root, path, "original".into(), true, true);
        edit(&mut state, root, path, "older draft");
        let older = state.drafts();
        state.generation = 1; // Background checkpoint was scheduled.
        edit(&mut state, root, path, "latest before close");
        state.persist().unwrap(); // Generation 2, synchronous close flush.
        state.storage.checkpoint(1, &older, None).unwrap(); // Older task starts late.
        let restored = CodeEdits::load_from(state.storage.path.clone());
        assert_eq!(
            restored.buffer(root, path).unwrap().draft.text,
            "latest before close"
        );
    }

    #[test]
    fn checkpoint_failure_keeps_memory_draft_and_can_be_retried() {
        let (temp, mut state) = state();
        let root = Path::new("/fixture/worktree");
        let path = Path::new("main.rs");
        fs::write(temp.path().join("session"), "blocking regular file").unwrap();
        state.loaded(root, path, "original".into(), true, true);
        edit(&mut state, root, path, "unsaved");
        assert!(state.persist().is_err());
        assert!(state.storage_error.is_some());
        assert_eq!(state.buffer(root, path).unwrap().draft.text, "unsaved");
        fs::remove_file(temp.path().join("session")).unwrap();
        state.persist().unwrap();
        assert!(state.storage_error.is_none());
        assert_eq!(
            CodeEdits::load_from(state.storage.path.clone())
                .buffer(root, path)
                .unwrap()
                .draft
                .text,
            "unsaved"
        );
    }

    #[test]
    fn malformed_draft_storage_is_reported_and_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path().join("code-drafts.json");
        fs::write(&storage, b"unreadable previous draft").unwrap();
        let mut state = CodeEdits::load_from(storage.clone());
        assert!(state.unreadable_storage && state.storage_error.is_some());
        assert!(state.persist().is_err());
        assert_eq!(fs::read(&storage).unwrap(), b"unreadable previous draft");
    }

    // Authored temporary repository only; no user/global configuration, hooks,
    // remotes, staging or commits are involved in either save path.
    fn git(root: &Path, args: &[&str]) -> Vec<u8> {
        let mut command = Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        let output = command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "--literal-pathspecs",
                "-c",
                "core.hooksPath=/dev/null",
                "-C",
            ])
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[test]
    fn real_save_changes_only_working_file_and_external_conflict_retains_draft() {
        let (temp, mut state) = state();
        let root = temp.path().join("authored repository");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = Path::new("hello.rs");
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join(path), "before\r\n").unwrap();
        // A separate session directory is outside the inspected repository.
        assert!(!state.storage.path.starts_with(&root));
        state.loaded(&root, path, "before\r\n".into(), true, true);
        edit(&mut state, &root, path, "after 😀\r\n");
        let draft = state.buffer(&root, path).unwrap().draft.clone();
        let index_before = git(&root, &["ls-files", "--stage", "--", "hello.rs"]);
        pokarak_core::operations::save_file(&root, path, &draft.expected, &draft.text).unwrap();
        state
            .buffers
            .get_mut(&draft.key)
            .unwrap()
            .saved(&draft.text);
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "after 😀\r\n");
        assert_eq!(
            git(&root, &["ls-files", "--stage", "--", "hello.rs"]),
            index_before
        );
        assert!(!state.dirty(&root, path));
        edit(&mut state, &root, path, "unsaved next edit");
        fs::write(root.join(path), "external writer").unwrap();
        let draft = state.buffer(&root, path).unwrap().draft.clone();
        assert!(
            pokarak_core::operations::save_file(&root, path, &draft.expected, &draft.text).is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join(path)).unwrap(),
            "external writer"
        );
        assert!(state.dirty(&root, path));
        assert_eq!(
            state.loaded(&root, path, "external writer".into(), true, true),
            "unsaved next edit"
        );
        fs::remove_file(root.join(path)).unwrap();
        assert!(
            pokarak_core::operations::save_file(&root, path, &draft.expected, &draft.text).is_err()
        );
        state.persist().unwrap();
        assert_eq!(
            CodeEdits::load_from(state.storage.path.clone())
                .buffer(&root, path)
                .unwrap()
                .draft
                .text,
            "unsaved next edit"
        );
        assert!(git(&root, &["ls-files", "--stage", "--", "hello.rs"]).is_empty());
        assert!(!root.join(".git/refs/heads/main").exists());
    }
    #[test]
    fn folder_move_guard_keeps_dirty_descendants_and_isolates_worktree() {
        let (_temp, mut state, root) = comparison_fixture();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/dirty.rs"), "disk\n").unwrap();
        std::fs::write(root.join("src/clean.rs"), "clean\n").unwrap();
        state.loaded(
            &root,
            Path::new("src/dirty.rs"),
            "disk\n".into(),
            true,
            true,
        );
        state.loaded(
            &root,
            Path::new("src/clean.rs"),
            "clean\n".into(),
            true,
            true,
        );
        edit(
            &mut state,
            &root,
            Path::new("src/dirty.rs"),
            "retained draft\n",
        );
        assert!(state.has_draft_under(&root, Path::new("src")));
        assert!(!state.has_draft_under(&root.join("other"), Path::new("src")));
        assert!(!state.has_draft_under(&root, Path::new("src/clean.rs")));
        state.forget_clean_under(&root, Path::new("src"));
        assert_eq!(
            state
                .buffer(&root, Path::new("src/dirty.rs"))
                .unwrap()
                .draft
                .text,
            "retained draft\n"
        );
        assert!(state.buffer(&root, Path::new("src/clean.rs")).is_none());
        assert_eq!(
            std::fs::read_to_string(root.join("src/dirty.rs")).unwrap(),
            "disk\n"
        );
    }
}

impl FileKey {
    pub(super) fn new(root: &Path, path: &Path) -> Option<Self> {
        let path = if path.is_absolute() {
            path.strip_prefix(root).ok()?
        } else {
            path
        };
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return None;
        }
        Some(Self {
            root: root.to_owned(),
            path: path.to_owned(),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Draft {
    key: FileKey,
    expected: String,
    text: String,
}

#[derive(Clone)]
struct Buffer {
    draft: Draft,
    editing: bool,
    stale: bool,
    retired: bool,
    saving: bool,
    error: Option<String>,
}

impl Buffer {
    fn new(draft: Draft) -> Self {
        Self {
            draft,
            editing: false,
            stale: false,
            retired: false,
            saving: false,
            error: None,
        }
    }

    fn dirty(&self) -> bool {
        self.draft.expected != self.draft.text
    }

    fn save_label(&self) -> &'static str {
        if self.saving {
            "저장 중…"
        } else if self.dirty() {
            "● 저장"
        } else {
            "저장"
        }
    }

    fn status(&self) -> &'static str {
        if self.stale {
            "외부 변경 감지 — 디스크 내용을 확인하세요."
        } else if self.dirty() {
            "미저장 변경 — ⌘S 저장"
        } else {
            "변경 없음"
        }
    }

    fn loaded(&mut self, disk: &str, available: bool) {
        // Even a queued refresh that began before the first keystroke must use
        // the buffer's state NOW, never its state at request time.
        if self.dirty() || self.saving {
            self.stale = !available || self.draft.expected != disk;
        } else {
            self.draft.expected = disk.to_owned();
            self.draft.text = disk.to_owned();
            self.stale = !available;
            self.error = None;
        }
    }

    fn saved(&mut self, submitted: &str) {
        // Keep any edits newer than the submitted snapshot. Normally the native
        // editor is read-only during a save, but queued input can still arrive.
        self.draft.expected = submitted.to_owned();
        self.saving = false;
        self.stale = false;
        self.error = None;
    }

    fn changed(&mut self, text: String) {
        if self.draft.text != text {
            self.draft.text = text;
            self.retired = false;
            self.error = None;
        }
        // `editing` is input eligibility, never evidence of an unsaved draft.
        // Keep a real external conflict until a disk read revalidates it.
    }
}

pub(super) struct CodeEdits {
    pub(super) tools: editor_tools::Tools,
    buffers: HashMap<FileKey, Buffer>,
    bindings: HashMap<FileKey, (EntityId, Subscription)>,
    storage: Arc<DraftStorage>,
    alternatives: Alternatives,
    generation: u64,
    storage_error: Option<String>,
    unreadable_storage: bool,
    commit_after_refresh: Option<(PathBuf, u64)>,
    comparison_request: u64,
}

impl CodeEdits {
    pub(super) fn load() -> Self {
        Self::load_from(crate::prefs::config_path().with_file_name("code-drafts.json"))
    }

    fn load_from(storage: PathBuf) -> Self {
        let mut storage = DraftStorage::new(storage);
        let result = storage.load();
        let mut state = Self {
            tools: editor_tools::Tools::default(),
            buffers: HashMap::new(),
            bindings: HashMap::new(),
            storage: Arc::new(storage),
            alternatives: Alternatives::new(),
            generation: 0,
            storage_error: None,
            unreadable_storage: false,
            commit_after_refresh: None,
            comparison_request: 0,
        };
        match result {
            Ok((drafts, alternatives)) => {
                state.alternatives = alternatives;
                for draft in drafts {
                    state.buffers.insert(draft.key.clone(), Buffer::new(draft));
                }
            }
            Err(error) => {
                state.storage_error = Some(format!("편집 초안을 복원하지 못했습니다: {error}"));
                state.unreadable_storage = true;
            }
        }
        state
    }

    fn accept_checkpoint(&mut self, result: io::Result<Checkpoint>) -> io::Result<Option<Draft>> {
        self.storage_error = result
            .as_ref()
            .err()
            .map(|e| format!("편집 초안을 보관할 수 없습니다: {e}"));
        let checkpoint = result?;
        self.alternatives = checkpoint.alternatives;
        for (key, buffer) in &mut self.buffers {
            buffer.retired = checkpoint.retired.contains(key);
        }
        Ok(checkpoint.recovered)
    }

    fn checkpoint(&mut self, recover: Option<(&FileKey, u64)>) -> io::Result<Option<Draft>> {
        if self.unreadable_storage {
            return Err(io::Error::other(
                self.storage_error.clone().unwrap_or_default(),
            ));
        }
        self.generation += 1;
        let result = self
            .storage
            .checkpoint(self.generation, &self.drafts(), recover);
        self.accept_checkpoint(result)
    }

    fn persist(&mut self) -> io::Result<()> {
        self.checkpoint(None).map(|_| ())
    }

    fn recover(&mut self, key: &FileKey, revision: u64) -> io::Result<()> {
        if self.buffers.get(key).is_some_and(|b| b.saving) {
            return Err(io::Error::other("파일 저장이 진행 중입니다."));
        }
        let draft = self.checkpoint(Some((key, revision)))?.ok_or_else(|| {
            io::Error::other("선택한 초안 버전이 변경되었습니다. 보관된 다른 버전을 확인하세요.")
        })?;
        self.buffers.insert(key.clone(), Buffer::new(draft));
        self.bindings.remove(key);
        Ok(())
    }

    fn discard(&mut self, expected: &Draft) -> io::Result<()> {
        let key = &expected.key;
        if self
            .buffers
            .get(key)
            .is_none_or(|b| b.saving || b.draft != *expected)
        {
            return Err(io::Error::other("초안이 변경되었습니다. 다시 확인하세요."));
        }
        let buffer = self.buffers.remove(key).unwrap();
        if let Err(error) = self.persist() {
            self.buffers.insert(key.clone(), buffer);
            return Err(error);
        }
        self.bindings.remove(key);
        Ok(())
    }

    fn drafts(&self) -> Vec<Draft> {
        self.buffers
            .values()
            .filter(|b| b.dirty())
            .map(|b| b.draft.clone())
            .collect()
    }

    fn loaded(
        &mut self,
        root: &Path,
        path: &Path,
        disk: String,
        available: bool,
        live: bool,
    ) -> String {
        if !live && let Some(key) = FileKey::new(root, path) {
            // Historical previews never inherit a live reader's Edit state.
            if self
                .buffers
                .get(&key)
                .is_some_and(|b| !b.dirty() && !b.saving)
            {
                self.buffers.remove(&key);
                self.bindings.remove(&key);
            }
            if let Some(buffer) = self.buffers.get_mut(&key) {
                buffer.editing = false;
            }
        }
        let Some(key) = FileKey::new(root, path).filter(|_| live) else {
            return disk;
        };
        let buffer = self.buffers.entry(key.clone()).or_insert_with(|| {
            Buffer::new(Draft {
                key,
                expected: disk.clone(),
                text: disk.clone(),
            })
        });
        buffer.editing = available
            && !self.unreadable_storage
            && disk.len() <= pokarak_core::MAX_TEXT_BYTES
            && !disk.as_bytes().contains(&0);
        buffer.loaded(&disk, available);
        buffer.draft.text.clone()
    }

    pub(super) fn expected_text(&self, root: &Path, path: &Path) -> Option<String> {
        self.buffer(root, path)
            .map(|buffer| buffer.draft.expected.clone())
    }

    pub(super) fn can_edit(&self, root: &Path, path: &Path) -> bool {
        !self.unreadable_storage && self.editing(root, path)
    }

    fn choices(&self, root: &Path, path: &Path) -> Option<&[storage::Choice]> {
        self.alternatives
            .get(&FileKey::new(root, path)?)
            .map(Vec::as_slice)
    }

    pub(super) fn has_draft_under(&self, root: &Path, path: &Path) -> bool {
        self.unreadable_storage
            || self
                .alternatives
                .keys()
                .any(|key| key.root == root && key.path.starts_with(path))
            || self.buffers.iter().any(|(key, buffer)| {
                key.root == root && key.path.starts_with(path) && (buffer.dirty() || buffer.saving)
            })
    }

    pub(super) fn forget_clean_under(&mut self, root: &Path, path: &Path) {
        self.bindings
            .retain(|key, _| !(key.root == root && key.path.starts_with(path)));
        self.buffers.retain(|key, buffer| {
            !(key.root == root && key.path.starts_with(path) && !buffer.dirty() && !buffer.saving)
        });
    }

    pub(super) fn dirty(&self, root: &Path, path: &Path) -> bool {
        self.buffer(root, path).is_some_and(Buffer::dirty)
    }

    pub(super) fn reader_status(&self, root: &Path, path: &Path) -> Option<&'static str> {
        self.buffer(root, path).map(Buffer::status)
    }

    pub(super) fn stale(&self, root: &Path, path: &Path) -> bool {
        self.buffer(root, path).is_some_and(|buffer| buffer.stale)
    }

    pub(super) fn saving_in(&self, root: &Path) -> bool {
        self.buffers
            .values()
            .any(|buffer| buffer.draft.key.root == root && buffer.saving)
    }

    fn buffer(&self, root: &Path, path: &Path) -> Option<&Buffer> {
        self.buffers.get(&FileKey::new(root, path)?)
    }

    pub(super) fn retained_text(&self, root: &Path, path: &Path) -> Option<String> {
        self.buffer(root, path)
            .filter(|b| b.dirty())
            .map(|b| b.draft.expected.clone())
    }

    /// Retained current working drafts for a snapshot-based language request.
    /// Historical sources never create these buffers.
    pub(super) fn analysis_drafts(&self, root: &Path) -> Vec<(PathBuf, String)> {
        self.buffers
            .iter()
            .filter(|(key, buffer)| key.root == root && buffer.dirty())
            .map(|(key, buffer)| (key.path.clone(), buffer.draft.text.clone()))
            .collect()
    }

    pub(super) fn editing(&self, root: &Path, path: &Path) -> bool {
        self.buffer(root, path)
            .is_some_and(|b| b.editing && !b.saving)
    }

    fn reconcile(
        &mut self,
        comparison: &DraftComparison,
        result: String,
        fresh_disk: &str,
    ) -> Result<(), String> {
        comparison.validate_disk(fresh_disk)?;
        if result.len() > pokarak_core::MAX_TEXT_BYTES || result.as_bytes().contains(&0) {
            return Err("조정 결과는 NUL이 없는 2 MiB 이하의 텍스트여야 합니다.".into());
        }
        let key = FileKey::new(&comparison.root, &comparison.path)
            .ok_or("워크트리 안의 파일을 선택하세요.")?;
        let buffer = self
            .buffers
            .get_mut(&key)
            .ok_or("편집 초안이 닫혔습니다.")?;
        if buffer.saving
            || buffer.draft.expected != comparison.original
            || buffer.draft.text != comparison.draft
        {
            return Err("비교 중 초안이 변경되었습니다. 최신 초안으로 다시 비교하세요.".into());
        }
        let previous = buffer.clone();
        buffer.draft.expected = fresh_disk.to_owned();
        buffer.draft.text = result;
        buffer.editing = true;
        buffer.stale = false;
        buffer.error = None;
        if let Err(error) = self.persist() {
            self.buffers.insert(key, previous);
            return Err(format!(
                "조정한 초안을 보관하지 못했습니다. 결과를 복사해 두세요: {error}"
            ));
        }
        Ok(())
    }
}

impl Pokarak {
    pub(super) fn capture_code_buffers(&mut self, cx: &App) {
        // Capture synchronously too: close/quit can precede a queued Change event.
        for (root, session) in &self.sessions {
            for (path, editor) in &session.documents {
                if let Some(key) = FileKey::new(root, path)
                    && self
                        .code_edits
                        .bindings
                        .get(&key)
                        .is_some_and(|(id, _)| *id == editor.entity_id())
                    && let Some(buffer) = self.code_edits.buffers.get_mut(&key)
                    && buffer.editing
                {
                    buffer.changed(editor.read(cx).value().to_string());
                }
            }
        }
    }

    pub(super) fn retain_code_drafts(&mut self, cx: &App) -> bool {
        self.capture_code_buffers(cx);
        self.code_edits.persist().is_ok()
    }

    pub(super) fn checkpoint_code_drafts(&mut self, cx: &mut Context<Self>) {
        if self.code_edits.unreadable_storage {
            return;
        }
        self.code_edits.generation += 1;
        let generation = self.code_edits.generation;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |view, cx| {
            executor.timer(Duration::from_millis(350)).await;
            let _ = view.update(cx, |app, cx| {
                if app.code_edits.generation != generation {
                    return;
                }
                let storage = app.code_edits.storage.clone();
                let drafts = app.code_edits.drafts();
                let task = cx
                    .background_spawn(async move { storage.checkpoint(generation, &drafts, None) });
                cx.spawn(async move |view, cx| {
                    let result = task.await;
                    let _ = view.update(cx, |app, cx| {
                        if app.code_edits.generation != generation {
                            return;
                        }
                        let _ = app.code_edits.accept_checkpoint(result);
                        cx.notify();
                    });
                })
                .detach();
            });
        })
        .detach();
    }

    pub(super) fn check_code_disk_changes(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let files: Vec<_> = self
            .code_edits
            .buffers
            .values()
            .filter(|b| b.draft.key.root == root && (b.editing || b.dirty()) && !b.saving)
            .map(|b| (b.draft.key.clone(), b.draft.expected.clone()))
            .collect();
        if files.is_empty() {
            return;
        }
        let task = cx.background_spawn(async move {
            files
                .into_iter()
                .map(|(key, expected)| {
                    let stale = pokarak_core::read_file(&key.root, &key.path)
                        .ok()
                        .as_deref()
                        != Some(&expected);
                    (key, expected, stale)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |view, cx| {
            let states = task.await;
            let _ = view.update(cx, |app, cx| {
                if app.current.as_ref() != Some(&root) {
                    return;
                }
                for (key, expected, stale) in states {
                    if let Some(buffer) = app.code_edits.buffers.get_mut(&key)
                        && buffer.draft.expected == expected
                        && !buffer.saving
                    {
                        buffer.stale = stale;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn block_code_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.retain_code_drafts(cx) {
            return false;
        }
        self.status =
            "초안 보관 실패 — 파일을 저장하거나 초안 저장 경로를 복구한 후 닫으세요.".into();
        cx.notify();
        true
    }

    pub(super) fn prepare_code_buffer(
        &mut self,
        root: &Path,
        path: &Path,
        disk: String,
        available: bool,
        live: bool,
    ) -> String {
        // The former editor.direct_edit preference is retired, including old
        // explicit false/project overrides. Eligible Code files always accept input.
        self.code_edits.loaded(root, path, disk, available, live)
    }

    pub(super) fn enable_direct_code_edit(
        &mut self,
        root: &Path,
        path: &Path,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let readonly = !self.code_edits.can_edit(root, path);
        editor.update(cx, |state, cx| state.set_readonly(readonly, cx));
    }

    pub(super) fn bind_code_editor(
        &mut self,
        root: &Path,
        path: &Path,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = FileKey::new(root, path) else {
            return;
        };
        if !self.code_edits.buffers.contains_key(&key)
            || self
                .code_edits
                .bindings
                .get(&key)
                .is_some_and(|(id, _)| *id == editor.entity_id())
        {
            return;
        }
        let changed_key = key.clone();
        let subscription = cx.subscribe(editor, move |app, editor, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            // A replaced editor can still have a queued Change event. It must
            // never write its old text over a reconciled replacement buffer.
            if !app
                .code_edits
                .bindings
                .get(&changed_key)
                .is_some_and(|(id, _)| *id == editor.entity_id())
            {
                return;
            }
            let Some(buffer) = app.code_edits.buffers.get_mut(&changed_key) else {
                return;
            };
            if !buffer.editing {
                return;
            }
            buffer.changed(editor.read(cx).value().to_string());
            let status = buffer.status();
            if app.current.as_ref() == Some(&changed_key.root)
                && app.selected().as_ref() == Some(&changed_key.path)
            {
                app.status = status.into();
            }
            app.pin_preview(&changed_key.root, &changed_key.path);
            app.refresh_inline_tools(&changed_key.root, &changed_key.path, &editor, cx);
            app.checkpoint_code_drafts(cx);
            cx.notify();
        });
        self.code_edits
            .bindings
            .insert(key, (editor.entity_id(), subscription));
    }

    pub(super) fn start_code_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_edits.unreadable_storage
            || self.git_history.active
            || self.file_loading
            || self.git_is_busy()
            || self.loading
            || window.has_active_dialog(cx)
            || !self.session().is_some_and(|s| s.saved.code)
        {
            return;
        }
        let (Some(root), Some(path), Some(editor)) =
            (self.current.clone(), self.selected(), self.active_editor())
        else {
            return;
        };
        let Some(key) = FileKey::new(&root, &path) else {
            return;
        };
        let Some(buffer) = self.code_edits.buffers.get_mut(&key) else {
            return;
        };
        if !buffer.editing || buffer.saving {
            return;
        }
        self.pin_preview(&root, &path);
        editor.update(cx, |state, cx| state.set_readonly(false, cx));
        editor.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn save_code_file(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cmd-S can arrive before the queued Change event for an undo. Inspect
        // the actual buffer before deciding whether format-on-save is needed.
        self.capture_code_buffers(cx);
        if self.prefs.option_bool(
            self.current_project.as_deref(),
            "editor.format_on_save",
            false,
        ) && self
            .current
            .as_ref()
            .zip(self.selected().as_ref())
            .is_some_and(|(root, path)| self.code_edits.dirty(root, path))
        {
            self.format_code_file(true, commit, window, cx);
        } else {
            self.save_code_file_after_format(commit, window, cx);
        }
    }

    pub(super) fn save_code_file_after_format(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_history.active
            || self.file_loading
            || self.loading
            || self.git_is_busy()
            || self.opening_path.is_some()
            || window.has_active_dialog(cx)
            || !self.session().is_some_and(|s| s.saved.code)
        {
            return;
        }
        let (Some(root), Some(path), Some(editor)) =
            (self.current.clone(), self.selected(), self.active_editor())
        else {
            return;
        };
        let Some(key) = FileKey::new(&root, &path) else {
            return;
        };
        self.retain_code_drafts(cx);
        let Some(buffer) = self.code_edits.buffers.get_mut(&key) else {
            return;
        };
        if !buffer.dirty() || buffer.saving {
            return;
        }
        // The operation always checks expected bytes, including when the watcher
        // has not delivered an external-write event yet. There is no force save.
        let expected = buffer.draft.expected.clone();
        let replacement = buffer.draft.text.clone();
        buffer.saving = true;
        buffer.error = None;
        editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
        let target = key.clone();
        let submitted = replacement.clone();
        let task = cx.background_spawn(async move {
            pokarak_core::operations::save_file(&target.root, &target.path, &expected, &replacement)
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                let Some(buffer) = app.code_edits.buffers.get_mut(&key) else {
                    return;
                };
                let success = match result {
                    Ok(()) => {
                        buffer.saved(&submitted);
                        true
                    }
                    Err(error) => {
                        buffer.saving = false;
                        buffer.error = Some(format!("저장하지 못했습니다: {error:#}"));
                        false
                    }
                };
                let editing = buffer.editing;
                editor.update(cx, |editor, cx| editor.set_readonly(!editing, cx));
                let _ = app.code_edits.persist();
                if app.current.as_ref() == Some(&root) && app.opening_path.is_none() {
                    if success {
                        app.status = format!("{} 저장됨", path.display());
                        let open_commit = commit
                            && !app.git_history.active
                            && app.selected().as_ref() == Some(&path);
                        app.refresh(window, cx);
                        if open_commit {
                            app.code_edits.commit_after_refresh = Some((root, app.request));
                        }
                    } else {
                        app.status =
                            "파일 저장 실패 — 초안을 유지했습니다. 디스크 내용을 확인하세요."
                                .into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn finish_code_save_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((root, request)) = self.code_edits.commit_after_refresh.take()
            && self.current.as_ref() == Some(&root)
            && self.request == request
        {
            self.open_git_commit(window, cx);
        }
    }

    pub(super) fn compare_code_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_loading || self.loading || self.git_is_busy() || window.has_active_dialog(cx) {
            return;
        }
        self.capture_code_buffers(cx);
        let (Some(root), Some(path)) = (self.current.clone(), self.selected()) else {
            return;
        };
        let Some(key) = FileKey::new(&root, &path) else {
            return;
        };
        let Some(buffer) = self.code_edits.buffers.get(&key).filter(|b| !b.saving) else {
            return;
        };
        let original = buffer.draft.expected.clone();
        let draft = buffer.draft.text.clone();
        self.code_edits.comparison_request += 1;
        let request = self.code_edits.comparison_request;
        let task =
            cx.background_spawn(
                async move { DraftComparison::read(root, key.path, original, draft) },
            );
        cx.spawn_in(window, async move |view, cx| {
            let comparison = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if app.code_edits.comparison_request != request || window.has_active_dialog(cx) {
                    return;
                }
                app.capture_code_buffers(cx);
                if !app.matches_draft_comparison(&comparison) {
                    return;
                }
                draft_reconciliation::open(cx.entity().downgrade(), comparison, window, cx);
            });
        })
        .detach();
    }

    fn matches_draft_comparison(&self, comparison: &DraftComparison) -> bool {
        if self.current.as_ref() != Some(&comparison.root) || self.opening_path.is_some() {
            return false;
        }
        let Some(key) = self
            .selected()
            .as_deref()
            .and_then(|path| FileKey::new(&comparison.root, path))
        else {
            return false;
        };
        key.path == comparison.path
            && self.code_edits.buffers.get(&key).is_some_and(|buffer| {
                !buffer.saving
                    && buffer.draft.expected == comparison.original
                    && buffer.draft.text == comparison.draft
            })
    }

    pub(super) fn apply_draft_comparison(
        &mut self,
        comparison: &DraftComparison,
        result: String,
        fresh_disk: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.capture_code_buffers(cx);
        if self.git_is_busy()
            || self.loading
            || self.file_loading
            || !self.matches_draft_comparison(comparison)
        {
            return Err("작업 위치나 초안이 변경되었습니다. 최신 초안으로 다시 비교하세요.".into());
        }
        let selected = self.selected().ok_or("선택한 파일이 없습니다.")?;
        let position = self
            .session()
            .and_then(|session| session.documents.get(&selected))
            .map(|editor| {
                (
                    editor.read(cx).cursor_position(),
                    editor.read(cx).scroll_offset(),
                )
            });
        self.code_edits
            .reconcile(comparison, result.clone(), fresh_disk)?;
        // Replace and rebind instead of changing the old editor underneath a
        // queued input callback. The binding identity check above rejects it.
        let key = FileKey::new(&comparison.root, &comparison.path).unwrap();
        self.code_edits.bindings.remove(&key);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language(&comparison.path))
                .line_number(true)
                .soft_wrap(false)
                .default_value(result)
        });
        editor.update(cx, |editor, cx| {
            editor.set_readonly(false, cx);
            if let Some((cursor, scroll)) = position {
                editor.set_cursor_position(cursor, window, cx);
                editor.set_scroll_offset(scroll, cx);
            }
        });
        let session = self
            .sessions
            .get_mut(&comparison.root)
            .ok_or("워크트리가 닫혔습니다.")?;
        session.documents.insert(selected.clone(), editor.clone());
        self.bind_code_editor(&comparison.root, &selected, &editor, cx);
        self.configure_code_tools(&comparison.root, &selected, &editor, window, cx);
        self.file_request += 1;
        self.status = "조정 결과를 초안에 반영했습니다. ⌘S로 파일을 저장하세요.".into();
        cx.notify();
        Ok(())
    }

    pub(super) fn recover_code_draft(
        &mut self,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_history.active
            || self.loading
            || self.file_loading
            || self.git_is_busy()
            || window.has_active_dialog(cx)
        {
            return;
        }
        let Some(path) = self.selected() else { return };
        let Some(key) = self
            .current
            .as_ref()
            .and_then(|root| FileKey::new(root, &path))
        else {
            return;
        };
        self.capture_code_buffers(cx);
        if let Err(error) = self.code_edits.recover(&key, revision) {
            self.status = error.to_string();
            cx.notify();
            return;
        }
        // Reopen through normal disk validation. Drop the old binding and
        // editor so neither a queued Change nor a formatter can target the replacement.
        self.capture_positions(cx);
        if let Some(session) = self.sessions.get_mut(&key.root) {
            session.documents.remove(&path);
            session.saved.code = true;
        }
        self.open_file(path, false, None, window, cx);
    }

    pub(super) fn discard_code_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(root), Some(path)) = (self.current.clone(), self.selected()) else {
            return;
        };
        let Some(key) = FileKey::new(&root, &path) else {
            return;
        };
        self.capture_code_buffers(cx);
        let Some(buffer) = self
            .code_edits
            .buffers
            .get(&key)
            .filter(|b| !b.saving && b.dirty())
        else {
            return;
        };
        let expected = Arc::new(buffer.draft.clone());
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let path = path.clone();
            let expected = expected.clone();
            let confirm = Button::new("confirm-discard-draft")
                .label("초안 버리기")
                .danger()
                .on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        window.close_dialog(cx);
                        if app.current.as_ref() != Some(&expected.key.root)
                            || app.selected().as_ref() != Some(&path)
                        {
                            return;
                        }
                        app.capture_code_buffers(cx);
                        if let Err(error) = app.code_edits.discard(&expected) {
                            app.status = error.to_string();
                            cx.notify();
                            return;
                        }
                        if let Some(session) = app.sessions.get_mut(&expected.key.root) {
                            session.documents.remove(&path);
                        }
                        app.open_file(path.clone(), false, None, window, cx);
                    });
                });
            dialog
                .title("현재 편집 초안을 버릴까요?")
                .child(format!(
                    "{}의 현재 초안을 삭제하고 디스크에서 다시 읽습니다.",
                    key.path.display()
                ))
                .keyboard(false)
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cancel-discard-draft")
                                .label("취소")
                                .ghost()
                                .on_click(|_, w, cx| w.close_dialog(cx)),
                        )
                        .child(confirm),
                )
        });
    }

    pub(super) fn code_edit_controls(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        let Some(buffer) = self
            .current
            .as_ref()
            .and_then(|root| self.code_edits.buffer(root, path))
        else {
            return div().into_any_element();
        };
        let busy = buffer.saving || self.file_loading || self.loading || self.git_is_busy();
        let name = buffer.save_label();
        let minimal = self
            .command_bar_density
            .get("working-reader")
            .copied()
            .unwrap_or_default()
            == reader_toolbar::Density::Minimal;
        Button::new("save-code")
            .icon(IconName::Save)
            .when(!minimal, |b| b.label(name))
            .accessibility_label(name)
            .ghost()
            .small()
            .flex_none()
            .h(ui_px(28.))
            .when(minimal, |b| b.w(ui_px(28.)))
            .loading(buffer.saving)
            .disabled(busy || self.code_edits.unreadable_storage || !buffer.dirty())
            .on_click(cx.listener(move |app, _, w, cx| app.save_code_file(false, w, cx)))
            .shortcut_tooltip("파일 저장", &SaveCodeFile, None)
            .into_any_element()
    }

    pub(super) fn reader_draft_menu(
        &self,
        mut menu: gpui_kit::component::native_menu::NativeMenu,
        identity: &reader_toolbar::Identity,
        path: &Path,
    ) -> gpui_kit::component::native_menu::NativeMenu {
        use reader_toolbar::Command;
        let buffer = self
            .current
            .as_ref()
            .and_then(|r| self.code_edits.buffer(r, path));
        let busy = self.loading
            || self.file_loading
            || self.git_is_busy()
            || buffer.is_some_and(|b| b.saving)
            || self.code_edits.unreadable_storage;
        for (label, command, disabled) in [
            (
                "저장  ⌘S",
                Command::Save,
                busy || buffer.is_none_or(|b| !b.dirty()),
            ),
            (
                "저장 + 커밋…",
                Command::SaveCommit,
                busy || buffer.is_none_or(|b| !b.dirty()),
            ),
            (
                "디스크와 비교…",
                Command::CompareDraft,
                busy || buffer.is_none_or(|b| !b.dirty() && !b.stale),
            ),
            (
                "초안 버리기…",
                Command::DiscardDraft,
                busy || buffer.is_none_or(|b| !b.dirty()),
            ),
        ] {
            menu = identity.item(
                menu,
                keycaps::menu_label(label, &self.prefs, self.current_project.as_deref()),
                command,
                disabled,
                false,
            );
        }
        if let Some(choices) = self
            .current
            .as_ref()
            .and_then(|root| self.code_edits.choices(root, path))
        {
            let mut versions = gpui_kit::component::native_menu::NativeMenu::new();
            for choice in choices {
                versions = identity.item(
                    versions,
                    format!("버전 {} · {}", choice.revision, choice.preview),
                    Command::RecoverDraft(choice.revision),
                    busy,
                    false,
                );
            }
            menu = menu.submenu("보관된 다른 초안 열기", versions);
        }
        menu
    }

    pub(super) fn code_draft_notice(&self, path: &Path, cx: &App) -> Option<AnyElement> {
        let root = self.current.as_ref()?;
        let buffer = self.code_edits.buffer(root, path);
        let choices = self.code_edits.choices(root, path);
        let notice = if let Some(error) = buffer.and_then(|b| b.error.as_ref()) {
            error.as_str()
        } else if buffer.is_some_and(|b| b.retired) {
            "다른 창에서 이 초안 버전을 교체하거나 정리했습니다. 현재 내용은 이 창에만 남아 있습니다."
        } else if buffer.is_some_and(|b| b.stale) {
            "디스크 내용이 변경되었거나 파일이 없습니다. 초안을 유지했습니다. ‘디스크와 비교…’에서 원본·초안·디스크를 확인하고 조정하세요."
        } else if buffer.is_some_and(Buffer::dirty) {
            "● 저장하지 않은 초안 — 탭·프로젝트를 닫거나 앱을 종료해도 보관됩니다. Git에는 아직 반영되지 않습니다."
        } else if choices.is_some() {
            ""
        } else {
            return None;
        };
        let notice = match choices {
            Some(choices) => format!(
                "{notice}  다른 초안 {}개 · 파일 명령 더 보기 → 편집 · 파일 → 보관된 다른 초안 열기",
                choices.len()
            ),
            None => notice.to_owned(),
        };
        Some(
            div()
                .px_3()
                .py_1()
                .text_size(ui_text(11.))
                .text_color(Palette::new(cx).rgb(BLUE))
                .child(notice)
                .into_any_element(),
        )
    }

    pub(super) fn code_drafts_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active = self
            .code_edits
            .buffers
            .values()
            .filter(|b| b.dirty() && !b.retired);
        let dirty = active.clone().count()
            + self
                .code_edits
                .alternatives
                .values()
                .map(Vec::len)
                .sum::<usize>();
        let error = self.code_edits.storage_error.as_deref();
        if dirty == 0 && error.is_none() {
            return None;
        }
        let root = self.current.as_ref();
        let mut drafts: Vec<_> = active
            .map(|b| &b.draft.key)
            .chain(self.code_edits.alternatives.keys())
            .filter(|key| Some(&key.root) == root)
            .map(|key| key.path.clone())
            .collect();
        drafts.sort();
        drafts.dedup();
        Some(
            div()
                .id("code-drafts-banner")
                .px_3()
                .py_1()
                .flex()
                .flex_wrap()
                .flex_none()
                .items_center()
                .gap_2()
                .text_size(ui_text(11.))
                .text_color(Palette::new(cx).rgb(if error.is_some() { RED } else { BLUE }))
                .child(
                    error
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("● 미저장 편집 초안 {dirty}개 보관 중")),
                )
                .children(drafts.into_iter().take(3).enumerate().map(|(i, path)| {
                    Button::new(("reopen-code-draft", i))
                        .label(path.display().to_string())
                        .small()
                        .ghost()
                        .max_w(ui_px(190.))
                        .overflow_hidden()
                        .on_click(cx.listener(move |app, _, w, cx| {
                            app.open_file(path.clone(), true, None, w, cx);
                            app.toggle_code(true, cx);
                        }))
                }))
                .into_any_element(),
        )
    }
}
