//! Explicit argv tasks. No shell, repository writes, or process creation on inspection.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskProfile {
    pub name: String,
    /// Program followed by literal arguments. Never parsed as shell text.
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
impl TaskProfile {
    pub fn validate(&self, root: &Path) -> Result<PathBuf> {
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 256,
            "작업 이름을 입력하세요"
        );
        ensure!(
            !self.command.is_empty() && !self.command[0].is_empty(),
            "실행 파일과 인수를 지정하세요"
        );
        ensure!(
            self.command.iter().all(|v| !v.contains('\0')),
            "명령에 NUL을 사용할 수 없습니다"
        );
        ensure!(
            self.env
                .iter()
                .all(|(k, v)| !k.is_empty() && !k.contains(['=', '\0']) && !v.contains('\0')),
            "환경변수 이름과 값을 확인하세요"
        );
        let root = root.canonicalize()?;
        let cwd = if self.cwd.as_os_str().is_empty() {
            root.clone()
        } else {
            root.join(&self.cwd)
                .canonicalize()
                .context("작업 폴더를 찾을 수 없습니다")?
        };
        ensure!(
            cwd.is_dir() && cwd.starts_with(&root),
            "작업 폴더는 현재 워크트리 안에 있어야 합니다"
        );
        Ok(cwd)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
    Host,
}
#[derive(Clone, Debug)]
pub struct LogLine {
    pub sequence: u64,
    pub stream: StreamKind,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Stopping,
    Exited(Option<i32>),
    Failed(String),
}
impl RunStatus {
    pub fn active(&self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }
    pub fn label(&self) -> String {
        match self {
            Self::Running => "실행 중".into(),
            Self::Stopping => "중지 중".into(),
            Self::Exited(Some(0)) => "완료".into(),
            Self::Exited(code) => {
                format!("종료 · {}", code.map_or("신호".into(), |n| n.to_string()))
            }
            Self::Failed(s) => format!("실패 · {s}"),
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LogOptions {
    pub auto_scroll: bool,
    pub word_wrap: bool,
    pub max_lines: usize,
}
impl Default for LogOptions {
    fn default() -> Self {
        Self {
            auto_scroll: true,
            word_wrap: true,
            max_lines: 10_000,
        }
    }
}
impl LogOptions {
    pub fn bounded(self) -> Self {
        Self {
            max_lines: self.max_lines.clamp(10, 100_000),
            ..self
        }
    }
}
const MAX_LINE: usize = 16 * 1024;
const MAX_LOG_BYTES: usize = 8 * 1024 * 1024;
#[derive(Debug)]
pub struct LogBuffer {
    lines: VecDeque<LogLine>,
    bytes: usize,
    pub revision: u64,
    pub options: LogOptions,
}
impl LogBuffer {
    pub fn new(options: LogOptions) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            revision: 0,
            options: options.bounded(),
        }
    }
    pub fn push(&mut self, stream: StreamKind, text: impl Into<String>) {
        let mut text = text.into();
        if text.len() > MAX_LINE {
            let mut end = MAX_LINE;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str(" …");
        }
        self.revision += 1;
        self.bytes += text.len();
        self.lines.push_back(LogLine {
            sequence: self.revision,
            stream,
            text,
        });
        self.trim();
    }
    fn trim(&mut self) {
        while self.lines.len() > self.options.bounded().max_lines || self.bytes > MAX_LOG_BYTES {
            if let Some(line) = self.lines.pop_front() {
                self.bytes -= line.text.len();
            } else {
                break;
            }
        }
    }
    pub fn configure(&mut self, options: LogOptions) {
        self.options = options.bounded();
        self.trim();
        self.revision += 1;
    }
    pub fn search(&self, query: &str) -> Vec<LogLine> {
        let q = query.to_lowercase();
        self.lines
            .iter()
            .filter(|l| q.is_empty() || l.text.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }
}
/// Owns exactly one process group. Drop requests stop and joins the reaper.
pub struct OwnedProcess {
    pub pid: u32,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<RunStatus>>,
}
impl OwnedProcess {
    pub fn spawn(
        mut command: Command,
        logs: Arc<Mutex<LogBuffer>>,
        capture_stdout: bool,
    ) -> Result<(
        Self,
        Option<std::process::ChildStdin>,
        Option<std::process::ChildStdout>,
    )> {
        use std::os::unix::process::CommandExt;
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("프로세스를 시작할 수 없습니다")?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take().expect("piped stderr");
        let status = Arc::new(Mutex::new(RunStatus::Running));
        let stop = Arc::new(AtomicBool::new(false));
        let reader_err = read_stream(stderr, logs.clone(), StreamKind::Stderr);
        let (stdout, reader_out) = if capture_stdout {
            (
                None,
                stdout.map(|out| read_stream(out, logs.clone(), StreamKind::Stdout)),
            )
        } else {
            (stdout, None)
        };
        let stopped = stop.clone();
        let state = status.clone();
        let worker = thread::spawn(move || {
            let mut stopping = None;
            let result = loop {
                if stopped.load(Ordering::Acquire) && stopping.is_none() {
                    *state.lock().unwrap() = RunStatus::Stopping;
                    signal_group(pid, false);
                    stopping = Some(Instant::now());
                }
                if stopping.is_some_and(|t: Instant| t.elapsed() >= Duration::from_millis(250)) {
                    signal_group(pid, true);
                }
                match child.try_wait() {
                    Ok(Some(exit)) => break Ok(exit),
                    Err(e) => break Err(e),
                    Ok(None) => thread::sleep(Duration::from_millis(15)),
                }
            };
            // Descendants which kept a pipe open belong to this group too.
            signal_group(pid, true);
            let _ = reader_err.join();
            if let Some(reader) = reader_out {
                let _ = reader.join();
            }
            *state.lock().unwrap() = match result {
                Ok(s) => RunStatus::Exited(s.code()),
                Err(e) => RunStatus::Failed(e.to_string()),
            };
            logs.lock().unwrap().push(
                StreamKind::Host,
                if stopped.load(Ordering::Acquire) {
                    "작업을 중지했습니다"
                } else {
                    "프로세스가 종료되었습니다"
                },
            );
        });
        Ok((
            Self {
                pid,
                stop,
                worker: Some(worker),
                status,
            },
            stdin,
            stdout,
        ))
    }
    pub(crate) fn cancellation(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn status(&self) -> RunStatus {
        self.status.lock().unwrap().clone()
    }
    pub fn stop_and_wait(&mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.stop_and_wait();
    }
}
fn signal_group(pid: u32, force: bool) {
    #[cfg(target_os = "macos")]
    if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process_group(
            pid,
            if force {
                rustix::process::Signal::KILL
            } else {
                rustix::process::Signal::TERM
            },
        );
    }
}
fn read_stream(
    mut reader: impl Read + Send + 'static,
    logs: Arc<Mutex<LogBuffer>>,
    stream: StreamKind,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut pending = Vec::new();
        let mut partial = None;
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &byte in &buf[..n] {
                        if byte == b'\n' || byte == b'\r' {
                            publish_partial(&logs, stream, &pending, &mut partial);
                            pending.clear();
                            partial = None;
                        } else {
                            pending.push(byte);
                            if pending.len() >= MAX_LINE {
                                publish_partial(&logs, stream, &pending, &mut partial);
                                pending.clear();
                                partial = None;
                            }
                        }
                    }
                    if !pending.is_empty() {
                        publish_partial(&logs, stream, &pending, &mut partial);
                    }
                }
            }
        }
    })
}
fn publish_partial(
    logs: &Mutex<LogBuffer>,
    stream: StreamKind,
    bytes: &[u8],
    sequence: &mut Option<u64>,
) {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let mut buffer = logs.lock().unwrap();
    if let Some(index) =
        sequence.and_then(|seq| buffer.lines.iter().position(|line| line.sequence == seq))
    {
        let previous = buffer.lines[index].text.len();
        buffer.bytes = buffer.bytes - previous + text.len();
        buffer.lines[index].text = text;
        buffer.revision += 1;
        buffer.trim();
    } else {
        buffer.push(stream, text);
        *sequence = Some(buffer.revision);
    }
}

