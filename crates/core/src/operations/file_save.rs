//! Whole-file editing is an explicit operation, never part of inspection.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{CStr, CString},
    fs::{File, Metadata},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::MAX_TEXT_BYTES;

/// Atomically replace an existing UTF-8 working file after comparing its exact
/// disk bytes with `expected`. Call only for an explicit user save action.
///
/// `root` must identify the selected Git worktree root; `path` must be relative,
/// without traversal, symlinks (including parent symlinks), or Git metadata.
/// NUL-containing text and files beyond the whole-file reader's size limit are
/// rejected. Permissions and bytes (including line endings) are preserved; no
/// filters, hooks, staging, or other Git writes run.
///
/// The file and directory identities, permissions, timestamps and contents are
/// rechecked immediately before the atomic rename. As with other filesystem
/// editors, unrelated writers must coordinate: POSIX has no conditional rename
/// that can exclude a write in the final check-to-rename interval.
pub fn save_file(root: &Path, path: &Path, expected: &str, replacement: &str) -> Result<()> {
    PreparedSave::new(root, path, expected, replacement)?.commit()
}

pub(super) struct Target {
    pub(super) root: PathBuf,
    pub(super) relative: PathBuf,
    pub(super) root_dir: File,
    pub(super) parent: File,
    pub(super) name: CString,
    pub(super) administration: Vec<(u64, u64)>,
}

impl Target {
    pub(super) fn open(root: &Path, path: &Path) -> Result<Self> {
        ensure!(
            !path.as_os_str().is_empty()
                && path.components().all(|component| matches!(component,
                    Component::Normal(name) if !name.as_bytes().eq_ignore_ascii_case(b".git")
                        && !name.as_bytes().contains(&0))),
            "Save path must be a relative worktree file without traversal or .git components"
        );
        let root = root
            .canonicalize()
            .context("The selected worktree no longer exists")?;
        ensure!(
            crate::repository_root(&root)? == root,
            "Save root must be the selected Git worktree root"
        );
        let root_dir = open_directory(&root)?;
        // Separate Git dirs and linked-worktree common dirs need not be named
        // `.git`. Protect their identities as well as lexical .git components.
        let mut administration = Vec::new();
        for option in ["--git-dir", "--git-common-dir"] {
            let bytes = crate::git::words(&root, &["rev-parse", "--path-format=absolute", option])?;
            let directory = crate::git::path(bytes.strip_suffix(b"\n").unwrap_or(&bytes));
            administration.push(identity(&File::open(directory)?.metadata()?));
        }
        let parent = open_parent(&root_dir, path, &administration)?;
        let name = CString::new(
            path.file_name()
                .context("Select a file to save")?
                .as_bytes(),
        )?;
        Ok(Self {
            root,
            relative: path.to_owned(),
            root_dir,
            parent,
            name,
            administration,
        })
    }

    pub(super) fn revalidate(&self) -> Result<()> {
        ensure!(
            identity(&open_directory(&self.root)?.metadata()?)
                == identity(&self.root_dir.metadata()?),
            "Worktree directory changed externally; reopen the file before saving"
        );
        let current = open_parent(&self.root_dir, &self.relative, &self.administration)?;
        ensure!(
            identity(&current.metadata()?) == identity(&self.parent.metadata()?),
            "File parent directory changed externally; reopen the file before saving"
        );
        Ok(())
    }

    fn read(&self) -> Result<DiskFile> {
        let mut file = open_at(&self.parent, &self.name, libc::O_RDONLY).context(
            "Cannot safely open working file; missing files and symlinks cannot be saved",
        )?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file(),
            "Only existing regular files can be saved"
        );
        ensure!(
            metadata.len() <= MAX_TEXT_BYTES as u64,
            "File exceeds the 2 MiB editing limit"
        );
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_TEXT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_TEXT_BYTES
                && bytes.len() as u64 == metadata.len()
                && unchanged(&metadata, &file.metadata()?),
            "Working file changed while reading; reopen it before saving"
        );
        ensure!(!bytes.contains(&0), "Binary files cannot be saved as text");
        std::str::from_utf8(&bytes).context("Non-UTF-8 files cannot be saved as text")?;
        // Keeping the descriptor alive also prevents inode reuse during save.
        Ok(DiskFile {
            _file: file,
            metadata,
            bytes,
        })
    }
}

struct DiskFile {
    _file: File,
    metadata: Metadata,
    bytes: Vec<u8>,
}

struct PreparedSave {
    target: Target,
    original: DiskFile,
    temporary: CString,
    file: File,
    temporary_metadata: Metadata,
    installed: bool,
}

