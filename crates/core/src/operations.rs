//! Explicit, user-requested Git and file writes. Inspection APIs never call this module.
//!
//! Commands are argv-only, scoped to a canonical worktree, noninteractive, and
//! bounded by a deadline and output limits. Credentials/SSH configuration are
//! inherited. Explicit Git writes honor configured Git hooks; submodule operations
//! are not recursive.
pub mod commit_actions;
mod file_save;
pub mod history_actions;
pub mod integration;
pub mod sync;
pub mod workflow;
pub mod worktrees;
pub use file_save::save_file;

use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 256 * 1024;
const ERROR_LIMIT: usize = 32 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    MergedOnly,
    /// Explicitly discard this branch reference even if commits are unmerged.
    ForceUnmerged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteUrlKind {
    Fetch,
    Push,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Create at this worktree's HEAD without switching branches.
    CreateBranch {
        name: String,
    },
    CheckoutBranch {
        name: String,
    },
    DeleteBranch {
        name: String,
        mode: DeleteMode,
    },
    Fetch {
        remote: String,
        prune: bool,
    },
    /// Only refs/heads/<branch>:refs/heads/<branch>; never matching or force.
    Push {
        remote: String,
        branch: String,
    },
    /// Fetch the named remote branch and fast-forward the named current branch.
    Pull {
        remote: String,
        remote_branch: String,
        local_branch: String,
    },
    AddRemote {
        name: String,
        url: String,
    },
    SetRemoteUrl {
        name: String,
        url: String,
        kind: RemoteUrlKind,
    },
    RenameRemote {
        name: String,
        new_name: String,
    },
    RemoveRemote {
        name: String,
    },
    /// Repository-local commit identity, not network authentication.
    SetIdentity {
        name: String,
        email: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub fetch_urls: Vec<String>,
    pub push_urls: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RepositoryState {
    pub root: PathBuf,
    pub current_branch: Option<String>,
    pub local_branches: Vec<String>,
    /// Includes the current worktree; these branches cannot be deleted.
    pub checked_out_branches: Vec<String>,
    pub remotes: Vec<Remote>,
    pub local_identity: Identity,
    pub effective_identity: Identity,
}

#[derive(Debug, Clone)]
pub struct OperationOutput {
    pub root: PathBuf,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

/// Cancellation for one explicitly owned operation, including its SSH/hook
/// children. Attach before execution. Cancelling also prevents subsequent
/// commands in a multi-step operation from starting.
#[derive(Debug, Clone, Default)]
pub struct OperationCancellation {
    state: Arc<Mutex<OperationProcess>>,
}

#[derive(Debug, Default)]
struct OperationProcess {
    cancelled: bool,
    child: Option<Child>,
}

impl OperationCancellation {
    /// Synchronously terminate and reap only this handle's owned child group.
    /// Call from app/window shutdown before dropping its background task.
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.cancelled = true;
        if let Some(mut child) = state.child.take() {
            // SAFETY: this Child was spawned in a new process group by capture.
            // The mutex serializes spawning, reaping and cancellation; no PID
            // discovered from a process listing or another job is ever used.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .cancelled
    }
}

/// A handle to one worktree. Call from a background task and serialize writes
/// in the caller. Git's own locks and worktree protections remain in effect.
#[derive(Debug, Clone)]
pub struct GitOperations {
    root: PathBuf,
    timeout: Duration,
    cancellation: OperationCancellation,
    #[cfg(test)]
    global_config: Option<PathBuf>,
}

#[derive(Debug)]
struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: Option<i32>,
    truncated: bool,
}

impl GitOperations {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path, DEFAULT_TIMEOUT)
    }

    fn open_inner(path: &Path, timeout: Duration) -> Result<Self> {
        let mut this = Self {
            root: path
                .canonicalize()
                .context("The selected worktree no longer exists")?,
            timeout,
            cancellation: OperationCancellation::default(),
            #[cfg(test)]
            global_config: None,
        };
        let root = this.metadata(&["rev-parse", "--show-toplevel"])?;
        this.root = crate::git::path(root.strip_suffix(b"\n").unwrap_or(&root)).canonicalize()?;
        Ok(this)
    }

    /// Share this job's lifetime with its UI owner. The owner must cancel on
    /// shutdown; normal UI close/quit should first guard in-flight writes.
    pub fn with_cancellation(mut self, cancellation: OperationCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state(&self) -> Result<RepositoryState> {
        let branches = self.metadata(&[
            "for-each-ref",
            "--format=%(refname:strip=2)",
            "--",
            "refs/heads/",
        ])?;
        Ok(RepositoryState {
            root: self.root.clone(),
            current_branch: self.current_branch()?,
            local_branches: lines(&branches)?,
            checked_out_branches: self.checked_out_branches()?,
            remotes: self.remotes()?,
            local_identity: self.identity(true)?,
            effective_identity: self.identity(false)?,
        })
    }

    pub fn remotes(&self) -> Result<Vec<Remote>> {
        let mut remotes = Vec::new();
        for name in lines(&self.metadata(&["remote"])?)? {
            validate_remote_name(&name)?;
            remotes.push(Remote {
                fetch_urls: lines(&self.metadata(&["remote", "get-url", "--all", "--", &name])?)?,
                push_urls: lines(
                    &self.metadata(&["remote", "get-url", "--push", "--all", "--", &name])?,
                )?,
                name,
            });
        }
        Ok(remotes)
    }

    pub fn execute(&self, operation: &Operation) -> Result<OperationOutput> {
        let result = match operation {
            Operation::CreateBranch { name } => {
                self.validate_branch(name)?;
                self.checked(&["branch", "--no-track", "--", name, "HEAD"])?
            }
            Operation::CheckoutBranch { name } => {
                self.require_local_branch(name)?;
                if self.current_branch()?.as_deref() != Some(name) {
                    self.require_unoccupied(name)?;
                }
                // Git refuses tracked/untracked collisions; ignored files are
                // protected too. No discard, force, merge, or autostash option.
                self.checked(&[
                    "switch",
                    "--no-guess",
                    "--no-overwrite-ignore",
                    "--no-recurse-submodules",
                    "--",
                    name,
                ])?
            }
            Operation::DeleteBranch { name, mode } => {
                self.require_local_branch(name)?;
                self.require_unoccupied(name)?;
                self.checked(&[
                    "branch",
                    match mode {
                        DeleteMode::MergedOnly => "-d",
                        DeleteMode::ForceUnmerged => "-D",
                    },
                    "--",
                    name,
                ])?
            }
            Operation::Fetch { remote, prune } => {
                self.require_remote(remote, false)?;
                // Explicit destination isolates even mirror/custom fetch config.
                let spec = format!("+refs/heads/*:refs/remotes/{remote}/*");
                self.checked(&[
                    "fetch",
                    "--no-tags",
                    "--no-prune-tags",
                    if *prune { "--prune" } else { "--no-prune" },
                    "--no-recurse-submodules",
                    "--refmap=",
                    "--",
                    remote,
                    &spec,
                ])?
            }
            Operation::Push { remote, branch } => {
                self.require_local_branch(branch)?;
                self.require_remote(remote, true)?;
                let mirror = format!("remote.{remote}.mirror=false");
                let reference = format!("refs/heads/{branch}:refs/heads/{branch}");
                self.checked(&[
                    "-c",
                    &mirror,
                    "push",
                    "--no-force",
                    "--no-follow-tags",
                    "--no-prune",
                    "--recurse-submodules=no",
                    "--",
                    remote,
                    &reference,
                ])?
            }
            Operation::Pull {
                remote,
                remote_branch,
                local_branch,
            } => {
                self.validate_branch(remote_branch)?;
                self.require_local_branch(local_branch)?;
                self.require_current(local_branch)?;
                self.require_clean()?;
                self.require_remote(remote, false)?;
                let reference = format!("refs/heads/{remote_branch}");
                let fetched = self.checked(&[
                    "fetch",
                    "--no-tags",
                    "--no-prune",
                    "--no-prune-tags",
                    "--no-recurse-submodules",
                    "--refmap=",
                    "--",
                    remote,
                    &reference,
                ])?;
                let oid = self.metadata(&[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    "FETCH_HEAD^{commit}",
                ])?;
                let oid = std::str::from_utf8(&oid)?.trim();
                self.require_current(local_branch)?;
                self.require_clean()?;
                let mut merged = self.checked(&[
                    "merge",
                    "--ff-only",
                    "--no-autostash",
                    "--no-overwrite-ignore",
                    "--",
                    oid,
                ])?;
                // Keep fetch diagnostics too, with the same total output bounds.
                merged.truncated |=
                    fetched.truncated || merged.stderr.len() + fetched.stderr.len() > ERROR_LIMIT;
                merged.stderr.extend(fetched.stderr);
                merged.stderr.truncate(ERROR_LIMIT);
                merged
            }
            Operation::AddRemote { name, url } => {
                validate_remote_name(name)?;
                validate_remote_url(url)?;
                self.checked(&["remote", "add", "--", name, url])?
            }
            Operation::SetRemoteUrl { name, url, kind } => {
                validate_remote_url(url)?;
                self.require_remote_exists(name)?;
                let urls = match kind {
                    RemoteUrlKind::Fetch => {
                        self.metadata(&["remote", "get-url", "--all", "--", name])?
                    }
                    RemoteUrlKind::Push => {
                        self.metadata(&["remote", "get-url", "--push", "--all", "--", name])?
                    }
                };
                ensure!(
                    lines(&urls)?.len() == 1,
                    "Remote {name:?} has multiple URLs; edit them with Git externally"
                );
                match kind {
                    RemoteUrlKind::Fetch => {
                        self.checked(&["remote", "set-url", "--", name, url])?
                    }
                    RemoteUrlKind::Push => {
                        self.checked(&["remote", "set-url", "--push", "--", name, url])?
                    }
                }
            }
            Operation::RenameRemote { name, new_name } => {
                self.require_remote_exists(name)?;
                validate_remote_name(new_name)?;
                self.checked(&["remote", "rename", "--", name, new_name])?
            }
            Operation::RemoveRemote { name } => {
                self.require_remote_exists(name)?;
                self.checked(&["remote", "remove", "--", name])?
            }
            Operation::SetIdentity { name, email } => {
                validate_identity(name, email)?;
                // Two config writes: report if only the first succeeded. Never
                // restore a whole config file and clobber concurrent edits.
                self.checked(&[
                    "config",
                    "--local",
                    "--replace-all",
                    "--",
                    "user.name",
                    name,
                ])?;
                self.checked(&["config", "--local", "--replace-all", "--", "user.email", email])
                    .context("user.name was saved, but user.email failed; retry saving the repository identity")?
            }
        };
        Ok(OperationOutput {
            root: self.root.clone(),
            stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            truncated: result.truncated,
        })
    }

    fn validate_branch(&self, name: &str) -> Result<()> {
        ensure!(
            !name.is_empty() && name.len() <= 1024 && !name.starts_with('-') && name != "HEAD",
            "Enter a local branch name (not HEAD or an option)"
        );
        let reference = format!("refs/heads/{name}");
        let result = self.run(&["check-ref-format", &reference])?;
        ensure!(result.code == Some(0), "Invalid branch name: {name:?}");
        Ok(())
    }

    fn require_local_branch(&self, name: &str) -> Result<()> {
        self.validate_branch(name)?;
        self.checked(&[
            "show-ref",
            "--verify",
            "--quiet",
            "--",
            &format!("refs/heads/{name}"),
        ])
        .with_context(|| format!("Local branch {name:?} does not exist; choose a local branch"))?;
        Ok(())
    }

    fn current_branch(&self) -> Result<Option<String>> {
        let result = self.run(&["symbolic-ref", "--quiet", "HEAD"])?;
        match result.code {
            Some(0) => Ok(Some(
                std::str::from_utf8(&result.stdout)?
                    .trim_end_matches('\n')
                    .strip_prefix("refs/heads/")
                    .context("HEAD is not attached to a local branch")?
                    .to_owned(),
            )),
            Some(1) => Ok(None),
            _ => bail!(
                "Cannot read the current branch: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
        }
    }

    fn checked_out_branches(&self) -> Result<Vec<String>> {
        let data = self.metadata(&["worktree", "list", "--porcelain", "-z"])?;
        data.split(|b| *b == 0)
            .filter_map(|field| field.strip_prefix(b"branch refs/heads/"))
            .map(|name| Ok(std::str::from_utf8(name)?.to_owned()))
            .collect()
    }

    fn require_unoccupied(&self, branch: &str) -> Result<()> {
        ensure!(
            !self
                .checked_out_branches()?
                .iter()
                .any(|name| name == branch),
            "Branch {branch:?} is checked out in this or another worktree. Switch that worktree to another branch first."
        );
        Ok(())
    }

    fn require_current(&self, branch: &str) -> Result<()> {
        ensure!(
            self.current_branch()?.as_deref() == Some(branch),
            "The checked-out branch changed. Reopen Git actions and confirm the pull destination."
        );
        Ok(())
    }

    fn require_clean(&self) -> Result<()> {
        ensure!(
            self.metadata(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--"
            ])?
            .is_empty(),
            "Pull requires a clean worktree. Commit or move local changes first; Forkarak never stashes or discards them."
        );
        Ok(())
    }

    fn require_remote_exists(&self, name: &str) -> Result<()> {
        validate_remote_name(name)?;
        ensure!(
            lines(&self.metadata(&["remote"])?)?
                .iter()
                .any(|r| r == name),
            "Remote {name:?} no longer exists; reopen Git actions"
        );
        Ok(())
    }

    fn require_remote(&self, name: &str, push: bool) -> Result<()> {
        self.require_remote_exists(name)?;
        let args = if push {
            vec!["remote", "get-url", "--push", "--all", "--", name]
        } else {
            vec!["remote", "get-url", "--all", "--", name]
        };
        let urls = lines(&self.metadata(&args)?)?;
        ensure!(
            urls.len() == 1,
            "Remote {name:?} has multiple URLs. Use Git externally to choose one URL before this operation."
        );
        validate_remote_url(&urls[0])
    }

    fn identity(&self, local: bool) -> Result<Identity> {
        Ok(Identity {
            name: self.config_value("user.name", local)?,
            email: self.config_value("user.email", local)?,
        })
    }

    fn config_value(&self, key: &str, local: bool) -> Result<Option<String>> {
        let args = if local {
            vec!["config", "--local", "--get", "--", key]
        } else {
            vec!["config", "--get", "--", key]
        };
        let result = self.run(&args)?;
        match result.code {
            Some(0) => {
                ensure!(!result.truncated, "Git configuration value is too large");
                Ok(Some(
                    std::str::from_utf8(&result.stdout)?
                        .trim_end_matches('\n')
                        .to_owned(),
                ))
            }
            Some(1) => Ok(None),
            _ => bail!(
                "Cannot read {key}: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
        }
    }

    fn metadata(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = self.checked(args)?;
        ensure!(
            !output.truncated,
            "Git metadata exceeds the safety limit; use Git externally to narrow it"
        );
        Ok(output.stdout)
    }

    fn checked(&self, args: &[&str]) -> Result<Captured> {
        let result = self.run(args)?;
        ensure!(
            result.code == Some(0),
            "Git operation failed in {}:\n{}\n{}\nFor authentication failures, configure your existing credential helper or SSH agent in a terminal, then retry. For rejected updates, fetch and review the branch first. No force or automatic stash is used.",
            self.root.display(),
            String::from_utf8_lossy(&result.stderr).trim(),
            String::from_utf8_lossy(&result.stdout).trim()
        );
        Ok(result)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        // Remove repository/config injection, preserving credentials, SSH agent,
        // SSH command, and GIT_CONFIG_GLOBAL/SYSTEM (also used by test fixtures).
        for (key, _) in std::env::vars_os() {
            let key_text = key.to_string_lossy();
            if matches!(
                key_text.as_ref(),
                "GIT_DIR"
                    | "GIT_COMMON_DIR"
                    | "GIT_WORK_TREE"
                    | "GIT_INDEX_FILE"
                    | "GIT_OBJECT_DIRECTORY"
                    | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                    | "GIT_SHALLOW_FILE"
                    | "GIT_QUARANTINE_PATH"
                    | "GIT_REPLACE_REF_BASE"
                    | "GIT_NAMESPACE"
                    | "GIT_PREFIX"
                    | "GIT_CONFIG"
                    | "GIT_CONFIG_COUNT"
                    | "GIT_CONFIG_PARAMETERS"
            ) || key_text.starts_with("GIT_CONFIG_KEY_")
                || key_text.starts_with("GIT_CONFIG_VALUE_")
            {
                command.env_remove(key);
            }
        }
        #[cfg(test)]
        if let Some(path) = &self.global_config {
            command
                .env("GIT_CONFIG_GLOBAL", path)
                .env("GIT_CONFIG_NOSYSTEM", "1");
        }
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("GIT_ASKPASS", "/usr/bin/false")
            .env("SSH_ASKPASS", "/usr/bin/false")
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("LC_ALL", "C")
            .args([
                "--no-pager",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
                "-c",
                "protocol.ext.allow=never",
                "-c",
                "submodule.recurse=false",
                "-C",
            ])
            .arg(&self.root)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        command
    }

    fn run(&self, args: &[&str]) -> Result<Captured> {
        self.run_command(self.command(args))
    }

    // Child operation modules use this for per-command environment changes.
    fn run_command(&self, command: Command) -> Result<Captured> {
        capture(command, self.timeout, &self.cancellation)
            .with_context(|| format!("Git in {}", self.root.display()))
    }
}

pub fn validate_remote_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            && !name.contains(".."),
        "Remote names must start with a letter or digit and contain only letters, digits, '.', '_' or '-' (no '..')"
    );
    Ok(())
}

