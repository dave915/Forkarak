//! File marks are independent of the single preview and partial-line marks.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Default)]
pub(super) struct FileSelection {
    root: Option<PathBuf>,
    generation: u64,
    side: Option<Side>,
    selected: BTreeSet<PathBuf>,
    anchor: Option<PathBuf>,
    explicit: bool,
}

pub(super) fn side_files(snapshot: &IndexSnapshot, side: Side) -> &[ChangedFile] {
    if side == Side::Staged {
        &snapshot.staged
    } else {
        &snapshot.unstaged
    }
}

impl FileSelection {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    pub(super) fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }
    pub(super) fn defers_refresh(&self) -> bool {
        self.explicit && !self.is_empty()
    }
    pub(super) fn contains(&self, side: Side, path: &Path) -> bool {
        self.side == Some(side) && self.selected.contains(path)
    }
    pub(super) fn count(&self, side: Side) -> usize {
        if self.side == Some(side) {
            self.selected.len()
        } else {
            0
        }
    }

    pub(super) fn select(
        &mut self,
        root: &Path,
        generation: u64,
        side: Side,
        files: &[ChangedFile],
        path: &Path,
        toggle: bool,
        range: bool,
    ) {
        if self.root.as_deref() != Some(root)
            || self.generation != generation
            || self.side != Some(side)
        {
            self.clear();
            self.root = Some(root.into());
            self.generation = generation;
            self.side = Some(side);
        }
        self.selected.retain(|p| files.iter().any(|f| &f.path == p));
        let Some(end) = files.iter().position(|f| f.path == path) else {
            return;
        };
        self.explicit = toggle || range;
        if range
            && let Some(start) = self
                .anchor
                .as_ref()
                .and_then(|p| files.iter().position(|f| &f.path == p))
        {
            if !toggle {
                self.selected.clear();
            }
            self.selected.extend(
                files[start.min(end)..=start.max(end)]
                    .iter()
                    .map(|f| f.path.clone()),
            );
        } else {
            if !toggle {
                self.selected.clear();
            }
            if !toggle || !self.selected.remove(path) {
                self.selected.insert(path.into());
            }
            self.anchor = Some(path.into());
        }
    }

    pub(super) fn paths(
        &self,
        snapshot: &IndexSnapshot,
        generation: u64,
        side: Side,
    ) -> Vec<PathBuf> {
        self.paths_for(&snapshot.root, generation, side, side_files(snapshot, side))
    }

    fn paths_for(
        &self,
        root: &Path,
        generation: u64,
        side: Side,
        files: &[ChangedFile],
    ) -> Vec<PathBuf> {
        if self.root.as_deref() != Some(root)
            || self.generation != generation
            || self.side != Some(side)
        {
            return Vec::new();
        }
        // Use only current rows. Renames bind the old and new path together.
        files
            .iter()
            .filter(|f| self.selected.contains(&f.path))
            .flat_map(|f| {
                f.old_path
                    .iter()
                    .cloned()
                    .chain(std::iter::once(f.path.clone()))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChangedFile, FileSelection, Side};
    use std::path::{Path, PathBuf};
    fn files() -> Vec<ChangedFile> {
        ["a", "b", "c", "d"]
            .map(|p| ChangedFile {
                path: p.into(),
                ..Default::default()
            })
            .into()
    }
    #[test]
    fn toggles_ranges_and_clear_keep_preview_independent() {
        let mut selection = FileSelection::default();
        let files = files();
        let root = Path::new("/w/a");
        selection.select(
            root,
            1,
            Side::Unstaged,
            &files,
            Path::new("b"),
            false,
            false,
        );
        selection.select(root, 1, Side::Unstaged, &files, Path::new("d"), false, true);
        assert_eq!(
            selection.paths_for(root, 1, Side::Unstaged, &files),
            vec![PathBuf::from("b"), "c".into(), "d".into()]
        );
        selection.select(root, 1, Side::Unstaged, &files, Path::new("c"), true, false);
        assert_eq!(selection.count(Side::Unstaged), 2);
        selection.clear();
        assert!(selection.is_empty());
    }
    #[test]
    fn ordinary_preview_keeps_live_refresh_but_explicit_batch_marks_defer_it() {
        let mut selection = FileSelection::default();
        let files = files();
        let root = Path::new("/w");
        selection.select(
            root,
            1,
            Side::Unstaged,
            &files,
            Path::new("a"),
            false,
            false,
        );
        assert_eq!(selection.count(Side::Unstaged), 1);
        assert!(!selection.defers_refresh());
        selection.select(root, 1, Side::Unstaged, &files, Path::new("b"), true, false);
        assert!(selection.defers_refresh());
        selection.select(
            root,
            1,
            Side::Unstaged,
            &files,
            Path::new("c"),
            false,
            false,
        );
        assert!(!selection.defers_refresh());
        selection.select(root, 1, Side::Unstaged, &files, Path::new("d"), false, true);
        assert!(selection.defers_refresh());
        selection.clear();
        assert!(!selection.defers_refresh());
    }
    #[test]
    fn root_side_and_snapshot_changes_cannot_reuse_file_marks() {
        let mut selection = FileSelection::default();
        let files = files();
        let root = Path::new("/w/a");
        selection.select(
            root,
            1,
            Side::Unstaged,
            &files,
            Path::new("b"),
            false,
            false,
        );
        for (root, generation, side) in [
            (Path::new("/w/b"), 1, Side::Unstaged),
            (root, 2, Side::Unstaged),
            (root, 1, Side::Staged),
        ] {
            assert!(
                selection
                    .paths_for(root, generation, side, &files)
                    .is_empty()
            );
        }
        selection.select(root, 2, Side::Staged, &files, Path::new("d"), false, true);
        assert_eq!(selection.count(Side::Staged), 1);
    }
    #[test]
    fn selected_renames_include_both_paths_and_never_unselected_rows() {
        let mut files = files();
        files[1].old_path = Some("old-b".into());
        let mut selection = FileSelection::default();
        let root = Path::new("/w");
        selection.select(root, 1, Side::Staged, &files, Path::new("b"), false, false);
        assert_eq!(
            selection.paths_for(root, 1, Side::Staged, &files),
            vec![PathBuf::from("b"), "old-b".into()]
        );
        files.remove(1);
        assert!(
            selection
                .paths_for(root, 1, Side::Staged, &files)
                .is_empty()
        );
    }
}