impl PreparedSave {
    fn new(root: &Path, path: &Path, expected: &str, replacement: &str) -> Result<Self> {
        ensure!(
            !expected.contains('\0') && !replacement.contains('\0'),
            "Binary text containing NUL cannot be saved"
        );
        ensure!(
            expected.len() <= MAX_TEXT_BYTES && replacement.len() <= MAX_TEXT_BYTES,
            "File exceeds the 2 MiB editing limit"
        );
        let target = Target::open(root, path)?;
        let original = target.read()?;
        ensure!(
            original.bytes == expected.as_bytes(),
            "Working file changed externally. Nothing saved; reopen the file to compare the new content"
        );
        let (temporary, file) = create_temporary(&target.parent)?;
        let temporary_metadata = file.metadata()?;
        let mut save = Self {
            target,
            original,
            temporary,
            file,
            temporary_metadata,
            installed: false,
        };
        // The guard cleans up even if writing, chmod, syncing or validation fails.
        save.file
            .write_all(replacement.as_bytes())
            .context("Cannot write saved contents")?;
        save.file
            .set_permissions(save.original.metadata.permissions())
            .context("Cannot preserve file permissions")?;
        save.file.sync_all().context("Cannot sync saved contents")?;
        save.temporary_metadata = save.file.metadata()?;
        Ok(save)
    }

    fn commit(mut self) -> Result<()> {
        self.target.revalidate()?;
        let current = self.target.read()?;
        ensure!(
            unchanged(&self.original.metadata, &current.metadata)
                && self.original.bytes == current.bytes,
            "Working file changed externally while preparing the save. Nothing saved; reopen the file"
        );
        let temporary = open_at(&self.target.parent, &self.temporary, libc::O_RDONLY)?;
        ensure!(
            unchanged(&temporary.metadata()?, &self.temporary_metadata),
            "Temporary save file changed externally; nothing saved"
        );
        // SAFETY: both names are single NUL-terminated components, and the
        // live parent descriptor pins the destination directory. renameat does
        // not follow either leaf symlink or expose partially written contents.
        if unsafe {
            libc::renameat(
                self.target.parent.as_raw_fd(),
                self.temporary.as_ptr(),
                self.target.parent.as_raw_fd(),
                self.target.name.as_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error())
                .context("Cannot atomically replace working file");
        }
        self.installed = true;
        Ok(())
    }
}

impl Drop for PreparedSave {
    fn drop(&mut self) {
        if self.installed {
            return;
        }
        // SAFETY: an owned directory FD and single component remain live. After
        // a successful rename the name is absent; on error only our temp is removed.
        unsafe {
            libc::unlinkat(self.target.parent.as_raw_fd(), self.temporary.as_ptr(), 0);
        }
    }
}

pub(super) fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

pub(super) fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    identity(a) == identity(b)
        && a.len() == b.len()
        && a.mode() == b.mode()
        && (a.mtime(), a.mtime_nsec(), a.ctime(), a.ctime_nsec())
            == (b.mtime(), b.mtime_nsec(), b.ctime(), b.ctime_nsec())
}

fn open_directory(path: &Path) -> Result<File> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: path is terminated; successful descriptor is uniquely owned below.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("Cannot safely open worktree directory");
    }
    // SAFETY: open returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn open_at(parent: &File, name: &CStr, flags: i32) -> io::Result<File> {
    // SAFETY: parent and terminated name stay live; creation always uses 0600.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_parent(root: &File, path: &Path, administration: &[(u64, u64)]) -> Result<File> {
    let mut directory = root.try_clone()?;
    for component in path
        .parent()
        .context("Select a relative file")?
        .components()
    {
        let name = CString::new(component.as_os_str().as_bytes())?;
        directory = open_at(&directory, &name, libc::O_RDONLY | libc::O_DIRECTORY)
            .context("Cannot safely open file parent; symlink directories are not allowed")?;
        ensure!(
            !administration.contains(&identity(&directory.metadata()?)),
            "Saving Git administration paths is not allowed"
        );
    }
    Ok(directory)
}

pub(super) fn create_temporary(parent: &File) -> Result<(CString, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    for _ in 0..128 {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = CString::new(format!(
            ".pokarak-save-{}-{time}-{serial}",
            std::process::id()
        ))?;
        match open_at(parent, &name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL) {
            Ok(file) => return Ok((name, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("Cannot create temporary save file"),
        }
    }
    anyhow::bail!("Cannot allocate a unique temporary save file")
}

#[cfg(test)]
mod tests;
