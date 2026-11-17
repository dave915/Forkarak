//! Code inspects working files against the worktree's explicitly applied base.
use super::{
    FileRevision, Path, PathBuf, Pokarak, Position, ReadingPoint, Session, Snapshot,
};

pub(super) struct PendingFile {
    pub root: PathBuf,
    pub snapshot_request: u64,
    pub file_request: u64,
    pub path: PathBuf,
    pub position: Option<Position>,
    pub history: bool,
    pub history_restore: Option<ReadingPoint>,
    pub target_revision: Option<FileRevision>,
}

impl PendingFile {
    fn accepts(&self, root: Option<&Path>, snapshot: u64, file: u64, graph: bool) -> bool {
        root == Some(self.root.as_path())
            && snapshot == self.snapshot_request
            && file == self.file_request
            && !graph
    }
}

impl Session {
    pub(super) fn snapshot_selection(&self, snapshot: &Snapshot) -> Option<PathBuf> {
        // A disappearing file should report its own error, never silently open
        // README (or the first changed file) instead of the exact reading path.
        self.saved
            .selected
            .clone()
            .or_else(|| snapshot.changes.first().map(|file| file.path.clone()))
    }

    /// Keep the chosen base, documents, drafts, tabs and reading coordinates.
    /// Only data from a different comparison needs replacing.
    pub(super) fn prepare_code_snapshot(&mut self) -> bool {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.compare != self.saved.comparison)
        {
            self.snapshot = None;
            self.diffs.clear();
        }
        self.snapshot.is_none()
    }
}

