//! Native PTY and Alacritty screen owned by each explicitly opened terminal tab.
mod input;
mod panel;
mod screen;

pub use panel::{DockAction, TerminalDock, bind_keys};

use alacritty_terminal::{
    Term,
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    sync::FairMutex,
    term::{Config, TermMode},
    tty,
};
use anyhow::{Context, Result, ensure};
use smol::channel::{Receiver, Sender};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Copy, Clone)]
struct GridSize(WindowSize);
impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
    fn screen_lines(&self) -> usize {
        self.0.num_lines as usize
    }
    fn columns(&self) -> usize {
        self.0.num_cols as usize
    }
}

struct Events {
    io: OnceLock<EventLoopSender>,
    wake: Sender<()>,
    exited: AtomicBool,
    status: Mutex<Option<std::process::ExitStatus>>,
    size: Mutex<WindowSize>,
}
#[derive(Clone)]
struct Listener(Arc<Events>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let reply = match event {
            Event::PtyWrite(bytes) => Some(bytes),
            Event::TextAreaSizeRequest(format) => Some(format(*self.0.size.lock().unwrap())),
            Event::ColorRequest(index, format) => Some(format(screen::default_color(index))),
            Event::ChildExit(status) => {
                *self.0.status.lock().unwrap() = Some(status);
                self.0.exited.store(true, Ordering::Release);
                let _ = self.0.wake.try_send(());
                None
            }
            Event::Exit => {
                self.0.exited.store(true, Ordering::Release);
                let _ = self.0.wake.try_send(());
                None
            }
            Event::Wakeup => {
                let _ = self.0.wake.try_send(());
                None
            }
            _ => None,
        };
        if let (Some(reply), Some(io)) = (reply, self.0.io.get()) {
            let _ = io.send(Msg::Input(reply.into_bytes().into()));
        }
    }
}

struct Shell {
    master: Arc<std::fs::File>,
    leader: rustix::process::Pid,
    term: Arc<FairMutex<Term<Listener>>>,
    listener: Listener,
    io: EventLoopSender,
}
impl Shell {
    fn open(root: &Path, extra_env: HashMap<String, String>) -> Result<(Self, Receiver<()>)> {
        ensure!(
            root.is_absolute() && root.is_dir(),
            "워크트리 폴더를 찾을 수 없습니다"
        );
        let (wake, rx) = smol::channel::bounded(1);
        let size = WindowSize {
            num_lines: 12,
            num_cols: 80,
            cell_width: 8,
            cell_height: 17,
        };
        let listener = Listener(Arc::new(Events {
            io: OnceLock::new(),
            wake,
            exited: AtomicBool::new(false),
            status: Mutex::new(None),
            size: Mutex::new(size),
        }));
        let mut env = HashMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("SHELL".into(), "/bin/zsh".into()),
            ("TERM_PROGRAM".into(), "Forkarak".into()),
            (
                "TERM_PROGRAM_VERSION".into(),
                env!("CARGO_PKG_VERSION").into(),
            ),
        ]);
        env.extend(extra_env);
        let options = tty::Options {
            shell: Some(tty::Shell::new("/bin/zsh".into(), vec!["-il".into()])),
            working_directory: Some(root.to_owned()),
            drain_on_exit: true,
            env,
        };
        let pty =
            tty::new(&options, size, 0).context("워크트리 PTY에서 zsh를 시작할 수 없습니다")?;
        let master = Arc::new(pty.file().try_clone()?);
        let leader = rustix::process::Pid::from_child(pty.child());
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &GridSize(size),
            listener.clone(),
        )));
        let events = EventLoop::new(term.clone(), listener.clone(), pty, true, false)?;
        let io = events.channel();
        let _ = listener.0.io.set(io.clone());
        // The I/O thread owns, hangs up, and reaps its PTY child, including on shutdown.
        drop(events.spawn());
        Ok((
            Self {
                term,
                listener,
                io,
                master,
                leader,
            },
            rx,
        ))
    }
    fn exited(&self) -> bool {
        self.listener.0.exited.load(Ordering::Acquire)
    }
    fn exit_status(&self) -> Option<std::process::ExitStatus> {
        *self.listener.0.status.lock().unwrap()
    }
    fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() || self.exited() {
            return;
        }
        {
            let mut term = self.term.lock();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
            }
            term.selection = None;
        }
        let _ = self.io.send(Msg::Input(bytes.into()));
    }
    fn resize(&self, size: WindowSize) {
        let mut old = self.listener.0.size.lock().unwrap();
        if (old.num_cols, old.num_lines, old.cell_width, old.cell_height)
            == (
                size.num_cols,
                size.num_lines,
                size.cell_width,
                size.cell_height,
            )
        {
            return;
        }
        *old = size;
        drop(old);
        self.term.lock().resize(GridSize(size));
        let _ = self.io.send(Msg::Resize(size));
    }
    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }
    fn paste(&self, text: &str) {
        if self.mode().contains(TermMode::BRACKETED_PASTE) {
            let clean = text.replace(['\x1b', '\u{0080}', '\u{009b}'], "");
            self.write(format!("\x1b[200~{clean}\x1b[201~").into_bytes());
        } else {
            self.write(text.replace("\r\n", "\n").replace('\n', "\r").into_bytes());
        }
    }
}

