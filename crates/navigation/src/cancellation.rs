use anyhow::{Result, bail};
use std::{
    io,
    process::{Child, Command, ExitStatus},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

/// Clonable, thread-safe, one-way cancellation for one Navigator instance.
/// Obtain before placing the Navigator behind an application mutex. Cancellation
/// signals its owned processes without taking that mutex or waiting for LSP I/O.
#[derive(Clone, Default)]
pub struct CancellationHandle {
    inner: Arc<CancellationState>,
}

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    processes: Mutex<Vec<Weak<OwnedProcess>>>,
}

impl CancellationHandle {
    /// Permanently cancel this Navigator and promptly signal all owned servers.
    /// On Unix this kills their process groups, including ordinary descendants.
    /// Does not wait for process exit or join I/O threads. Check the result for OS
    /// signaling failures. Safe to call concurrently, repeatedly, or after Drop.
    pub fn cancel(&self) -> Result<()> {
        self.inner.cancelled.store(true, Ordering::Release);
        let processes = self
            .inner
            .processes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut errors = Vec::new();
        // Registration and spawn are serialized with this short lifecycle lock.
        // It is never held while waiting for a server response or process exit.
        for process in processes.iter().filter_map(Weak::upgrade) {
            if let Err(error) = process.terminate() {
                errors.push(error.to_string());
            }
            let process = Arc::clone(&process);
            std::thread::spawn(move || process.reap());
        }
        if !errors.is_empty() {
            bail!("Language server cancellation failed: {}", errors.join("; "));
        }
        Ok(())
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            bail!("Navigation cancelled");
        }
        Ok(())
    }

    pub(crate) fn spawn(&self, command: &mut Command) -> Result<Arc<OwnedProcess>> {
        let mut processes = self
            .inner
            .processes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.check()?;
        let child = command.spawn()?;
        let process = Arc::new(OwnedProcess(Mutex::new(ProcessState {
            child: Some(child),
            signal_allowed: true,
            exit_status: None,
            signal_error: None,
        })));
        processes.retain(|entry| entry.strong_count() != 0);
        processes.push(Arc::downgrade(&process));
        // cancel() may have set its flag while the OS was spawning the process.
        // Terminate before releasing registration, so no spawned child escapes.
        if self.is_cancelled() {
            process.terminate()?;
            self.check()?;
        }
        Ok(process)
    }
}

struct ProcessState {
    child: Option<Child>,
    // Must become false BEFORE any wait/try_wait that can reap the child.
    signal_allowed: bool,
    exit_status: Option<ProcessExit>,
    signal_error: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum ProcessExit {
    Code(i32),
    #[cfg(unix)]
    Signal(i32),
    OwnershipLost,
}

impl From<ExitStatus> for ProcessExit {
    fn from(status: ExitStatus) -> Self {
        #[cfg(unix)]
        if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
            return Self::Signal(signal);
        }
        Self::Code(status.code().expect("exited child has an exit code"))
    }
}

impl std::fmt::Display for ProcessExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code(code) => write!(f, "exit code {code}"),
            #[cfg(unix)]
            Self::Signal(signal) => write!(f, "signal {signal}"),
            Self::OwnershipLost => f.write_str("process ownership was lost; refusing PID signals"),
        }
    }
}

pub(crate) struct OwnedProcess(Mutex<ProcessState>);

