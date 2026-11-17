//! Explicit file staging and index-only commits, using the shared cancellable runner.
mod partial;

use super::*;
use crate::index::{self, IndexSnapshot};
use std::{os::unix::ffi::OsStrExt, path::Component};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageScope {
    Files(Vec<PathBuf>),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitMode {
    New,
    /// Only construct after the user explicitly confirms replacing this commit.
    Amend {
        confirmed_head: String,
    },
}

impl GitOperations {
    fn require_commit_snapshot(&self, snapshot: &IndexSnapshot) -> Result<()> {
        ensure!(
            self.root == snapshot.root && index::guard(&self.root)? == snapshot.guard,
            "Worktree, HEAD or index changed; refresh the commit view before retrying"
        );
        Ok(())
    }

    /// Stage current file bytes (including configured clean filters), only on
    /// this explicit action. The staged preview itself never runs filters.
    pub fn stage(&self, snapshot: &IndexSnapshot, scope: &StageScope) -> Result<OperationOutput> {
        self.change_index(snapshot, scope, true)
    }

    /// Restore only index entries to HEAD; unborn repositories remove entries
    /// from the index. Working bytes and unrelated staged paths are preserved.
    pub fn unstage(&self, snapshot: &IndexSnapshot, scope: &StageScope) -> Result<OperationOutput> {
        self.change_index(snapshot, scope, false)
    }

    fn change_index(
        &self,
        snapshot: &IndexSnapshot,
        scope: &StageScope,
        stage: bool,
    ) -> Result<OperationOutput> {
        let paths = match scope {
            StageScope::All => vec![PathBuf::from(".")],
            StageScope::Files(paths) => {
                ensure!(!paths.is_empty(), "Select at least one file");
                let files = if stage {
                    &snapshot.unstaged
                } else {
                    &snapshot.staged
                };
                let indexed = crate::git::index(&self.root)?;
                let baseline = crate::git::tree(&self.root, snapshot.guard.head.as_deref())?;
                for path in paths {
                    validate_file(path)?;
                    // Literal Git pathspecs can still match descendants. A file
                    // action must never silently stage/unstage a directory tree.
                    let descendants = indexed
                        .keys()
                        .chain(baseline.keys())
                        .any(|candidate| candidate != path && candidate.starts_with(path));
                    let directory = self
                        .root
                        .join(path)
                        .symlink_metadata()
                        .is_ok_and(|metadata| metadata.is_dir());
                    let gitlink = indexed
                        .get(path)
                        .and_then(|stages| stages.get(&0))
                        .is_some_and(|entry| entry.mode == "160000");
                    ensure!(
                        !descendants && (!directory || gitlink),
                        "This path now covers a directory. Use the explicit All action or external Git to choose individual entries"
                    );
                    ensure!(
                        files
                            .iter()
                            .any(|file| file.path == *path || file.old_path.as_ref() == Some(path)),
                        "File is absent from this view; refresh before staging"
                    );
                }
                let mut selected = paths.clone();
                if stage {
                    // A previously staged rename can still carry its HEAD old
                    // path in the working-change row. Git add rejects that
                    // now-absent source, so include it only while it exists.
                    selected.retain(|path| {
                        indexed.contains_key(path)
                            || self.root.join(path).symlink_metadata().is_ok()
                            || !files
                                .iter()
                                .any(|file| file.old_path.as_ref() == Some(path))
                    });
                }
                ensure!(
                    !selected.is_empty(),
                    "Files no longer exist; refresh before staging"
                );
                selected
            }
        };
        let mut command = if stage {
            self.command(&["add", "--all", "--"])
        } else if snapshot.previous.is_some() {
            self.command(&["reset", "--quiet", "HEAD", "--"])
        } else {
            self.command(&["rm", "--cached", "--force", "-r", "--ignore-unmatch", "--"])
        };
        command.args(paths);
        self.require_commit_snapshot(snapshot)?;
        self.commit_output(self.run_command(command)?)
    }

    /// No -a, path arguments, reset-author, no-verify, or signing overrides:
    /// commits exactly the current index using local identity and normal hooks.
    pub fn commit_index(
        &self,
        snapshot: &IndexSnapshot,
        message: &str,
        mode: &CommitMode,
    ) -> Result<OperationOutput> {
        ensure!(
            !message.trim().is_empty() && !message.contains('\0') && message.len() <= 64 * 1024,
            "Enter a nonempty commit message of at most 64 KiB without NUL bytes"
        );
        ensure!(
            !snapshot.has_conflicts,
            "Resolve unmerged index entries before committing"
        );
        ensure!(
            self.in_progress()?.is_none(),
            "A merge/rebase/cherry-pick/revert is in progress; use its Continue action"
        );
        let mut command = self.command(&["commit", "--message", message]);
        match mode {
            CommitMode::New => ensure!(
                !snapshot.staged.is_empty(),
                "Stage changes before committing"
            ),
            CommitMode::Amend { confirmed_head } => {
                ensure!(
                    snapshot
                        .previous
                        .as_ref()
                        .is_some_and(|previous| &previous.oid == confirmed_head),
                    "Explicit confirmation of the previous commit is required to amend"
                );
                command.arg("--amend");
            }
        }
        command.arg("--");
        command.env("GIT_EDITOR", "/usr/bin/false");
        self.require_commit_snapshot(snapshot)?;
        self.commit_output(self.run_command(command)?)
    }

    fn commit_output(&self, result: Captured) -> Result<OperationOutput> {
        ensure!(
            result.code == Some(0),
            "Git stage/commit failed in {}. Refresh before retrying:\n{}\n{}",
            self.root.display(),
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout)
        );
        Ok(OperationOutput {
            root: self.root.clone(),
            stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            truncated: result.truncated,
        })
    }
}

