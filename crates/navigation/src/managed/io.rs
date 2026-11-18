use super::{InstallControl, InstallPhase, ToolArtifact};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256, Sha512};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) fn run(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    control: &InstallControl,
    monitor: Option<(&Path, &Path)>,
) -> Result<String> {
    control.check()?;
    let tmp = cwd.join(".process-tmp");
    fs::create_dir_all(&tmp)?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", cwd)
        .env("TMPDIR", &tmp)
        .env("LANG", "en_US.UTF-8")
        .envs(env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let process = control.inner.cancellation.spawn(&mut command)?;
    let result = (|| -> Result<String> {
        let (stdin, stdout, stderr) = process.take_stdio()?;
        drop(stdin);
        let (tx, rx) = mpsc::channel();
        let mut readers = Vec::new();
        for (kind, mut pipe) in [
            (0, Box::new(stdout) as Box<dyn Read + Send>),
            (1, Box::new(stderr) as Box<dyn Read + Send>),
        ] {
            let tx = tx.clone();
            readers.push(thread::spawn(move || {
                let mut tail = Vec::new();
                let mut buf = [0; 8192];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            tail.extend_from_slice(&buf[..n]);
                            if tail.len() > 2 * 1024 * 1024 {
                                tail.drain(..tail.len() - 2 * 1024 * 1024);
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.send((kind, tail));
            }));
        }
        drop(tx);
        let deadline = Instant::now() + Duration::from_secs(900);
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut done = 0;
        while done < 2 || process.exit_status().is_none() {
            control.check()?;
            if Instant::now() > deadline {
                bail!("Tool installer subprocess exceeded its deadline");
            }
            if let Some((file, headers)) = monitor {
                let count = fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                let total = fs::read(headers)
                    .ok()
                    .and_then(|s| final_response(&s).length);
                control.bytes(count, total);
                if count > 2 * 1024 * 1024 * 1024 {
                    bail!("Download exceeds 2 GiB limit");
                }
            }
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok((kind, data)) => {
                    done += 1;
                    if kind == 0 {
                        stdout = String::from_utf8_lossy(&data).into_owned();
                    } else {
                        stderr = String::from_utf8_lossy(&data).into_owned();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        for reader in readers {
            let _ = reader.join();
        }
        let status = process
            .exit_status()
            .context("Missing subprocess exit status")?;
        if !matches!(status, crate::cancellation::ProcessExit::Code(0)) {
            let name = program
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("installer");
            bail!("{name} failed ({status})\n{}", stderr.trim());
        }
        Ok(stdout)
    })();
    let already_exited = process.exit_status().is_some();
    let terminated = process.terminate();
    process.reap();
    // macOS may reject a signal to an already-exited signed executable.
    // Its PID is still reserved and reap above completed; stdout/stderr are closed.
    // Never suppress a signaling failure for a live process while it is live.
    if let Err(error) = terminated
        && !(already_exited && error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return Err(error).context("Terminating installer process group");
    }
    result
}
#[derive(Default)]
struct HttpResponse {
    status: Option<u16>,
    length: Option<u64>,
}

fn final_response(headers: &[u8]) -> HttpResponse {
    let normalized = String::from_utf8_lossy(headers).replace("\r\n", "\n");
    // Curl writes a status line for each response (proxy, informational, redirect,
    // origin). Even a truncated last block supersedes the previous response.
    let start = normalized.rfind("\nHTTP/").map_or(0, |i| i + 1);
    let block = &normalized[start..];
    let mut lines = block.lines();
    let status = lines
        .next()
        .and_then(|l| l.strip_prefix("HTTP/"))
        .and_then(|l| l.split_ascii_whitespace().nth(1))
        .filter(|s| s.len() == 3)
        .and_then(|s| s.parse().ok())
        .filter(|code| (100..=599).contains(code));
    let mut length = None;
    if status == Some(200) && block.contains("\n\n") {
        for line in lines.take_while(|l| !l.is_empty()) {
            if let Some((key, value)) = line.split_once(':') {
                if key.eq_ignore_ascii_case("transfer-encoding") {
                    length = None;
                    break;
                }
                if key.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().ok();
                }
            }
        }
    }
    HttpResponse { status, length }
}

#[derive(Debug)]
pub(super) struct DownloadFailure {
    component: &'static str,
    stage: &'static str,
    host: String,
    status: Option<u16>,
}

impl std::fmt::Display for DownloadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let outcome = if self.status == Some(403) {
            "다운로드 요청 거부"
        } else {
            "다운로드 실패"
        };
        write!(
            f,
            "{} {} {outcome} · {}",
            self.component, self.stage, self.host
        )?;
        if let Some(status) = self.status {
            write!(f, " · HTTP {status}")?;
        }
        Ok(())
    }
}

fn valid_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Official tool downloads require credential-free HTTPS URLs");
    }
    Ok(())
}
fn download(
    url: &str,
    stage: &'static str,
    path: &Path,
    scratch: &Path,
    control: &InstallControl,
) -> Result<()> {
    valid_url(url)?;
    download_curl(url, stage, path, scratch, control, "=https")
}
fn download_curl(
    url: &str,
    stage: &'static str,
    path: &Path,
    scratch: &Path,
    control: &InstallControl,
    initial_protocol: &str,
) -> Result<()> {
    let parsed = url::Url::parse(url)?;
    let host = parsed.host_str().context("Download URL has no host")?;
    control.phase(
        InstallPhase::Downloading,
        format!("{} {stage} 다운로드 중 · {host}", control.component),
    );
    // Redirect headers may contain signed locations. Keep them only for this transfer.
    let headers = tempfile::NamedTempFile::new_in(scratch)?;
    let args = [
        "--disable",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--max-redirs",
        "5",
        "--proto",
        initial_protocol,
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "15",
        "--max-time",
        "600",
        "--speed-limit",
        "1024",
        "--speed-time",
        "30",
        "--user-agent",
        "Forkarak-managed-tools/1",
        "--dump-header",
        headers.path().to_str().context("Non-UTF8 scratch")?,
        "--output",
        path.to_str().context("Non-UTF8 scratch")?,
        "--url",
        url,
    ]
    .map(str::to_owned);
    let curl = Path::new("/usr/bin/curl");
    #[cfg(test)]
    let curl = control.curl.as_deref().unwrap_or(curl);
    let result = run(
        curl,
        &args,
        scratch,
        &[],
        control,
        Some((path, headers.path())),
    );
    let response = fs::read(headers.path())
        .map(|data| final_response(&data))
        .unwrap_or_default();
    let result = result.and_then(|_| {
        if response.status.is_some_and(|status| status >= 400) {
            bail!("curl completed with an unsuccessful HTTP response");
        }
        Ok(())
    });
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        control.check()?;
        return Err(error.context(DownloadFailure {
            component: control.component,
            stage,
            host: host.to_owned(),
            status: response.status,
        }));
    }
    control.bytes(fs::metadata(path)?.len(), response.length);
    Ok(())
}
pub(super) fn text(url: &str, scratch: &Path, control: &InstallControl) -> Result<String> {
    download_text(url, "메타데이터", scratch, control)
}
fn download_text(
    url: &str,
    stage: &'static str,
    scratch: &Path,
    control: &InstallControl,
) -> Result<String> {
    let temp = tempfile::NamedTempFile::new_in(scratch)?;
    download(url, stage, temp.path(), scratch, control)?;
    if fs::metadata(temp.path())?.len() > 8 * 1024 * 1024 {
        bail!("Official metadata exceeds limit");
    }
    Ok(fs::read_to_string(temp.path())?)
}
pub(super) fn json(
    url: &str,
    scratch: &Path,
    control: &InstallControl,
) -> Result<serde_json::Value> {
    Ok(serde_json::from_str(&text(url, scratch, control)?)?)
}
pub(super) fn verify(path: &Path, digest: &str, control: &InstallControl) -> Result<()> {
    control.phase(InstallPhase::Verifying, "Verifying pinned checksum");
    let (algorithm, wanted) = digest
        .split_once(':')
        .context("Checksum must name algorithm")?;
    if !wanted.chars().all(|c| c.is_ascii_hexdigit())
        || !matches!((algorithm, wanted.len()), ("sha256", 64) | ("sha512", 128))
    {
        bail!("Invalid published digest");
    }
    let mut file = fs::File::open(path)?;
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    let mut buf = [0; 64 * 1024];
    loop {
        control.check()?;
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if algorithm == "sha256" {
            sha256.update(&buf[..n]);
        } else {
            sha512.update(&buf[..n]);
        }
    }
    let actual = if algorithm == "sha256" {
        format!("{:x}", sha256.finalize())
    } else {
        format!("{:x}", sha512.finalize())
    };
    if !actual.eq_ignore_ascii_case(wanted) {
        bail!("Pinned checksum mismatch; previous working version retained");
    }
    Ok(())
}
pub(super) fn artifact(
    artifact: &ToolArtifact,
    scratch: &Path,
    control: &InstallControl,
) -> Result<PathBuf> {
    let (algorithm, digest) = artifact
        .checksum
        .split_once(':')
        .context("Archive digest missing algorithm")?;
    if !matches!(algorithm, "sha256" | "sha512") || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("Unsupported archive digest");
    }
    let cache = scratch
        .parent()
        .context("Scratch requires app-owned parent")?
        .join("cache");
    fs::create_dir_all(&cache)?;
    let target = cache.join(format!("{algorithm}-{digest}"));
    if target.is_file() {
        if verify(&target, &artifact.checksum, control).is_ok() {
            return Ok(target);
        }
        control.check()?;
        fs::remove_file(&target)?;
    }
    let file = tempfile::NamedTempFile::new_in(&cache)?;
    download(
        &artifact.source_url,
        "압축 파일",
        file.path(),
        scratch,
        control,
    )?;
    verify(file.path(), &artifact.checksum, control)?;
    file.persist(&target).map_err(|e| e.error)?;
    Ok(target)
}
pub(super) fn checksum(
    url: &str,
    length: usize,
    scratch: &Path,
    control: &InstallControl,
) -> Result<String> {
    let value = download_text(url, "체크섬", scratch, control)?;
    let digest = value
        .split_whitespace()
        .next()
        .context("Empty official checksum")?;
    if digest.len() != length || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("Invalid official checksum");
    }
    Ok(digest.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_https_and_credentials() {
        for u in [
            "http://localhost/test",
            "file:///tmp/test",
            "https://user:pass@example.com/test",
        ] {
            assert!(valid_url(u).is_err());
        }
    }
    #[test]
    fn uses_only_final_response_status_and_total() {
        let response = final_response(b"HTTP/1.1 302 Found\r\nContent-Length: 10\r\n\r\nHTTP/2 200\r\ncontent-length: 99\r\n\r\n");
        assert_eq!((response.status, response.length), (Some(200), Some(99)));
        let response =
            final_response(b"HTTP/2 200\ncontent-length: 50\ntransfer-encoding: chunked\n\n");
        assert_eq!(response.length, None);
        let response = final_response(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/2 307\r\nlocation: https://cdn.invalid/?secret\r\n\r\nHTTP/3 403\r\nx-info: \xff\r\n\r\n");
        assert_eq!((response.status, response.length), (Some(403), None));
        let partial = final_response(b"HTTP/1.1 301 Moved\r\n\r\nHTTP/2 429\r\n");
        assert_eq!((partial.status, partial.length), (Some(429), None));
    }
    #[test]
    fn actual_curl_refuses_redirect_downgrades() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let t = tempfile::tempdir().unwrap();
        let first = TcpListener::bind("127.0.0.1:0").unwrap();
        let second = TcpListener::bind("127.0.0.1:0").unwrap();
        second.set_nonblocking(true).unwrap();
        let port = first.local_addr().unwrap().port();
        let target = second.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = first.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            write!(stream,"HTTP/1.1 302 Found\r\nLocation: http://{target}/must-not-be-requested\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        // Only this private test permits an initial local HTTP fixture. The shared
        // downloader keeps the exact production HTTPS-only redirect policy.
        let result = download_curl(
            &format!("http://127.0.0.1:{port}/redirect"),
            "메타데이터",
            &t.path().join("download"),
            t.path(),
            &InstallControl::new("test"),
            "=http",
        );
        server.join().unwrap();
        assert!(result.is_err());
        assert_eq!(
            second.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn actual_curl_403_reports_the_host_and_retains_only_safe_details() {
        use std::{io::Write, net::TcpListener};
        let t = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/metadata?query=PRIVATE",
            listener.local_addr().unwrap()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.read(&mut [0; 4096]).unwrap();
            stream.write_all(b"HTTP/1.1 403 Forbidden\r\nX-Private: PRIVATE\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let c = InstallControl::new("kotlin");
        let file = t.path().join("body");
        let error = download_curl(&url, "메타데이터", &file, t.path(), &c, "=http").unwrap_err();
        server.join().unwrap();
        assert!(!c.snapshot().message.contains("PRIVATE"));
        let attempt = t.path().join("attempt.json");
        c.finish(Err(error), &attempt);
        let progress = c.snapshot();
        assert_eq!(progress.phase, InstallPhase::Failed);
        assert!(progress.message.contains("127.0.0.1 · HTTP 403"));
        assert!(
            progress
                .error
                .unwrap()
                .contains("curl failed (exit code 22)")
        );
        assert!(!fs::read_to_string(attempt).unwrap().contains("PRIVATE"));
        assert!(!file.exists());
    }
    #[test]
    fn cancellation_terminates_an_actual_curl_download() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let t = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_write_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: 10485760\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            for _ in 0..200 {
                if socket.write_all(&[1; 65536]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        let c = InstallControl::new("test");
        let worker_control = c.clone();
        let scratch = t.path().to_path_buf();
        let worker = thread::spawn(move || {
            download_curl(
                &format!("http://127.0.0.1:{port}/slow"),
                "압축 파일",
                &scratch.join("download"),
                &scratch,
                &worker_control,
                "=http",
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.snapshot().downloaded_bytes == 0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(c.snapshot().total_bytes, Some(10485760));
        c.cancel().unwrap();
        assert!(worker.join().unwrap().is_err());
        server.join().unwrap();
        assert!(!t.path().join("download").exists());
    }
    #[test]
    fn corruption_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("file");
        fs::write(&p, b"real").unwrap();
        let c = InstallControl::new("test");
        assert!(verify(&p, &format!("sha256:{:x}", Sha256::digest(b"real")), &c).is_ok());
        assert!(verify(&p, &format!("sha256:{:x}", Sha256::digest(b"corrupt")), &c).is_err());
    }
    #[test]
    fn cancel_kills_owned_subprocess_and_descendant() {
        let t = tempfile::tempdir().unwrap();
        let marker = t.path().join("escaped");
        let c = InstallControl::new("test");
        let other = c.clone();
        let cwd = t.path().to_path_buf();
        let p = marker.clone();
        let worker = thread::spawn(move || {
            run(
                Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    format!("(sleep 2; touch '{}') & wait", p.display()),
                ],
                &cwd,
                &[],
                &other,
                None,
            )
        });
        thread::sleep(Duration::from_millis(150));
        c.cancel().unwrap();
        assert!(worker.join().unwrap().is_err());
        thread::sleep(Duration::from_millis(2200));
        assert!(!marker.exists());
    }
}