/// Also retain closing PTYs: quitting immediately after closing a tab must not
/// abandon the asynchronous graceful teardown already in progress.
static CLOSING: OnceLock<Mutex<Vec<Shutdown>>> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct Shutdown {
    state: Arc<Events>,
    master: Arc<std::fs::File>,
    leader: rustix::process::Pid,
}
impl Shutdown {
    fn for_shell(shell: &Shell) -> Self {
        Self {
            state: shell.listener.0.clone(),
            master: shell.master.clone(),
            leader: shell.leader,
        }
    }

    fn force(&self) {
        use rustix::{
            process::{Signal, getsid, kill_process_group},
            termios::tcgetpgrp,
        };
        if self.state.exited.load(Ordering::Acquire) {
            return;
        }
        // GPUI grants quit handlers 200ms. Finish owned foreground jobs before
        // the UI process exits; never signal a group from another PTY session.
        if let Ok(group) = tcgetpgrp(&*self.master)
            && group != self.leader
            && getsid(Some(group)) == Ok(self.leader)
        {
            let _ = kill_process_group(group, Signal::KILL);
        }
        if getsid(Some(self.leader)) == Ok(self.leader) {
            let _ = kill_process_group(self.leader, Signal::KILL);
        }
    }

    pub(crate) async fn wait(self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !self.state.exited.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            smol::Timer::after(std::time::Duration::from_millis(10)).await;
        }
    }
}

pub(crate) fn finish_pending_shutdowns() -> Vec<Shutdown> {
    let pending = std::mem::take(&mut *CLOSING.get_or_init(Default::default).lock().unwrap());
    for shutdown in &pending {
        shutdown.force();
    }
    pending
}