pub fn validate_remote_url(url: &str) -> Result<()> {
    ensure!(
        !url.is_empty()
            && url.len() <= 4096
            && !url.starts_with('-')
            && url.trim() == url
            && !url.chars().any(char::is_control),
        "Enter a nonempty remote URL or local path without leading options or control characters"
    );
    if url.starts_with('/') || url.starts_with("./") || url.starts_with("../") {
        return Ok(());
    }
    ensure!(
        !url.chars().any(char::is_whitespace) && !url.contains("::"),
        "Remote URLs cannot contain whitespace or remote-helper syntax"
    );
    if let Some((scheme, rest)) = url.split_once("://") {
        ensure!(
            ["https", "http", "ssh", "git", "file"].contains(&scheme)
                && !rest.is_empty()
                && !rest.starts_with('-'),
            "Use an HTTPS, SSH, Git, file URL, or an absolute local path"
        );
    } else if let Some((host, path)) = url.split_once(':') {
        ensure!(
            !host.is_empty()
                && !host.starts_with('-')
                && !host.contains('/')
                && !path.is_empty()
                && !path.starts_with('-'),
            "Use host:path or user@host:path for SSH remotes"
        );
    } else {
        bail!("Use an HTTPS/SSH URL or an absolute local repository path");
    }
    Ok(())
}

