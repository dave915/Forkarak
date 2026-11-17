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
                let total = fs::read_to_string(headers)
                    .ok()
                    .and_then(|s| content_length(&s));
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
        if !matches!(
            status.as_str(),
            "Language server exited: exit code 0" | "Language server exited: exit status: 0"
        ) {
            bail!("Tool installer failed ({status}): {stderr}");
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
fn content_length(headers: &str) -> Option<u64> {
    // Only the final complete HTTP response, never a redirect's Content-Length.
    let normalized = headers.replace("\r\n", "\n");
    let blocks: Vec<_> = normalized
        .split("\n\n")
        .filter(|b| b.starts_with("HTTP/"))
        .collect();
    let final_block = blocks.last()?;
    let status = final_block.lines().next()?.split_whitespace().nth(1)?;
    if status != "200" {
        return None;
    }
    if final_block
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:"))
    {
        return None;
    }
    final_block.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
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
pub(super) fn download(
    url: &str,
    path: &Path,
    scratch: &Path,
    control: &InstallControl,
) -> Result<()> {
    valid_url(url)?;
    download_curl(url, path, scratch, control, "=https")
}
fn download_curl(
    url: &str,
    path: &Path,
    scratch: &Path,
    control: &InstallControl,
    initial_protocol: &str,
) -> Result<()> {
    control.phase(InstallPhase::Downloading, format!("Downloading {url}"));
    let headers = path.with_extension("headers");
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
        headers.to_str().context("Non-UTF8 scratch")?,
        "--output",
        path.to_str().context("Non-UTF8 scratch")?,
        "--url",
        url,
    ]
    .map(str::to_owned);
    let result = run(
        Path::new("/usr/bin/curl"),
        &args,
        scratch,
        &[],
        control,
        Some((path, &headers)),
    );
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result?;
    control.bytes(
        fs::metadata(path)?.len(),
        fs::read_to_string(headers)
            .ok()
            .and_then(|s| content_length(&s)),
    );
    Ok(())
}
pub(super) fn text(url: &str, scratch: &Path, control: &InstallControl) -> Result<String> {
    let temp = tempfile::NamedTempFile::new_in(scratch)?;
    download(url, temp.path(), scratch, control)?;
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
    download(&artifact.source_url, file.path(), scratch, control)?;
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
    let value = text(url, scratch, control)?;
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
    fn uses_only_final_response_total() {
        assert_eq!(
            content_length(
                "HTTP/1.1 302 Found\r\nContent-Length: 10\r\n\r\nHTTP/2 200\r\ncontent-length: 99\r\n\r\n"
            ),
            Some(99)
        );
        assert_eq!(
            content_length("HTTP/2 302\r\ncontent-length: 10\r\n\r\n"),
            None
        );
        assert_eq!(
            content_length("HTTP/2 200\ncontent-length: 50\ntransfer-encoding: chunked\n\n"),
            None
        );
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
