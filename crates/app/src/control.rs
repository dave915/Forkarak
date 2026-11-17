//! Session-scoped local GUI navigation. Socket I/O never runs on GPUI's thread.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

const LIMIT: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub action: String,
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    #[serde(default = "first_line")]
    pub line: u32,
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<crate::cli::OpenTarget>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub activate: bool,
}
impl Request {
    pub fn status() -> Self {
        Self {
            action: "status".into(),
            repo: None,
            path: None,
            line: 1,
            mode: None,
            target: None,
            activate: false,
        }
    }
}
fn first_line() -> u32 {
    1
}
pub struct Pending {
    pub request: Request,
    pub reply: SyncSender<Result<Value, String>>,
    pub deadline: Instant,
}

fn uid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid has no arguments or preconditions.
    unsafe { geteuid() }
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.uid() == uid() && meta.mode() & 0o077 == 0 && !meta.file_type().is_symlink(),
        "Control path is not private to the current user"
    );
    ensure!(
        if directory {
            meta.is_dir()
        } else {
            meta.file_type().is_socket()
        },
        "Invalid control path type"
    );
    Ok(())
}
fn directory() -> Result<PathBuf> {
    // Keep below macOS's Unix socket path limit, independently of --session-dir.
    let path = PathBuf::from(format!("/tmp/pokarak-ctl-{}", uid()));
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    private(&path, true)?;
    Ok(path)
}
fn validate_socket(socket: &Path) -> Result<()> {
    ensure!(
        socket.is_absolute(),
        "--session must be an absolute socket path from ctl sessions"
    );
    let base = directory()?;
    let parent = socket.parent().context("Missing socket directory")?;
    ensure!(
        parent.parent() == Some(base.as_path())
            && socket.file_name().is_some_and(|n| n == "control.sock"),
        "Choose a session socket reported by ctl sessions"
    );
    private(parent, true)?;
    private(socket, false)
}
fn line(stream: &mut UnixStream) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take(LIMIT as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    ensure!(
        bytes.len() <= LIMIT && bytes.last() == Some(&b'\n'),
        "Control message exceeds limit or is incomplete"
    );
    Ok(bytes)
}
fn send(stream: &mut UnixStream, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() < LIMIT, "Control response exceeds limit");
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    Ok(())
}

pub fn call(socket: &Path, request: &Request) -> Result<Value> {
    validate_socket(socket)?;
    let mut stream = UnixStream::connect(socket)
        .context("GUI session is unavailable; run ctl sessions again")?;
    stream.set_read_timeout(Some(if request.action == "status" {
        Duration::from_millis(750)
    } else {
        TIMEOUT + Duration::from_secs(1)
    }))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    send(&mut stream, &serde_json::to_value(request)?)?;
    let response: Value = serde_json::from_slice(&line(&mut stream)?)?;
    ensure!(
        response.get("ok").and_then(Value::as_bool) == Some(true),
        "{}",
        response
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("GUI request failed")
    );
    response
        .get("result")
        .cloned()
        .context("Invalid GUI response")
}
pub fn sessions() -> Result<Value> {
    let base = directory()?;
    let mut sessions = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let socket = entry.path().join("control.sock");
        if validate_socket(&socket).is_ok() {
            sessions.push(socket);
        }
    }
    sessions.sort();
    Ok(json!({"sessions":sessions}))
}