fn validate_identity(name: &str, email: &str) -> Result<()> {
    for (label, value) in [("Author name", name), ("Author email", email)] {
        ensure!(
            !value.trim().is_empty()
                && value.len() <= 1024
                && value.trim() == value
                && !value.chars().any(char::is_control)
                && !value.contains(['<', '>']),
            "{label} must be nonempty, without surrounding whitespace, control characters or angle brackets"
        );
    }
    ensure!(
        email.contains('@') && !email.chars().any(char::is_whitespace),
        "Enter an author email address containing '@' and no spaces"
    );
    Ok(())
}

fn lines(bytes: &[u8]) -> Result<Vec<String>> {
    Ok(std::str::from_utf8(bytes)?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn nonblocking(stream: &impl AsRawFd) -> io::Result<()> {
    let fd = stream.as_raw_fd();
    // SAFETY: fd belongs to a live pipe and fcntl does not retain it.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags == -1 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn drain(
    stream: &mut impl Read,
    kept: &mut Vec<u8>,
    limit: usize,
    truncated: &mut bool,
) -> io::Result<bool> {
    let mut buffer = [0u8; 8192];
    // Bound each polling iteration even if a child writes continuously.
    for _ in 0..32 {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                let retain = n.min(limit.saturating_sub(kept.len()));
                kept.extend_from_slice(&buffer[..retain]);
                *truncated |= retain != n;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

fn capture(
    command: Command,
    timeout: Duration,
    cancellation: &OperationCancellation,
) -> Result<Captured> {
    capture_with_limit(command, timeout, cancellation, OUTPUT_LIMIT)
}

fn capture_with_limit(
    mut command: Command,
    timeout: Duration,
    cancellation: &OperationCancellation,
    stdout_limit: usize,
) -> Result<Captured> {
    // Register the Child under the same lock used by shutdown. Cancellation can
    // never fall into a gap between checking a flag and starting a write.
    let (mut stdout, mut stderr) = {
        let mut state = cancellation
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        ensure!(
            !state.cancelled,
            "Git operation cancelled; refresh repository state before retrying"
        );
        ensure!(
            state.child.is_none(),
            "Another command is already running in this Git job"
        );
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .context("Cannot launch Git; install Git and ensure it is on PATH")?;
        let stdout = child.stdout.take().context("Git stdout must be piped")?;
        let stderr = child.stderr.take().context("Git stderr must be piped")?;
        state.child = Some(child);
        (stdout, stderr)
    };
    let result = (|| {
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let mut output = Captured {
            stdout: vec![],
            stderr: vec![],
            code: None,
            truncated: false,
        };
        let deadline = Instant::now() + timeout;
        loop {
            let out_done = drain(
                &mut stdout,
                &mut output.stdout,
                stdout_limit,
                &mut output.truncated,
            )?;
            let err_done = drain(
                &mut stderr,
                &mut output.stderr,
                ERROR_LIMIT,
                &mut output.truncated,
            )?;
            let mut state = cancellation
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            ensure!(
                !state.cancelled,
                "Git operation cancelled; refresh repository state before retrying because it may have partially completed"
            );
            if let Some(status) = state
                .child
                .as_mut()
                .context("Git process is no longer owned by this job")?
                .try_wait()?
                && out_done
                && err_done
            {
                output.code = status.code();
                state.child.take();
                return Ok(output);
            }
            drop(state);
            ensure!(
                Instant::now() < deadline,
                "Git timed out after {} seconds. Check connectivity and your credential helper/SSH agent, then refresh repository state before retrying; the operation may have partially completed.",
                timeout.as_secs()
            );
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        cancellation.cancel();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        temp: TempDir,
        root: PathBuf,
        global: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("repository with spaces");
            let global = temp.path().join("isolated.gitconfig");
            fs::write(&global, "[user]\n name = Global Test\n email = global@example.test\n[credential]\n helper =\n").unwrap();
            fs::create_dir(&root).unwrap();
            let this = Self { temp, root, global };
            this.git(&["init", "-b", "main"]);
            this.git(&["config", "--local", "user.name", "Fixture"]);
            this.git(&["config", "--local", "user.email", "fixture@example.test"]);
            this.commit("tracked.txt", "initial\n");
            this
        }

        fn command(&self, root: &Path) -> Command {
            let mut command = Command::new("git");
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(key);
                }
            }
            command
                .env("GIT_CONFIG_GLOBAL", &self.global)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_CONFIG_COUNT", "0")
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgSign=false",
                    "-C",
                ])
                .arg(root);
            command
        }

        fn at(&self, root: &Path, args: &[&str]) -> String {
            let output = self.command(root).args(args).output().unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        fn git(&self, args: &[&str]) -> String {
            self.at(&self.root, args)
        }

        fn op(&self) -> GitOperations {
            let mut op = GitOperations::open(&self.root).unwrap();
            op.global_config = Some(self.global.clone());
            op
        }

        fn commit(&self, path: &str, content: &str) {
            fs::write(self.root.join(path), content).unwrap();
            self.git(&["add", "--", path]);
            self.git(&["commit", "-m", "Fixture commit"]);
        }

        fn bare(&self) -> PathBuf {
            let path = self.temp.path().join("remote.git");
            self.git(&["init", "--bare", path.to_str().unwrap()]);
            path
        }

        fn add_origin(&self) -> PathBuf {
            let remote = self.bare();
            self.op()
                .execute(&Operation::AddRemote {
                    name: "origin".into(),
                    url: remote.to_str().unwrap().into(),
                })
                .unwrap();
            remote
        }
    }

    mod branch_integration {
        use super::*;
        include!("operations/integration/tests.rs");
    }

    mod branch_sync {
        use super::*;
        include!("operations/sync/tests.rs");
    }

    #[test]
    fn branch_create_checkout_delete_and_worktree_protection() {
        let f = Fixture::new();
        let op = f.op();
        op.execute(&Operation::CreateBranch {
            name: "feature/topic".into(),
        })
        .unwrap();
        assert_eq!(f.git(&["branch", "--show-current"]), "main");
        op.execute(&Operation::CheckoutBranch {
            name: "feature/topic".into(),
        })
        .unwrap();
        for mode in [DeleteMode::MergedOnly, DeleteMode::ForceUnmerged] {
            assert!(
                op.execute(&Operation::DeleteBranch {
                    name: "feature/topic".into(),
                    mode
                })
                .unwrap_err()
                .to_string()
                .contains("checked out")
            );
        }
        f.commit("feature.txt", "unmerged\n");
        op.execute(&Operation::CheckoutBranch {
            name: "main".into(),
        })
        .unwrap();
        assert!(
            op.execute(&Operation::DeleteBranch {
                name: "feature/topic".into(),
                mode: DeleteMode::MergedOnly
            })
            .is_err()
        );
        let linked = f.temp.path().join("other-worktree");
        f.git(&["worktree", "add", linked.to_str().unwrap(), "feature/topic"]);
        assert!(
            op.execute(&Operation::DeleteBranch {
                name: "feature/topic".into(),
                mode: DeleteMode::ForceUnmerged
            })
            .is_err()
        );
        assert!(
            op.execute(&Operation::CheckoutBranch {
                name: "feature/topic".into()
            })
            .is_err()
        );
        f.git(&["worktree", "remove", linked.to_str().unwrap()]);
        op.execute(&Operation::DeleteBranch {
            name: "feature/topic".into(),
            mode: DeleteMode::ForceUnmerged,
        })
        .unwrap();
        op.execute(&Operation::CreateBranch {
            name: "merged".into(),
        })
        .unwrap();
        op.execute(&Operation::DeleteBranch {
            name: "merged".into(),
            mode: DeleteMode::MergedOnly,
        })
        .unwrap();
        assert_eq!(op.state().unwrap().local_branches, ["main"]);
    }

    #[test]
    fn checkout_protects_dirty_tracked_untracked_and_ignored_files() {
        let f = Fixture::new();
        let op = f.op();
        op.execute(&Operation::CreateBranch {
            name: "topic".into(),
        })
        .unwrap();
        op.execute(&Operation::CheckoutBranch {
            name: "topic".into(),
        })
        .unwrap();
        f.commit("tracked.txt", "topic content\n");
        f.commit("collision.txt", "topic collision\n");
        op.execute(&Operation::CheckoutBranch {
            name: "main".into(),
        })
        .unwrap();
        fs::write(f.root.join("tracked.txt"), "unsaved work\n").unwrap();
        assert!(
            op.execute(&Operation::CheckoutBranch {
                name: "topic".into()
            })
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(f.root.join("tracked.txt")).unwrap(),
            "unsaved work\n"
        );
        fs::write(f.root.join("tracked.txt"), "initial\n").unwrap();
        fs::write(f.root.join("collision.txt"), "untracked work\n").unwrap();
        assert!(
            op.execute(&Operation::CheckoutBranch {
                name: "topic".into()
            })
            .is_err()
        );
        fs::write(f.root.join(".git/info/exclude"), "collision.txt\n").unwrap();
        assert!(
            op.execute(&Operation::CheckoutBranch {
                name: "topic".into()
            })
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(f.root.join("collision.txt")).unwrap(),
            "untracked work\n"
        );
        assert_eq!(f.git(&["branch", "--show-current"]), "main");
    }

    #[test]
    fn push_is_one_named_branch_even_with_dangerous_defaults() {
        let f = Fixture::new();
        let remote = f.add_origin();
        let op = f.op();
        op.execute(&Operation::CreateBranch {
            name: "not-pushed".into(),
        })
        .unwrap();
        f.git(&["config", "remote.origin.mirror", "true"]);
        f.git(&["config", "remote.origin.push", "+refs/heads/*:refs/heads/*"]);
        f.git(&["config", "push.followTags", "true"]);
        f.git(&["tag", "-a", "do-not-push", "-m", "Fixture tag"]);
        op.execute(&Operation::Push {
            remote: "origin".into(),
            branch: "main".into(),
        })
        .unwrap();
        assert_eq!(
            f.at(&remote, &["for-each-ref", "--format=%(refname)"]),
            "refs/heads/main"
        );
        assert!(
            op.execute(&Operation::Push {
                remote: "origin".into(),
                branch: "HEAD".into()
            })
            .is_err()
        );
        // Remote advances, local diverges: explicit push must not force it back.
        let clone = f.temp.path().join("peer");
        f.git(&[
            "clone",
            "--branch",
            "main",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ]);
        fs::write(clone.join("peer.txt"), "peer\n").unwrap();
        f.at(&clone, &["add", "--", "peer.txt"]);
        f.at(&clone, &["commit", "-m", "peer"]);
        f.at(&clone, &["push", "origin", "main"]);
        let remote_head = f.at(&remote, &["rev-parse", "main"]);
        f.commit("local.txt", "local\n");
        assert!(
            op.execute(&Operation::Push {
                remote: "origin".into(),
                branch: "main".into()
            })
            .is_err()
        );
        assert_eq!(f.at(&remote, &["rev-parse", "main"]), remote_head);
    }

    #[test]
    fn pull_fast_forwards_but_refuses_divergence_dirty_and_stale_branch() {
        let f = Fixture::new();
        let remote = f.add_origin();
        let op = f.op();
        op.execute(&Operation::Push {
            remote: "origin".into(),
            branch: "main".into(),
        })
        .unwrap();
        let peer = f.temp.path().join("peer");
        f.git(&[
            "clone",
            "--branch",
            "main",
            remote.to_str().unwrap(),
            peer.to_str().unwrap(),
        ]);
        fs::write(peer.join("peer.txt"), "peer one\n").unwrap();
        f.at(&peer, &["add", "--", "peer.txt"]);
        f.at(&peer, &["commit", "-m", "peer"]);
        f.at(&peer, &["push", "origin", "main"]);
        f.git(&["config", "pull.rebase", "true"]);
        f.git(&["config", "merge.autostash", "true"]);
        let pull = Operation::Pull {
            remote: "origin".into(),
            remote_branch: "main".into(),
            local_branch: "main".into(),
        };
        op.execute(&pull).unwrap();
        assert_eq!(
            f.git(&["rev-parse", "HEAD"]),
            f.at(&peer, &["rev-parse", "HEAD"])
        );
        fs::write(f.root.join("tracked.txt"), "dirty\n").unwrap();
        assert!(
            op.execute(&pull)
                .unwrap_err()
                .to_string()
                .contains("clean worktree")
        );
        assert_eq!(f.git(&["stash", "list"]), "");
        fs::write(f.root.join("tracked.txt"), "initial\n").unwrap();
        f.commit("local.txt", "local divergence\n");
        fs::write(peer.join("peer.txt"), "peer two\n").unwrap();
        f.at(&peer, &["commit", "-am", "peer two"]);
        f.at(&peer, &["push", "origin", "main"]);
        let before = f.git(&["rev-parse", "HEAD"]);
        assert!(op.execute(&pull).is_err());
        assert_eq!(f.git(&["rev-parse", "HEAD"]), before);
        assert!(!f.root.join(".git/MERGE_HEAD").exists());
        op.execute(&Operation::CreateBranch {
            name: "topic".into(),
        })
        .unwrap();
        op.execute(&Operation::CheckoutBranch {
            name: "topic".into(),
        })
        .unwrap();
        assert!(
            op.execute(&pull)
                .unwrap_err()
                .to_string()
                .contains("checked-out branch changed")
        );
    }

    #[test]
    fn fetch_pruning_is_explicit_and_custom_refspec_cannot_change_local_branch() {
        let f = Fixture::new();
        f.add_origin();
        let op = f.op();
        op.execute(&Operation::Push {
            remote: "origin".into(),
            branch: "main".into(),
        })
        .unwrap();
        f.git(&["config", "fetch.prune", "true"]);
        f.git(&[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/heads/*",
        ]);
        f.git(&["update-ref", "refs/remotes/origin/stale", "HEAD"]);
        op.execute(&Operation::Fetch {
            remote: "origin".into(),
            prune: false,
        })
        .unwrap();
        assert!(
            !f.git(&["rev-parse", "refs/remotes/origin/stale"])
                .is_empty()
        );
        assert!(!f.git(&["rev-parse", "refs/remotes/origin/main"]).is_empty());
        op.execute(&Operation::Fetch {
            remote: "origin".into(),
            prune: true,
        })
        .unwrap();
        assert_eq!(
            f.git(&["for-each-ref", "--format=%(refname)", "refs/remotes/"]),
            "refs/remotes/origin/main"
        );
        assert_eq!(f.git(&["branch", "--show-current"]), "main");
    }

    #[test]
    fn remote_management_and_local_identity_preserve_unrelated_config() {
        let f = Fixture::new();
        let remote = f.bare();
        let op = f.op();
        f.git(&["config", "credential.helper", "test-helper"]);
        f.git(&["config", "core.sshCommand", "custom-ssh -i custom-key"]);
        f.git(&["config", "custom.untouched", "keep me"]);
        let global_before = fs::read(&f.global).unwrap();
        op.execute(&Operation::AddRemote {
            name: "origin".into(),
            url: remote.to_str().unwrap().into(),
        })
        .unwrap();
        op.execute(&Operation::SetRemoteUrl {
            name: "origin".into(),
            url: "https://example.test/new.git".into(),
            kind: RemoteUrlKind::Fetch,
        })
        .unwrap();
        op.execute(&Operation::SetRemoteUrl {
            name: "origin".into(),
            url: "git@example.test:new.git".into(),
            kind: RemoteUrlKind::Push,
        })
        .unwrap();
        op.execute(&Operation::RenameRemote {
            name: "origin".into(),
            new_name: "upstream".into(),
        })
        .unwrap();
        let remotes = op.remotes().unwrap();
        assert_eq!(remotes[0].name, "upstream");
        assert_eq!(remotes[0].fetch_urls, ["https://example.test/new.git"]);
        assert_eq!(remotes[0].push_urls, ["git@example.test:new.git"]);
        op.execute(&Operation::SetIdentity {
            name: "New Author".into(),
            email: "new@example.test".into(),
        })
        .unwrap();
        let state = op.state().unwrap();
        assert_eq!(state.local_identity.name.as_deref(), Some("New Author"));
        assert_eq!(
            state.effective_identity.email.as_deref(),
            Some("new@example.test")
        );
        assert_eq!(f.git(&["config", "credential.helper"]), "test-helper");
        assert_eq!(
            f.git(&["config", "core.sshCommand"]),
            "custom-ssh -i custom-key"
        );
        assert_eq!(f.git(&["config", "custom.untouched"]), "keep me");
        assert_eq!(fs::read(&f.global).unwrap(), global_before);
        op.execute(&Operation::RemoveRemote {
            name: "upstream".into(),
        })
        .unwrap();
        assert!(op.remotes().unwrap().is_empty());
    }

    #[test]
    fn invalid_names_and_argv_injection_do_not_mutate_config_or_execute() {
        let f = Fixture::new();
        let op = f.op();
        let before = fs::read(f.root.join(".git/config")).unwrap();
        for name in [
            "",
            "-D",
            "--help",
            "HEAD",
            "@{-1}",
            "bad..name",
            "bad name",
            "x\ny",
        ] {
            assert!(
                op.execute(&Operation::CreateBranch { name: name.into() })
                    .is_err(),
                "{name}"
            );
        }
        for name in ["", "-x", "a/b", "a\nb", "x;touch", "a..b"] {
            assert!(
                op.execute(&Operation::AddRemote {
                    name: name.into(),
                    url: "/tmp/repo".into()
                })
                .is_err()
            );
        }
        for url in [
            "",
            "--upload-pack=evil",
            "ext::touch x",
            "evil::anything",
            "https://x\nfoo",
            "ssh://",
            "relative-path",
        ] {
            assert!(
                op.execute(&Operation::AddRemote {
                    name: "origin".into(),
                    url: url.into()
                })
                .is_err(),
                "{url}"
            );
        }
        assert!(
            op.execute(&Operation::SetIdentity {
                name: "New\n[core]".into(),
                email: "bad".into()
            })
            .is_err()
        );
        assert_eq!(fs::read(f.root.join(".git/config")).unwrap(), before);
        // Valid shell punctuation remains literal argv, never a command.
        op.execute(&Operation::CreateBranch {
            name: "literal;echo-pwn".into(),
        })
        .unwrap();
        assert!(
            op.state()
                .unwrap()
                .local_branches
                .contains(&"literal;echo-pwn".into())
        );
        assert!(!f.root.join("pwn").exists());
    }

    #[test]
    fn linked_worktree_identity_is_repository_local() {
        let f = Fixture::new();
        let linked = f.temp.path().join("linked");
        f.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
        let mut op = GitOperations::open(&linked).unwrap();
        op.global_config = Some(f.global.clone());
        op.execute(&Operation::SetIdentity {
            name: "Shared author".into(),
            email: "shared@example.test".into(),
        })
        .unwrap();
        assert_eq!(
            f.op().state().unwrap().local_identity.name.as_deref(),
            Some("Shared author")
        );
        assert_eq!(
            op.state().unwrap().current_branch.as_deref(),
            Some("linked")
        );
    }

    #[test]
    fn process_output_is_bounded_and_deadline_kills_helpers() {
        // Self-authored subprocesses, no real remote or credential invocation.
        let mut output = Command::new("/usr/bin/python3");
        output
            .args([
                "-c",
                "import sys; sys.stdout.write('x'*400000); sys.stderr.write('y'*100000)",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .process_group(0);
        let result = capture(
            output,
            Duration::from_secs(10),
            &OperationCancellation::default(),
        )
        .unwrap();
        assert!(result.truncated);
        assert_eq!(result.stdout.len(), OUTPUT_LIMIT);
        assert_eq!(result.stderr.len(), ERROR_LIMIT);
        let mut slow = Command::new("/usr/bin/python3");
        slow.args([
            "-c",
            "import subprocess, time; subprocess.Popen(['/bin/sleep', '30']); time.sleep(30)",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .process_group(0);
        let start = Instant::now();
        assert!(
            capture(
                slow,
                Duration::from_millis(150),
                &OperationCancellation::default()
            )
            .unwrap_err()
            .to_string()
            .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn cancellation_reaps_owned_child_and_prevents_followup_writes() {
        let f = Fixture::new();
        let token = OperationCancellation::default();
        let mut unrelated = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command
            .args([
                "-c",
                "import subprocess, time; subprocess.Popen(['/bin/sleep', '30']); time.sleep(30)",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        let worker_token = token.clone();
        let worker =
            thread::spawn(move || capture(command, Duration::from_secs(30), &worker_token));
        let deadline = Instant::now() + Duration::from_secs(3);
        while token.state.lock().unwrap().child.is_none() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        token.cancel();
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(token.state.lock().unwrap().child.is_none());
        assert!(
            unrelated.try_wait().unwrap().is_none(),
            "unrelated process must remain alive"
        );
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        let op = f.op().with_cancellation(token);
        assert!(
            op.execute(&Operation::CreateBranch {
                name: "must-not-exist".into()
            })
            .is_err()
        );
        assert_eq!(
            f.git(&["for-each-ref", "--format=%(refname)", "refs/heads/"]),
            "refs/heads/main"
        );
    }

    #[test]
    fn explicit_operations_honor_configured_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let hooks = f.temp.path().join("authored-hooks");
        fs::create_dir(&hooks).unwrap();
        let checkout = hooks.join("post-checkout");
        fs::write(&checkout, "#!/bin/sh\nprintf 'ran' > checkout-hook-ran\n").unwrap();
        fs::set_permissions(&checkout, fs::Permissions::from_mode(0o755)).unwrap();
        f.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
        let op = f.op();
        op.execute(&Operation::CreateBranch {
            name: "hooks".into(),
        })
        .unwrap();
        op.execute(&Operation::CheckoutBranch {
            name: "hooks".into(),
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(f.root.join("checkout-hook-ran")).unwrap(),
            "ran"
        );
        let remote = f.add_origin();
        let push = hooks.join("pre-push");
        fs::write(
            &push,
            "#!/bin/sh\nprintf 'Fixture push policy rejected this push' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&push, fs::Permissions::from_mode(0o755)).unwrap();
        let error = op
            .execute(&Operation::Push {
                remote: "origin".into(),
                branch: "hooks".into(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("Fixture push policy"));
        assert_eq!(f.at(&remote, &["for-each-ref", "--format=%(refname)"]), "");
    }

    mod worktree_management {
        use super::*;
        include!("operations/worktrees/tests.rs");
    }

    #[test]
    fn command_preserves_ssh_and_does_not_rewrite_home() {
        let f = Fixture::new();
        let command = f.op().command(&["status"]);
        let env = command.get_envs().collect::<Vec<_>>();
        assert!(!env.iter().any(|(key, _)| *key == "HOME"
            || *key == "GIT_SSH_COMMAND"
            || *key == "SSH_AUTH_SOCK"));
        assert!(env.iter().any(|(key, value)| *key == "GIT_TERMINAL_PROMPT"
            && value == &Some(std::ffi::OsStr::new("0"))));
    }
}
