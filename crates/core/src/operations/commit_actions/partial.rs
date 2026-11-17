//! Publish one selected change set under Git's regular per-worktree index lock.
use super::*;
use crate::index::partial::PartialPreview;
use std::{fs, io::ErrorKind};

struct IndexLock(PathBuf);

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl GitOperations {
    /// Stage/unstage the exact changed rows that were displayed, atomically.
    /// The original index stays intact on cancellation, stale input or failure.
    pub fn apply_selected_lines(
        &self,
        preview: &PartialPreview,
        selected: &[usize],
    ) -> Result<OperationOutput> {
        ensure!(
            self.root == preview.root(),
            "작업 대상 워크트리가 바뀌었습니다."
        );
        ensure!(
            !self.cancellation.is_cancelled(),
            "부분 스테이징이 취소되었습니다."
        );
        let target = preview.selected_content(selected)?;
        let path = index::git_path(&self.root, "index")?;
        let lock_path = PathBuf::from({
            let mut name = path.as_os_str().to_owned();
            name.push(".lock");
            name
        });
        fs::File::create_new(&lock_path)
            .context("Git 인덱스가 잠겨 있습니다. 다른 작업이 끝난 후 다시 시도하세요.")?;
        let _lock = IndexLock(lock_path);
        preview.validate(&self.root)?;
        let scratch = tempfile::Builder::new()
            .prefix("pokarak-stage-")
            .tempdir_in(path.parent().context("Missing Git directory")?)?;
        let temporary_index = scratch.path().join("index");
        let permissions = match fs::metadata(&path) {
            Ok(metadata) => {
                fs::copy(&path, &temporary_index)?;
                Some(metadata.permissions())
            }
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        let mut update = self.command(&["update-index"]);
        update.env("GIT_INDEX_FILE", &temporary_index);
        match target {
            Some((mode, content)) => {
                let blob_file = scratch.path().join("content");
                fs::write(&blob_file, content.as_bytes())?;
                let mut hash = self.command(&["hash-object", "-w", "--no-filters", "--"]);
                hash.arg(blob_file);
                let hashed = self.run_command(hash)?;
                ensure!(
                    hashed.code == Some(0) && !hashed.truncated,
                    "Git blob write failed: {}",
                    String::from_utf8_lossy(&hashed.stderr)
                );
                let oid = crate::git::line(&hashed.stdout)?;
                // --cacheinfo consumes mode, object and the literal pathname.
                update
                    .args(["--add", "--cacheinfo", mode, oid])
                    .arg(preview.path())
                    .arg("--");
            }
            None => {
                update.args(["--force-remove", "--"]).arg(preview.path());
            }
        }
        let output = self.commit_output(self.run_command(update)?)?;
        if let Some(perms) = permissions {
            fs::set_permissions(&temporary_index, perms)?;
        }
        preview.validate(&self.root)?;
        fs::File::open(&temporary_index)?.sync_all()?;
        // Coordinate the single publish point with the operation runner's quit
        // cancellation. Git readers see either complete index; other writers
        // respect index.lock throughout the compare + replace transaction.
        let state = self
            .cancellation
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ensure!(!state.cancelled, "부분 스테이징이 취소되었습니다.");
        fs::rename(temporary_index, path)?;
        Ok(output)
    }
}