pub struct Server {
    pub receiver: Receiver<Pending>,
    pub socket: PathBuf,
    stopped: Arc<AtomicBool>,
    _directory: tempfile::TempDir,
}
impl Server {
    pub fn start() -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("session-")
            .tempdir_in(directory()?)?;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;
        let socket = dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let (sender, receiver) = mpsc::sync_channel(8);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        thread::spawn(move || {
            let clients = Arc::new(AtomicUsize::new(0));
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if clients
                            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                                (n < 8).then_some(n + 1)
                            })
                            .is_err()
                        {
                            continue;
                        }
                        let clients = clients.clone();
                        let sender = sender.clone();
                        thread::spawn(move || {
                            let result = (|| -> Result<Value> {
                                stream.set_read_timeout(Some(TIMEOUT))?;
                                stream.set_write_timeout(Some(TIMEOUT))?;
                                let mut request: Request =
                                    serde_json::from_slice(&line(&mut stream)?)?;
                                validate_request(&mut request)?;
                                let (reply, receive) = mpsc::sync_channel(1);
                                let deadline = Instant::now() + TIMEOUT;
                                sender
                                    .try_send(Pending {
                                        request,
                                        reply,
                                        deadline,
                                    })
                                    .context("GUI queue is busy; retry after checking status")?;
                                receive
                                    .recv_timeout(TIMEOUT)
                                    .context("GUI did not process request in time")?
                                    .map_err(anyhow::Error::msg)
                            })();
                            let response = match result {
                                Ok(value) => json!({"ok":true,"result":value}),
                                Err(error) => {
                                    json!({"ok":false,"error":{"code":"gui_failed","message":format!("{error:#}")}})
                                }
                            };
                            let _ = send(&mut stream, &response);
                            clients.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            receiver,
            socket,
            stopped,
            _directory: dir,
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}
fn validate_request(request: &mut Request) -> Result<()> {
    ensure!(
        [
            "status",
            "open-project",
            "open-target",
            "select-worktree",
            "open-file",
            "view",
            "refresh"
        ]
        .contains(&request.action.as_str()),
        "Unknown GUI action"
    );
    if request.action == "status" {
        return Ok(());
    }
    if request.action == "open-target" {
        return crate::cli::launch::validate_target(
            request
                .target
                .as_ref()
                .context("open-target requires target")?,
        );
    }
    let repo = request.repo.as_ref().context("GUI action requires repo")?;
    ensure!(repo.is_absolute(), "GUI repo must be absolute");
    let root = pokarak_core::repository_root(repo)?;
    ensure!(
        &root == repo,
        "GUI repo must identify a canonical worktree root"
    );
    if request.action == "open-file" {
        let path = crate::cli::ctl::relative(
            &root,
            request.path.as_ref().context("open-file requires path")?,
        )?;
        pokarak_core::read_file(&root, &path)?;
        request.path = Some(path);
        ensure!(request.line > 0, "line must be positive");
    }
    if request.action == "view" {
        ensure!(
            request
                .mode
                .as_deref()
                .is_some_and(|m| ["graph", "code"].contains(&m)),
            "view mode must be graph or code"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Request, Server, call, line, private, send, sessions};
    use serde_json::json;
    use std::{
        io::Write,
        os::unix::{fs::PermissionsExt, net::UnixStream},
        sync::mpsc,
        thread,
        time::Duration,
    };
    #[test]
    fn private_socket_queues_validated_navigation_and_returns_json() {
        let server = Server::start().unwrap();
        private(&server.socket, false).unwrap();
        private(server.socket.parent().unwrap(), true).unwrap();
        assert_eq!(
            std::fs::metadata(&server.socket)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            sessions().unwrap()["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == &json!(server.socket))
        );
        let socket = server.socket.clone();
        let worker = thread::spawn(move || {
            call(
                &socket,
                &Request {
                    action: "status".into(),
                    repo: None,
                    path: None,
                    line: 1,
                    mode: None,
                    target: None,
                    activate: false,
                },
            )
        });
        let pending = server
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(pending.request.action, "status");
        pending
            .reply
            .try_send(Ok(json!({"repo":null,"loading":false})))
            .unwrap();
        assert_eq!(worker.join().unwrap().unwrap()["loading"], false);
        let socket = server.socket.clone();
        drop(server);
        assert!(!socket.exists());
    }
    #[test]
    fn invalid_actions_and_outside_files_never_reach_ui_queue() {
        let server = Server::start().unwrap();
        for value in [
            json!({"action":"commit","repo":null,"path":null,"mode":null}),
            json!({"action":"status","extra":"forbidden"}),
        ] {
            let mut stream = UnixStream::connect(&server.socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            send(&mut stream, &value).unwrap();
            let response: serde_json::Value =
                serde_json::from_slice(&line(&mut stream).unwrap()).unwrap();
            assert_eq!(response["ok"], false);
            assert!(matches!(
                server.receiver.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
        let mut stream = UnixStream::connect(&server.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(b"bad json\n").unwrap();
        let response: serde_json::Value =
            serde_json::from_slice(&line(&mut stream).unwrap()).unwrap();
        assert_eq!(response["ok"], false);
    }
}
