//! Compare actual source bytes, independent of HEAD/index/ref metadata. A
//! bounded or failed read is unknown and must invalidate source-dependent state.
use super::*;
use sha2::{Digest, Sha256};
use std::io::Read;

pub(super) fn stamp(root: &Path) -> anyhow::Result<[u8; 32]> {
    let snapshot = pokarak_core::snapshot_current(root, &CompareMode::Uncommitted)?;
    let mut paths = snapshot.files;
    paths.sort();
    anyhow::ensure!(
        paths.len() <= 100_000,
        "Source inventory exceeds verification limit"
    );
    let mut digest = Sha256::new();
    let mut remaining = 256 * 1024 * 1024u64;
    for path in paths {
        let absolute = root.join(&path);
        let meta = std::fs::symlink_metadata(&absolute)?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bytes = path.as_os_str().as_bytes();
            digest.update(bytes.len().to_le_bytes());
            digest.update(bytes);
        }
        if meta.is_symlink() {
            digest.update(b"symlink");
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                digest.update(std::fs::read_link(&absolute)?.as_os_str().as_bytes());
            }
        } else if meta.is_file() {
            anyhow::ensure!(
                meta.len() <= remaining,
                "Source bytes exceed verification limit"
            );
            remaining -= meta.len();
            digest.update(b"file");
            digest.update(meta.len().to_le_bytes());
            let mut file = std::fs::File::open(&absolute)?.take(meta.len() + 1);
            let mut buffer = [0; 16 * 1024];
            let mut read = 0;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                read += count as u64;
                digest.update(&buffer[..count]);
            }
            anyhow::ensure!(read == meta.len(), "Source changed while verifying");
        } else {
            digest.update(b"directory");
        }
    }
    Ok(digest.finalize().into())
}

pub(super) fn unchanged(
    before: anyhow::Result<Option<[u8; 32]>>,
    after: anyhow::Result<[u8; 32]>,
) -> bool {
    matches!((before, after), (Ok(Some(a)), Ok(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn actual_sources_survive_metadata_writes_and_detect_content_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", temp.path().join("no-global"))
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-b", "main", "--", "."]);
        std::fs::write(root.join("a.rs"), "old\n").unwrap();
        git(&["add", "--", "a.rs"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "fixture",
        ]);
        std::fs::write(root.join("a.rs"), "new\n").unwrap();
        let before = stamp(&root).unwrap();
        git(&["add", "--", "a.rs"]);
        assert!(unchanged(Ok(Some(before)), stamp(&root)));
        git(&["branch", "--", "other"]);
        assert!(unchanged(Ok(Some(before)), stamp(&root)));
        std::fs::write(root.join("a.rs"), "now\n").unwrap();
        assert!(!unchanged(Ok(Some(before)), stamp(&root)));
        assert!(!unchanged(Err(anyhow::anyhow!("unreadable")), stamp(&root)));
    }
}