fn validate_file(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty() && path.components().all(|part| matches!(part,
        Component::Normal(name) if !name.as_bytes().contains(&0) && !name.as_bytes().eq_ignore_ascii_case(b".git"))),
        "Select a relative file path without traversal or .git components");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileStatus;
    use std::{
        ffi::OsString,
        fs,
        os::unix::{
            ffi::OsStringExt,
            fs::{PermissionsExt, symlink},
        },
    };
    use tempfile::TempDir;

    struct Repo {
        _temp: TempDir,
        root: PathBuf,
        global: PathBuf,
    }
    impl Repo {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("repo 한 with spaces");
            let global = temp.path().join("empty-global");
            fs::create_dir(&root).unwrap();
            fs::write(&global, "").unwrap();
            let this = Self {
                _temp: temp,
                root,
                global,
            };
            this.git(&["init", "-b", "main"]);
            for (key, value) in [
                ("user.name", "Local Fixture"),
                ("user.email", "local@example.test"),
                ("commit.gpgSign", "false"),
                ("core.autocrlf", "false"),
                ("core.hooksPath", ".git/hooks"),
            ] {
                this.git(&["config", "--local", key, value]);
            }
            this
        }
        fn op(&self) -> GitOperations {
            let mut operations = GitOperations::open(&self.root).unwrap();
            operations.global_config = Some(self.global.clone());
            operations
        }
        fn git(&self, args: &[&str]) -> String {
            let mut command = Command::new("git");
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(key);
                }
            }
            let result = command
                .env("GIT_CONFIG_GLOBAL", &self.global)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["--literal-pathspecs", "-C"])
                .arg(&self.root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout)
                .unwrap()
                .trim_end_matches('\n')
                .into()
        }
        fn write(&self, path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
            fs::write(self.root.join(path), text).unwrap();
        }
        fn snap(&self) -> IndexSnapshot {
            index::snapshot(&self.root).unwrap()
        }
        fn stage_all(&self) {
            self.op().stage(&self.snap(), &StageScope::All).unwrap();
        }
        fn seed(&self) {
            self.write("file", "base\n");
            self.stage_all();
            self.op()
                .commit_index(&self.snap(), "initial", &CommitMode::New)
                .unwrap();
        }
        fn hook(&self, name: &str, text: &str) {
            let path = self.root.join(".git/hooks").join(name);
            fs::write(&path, text).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fn files(paths: &[&str]) -> StageScope {
        StageScope::Files(paths.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn preview_and_commit_use_index_not_working_bytes() {
        let repo = Repo::new();
        repo.seed();
        repo.write("file", "staged\n");
        repo.stage_all();
        repo.write("file", "working only\n");
        repo.write("untracked", "keep\n");
        let index_before = fs::read(repo.root.join(".git/index")).unwrap();
        let snapshot = repo.snap();
        let document = index::diff(&snapshot, Path::new("file")).unwrap();
        assert_eq!(document.old_text, "base\n");
        assert_eq!(document.new_text, "staged\n");
        assert!(!document.patch.contains("working only"));
        assert_eq!(
            fs::read(repo.root.join(".git/index")).unwrap(),
            index_before
        );
        repo.op()
            .commit_index(&snapshot, "index only\n\nbody", &CommitMode::New)
            .unwrap();
        assert_eq!(repo.git(&["show", "HEAD:file"]), "staged");
        assert_eq!(
            repo.git(&["show", "--no-patch", "--format=%an <%ae>", "HEAD", "--"]),
            "Local Fixture <local@example.test>"
        );
        assert_eq!(
            fs::read_to_string(repo.root.join("file")).unwrap(),
            "working only\n"
        );
        assert_eq!(
            fs::read_to_string(repo.root.join("untracked")).unwrap(),
            "keep\n"
        );
        assert!(repo.snap().staged.is_empty());
    }

    #[test]
    fn unborn_file_and_all_unstage_preserve_disk() {
        let repo = Repo::new();
        repo.write("one", "first\n");
        repo.write("two", "second\n");
        repo.op().stage(&repo.snap(), &files(&["one"])).unwrap();
        assert_eq!(repo.snap().staged.len(), 1);
        repo.write("one", "diverged\n");
        repo.op().unstage(&repo.snap(), &files(&["one"])).unwrap();
        assert!(repo.snap().staged.is_empty());
        repo.stage_all();
        assert_eq!(repo.snap().staged.len(), 2);
        repo.op().unstage(&repo.snap(), &StageScope::All).unwrap();
        assert!(repo.snap().staged.is_empty());
        assert!(repo.snap().previous.is_none());
        assert_eq!(
            fs::read_to_string(repo.root.join("one")).unwrap(),
            "diverged\n"
        );
        repo.stage_all();
        repo.op()
            .commit_index(&repo.snap(), "first commit", &CommitMode::New)
            .unwrap();
        assert!(repo.snap().previous.is_some());
    }

    #[test]
    fn rename_deletion_and_individual_unstage_preserve_other_staged_content() {
        let repo = Repo::new();
        repo.seed();
        repo.write("deleted", "delete me\n");
        repo.stage_all();
        repo.op()
            .commit_index(&repo.snap(), "second", &CommitMode::New)
            .unwrap();
        fs::rename(repo.root.join("file"), repo.root.join("renamed")).unwrap();
        fs::remove_file(repo.root.join("deleted")).unwrap();
        repo.op()
            .stage(&repo.snap(), &files(&["file", "renamed"]))
            .unwrap();
        let snapshot = repo.snap();
        let renamed = snapshot
            .staged
            .iter()
            .find(|file| file.path == Path::new("renamed"))
            .unwrap();
        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some(Path::new("file")));
        assert!(
            index::diff(&snapshot, Path::new("renamed"))
                .unwrap()
                .patch
                .contains("rename from")
        );
        repo.op().stage(&snapshot, &files(&["deleted"])).unwrap();
        assert_eq!(repo.snap().staged.len(), 2);
        repo.op()
            .unstage(&repo.snap(), &files(&["file", "renamed"]))
            .unwrap();
        assert_eq!(repo.snap().staged.len(), 1);
        assert_eq!(repo.snap().staged[0].status, FileStatus::Deleted);
        assert!(repo.root.join("renamed").exists());
        assert!(!repo.root.join("file").exists());
        let deleted = index::diff(&repo.snap(), Path::new("deleted")).unwrap();
        assert!(deleted.new_text.is_empty());
        assert_eq!(deleted.old_text, "delete me\n");
    }

    #[test]
    fn stale_root_branch_head_and_index_are_rejected() {
        let repo = Repo::new();
        repo.seed();
        repo.write("file", "staged\n");
        repo.stage_all();
        let snapshot = repo.snap();
        let other = Repo::new();
        other.seed();
        assert!(
            other
                .op()
                .commit_index(&snapshot, "wrong root", &CommitMode::New)
                .is_err()
        );
        repo.git(&["branch", "equal-head"]);
        repo.git(&["symbolic-ref", "HEAD", "refs/heads/equal-head"]);
        assert!(
            repo.op()
                .commit_index(&snapshot, "wrong branch", &CommitMode::New)
                .is_err()
        );
        repo.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        let snapshot = repo.snap();
        repo.write("file", "new index\n");
        repo.stage_all();
        assert!(
            repo.op()
                .commit_index(&snapshot, "old index", &CommitMode::New)
                .is_err()
        );
        assert!(repo.op().unstage(&snapshot, &StageScope::All).is_err());
        let snapshot = repo.snap();
        repo.op()
            .commit_index(&snapshot, "advance", &CommitMode::New)
            .unwrap();
        assert!(
            repo.op()
                .commit_index(
                    &snapshot,
                    "old HEAD",
                    &CommitMode::Amend {
                        confirmed_head: snapshot.previous.as_ref().unwrap().oid.clone()
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn amend_requires_exact_confirmation_and_does_not_stage() {
        let repo = Repo::new();
        assert!(
            repo.op()
                .commit_index(
                    &repo.snap(),
                    "no parent",
                    &CommitMode::Amend {
                        confirmed_head: String::new()
                    }
                )
                .is_err()
        );
        repo.seed();
        let snapshot = repo.snap();
        repo.write("file", "do not stage\n");
        assert!(
            repo.op()
                .commit_index(
                    &snapshot,
                    "message only",
                    &CommitMode::Amend {
                        confirmed_head: "wrong".into()
                    }
                )
                .is_err()
        );
        assert!(
            repo.op()
                .commit_index(&snapshot, "", &CommitMode::New)
                .is_err()
        );
        assert!(
            repo.op()
                .commit_index(&snapshot, "empty index", &CommitMode::New)
                .is_err()
        );
        repo.op()
            .commit_index(
                &snapshot,
                "message only",
                &CommitMode::Amend {
                    confirmed_head: snapshot.previous.clone().unwrap().oid,
                },
            )
            .unwrap();
        assert_eq!(repo.git(&["rev-list", "--count", "HEAD", "--"]), "1");
        assert_eq!(repo.git(&["show", "HEAD:file"]), "base");
        assert_eq!(
            repo.git(&["log", "-1", "--format=%B", "--"]),
            "message only"
        );
        assert_eq!(
            fs::read_to_string(repo.root.join("file")).unwrap(),
            "do not stage\n"
        );
    }

    #[test]
    fn hooks_run_and_failures_keep_head_index_and_draft_retryable() {
        let repo = Repo::new();
        repo.seed();
        repo.write("file", "staged\n");
        repo.stage_all();
        let snapshot = repo.snap();
        let before = repo.git(&["rev-parse", "HEAD"]);
        repo.hook(
            "pre-commit",
            "#!/bin/sh\nprintf 'authored hook rejects commit' >&2\nexit 1\n",
        );
        let error = repo
            .op()
            .commit_index(&snapshot, "rejected", &CommitMode::New)
            .unwrap_err();
        assert!(error.to_string().contains("authored hook rejects"));
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
        assert_eq!(
            index::diff(&repo.snap(), Path::new("file"))
                .unwrap()
                .new_text,
            "staged\n"
        );
        repo.hook("pre-commit", "#!/bin/sh\nprintf 'ran' > hook-ran\n");
        repo.op()
            .commit_index(&repo.snap(), "accepted", &CommitMode::New)
            .unwrap();
        assert_eq!(
            fs::read_to_string(repo.root.join("hook-ran")).unwrap(),
            "ran"
        );
        assert!(
            !repo
                .git(&["ls-files", "--", "hook-ran"])
                .contains("hook-ran")
        );
    }

    #[test]
    fn literal_paths_and_symlinks_remain_safe() {
        let repo = Repo::new();
        repo.write(":(glob)*", "literal\n");
        repo.write("other", "other\n");
        repo.op()
            .stage(&repo.snap(), &files(&[":(glob)*"]))
            .unwrap();
        assert_eq!(repo.snap().staged.len(), 1);
        assert_eq!(
            index::diff(&repo.snap(), Path::new(":(glob)*"))
                .unwrap()
                .new_text,
            "literal\n"
        );
        symlink("other", repo.root.join("link")).unwrap();
        repo.op().stage(&repo.snap(), &files(&["link"])).unwrap();
        assert_eq!(
            index::diff(&repo.snap(), Path::new("link"))
                .unwrap()
                .new_text,
            "other"
        );
        for path in [".", "..", "../escape", ".git/config", "/tmp/escape"] {
            assert!(repo.op().stage(&repo.snap(), &files(&[path])).is_err());
        }
        // APFS rejects non-UTF8 filesystem names. Still verify their raw argv
        // is never rewritten to a lossy sibling; on supporting filesystems run
        // the real file lifecycle too.
        let odd = PathBuf::from(OsString::from_vec(b"odd-\xff[*]\n".to_vec()));
        let mut command = repo.op().command(&["add", "--all", "--"]);
        command.arg(&odd);
        assert_eq!(command.get_args().last(), Some(odd.as_os_str()));
        if cfg!(target_os = "macos") {
            assert!(
                repo.op()
                    .stage(&repo.snap(), &StageScope::Files(vec![odd]))
                    .is_err()
            );
        } else {
            repo.write(&odd, "odd\n");
            repo.op()
                .stage(&repo.snap(), &StageScope::Files(vec![odd.clone()]))
                .unwrap();
            assert_eq!(index::diff(&repo.snap(), &odd).unwrap().new_text, "odd\n");
            repo.op()
                .unstage(&repo.snap(), &StageScope::Files(vec![odd.clone()]))
                .unwrap();
            assert!(repo.root.join(odd).exists());
        }
    }

    #[test]
    fn already_staged_rename_can_stage_new_destination_edits() {
        let repo = Repo::new();
        repo.seed();
        fs::rename(repo.root.join("file"), repo.root.join("renamed")).unwrap();
        repo.stage_all();
        repo.write("renamed", "base\nmore\n");
        repo.op()
            .stage(&repo.snap(), &files(&["file", "renamed"]))
            .unwrap();
        assert_eq!(
            index::diff(&repo.snap(), Path::new("renamed"))
                .unwrap()
                .new_text,
            "base\nmore\n"
        );
    }

    #[test]
    fn file_actions_refuse_recursive_directory_pathspecs() {
        let repo = Repo::new();
        repo.seed();
        fs::remove_file(repo.root.join("file")).unwrap();
        fs::create_dir(repo.root.join("file")).unwrap();
        repo.write("file/child", "new child\n");
        let before = fs::read(repo.root.join(".git/index")).unwrap();
        assert!(repo.op().stage(&repo.snap(), &files(&["file"])).is_err());
        assert_eq!(fs::read(repo.root.join(".git/index")).unwrap(), before);
        repo.stage_all();
        assert!(repo.op().unstage(&repo.snap(), &files(&["file"])).is_err());
        assert!(
            repo.snap()
                .staged
                .iter()
                .any(|file| file.path == Path::new("file/child"))
        );
    }

    #[test]
    fn intent_to_add_is_not_a_staged_commit_and_preview_skips_filters() {
        let repo = Repo::new();
        repo.write("intent", "not staged\n");
        repo.git(&["add", "--intent-to-add", "--", "intent"]);
        assert!(repo.snap().staged.is_empty());
        repo.write(".gitattributes", "file filter=authored\n");
        repo.write("file", "disk\n");
        repo.git(&[
            "config",
            "filter.authored.clean",
            "printf 'index filter content\\n'",
        ]);
        repo.stage_all();
        repo.git(&[
            "config",
            "filter.authored.clean",
            "touch filter-must-not-run; cat",
        ]);
        let snapshot = repo.snap();
        assert_eq!(
            index::diff(&snapshot, Path::new("file")).unwrap().new_text,
            "index filter content\n"
        );
        assert!(!repo.root.join("filter-must-not-run").exists());
    }

    #[test]
    fn cancellation_and_in_progress_operation_prevent_commit() {
        let repo = Repo::new();
        repo.seed();
        repo.write("file", "staged\n");
        repo.stage_all();
        let token = OperationCancellation::default();
        token.cancel();
        assert!(
            repo.op()
                .with_cancellation(token)
                .commit_index(&repo.snap(), "cancelled", &CommitMode::New)
                .is_err()
        );
        let before = repo.git(&["rev-parse", "HEAD"]);
        fs::write(repo.root.join(".git/MERGE_HEAD"), format!("{before}\n")).unwrap();
        assert!(
            repo.op()
                .commit_index(&repo.snap(), "in progress", &CommitMode::New)
                .unwrap_err()
                .to_string()
                .contains("in progress")
        );
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    }

    mod partial_tests;
}
