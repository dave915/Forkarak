use crate::{CancellationHandle, cancellation::OwnedProcess};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_HEADER: usize = 8 * 1024;
const MAX_BODY: usize = 16 * 1024 * 1024;
const LOG_LIMIT: usize = 16 * 1024;
type WriteJob = (Vec<u8>, SyncSender<std::result::Result<(), String>>);

pub(crate) fn read_frame(reader: &mut impl BufRead) -> Result<Value> {
    let mut length = None;
    let mut used = 0;
    loop {
        let mut line = Vec::new();
        let read = reader
            .take((MAX_HEADER - used + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            bail!("Language server closed stdout (unexpected EOF)");
        }
        used += read;
        if used > MAX_HEADER {
            bail!("LSP header exceeds {MAX_HEADER} bytes");
        }
        if line == b"\r\n" {
            break;
        }
        if !line.ends_with(b"\r\n") {
            bail!("Malformed LSP header: expected CRLF");
        }
        let line = std::str::from_utf8(&line)?.trim_end();
        let (key, value) = line.split_once(':').context("Malformed LSP header")?;
        if key.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                bail!("Duplicate Content-Length header");
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("Invalid Content-Length")?,
            );
        }
    }
    let length = length.context("Missing Content-Length header")?;
    if length == 0 || length > MAX_BODY {
        bail!("Invalid LSP body length {length} (maximum {MAX_BODY})");
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).context("Truncated LSP body")?;
    let value: Value = serde_json::from_slice(&body).context("Invalid LSP JSON")?;
    if !value.is_object() || value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        bail!("Invalid JSON-RPC 2.0 message");
    }
    Ok(value)
}

pub(crate) struct Transport {
    process: Arc<OwnedProcess>,
    cancellation: CancellationHandle,
    rx: Option<Receiver<std::result::Result<Value, String>>>,
    tx: Option<SyncSender<WriteJob>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    threads: Vec<JoinHandle<()>>,
    stopped: bool,
}

impl Transport {
    pub fn spawn(
        program: &Path,
        args: &[String],
        root: &Path,
        cancellation: &CancellationHandle,
    ) -> Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // JDT otherwise inherits an editor's socket transport settings.
        command.env_remove("CLIENT_PORT").env_remove("CLIENT_HOST");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let process = cancellation.spawn(&mut command).with_context(|| {
            format!(
                "Cannot start language server {} with argv {:?}",
                program.display(),
                args
            )
        })?;
        let (mut stdin, stdout, mut stderr) = process.take_stdio()?;
        let logs = Arc::new(Mutex::new(VecDeque::new()));
        let capture = Arc::clone(&logs);
        let (response_tx, response_rx) = mpsc::sync_channel(128);
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let frame = read_frame(&mut reader).map_err(|e| format!("{e:#}"));
                let failed = frame.is_err();
                if response_tx.send(frame).is_err() || failed {
                    break;
                }
            }
        });
        let log_reader = thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                match stderr.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let mut log = capture.lock().unwrap_or_else(|e| e.into_inner());
                        log.extend(&buffer[..count]);
                        while log.len() > LOG_LIMIT {
                            log.pop_front();
                        }
                    }
                }
            }
        });
        let (write_tx, write_rx) = mpsc::sync_channel::<WriteJob>(8);
        let writer = thread::spawn(move || {
            while let Ok((body, ack)) = write_rx.recv() {
                let result = (|| -> std::io::Result<()> {
                    write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
                    stdin.write_all(&body)?;
                    stdin.flush()
                })()
                .map_err(|e| e.to_string());
                let failed = result.is_err();
                let _ = ack.send(result);
                if failed {
                    break;
                }
            }
        });
        Ok(Self {
            process,
            cancellation: cancellation.clone(),
            rx: Some(response_rx),
            tx: Some(write_tx),
            stderr: logs,
            threads: vec![reader, log_reader, writer],
            stopped: false,
        })
    }

    pub fn send(&self, message: &Value, deadline: Instant) -> Result<()> {
        self.cancellation.check()?;
        let body = serde_json::to_vec(message)?;
        if body.len() > MAX_BODY {
            bail!("Outgoing LSP message exceeds {MAX_BODY} bytes");
        }
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        self.tx
            .as_ref()
            .context("Language server is stopped")?
            .try_send((body, ack_tx))
            .map_err(|e| anyhow::anyhow!("Cannot queue LSP write: {e}"))?;
        self.wait(&ack_rx, deadline, "LSP write")?
            .map_err(|e| anyhow::anyhow!("LSP write failed: {e}"))
    }

    pub fn receive(&self, deadline: Instant) -> Result<Value> {
        self.wait(
            self.rx.as_ref().context("Language server is stopped")?,
            deadline,
            "LSP response",
        )?
        .map_err(anyhow::Error::msg)
    }

    fn wait<T>(&self, receiver: &Receiver<T>, deadline: Instant, operation: &str) -> Result<T> {
        loop {
            self.cancellation.check()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .with_context(|| format!("{operation} timed out"))?;
            match receiver.recv_timeout(remaining.min(Duration::from_millis(20))) {
                Ok(value) => {
                    self.cancellation.check()?;
                    return Ok(value);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.cancellation.check()?;
                    bail!("{operation} worker exited");
                }
            }
        }
    }

    pub fn exit_status(&self) -> Option<String> {
        self.process.exit_status()
    }

    pub fn stderr(&self) -> String {
        let bytes: Vec<_> = self
            .stderr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect();
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }

    /// Grace period followed by process-group cleanup (includes tsserver children).
    pub fn stop(&mut self, grace: Duration) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.tx.take();
        self.rx.take();
        let deadline = Instant::now() + grace;
        while !self.cancellation.is_cancelled()
            && self.exit_status().is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        let _ = self.process.terminate();
        self.process.reap();
        // A server which deliberately escaped its process group might retain a pipe.
        // Never let such a pipe hold up application shutdown.
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.threads.iter().any(|t| !t.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        for handle in self.threads.drain(..) {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.stop(Duration::ZERO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_is_byte_counted_and_bounded() {
        let json = r#"{"jsonrpc":"2.0","result":"한😀"}"#;
        let frame = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{json}",
            json.len()
        );
        let mut input = std::io::Cursor::new(format!("{frame}{frame}"));
        assert_eq!(read_frame(&mut input).unwrap()["result"], "한😀");
        assert_eq!(read_frame(&mut input).unwrap()["result"], "한😀");
        for invalid in [
            "Content-Length: 9999999999\r\n\r\n",
            "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "Content-Length: 100\r\n\r\n{}",
            "Content-Length: 2\n\n{}",
        ] {
            assert!(read_frame(&mut std::io::Cursor::new(invalid)).is_err());
        }
        assert!(read_frame(&mut std::io::Cursor::new(vec![b'x'; MAX_HEADER + 2])).is_err());
    }
}