impl Pokarak {
    pub(super) fn take_pending_cli_file(&mut self) -> Option<PendingFile> {
        let pending = self.pending_cli_file.take()?;
        pending
            .accepts(
                self.current.as_deref(),
                self.request,
                self.file_request,
                self.git_history.active,
            )
            .then_some(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::PendingFile;
    use crate::prefs::{SavedPosition, SavedView};
    use crate::ui::{ReadingPoint, Session};
    use gpui_kit::component::input::Position;
    use pokarak_core::{CompareMode, Snapshot};
    use std::{collections::HashMap, fs, path::Path, process::Command};

    fn legacy_view() -> SavedView {
        let path = std::path::PathBuf::from("src/선택.kt");
        SavedView {
            selected: Some(path.clone()),
            tabs: vec!["README.md".into(), path.clone()],
            comparison: CompareMode::Branch("main".into()),
            code: true,
            all_files: true,
            line: 47,
            column: 9,
            scroll_y: -540.,
            positions: HashMap::from([(
                path,
                SavedPosition {
                    line: 47,
                    column: 9,
                    scroll_y: -540.,
                    ..Default::default()
                },
            )]),
            collapsed_graph_sections: ["tags".into()].into(),
            ..Default::default()
        }
    }

    #[test]
    fn saved_branch_review_restores_the_applied_base_and_exact_reading_state() {
        let saved = legacy_view();
        let expected = serde_json::to_value(&saved).unwrap();
        let restored: SavedView =
            serde_json::from_value(serde_json::to_value(saved).unwrap()).unwrap();
        let session = Session::new(restored);
        assert_eq!(serde_json::to_value(&session.saved).unwrap(), expected);
        assert!(session.restoring);
    }

    #[test]
    fn changing_the_baseline_invalidates_only_that_worktrees_snapshot() {
        let first = Path::new("/fixture/main");
        let second = Path::new("/fixture/linked");
        let mut sessions = HashMap::from([
            (first, Session::new(legacy_view())),
            (second, Session::new(legacy_view())),
        ]);
        for session in sessions.values_mut() {
            session.saved.comparison = CompareMode::Branch("main".into());
            session.snapshot = Some(Snapshot {
                compare: CompareMode::Branch("main".into()),
                ..Default::default()
            });
        }
        let before = serde_json::to_value(&sessions[second].saved).unwrap();
        assert!(!sessions.get_mut(first).unwrap().prepare_code_snapshot());
        sessions.get_mut(first).unwrap().saved.comparison = CompareMode::Uncommitted;
        assert!(sessions.get_mut(first).unwrap().prepare_code_snapshot());
        assert_eq!(sessions[first].saved.line, 47);
        assert_eq!(sessions[first].saved.selected, Some("src/선택.kt".into()));
        assert!(sessions[first].snapshot.is_none());
        assert_eq!(
            serde_json::to_value(&sessions[second].saved).unwrap(),
            before
        );
        assert!(sessions[second].snapshot.is_some());
        sessions.get_mut(first).unwrap().snapshot = Some(Snapshot::default());
        assert!(!sessions.get_mut(first).unwrap().prepare_code_snapshot());
    }

    fn pending() -> PendingFile {
        PendingFile {
            root: "/fixture/main".into(),
            snapshot_request: 5,
            file_request: 9,
            path: "src/선택.kt".into(),
            position: Some(Position::new(47, 9)),
            history: true,
            history_restore: Some(ReadingPoint {
                path: "src/선택.kt".into(),
                code: true,
                row: 8,
                line: 47,
                column: 9,
                scroll_x: -15.,
                scroll_y: -540.,
            }),
            target_revision: None,
        }
    }

    #[test]
    fn explicit_cli_and_history_location_survive_only_the_matching_snapshot() {
        let pending = pending();
        assert!(pending.accepts(Some(Path::new("/fixture/main")), 5, 9, false));
        for (root, snapshot, file, graph) in [
            (Some(Path::new("/fixture/linked")), 5, 9, false),
            (None, 5, 9, false),
            (Some(Path::new("/fixture/main")), 6, 9, false),
            (Some(Path::new("/fixture/main")), 5, 10, false),
            (Some(Path::new("/fixture/main")), 5, 9, true),
        ] {
            assert!(!pending.accepts(root, snapshot, file, graph));
        }
        assert_eq!(pending.path, Path::new("src/선택.kt"));
        assert_eq!(pending.position, Some(Position::new(47, 9)));
        let restore = pending.history_restore.unwrap();
        assert_eq!((restore.line, restore.column, restore.row), (47, 9, 8));
        assert_eq!((restore.scroll_x, restore.scroll_y), (-15., -540.));
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn branch_review_survives_refresh_and_can_return_to_head_without_moving_the_reader() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        git(root, &["init", "-b", "main"]);
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("README.md"), "readme\n").unwrap();
        fs::write(root.join("src/선택.kt"), "base\n").unwrap();
        git(root, &["add", "--", "README.md", "src/선택.kt"]);
        git(root, &["commit", "-m", "base"]);
        git(root, &["switch", "-c", "feature"]);
        fs::write(root.join("src/선택.kt"), "committed\n").unwrap();
        git(root, &["commit", "-am", "feature"]);
        fs::write(root.join("README.md"), "working\n").unwrap();
        let branch = pokarak_core::snapshot(root, &CompareMode::Branch("main".into())).unwrap();
        assert!(
            branch
                .changes
                .iter()
                .any(|file| file.path == Path::new("src/선택.kt"))
        );
        let mut session = Session::new(legacy_view());
        session.snapshot = Some(branch);
        assert!(!session.prepare_code_snapshot());
        let refreshed = pokarak_core::snapshot(root, &session.saved.comparison).unwrap();
        assert!(
            refreshed
                .changes
                .iter()
                .any(|file| file.path == Path::new("src/선택.kt"))
        );
        // A failed candidate is never installed, retaining the successfully
        // applied base, snapshot and exact reading position.
        let saved_before = serde_json::to_value(&session.saved).unwrap();
        let fingerprint_before = session.snapshot.as_ref().unwrap().fingerprint.clone();
        assert!(
            pokarak_core::snapshot(root, &CompareMode::Branch("missing-branch".into())).is_err()
        );
        assert_eq!(serde_json::to_value(&session.saved).unwrap(), saved_before);
        assert_eq!(
            session.snapshot.as_ref().unwrap().fingerprint,
            fingerprint_before
        );
        session.saved.comparison = CompareMode::Uncommitted;
        assert!(session.prepare_code_snapshot());
        let head = pokarak_core::snapshot(root, &session.saved.comparison).unwrap();
        assert_eq!(head.compare, CompareMode::Uncommitted);
        assert!(
            !head
                .changes
                .iter()
                .any(|file| file.path == Path::new("src/선택.kt"))
        );
        assert_eq!(
            session.snapshot_selection(&head),
            Some("src/선택.kt".into())
        );
        // Even a missing selected path must never turn into the changed README.
        session.saved.selected = Some("missing.kt".into());
        assert_eq!(session.snapshot_selection(&head), Some("missing.kt".into()));
        assert_eq!(session.saved.line, 47);
        assert_eq!(
            fs::read_to_string(root.join("src/선택.kt")).unwrap(),
            "committed\n"
        );
    }
}
