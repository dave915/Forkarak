//! Explicit Clone/Init entrypoints. Unlike GitOperations::open, these accept a
//! destination that is not a repository yet. No existing directory is reused.
use super::file_save::{identity, open_at, open_directory};
use super::*;
use std::{
    ffi::CString,
    fs,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    path::Component,
};

pub fn init_repository(destination: &Path, branch: &str) -> Result<OperationOutput> {
    init_repository_with_control(
        destination,
        branch,
        OperationCancellation::default(),
        OperationProgress::default(),
    )
}

pub fn init_repository_with_control(
    destination: &Path,
    branch: &str,
    cancellation: OperationCancellation,
    progress: OperationProgress,
) -> Result<OperationOutput> {
    ensure!(
        !cancellation.is_cancelled(),
        "Init cancelled before destination creation"
    );
    let (parent, destination) = destination_path(destination)?;
    let mut op = destination_runner(&parent, cancellation, progress);
    op.validate_branch(branch)?;
    let directory = reserve_destination(&parent, &destination)?;
    op.root = destination;
    let result = run_destination(
        &op,
        &directory,
        &["init", "--initial-branch", branch, "--", "."],
    )
    .context("Init failed; inspect the selected destination before retrying")?;
    op.workflow_output(result)
}

pub fn clone_repository(
    url: &str,
    destination: &Path,
    cancellation: OperationCancellation,
    progress: OperationProgress,
) -> Result<OperationOutput> {
    validate_remote_url(url)?;
    ensure!(
        !cancellation.is_cancelled(),
        "Clone cancelled before destination creation"
    );
    let (parent, destination) = destination_path(destination)?;
    // Relative local sources refer to the caller's current directory, not the
    // chosen destination's parent. Remote transports retain their exact URL.
    let url = if url.starts_with("./") || url.starts_with("../") {
        std::env::current_dir()?
            .join(url)
            .canonicalize()?
            .to_str()
            .context("Local clone source must be UTF-8")?
            .to_owned()
    } else {
        url.to_owned()
    };
    let mut op = destination_runner(&parent, cancellation, progress);
    let directory = reserve_destination(&parent, &destination)?;
    // Use this new empty directory as the explicit destination and cwd. Output
    // events are bound to that canonical path from the first emitted byte.
    op.root = destination;
    let result = run_destination(&op, &directory, &["clone", "--progress", "--no-recurse-submodules", "--origin", "origin", "--", &url, "."])
        .context("Clone did not complete; inspect the destination and repository state before retrying. Partial data is retained when Git leaves it behind")?;
    op.workflow_output(result)
}

fn destination_path(path: &Path) -> Result<(PathBuf, PathBuf)> {
    ensure!(
        path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
        "Choose an absolute destination without traversal"
    );
    ensure!(
        !path.components().any(|c| c
            .as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b".git")),
        "Repositories cannot be created in Git administration paths"
    );
    let parent = path
        .parent()
        .context("Choose a new folder, not the filesystem root")?
        .canonicalize()
        .context("Destination parent must already exist")?;
    ensure!(
        !parent.components().any(|c| c
            .as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b".git")),
        "Repositories cannot be created in Git administration paths"
    );
    let destination = parent.join(
        path.file_name()
            .context("Choose a destination folder name")?,
    );
    ensure!(
        matches!(fs::symlink_metadata(&destination), Err(e) if e.kind() == io::ErrorKind::NotFound),
        "Destination must not exist; choose a new folder"
    );
    let probe = destination_runner(
        &parent,
        OperationCancellation::default(),
        OperationProgress::default(),
    )
    .run(&["rev-parse", "--is-inside-git-dir"])?;
    ensure!(
        probe.code != Some(0) || probe.stdout != b"true\n",
        "Cannot create a repository inside Git administration data"
    );
    Ok((parent, destination))
}
fn destination_runner(
    root: &Path,
    cancellation: OperationCancellation,
    progress: OperationProgress,
) -> GitOperations {
    GitOperations {
        root: root.into(),
        timeout: Duration::from_secs(15 * 60),
        cancellation,
        progress: Some(progress),
        #[cfg(test)]
        global_config: Some(PathBuf::from("/dev/null")),
    }
}

// mkdirat atomically reserves a fresh leaf under a pinned parent. Git operates
// from the pinned new directory, so replacing its pathname cannot redirect it
// into another repository. Partial results are never force-removed.
fn reserve_destination(parent: &Path, destination: &Path) -> Result<fs::File> {
    let parent_dir = open_directory(parent)?;
    let name = CString::new(
        destination
            .file_name()
            .context("Missing destination name")?
            .as_bytes(),
    )?;
    if unsafe { libc::mkdirat(parent_dir.as_raw_fd(), name.as_ptr(), 0o755) } != 0 {
        return Err(io::Error::last_os_error())
            .context("Destination already exists or cannot be created");
    }
    let directory = open_at(&parent_dir, &name, libc::O_RDONLY | libc::O_DIRECTORY)?;
    ensure!(
        identity(&open_directory(parent)?.metadata()?) == identity(&parent_dir.metadata()?),
        "Destination parent changed; inspect the partial destination before retrying"
    );
    Ok(directory)
}
fn run_destination(op: &GitOperations, directory: &fs::File, args: &[&str]) -> Result<Captured> {
    ensure!(
        identity(&open_directory(&op.root)?.metadata()?) == identity(&directory.metadata()?),
        "Destination changed; inspect it before retrying"
    );
    op.check_result(op.run_command(op.command_at(args, Some(directory.as_raw_fd())))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserved_destination_swaps_cannot_initialize_another_directory() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().canonicalize().unwrap();
        let destination = parent.join("destination");
        let directory = reserve_destination(&parent, &destination).unwrap();
        let moved = parent.join("moved");
        let outside = parent.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::rename(&destination, &moved).unwrap();
        std::os::unix::fs::symlink(&outside, &destination).unwrap();
        let op = destination_runner(
            &destination,
            OperationCancellation::default(),
            OperationProgress::default(),
        );
        assert!(run_destination(&op, &directory, &["init", "--", "."]).is_err());
        assert!(!outside.join(".git").exists());
        assert!(!moved.join(".git").exists());
        // Even if a pathname changes after validation, the child is anchored
        // by the original FD; a second -C pathname resolution is never used.
        let command = op.command_at(
            &["init", "--template=", "--", "."],
            Some(directory.as_raw_fd()),
        );
        op.check_result(op.run_command(command).unwrap()).unwrap();
        assert!(moved.join(".git").exists());
        assert!(!outside.join(".git").exists());
    }
}