impl Drop for Shell {
    fn drop(&mut self) {
        use rustix::{
            process::{Signal, getsid, kill_process_group},
            termios::tcgetpgrp,
        };
        use std::{thread, time::Duration};
        if self.exited() {
            return;
        }
        {
            let mut pending = CLOSING.get_or_init(Default::default).lock().unwrap();
            pending.retain(|shutdown| !shutdown.state.exited.load(Ordering::Acquire));
            pending.push(Shutdown::for_shell(self));
        }
        let master = self.master.clone();
        let leader = self.leader;
        let state = self.listener.0.clone();
        // Keep the I/O loop draining while jobs exit. Stopping the loop before
        // zsh flushes its last output can deadlock Darwin's PTY teardown.
        // Re-read the foreground group from this PTY before every signal. A
        // foreground program may hand the terminal back to zsh between steps.
        let _ = thread::Builder::new()
            .name("terminal-close".into())
            .spawn(move || {
                for signal in [Signal::HUP, Signal::TERM, Signal::KILL] {
                    if state.exited.load(Ordering::Acquire) {
                        return;
                    }
                    let Ok(group) = tcgetpgrp(&*master) else {
                        return;
                    };
                    if group != leader && getsid(Some(group)) == Ok(leader) {
                        let _ = kill_process_group(group, signal);
                        for _ in 0..12 {
                            thread::sleep(Duration::from_millis(20));
                            if state.exited.load(Ordering::Acquire) {
                                return;
                            }
                            if tcgetpgrp(&*master) != Ok(group) {
                                break;
                            }
                        }
                    }
                    if (tcgetpgrp(&*master) == Ok(leader) || signal == Signal::KILL)
                        && getsid(Some(leader)) == Ok(leader)
                    {
                        let _ = kill_process_group(leader, signal);
                    }
                    for _ in 0..12 {
                        thread::sleep(Duration::from_millis(20));
                        if state.exited.load(Ordering::Acquire) {
                            return;
                        }
                    }
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{Shell, WindowSize};
    use alacritty_terminal::{
        grid::Dimensions,
        term::TermMode,
        vte::ansi::{Color, Rgb},
    };
    use std::{
        collections::HashMap,
        fs,
        path::Path,
        process::Command,
        thread,
        time::{Duration, Instant},
    };

    fn wait_for(description: &str, test: impl Fn() -> bool) {
        let start = Instant::now();
        while !test() {
            assert!(start.elapsed() < Duration::from_secs(12), "{description}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn fixture(root: &Path) -> Shell {
        fixture_named(root, "shell")
    }
    fn fixture_named(root: &Path, name: &str) -> Shell {
        let dot = root.join(format!("dotfiles-{name}"));
        fs::create_dir(&dot).unwrap();
        fs::write(dot.join(".zprofile"), "export FIXTURE_PROFILE=login\n").unwrap();
        fs::write(dot.join(".zshrc"),"export FIXTURE_RC=interactive\nunsetopt BEEP\nPROMPT=''\nHISTFILE=/dev/null\nprint -r -- $$ > \"$FIXTURE_PID\"\n").unwrap();
        let pid_file = root.join(format!("{name}.pid"));
        let env = HashMap::from([
            ("ZDOTDIR".into(), dot.to_string_lossy().into_owned()),
            ("HOME".into(), dot.to_string_lossy().into_owned()),
            ("SESSION_VALUE".into(), "hello 꿀".into()),
            (
                "FIXTURE_PID".into(),
                pid_file.to_string_lossy().into_owned(),
            ),
        ]);
        let (shell, _rx) = Shell::open(root, env).unwrap();
        wait_for("zsh should load its interactive startup", || {
            pid_file.is_file()
        });
        shell
    }
    fn text(shell: &Shell) -> String {
        shell
            .term
            .lock()
            .grid()
            .display_iter()
            .map(|i| i.c)
            .collect()
    }
    fn running(pid: &str) -> bool {
        Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap()
            .status
            .success()
    }
    #[test]
    fn actual_zsh_and_full_screen_pty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo ' 한");
        fs::create_dir(&root).unwrap();
        let shell = fixture(&root);
        shell.write(b"print -r -- \"$PWD|$SESSION_VALUE|$TERM|$TERM_PROGRAM|$FIXTURE_PROFILE|$FIXTURE_RC\" > env.out; print -ru2 STDERR_FIXTURE\r".to_vec());
        wait_for("stderr should reach the terminal", || {
            text(&shell).contains("STDERR_FIXTURE") && root.join("env.out").is_file()
        });
        assert_eq!(
            fs::read_to_string(root.join("env.out")).unwrap().trim(),
            format!(
                "{}|hello 꿀|xterm-256color|Forkarak|login|interactive",
                root.display()
            )
        );
        fs::write(
            root.join("tui.py"),
            r#"import os, tty
tty.setraw(0)
os.write(1, b'\x1b[?1049h\x1b[2J\x1b[H\x1b[?2004h\x1b[?1h\x1b[38;2;32;80;160mTUI\x1b[3;9H\x1b[6n')
data=b''
while b'\x04' not in data:
    data+=os.read(0,4096)
with open('input.bin','wb') as f: f.write(data)
with open('size.out','w') as f: f.write(str(tuple(os.get_terminal_size())))
os.write(1,b'\x1b[?2004l\x1b[?1l\x1b[?1049l')
"#,
        )
        .unwrap();
        shell.write(b"/usr/bin/python3 tui.py\r".to_vec());
        wait_for("real CLI alternate screen and color", || {
            let term = shell.term.lock();
            term.mode()
                .contains(TermMode::ALT_SCREEN | TermMode::BRACKETED_PASTE | TermMode::APP_CURSOR)
                && term.grid().display_iter().any(|i| {
                    i.c == 'T'
                        && i.fg
                            == Color::Spec(Rgb {
                                r: 32,
                                g: 80,
                                b: 160,
                            })
                })
        });
        shell.resize(WindowSize {
            num_cols: 103,
            num_lines: 19,
            cell_width: 8,
            cell_height: 18,
        });
        assert_eq!(shell.term.lock().columns(), 103);
        shell.paste("한글\nsecond");
        shell.write(vec![4]);
        wait_for("TUI should return to shell", || {
            root.join("size.out").is_file() && !shell.mode().contains(TermMode::ALT_SCREEN)
        });
        let data = fs::read(root.join("input.bin")).unwrap();
        let input = String::from_utf8_lossy(&data);
        assert!(input.contains("\x1b[3;9R"), "cursor report {input:?}");
        assert!(
            input.contains("\x1b[200~한글\nsecond\x1b[201~"),
            "paste {input:?}"
        );
        assert_eq!(
            fs::read_to_string(root.join("size.out")).unwrap(),
            "(103, 19)"
        );
        shell.write(b"for n in {1..40}; do print -r -- history-$n; done\r".to_vec());
        wait_for("scrollback fixture output", || {
            text(&shell).contains("history-40")
        });
        shell
            .term
            .lock()
            .scroll_display(alacritty_terminal::grid::Scroll::Delta(28));
        assert!(text(&shell).contains("history-4"));
        shell.write(b"\r".to_vec());
        assert_eq!(shell.term.lock().grid().display_offset(), 0);
        shell.write(b"exit\r".to_vec());
        wait_for("shell exit", || shell.exited());
    }
    #[test]
    fn dropping_a_session_ends_its_shell_and_foreground_job() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let shell = fixture(&root);
        shell.write(
            b"/bin/sh -c 'trap \"\" HUP TERM; echo $$ > job.pid; exec /bin/sleep 45'\r".to_vec(),
        );
        wait_for("foreground job should start", || {
            root.join("job.pid").is_file()
        });
        let shell_pid = fs::read_to_string(root.join("shell.pid")).unwrap();
        let job_pid = fs::read_to_string(root.join("job.pid")).unwrap();
        assert!(running(&shell_pid) && running(&job_pid));
        drop(shell);
        wait_for("closed worktree must release its PTY process group", || {
            !running(&shell_pid) && !running(&job_pid)
        });
    }

    #[test]
    fn multiple_tabs_share_root_but_close_only_their_owned_shell_and_job() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let first = fixture_named(&root, "first");
        let second = fixture_named(&root, "second");
        let other_root = root.join("other worktree");
        fs::create_dir(&other_root).unwrap();
        let other = fixture_named(&other_root, "other");
        second.write(
            b"/bin/sh -c 'trap \"\" HUP TERM; echo $$ > job.pid; exec /bin/sleep 45'\r".to_vec(),
        );
        wait_for("second tab foreground job", || {
            root.join("job.pid").is_file()
        });
        let first_pid = fs::read_to_string(root.join("first.pid")).unwrap();
        let second_pid = fs::read_to_string(root.join("second.pid")).unwrap();
        let job_pid = fs::read_to_string(root.join("job.pid")).unwrap();
        let other_pid = fs::read_to_string(other_root.join("other.pid")).unwrap();
        drop(second);
        wait_for(
            "only the closed tab's shell and foreground group exit",
            || !running(&second_pid) && !running(&job_pid),
        );
        assert!(running(&first_pid) && running(&other_pid));
        first.write(b"print FIRST_STILL_ALIVE > first.out\r".to_vec());
        other.write(b"print OTHER_STILL_ALIVE > other.out\r".to_vec());
        wait_for("remaining tabs accept independent input", || {
            root.join("first.out").is_file() && other_root.join("other.out").is_file()
        });
        assert_eq!(
            fs::read_to_string(root.join("first.out")).unwrap().trim(),
            "FIRST_STILL_ALIVE"
        );
        assert_eq!(
            fs::read_to_string(other_root.join("other.out"))
                .unwrap()
                .trim(),
            "OTHER_STILL_ALIVE"
        );
        drop((first, other));
        wait_for("project close releases every remaining shell", || {
            !running(&first_pid) && !running(&other_pid)
        });
    }

    #[test]
    fn exit_status_is_retained_and_restart_uses_a_fresh_process() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let old = fixture_named(&root, "old");
        let old_pid = fs::read_to_string(root.join("old.pid")).unwrap();
        old.write(b"exit 37\r".to_vec());
        wait_for("nonzero exit status should reach inactive tab", || {
            old.exit_status().is_some()
        });
        assert_eq!(old.exit_status().unwrap().code(), Some(37));
        assert!(old.exited());
        assert!(!running(&old_pid));
        let new = fixture_named(&root, "restarted");
        assert!(!new.exited());
        assert!(new.exit_status().is_none());
        let new_pid = fs::read_to_string(root.join("restarted.pid")).unwrap();
        assert_ne!(old_pid, new_pid);
        drop(old);
        assert!(running(&new_pid));
        drop(new);
        wait_for("fresh shell is independently owned", || !running(&new_pid));
    }

    #[test]
    fn quit_finishes_already_closing_tabs_and_all_foreground_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let shells = [fixture_named(&root, "one"), fixture_named(&root, "two")];
        for (i, shell) in shells.iter().enumerate() {
            shell.write(
                format!(
                    "/bin/sh -c 'trap \"\" HUP TERM; echo $$ > job-{i}.pid; exec /bin/sleep 45'\r"
                )
                .into_bytes(),
            );
        }
        wait_for("foreground jobs in both tabs", || {
            root.join("job-0.pid").is_file() && root.join("job-1.pid").is_file()
        });
        let pids = ["one.pid", "two.pid", "job-0.pid", "job-1.pid"]
            .map(|name| fs::read_to_string(root.join(name)).unwrap());
        let shutdowns = shells
            .iter()
            .map(super::Shutdown::for_shell)
            .collect::<Vec<_>>();
        drop(shells);
        // Exercise the same force step used at app quit, without touching other
        // tests' PTYs in the process-wide pending list.
        for shutdown in shutdowns {
            shutdown.force();
        }
        wait_for(
            "quit must reap every owned shell and foreground job",
            || pids.iter().all(|pid| !running(pid)),
        );
    }
}
