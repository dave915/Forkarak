//! Explicit worktree filesystem actions. Existing parents are required; Git is
//! never staged, checked out, or otherwise modified by these operations.
use super::file_save::{Target, create_temporary, identity, open_at, unchanged};
use anyhow::{Context, Result, ensure};
use std::{ffi::{CStr, CString}, fs::{File, Metadata}, io::{self, Write}, os::{fd::AsRawFd, unix::ffi::OsStrExt}, path::{Path, PathBuf}};

/// Confirmation identity for rename/move. Source replacement or edits require
/// a fresh inspection. As with save_file, POSIX cannot exclude an uncoordinated
/// source replacement in the final validation-to-rename interval.
#[derive(Clone, Debug)]
pub struct PathIdentity {
    pub root: PathBuf,
    pub path: PathBuf,
    pub is_directory: bool,
    metadata: Metadata,
    bound_path: PathBuf,
    bound_root: PathBuf,
}

pub fn inspect_path(root: &Path, path: &Path) -> Result<PathIdentity> {
    let target = guarded(root, path)?;
    let file = open_at(&target.parent, &target.name, libc::O_RDONLY)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file() || metadata.is_dir(), "Only regular files and directories can be moved");
    if metadata.is_dir() { guard_subtree(&file, &target.administration, &mut 100_000)?; }
    Ok(PathIdentity { bound_root: target.root.clone(), bound_path: path.into(), root: target.root, path: path.into(), is_directory: metadata.is_dir(), metadata })
}

pub fn create_file(root: &Path, path: &Path, contents: &[u8]) -> Result<()> {
    let target = guarded(root, path)?;
    let (name, mut file) = create_temporary(&target.parent)?;
    let result = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        target.revalidate()?;
        rename_exclusive(&target.parent, &name, &target.parent, &target.name)
    })();
    if result.is_err() {
        // SAFETY: only the temporary name created by this operation is removed.
        unsafe { libc::unlinkat(target.parent.as_raw_fd(), name.as_ptr(), 0); }
    }
    result
}

pub fn create_directory(root: &Path, path: &Path) -> Result<()> {
    let target = guarded(root, path)?;
    target.revalidate()?;
    // SAFETY: pinned parent descriptor and one terminated component. mkdirat
    // refuses any existing leaf, including symlinks and empty directories.
    if unsafe { libc::mkdirat(target.parent.as_raw_fd(), target.name.as_ptr(), 0o755) } != 0 {
        return Err(io::Error::last_os_error()).context("Cannot create directory; destination must not exist");
    }
    Ok(())
}

pub fn move_path(root: &Path, expected: &PathIdentity, destination: &Path) -> Result<()> {
    ensure!(expected.root == expected.bound_root && expected.path == expected.bound_path, "Move confirmation changed");
    let source = guarded(root, &expected.bound_path)?;
    ensure!(source.root == expected.root, "Move belongs to a different worktree");
    let target = guarded(root, destination)?;
    ensure!(expected.path != destination, "Choose a different destination");
    ensure!(!expected.is_directory || !destination.starts_with(&expected.path), "A directory cannot move into itself");
    source.revalidate()?;
    target.revalidate()?;
    let file = open_at(&source.parent, &source.name, libc::O_RDONLY)?;
    ensure!(unchanged(&file.metadata()?, &expected.metadata), "Source changed externally; inspect it before moving");
    if expected.is_directory { guard_subtree(&file, &source.administration, &mut 100_000)?; }
    rename_exclusive(&source.parent, &source.name, &target.parent, &target.name)
}

fn guarded(root: &Path, path: &Path) -> Result<Target> {
    let target = Target::open(root, path)?;
    let mut directory = target.root_dir.try_clone()?;
    for component in path.parent().context("Select a relative path")?.components() {
        directory = open_at(&directory, &CString::new(component.as_os_str().as_bytes())?, libc::O_RDONLY | libc::O_DIRECTORY)?;
        guard_directory(&directory, &target.administration)?;
    }
    Ok(target)
}

fn guard_directory(directory: &File, administration: &[(u64,u64)]) -> Result<()> {
    ensure!(!administration.contains(&identity(&directory.metadata()?)), "Git administration directories cannot be changed");
    let dot_git = c".git";
    // SAFETY: fstatat does not follow symlinks; presence of any .git entry marks
    // a nested repository and blocks actions within it.
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe { libc::fstatat(directory.as_raw_fd(), dot_git.as_ptr(), metadata.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    ensure!(result != 0, "Nested repositories cannot be changed from the enclosing worktree");
    let error = io::Error::last_os_error();
    if error.kind() != io::ErrorKind::NotFound { return Err(error).context("Cannot inspect nested-repository boundary"); }
    Ok(())
}

fn guard_subtree(directory: &File, administration: &[(u64,u64)], budget: &mut usize) -> Result<()> {
    guard_directory(directory, administration)?;
    // SAFETY: a new duplicated FD is transferred to fdopendir; closedir owns it.
    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    ensure!(duplicate >= 0, "Cannot inspect directory: {}", io::Error::last_os_error());
    let raw = unsafe { libc::fdopendir(duplicate) };
    if raw.is_null() { unsafe { libc::close(duplicate); } return Err(io::Error::last_os_error()).context("Cannot inspect directory"); }
    struct Entries(*mut libc::DIR);
    impl Drop for Entries { fn drop(&mut self) { unsafe { libc::closedir(self.0); } } }
    let entries = Entries(raw);
    loop {
        let entry = unsafe { libc::readdir(entries.0) };
        if entry.is_null() { break; }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") { continue; }
        ensure!(*budget > 0, "Directory move exceeds the 100000-entry safety limit");
        *budget -= 1;
        ensure!(!name.to_bytes().eq_ignore_ascii_case(b".git"), "Nested Git metadata cannot be moved");
        match open_at(directory, name, libc::O_RDONLY | libc::O_DIRECTORY) {
            Ok(child) => guard_subtree(&child, administration, budget)?,
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => (),
            Err(e) => return Err(e).context("Directory changed while inspecting move"),
        }
    }
    Ok(())
}

pub(super) fn rename_exclusive(from: &File, name: &CStr, to: &File, destination: &CStr) -> Result<()> {
    // SAFETY: descriptor-relative rename with the platform's atomic exclusive
    // flag guarantees no destination overwrite, including concurrent creation.
    #[cfg(target_os="macos")]
    let result = unsafe { libc::renameatx_np(from.as_raw_fd(), name.as_ptr(), to.as_raw_fd(), destination.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(not(target_os="macos"))]
    let result = unsafe { libc::renameat2(from.as_raw_fd(), name.as_ptr(), to.as_raw_fd(), destination.as_ptr(), libc::RENAME_NOREPLACE) };
    if result != 0 { return Err(io::Error::last_os_error()).context("Cannot move/create path; destination must not exist"); }
    Ok(())
}