pub struct TaskRun {
    pub id: RunId,
    pub profile: TaskProfile,
    pub root: PathBuf,
    pub logs: Arc<Mutex<LogBuffer>>,
    pub process: OwnedProcess,
    pub started: Instant,
}
#[derive(Default)]
pub struct TaskRuntime {
    runs: HashMap<(PathBuf, String), TaskRun>,
    next: AtomicU64,
}
impl TaskRuntime {
    pub fn start(
        &mut self,
        root: &Path,
        profile: TaskProfile,
        trusted: bool,
        options: LogOptions,
    ) -> Result<RunId> {
        ensure!(trusted, "이 워크트리의 실행을 먼저 허용하세요");
        let root = root.canonicalize()?;
        let cwd = profile.validate(&root)?;
        let key = (root.clone(), profile.name.clone());
        ensure!(
            !self
                .runs
                .get(&key)
                .is_some_and(|r| r.process.status().active()),
            "이미 실행 중인 작업입니다"
        );
        self.runs.remove(&key);
        let mut cmd = Command::new(&profile.command[0]);
        cmd.args(&profile.command[1..])
            .current_dir(cwd)
            .envs(&profile.env);
        let logs = Arc::new(Mutex::new(LogBuffer::new(options)));
        logs.lock().unwrap().push(
            StreamKind::Host,
            format!("{} · {}", profile.name, root.display()),
        );
        let (process, stdin, _) = OwnedProcess::spawn(cmd, logs.clone(), true)?;
        drop(stdin);
        let id = RunId(self.next.fetch_add(1, Ordering::Relaxed) + 1);
        self.runs.insert(
            key,
            TaskRun {
                id,
                profile,
                root,
                logs,
                process,
                started: Instant::now(),
            },
        );
        Ok(id)
    }
    pub fn run(&self, root: &Path, name: &str) -> Option<&TaskRun> {
        self.runs.get(&(root.to_path_buf(), name.into()))
    }
    pub fn runs(&self, root: &Path) -> Vec<&TaskRun> {
        let mut runs: Vec<_> = self.runs.values().filter(|r| r.root == root).collect();
        runs.sort_by_key(|r| r.id.0);
        runs
    }
    pub fn stop(&self, root: &Path, name: &str, id: RunId) -> bool {
        if let Some(run) = self.run(root, name).filter(|r| r.id == id) {
            run.process.request_stop();
            true
        } else {
            false
        }
    }
    pub fn restart(
        &mut self,
        root: &Path,
        profile: TaskProfile,
        trusted: bool,
        options: LogOptions,
    ) -> Result<RunId> {
        ensure!(trusted, "이 워크트리의 실행을 먼저 허용하세요");
        profile.validate(root)?;
        if let Some(mut run) = self
            .runs
            .remove(&(root.to_path_buf(), profile.name.clone()))
        {
            run.process.stop_and_wait();
        }
        self.start(root, profile, trusted, options)
    }
    pub fn shutdown_root(&mut self, root: &Path) {
        for run in self.runs.values().filter(|run| run.root == root) {
            run.process.request_stop();
        }
        self.runs.retain(|(p, _), _| p != root);
    }
}
impl Drop for TaskRuntime {
    fn drop(&mut self) {
        // Signal every group before joining any reaper so app quit does not
        // serialize one escalation interval per task.
        for run in self.runs.values() {
            run.process.request_stop();
        }
        self.runs.clear();
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackLocation {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}
/// Resolve only existing source files in this canonical worktree. Symlink escapes are rejected.
pub fn resolve_stack_location(root: &Path, text: &str) -> Option<StackLocation> {
    let root = root.canonicalize().ok()?;
    for (colon, _) in text.match_indices(':') {
        let suffix = &text[colon + 1..];
        let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            continue;
        }
        let line = suffix[..digits].parse::<u32>().ok()?;
        if line == 0 {
            continue;
        }
        let remaining = &suffix[digits..];
        let column = if let Some(column) = remaining.strip_prefix(':') {
            let digits = column.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                1
            } else {
                column[..digits].parse::<u32>().unwrap_or(1)
            }
        } else {
            1
        };
        let prefix = &text[..colon];
        // Try delimiter boundaries without splitting the path itself. Quoted or
        // absolute paths containing spaces therefore retain their full spelling.
        let starts = std::iter::once(0).chain(
            prefix
                .char_indices()
                .filter(|(_, ch)| {
                    ch.is_whitespace() || matches!(ch, '(' | ')' | '[' | ']' | '"' | '\'' | ',')
                })
                .map(|(at, ch)| at + ch.len_utf8()),
        );
        for start in starts.take(256) {
            let file = prefix[start..].trim();
            if file.is_empty() {
                continue;
            }
            if let Ok(path) = root.join(file).canonicalize()
                && path.is_file() && path.starts_with(&root)
            {
                return Some(StackLocation {
                    path,
                    line: line - 1,
                    column: column.saturating_sub(1),
                });
            }
            // JVM traces omit the source path; derive only the exact package
            // under standard source roots. Never guess a different worktree.
            if !file.contains('/') && (file.ends_with(".java") || file.ends_with(".kt")) {
                let frame = prefix[..start]
                    .trim_end_matches('(')
                    .trim()
                    .trim_start_matches("at ");
                let mut components: Vec<_> = frame.split('.').collect();
                if components.len() < 2 {
                    continue;
                }
                components.pop();
                components.pop();
                let package = components.join("/");
                for source in [
                    "src/main/java",
                    "src/test/java",
                    "src/main/kotlin",
                    "src/test/kotlin",
                ] {
                    if let Ok(path) = root.join(source).join(&package).join(file).canonicalize()
                        && path.is_file() && path.starts_with(&root)
                    {
                        return Some(StackLocation {
                            path,
                            line: line - 1,
                            column: 0,
                        });
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(code: &str) -> TaskProfile {
        TaskProfile {
            name: "fixture".into(),
            command: vec!["/bin/sh".into(), "-c".into(), code.into()],
            cwd: "".into(),
            env: BTreeMap::new(),
        }
    }
    fn wait(runtime: &TaskRuntime, root: &Path) -> RunStatus {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let s = runtime.run(root, "fixture").unwrap().process.status();
            if !s.active() {
                return s;
            }
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn task_argv_streams_env_exit_and_trust() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut rt = TaskRuntime::default();
        let mut p = profile("printf '%s\\n' \"$TASK_FIXTURE\"; printf 'err\\n' >&2; exit 3");
        p.env
            .insert("TASK_FIXTURE".into(), "literal $() `x`".into());
        assert!(
            rt.start(&root, p.clone(), false, LogOptions::default())
                .is_err()
        );
        rt.start(&root, p, true, LogOptions::default()).unwrap();
        assert_eq!(wait(&rt, &root), RunStatus::Exited(Some(3)));
        let run = rt.run(&root, "fixture").unwrap();
        let lines = run.logs.lock().unwrap().search("");
        assert!(
            lines
                .iter()
                .any(|l| l.stream == StreamKind::Stdout && l.text.contains("literal $() `x`"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.stream == StreamKind::Stderr && l.text.contains("err"))
        );
    }
    #[test]
    fn stop_restart_isolation_and_stale_id() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let a = a.path().canonicalize().unwrap();
        let b = b.path().canonicalize().unwrap();
        let mut rt = TaskRuntime::default();
        let p = profile("sleep 20 & wait");
        let old = rt
            .start(&a, p.clone(), true, LogOptions::default())
            .unwrap();
        rt.start(&b, p.clone(), true, LogOptions::default())
            .unwrap();
        assert!(!rt.stop(&a, "fixture", RunId(old.0 + 100)));
        assert!(rt.stop(&a, "fixture", old));
        assert!(!wait(&rt, &a).active());
        assert!(rt.run(&b, "fixture").unwrap().process.status().active());
        let new = rt
            .restart(&a, profile("printf restarted"), true, LogOptions::default())
            .unwrap();
        assert_ne!(old, new);
        assert!(!rt.stop(&a, "fixture", old));
        assert_eq!(wait(&rt, &a), RunStatus::Exited(Some(0)));
        rt.shutdown_root(&b);
    }
    #[test]
    fn bounded_logs_and_stack_paths() {
        let mut logs = LogBuffer::new(LogOptions {
            max_lines: 10,
            ..Default::default()
        });
        for n in 0..40 {
            logs.push(StreamKind::Stdout, format!("line {n}"));
        }
        assert_eq!(logs.search("").len(), 10);
        assert_eq!(logs.search("LINE 39").len(), 1);
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.rs"), "x").unwrap();
        assert_eq!(
            resolve_stack_location(root.path(), "error a.rs:12:3")
                .unwrap()
                .line,
            11
        );
        assert!(resolve_stack_location(root.path(), "/etc/passwd:1").is_none());
        let other = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(other.path(), root.path().join("escape")).unwrap();
        assert!(resolve_stack_location(root.path(), "escape:1").is_none());
        std::fs::write(root.path().join("with spaces.rs"), "fixture").unwrap();
        assert_eq!(
            resolve_stack_location(root.path(), "error \"with spaces.rs:3:2\"")
                .unwrap()
                .column,
            1
        );
        let package = root.path().join("src/main/java/com/example");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("App.java"), "fixture").unwrap();
        assert_eq!(
            resolve_stack_location(root.path(), "at com.example.App.main(App.java:7)")
                .unwrap()
                .line,
            6
        );
        assert!(profile("x").validate(root.path()).is_ok());
        let mut p = profile("x");
        p.cwd = "..".into();
        assert!(p.validate(root.path()).is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn stop_reaps_term_resistant_group_and_descendant() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut runtime = TaskRuntime::default();
        let task = TaskProfile {
            name: "owned group".into(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "trap '' TERM; /bin/sh -c 'trap \"\" TERM; sleep 20' & echo $! > child.pid; wait"
                    .into(),
            ],
            cwd: "".into(),
            env: BTreeMap::new(),
        };
        let id = runtime
            .start(&root, task, true, LogOptions::default())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !root.join("child.pid").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let child = std::fs::read_to_string(root.join("child.pid"))
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        runtime.stop(&root, "owned group", id);
        while runtime
            .run(&root, "owned group")
            .unwrap()
            .process
            .status()
            .active()
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        #[cfg(target_os = "macos")]
        {
            let pid = rustix::process::Pid::from_raw(child).unwrap();
            while rustix::process::test_kill_process(pid).is_ok() {
                assert!(Instant::now() < deadline, "descendant survived group stop");
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    #[test]
    fn streamed_partial_utf8_lines_stay_joined_and_bounded() {
        let logs = Arc::new(Mutex::new(LogBuffer::new(LogOptions::default())));
        let mut seq = None;
        publish_partial(&logs, StreamKind::Stdout, b"abc", &mut seq);
        publish_partial(&logs, StreamKind::Stdout, "abc한글".as_bytes(), &mut seq);
        assert_eq!(logs.lock().unwrap().search("")[0].text, "abc한글");
        assert_eq!(logs.lock().unwrap().search("").len(), 1);
        let reader = std::io::Cursor::new(vec![b'x'; MAX_LINE * 3]);
        read_stream(reader, logs.clone(), StreamKind::Stderr)
            .join()
            .unwrap();
        assert!(
            logs.lock()
                .unwrap()
                .search("")
                .iter()
                .all(|l| l.text.len() <= MAX_LINE + 8)
        );
    }
}