impl OwnedProcess {
    pub fn take_stdio(
        &self,
    ) -> Result<(
        std::process::ChildStdin,
        std::process::ChildStdout,
        std::process::ChildStderr,
    )> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let child = state
            .child
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Language server is stopped"))?;
        Ok((
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("Missing server stdin"))?,
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Missing server stdout"))?,
            child
                .stderr
                .take()
                .ok_or_else(|| anyhow::anyhow!("Missing server stderr"))?,
        ))
    }

    pub fn exit_status(&self) -> Option<ProcessExit> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(status) = state.exit_status {
            return Some(status);
        }
        let child = state.child.as_mut()?;
        match observe_exit(child) {
            Ok(status) => {
                state.exit_status = status;
                status
            }
            Err(error) if error.raw_os_error() == Some(no_child_error()) => {
                // An external reaper took our child. Numeric PID ownership can no
                // longer be established, so do not signal it or its old group.
                state.signal_allowed = false;
                state.child.take();
                state.exit_status = Some(ProcessExit::OwnershipLost);
                state.exit_status
            }
            Err(_) => None,
        }
    }

    pub fn terminate(&self) -> io::Result<()> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !state.signal_allowed {
            return state
                .signal_error
                .as_ref()
                .map_or(Ok(()), |error| Err(io::Error::other(error.clone())));
        }
        let Some(child) = state.child.as_mut() else {
            return Ok(());
        };
        let observed = match observe_exit(child) {
            Ok(status) => status,
            Err(error) => {
                if error.raw_os_error() == Some(no_child_error()) {
                    state.signal_allowed = false;
                    state.child.take();
                }
                return Err(error);
            }
        };
        state.signal_allowed = false;
        state.exit_status = observed;
        let child = state.child.as_mut().unwrap();
        let result = (|| {
            #[cfg(unix)]
            {
                // SAFETY: Command creates a fresh process group led by this child.
                // WNOWAIT reserves its PID until after this last group signal.
                let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                if result != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
            }
            // Also handles a direct child which left its original process group.
            // It is still ours and unreaped here; never signal a cached bare PID.
            if observed.is_none()
                && let Err(error) = child.kill()
            {
                // SIGKILL to the group may already have finished the child.
                #[cfg(unix)]
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
                #[cfg(not(unix))]
                if child.try_wait()?.is_none() {
                    return Err(error);
                }
            }
            Ok(())
        })();
        state.signal_error = result.as_ref().err().map(ToString::to_string);
        result
    }

    pub fn reap(&self) {
        let child = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if state.signal_allowed || (state.signal_error.is_some() && state.exit_status.is_none())
            {
                return;
            }
            state.child.take()
        };
        if let Some(mut child) = child {
            let status = child.wait().ok().map(ProcessExit::from);
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if status.is_some() {
                state.exit_status = status;
            }
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.terminate();
        let state = self.0.get_mut().unwrap_or_else(|e| e.into_inner());
        if !state.signal_allowed && (state.signal_error.is_none() || state.exit_status.is_some()) {
            // The last strong reference may be the UI cancellation call racing a
            // Transport drop. Never make that caller wait for OS process exit.
            if let Some(mut child) = state.child.take() {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
    }
}

#[cfg(unix)]
fn no_child_error() -> i32 {
    libc::ECHILD
}
#[cfg(not(unix))]
fn no_child_error() -> i32 {
    -1
}

/// Observe without reaping: retaining the leader prevents its PID/PGID from
/// being recycled between status(), cancellation, and ordinary shutdown.
#[cfg(unix)]
fn observe_exit(child: &mut Child) -> io::Result<Option<ProcessExit>> {
    // SAFETY: siginfo_t is an output-only C POD; waitid initializes its fields.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = loop {
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break result;
        }
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: waitid succeeded with WEXITED, so these child-status fields apply.
    if unsafe { info.si_pid() } == 0 {
        return Ok(None);
    }
    let status = unsafe { info.si_status() };
    Ok(Some(if info.si_code == libc::CLD_EXITED {
        ProcessExit::Code(status)
    } else {
        ProcessExit::Signal(status)
    }))
}

#[cfg(not(unix))]
fn observe_exit(child: &mut Child) -> io::Result<Option<ProcessExit>> {
    // On Windows Child owns a kernel process handle, rather than a reusable PID.
    child.try_wait().map(|status| status.map(ProcessExit::from))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        os::unix::process::CommandExt,
        time::{Duration, Instant},
    };

    #[test]
    fn externally_reaped_child_revokes_pid_signals() {
        let handle = CancellationHandle::default();
        let mut command = Command::new("/usr/bin/true");
        command.process_group(0);
        let process = handle.spawn(&mut command).unwrap();
        let pid = process.0.lock().unwrap().child.as_ref().unwrap().id() as libc::pid_t;
        let deadline = Instant::now() + Duration::from_secs(2);
        while process.exit_status().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        // Simulate a host SIGCHLD reaper taking ownership outside this crate.
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            pid
        );
        assert!(
            handle
                .cancel()
                .unwrap_err()
                .to_string()
                .contains("cancellation failed")
        );
        let state = process.0.lock().unwrap();
        assert!(!state.signal_allowed);
        assert!(state.child.is_none());
        drop(state);
        // Nothing retains a signalable PID after the failed ownership check.
        handle.cancel().unwrap();
    }
}
