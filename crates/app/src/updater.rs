//! The verified installer stages the replacement while the app is still running.
//! Dropping a prepared update closes its input and cancels the staged transaction.
use std::{
    env,
    io::{self, Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const READY: &[u8] = b"POKARAK_UPDATE_READY";
const RESTARTING: &[u8] = b"POKARAK_UPDATE_RESTARTING";
const CHECK_TIMEOUT: Duration = Duration::from_secs(45);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
const DIAGNOSTIC_LIMIT: usize = 4096;
const POLL: Duration = Duration::from_millis(20);

pub(crate) fn installed_bundle() -> Option<PathBuf> {
    bundle_for_executable(&env::current_exe().ok()?)
}

fn bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    let exe = executable.canonicalize().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (exe.file_name()? == "forkarak"
        && macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app"
        && contents.join("Info.plist").is_file())
    .then(|| bundle.to_path_buf())
}

fn installer(bundle: Option<&Path>) -> Result<Command, String> {
    let mut command = Command::new("/bin/bash");
    command
        .current_dir("/")
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "en_US.UTF-8");
    for key in [
        "HOME",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
    match bundle {
        Some(bundle) => {
            let script = bundle.join("Contents/Resources/install.sh");
            if !script.is_file() || script.canonicalize().ok().as_deref() != Some(&script) {
                return Err("설치된 앱에서 업데이트 스크립트를 찾을 수 없습니다.".into());
            }
            command.arg(script);
        }
        None => {
            // A developer build may check on explicit request. Never execute a file
            // from the inspected repository (which is often the current directory).
            command.args([
                "-c",
                include_str!("../../../scripts/install.sh"),
                "install.sh",
            ]);
        }
    }
    Ok(command)
}

fn version(value: &str) -> Result<[u64; 3], String> {
    let mut parts = value.split('.');
    let mut number = [0; 3];
    for slot in &mut number {
        let part = parts.next().unwrap_or_default();
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err("정식 릴리스 버전 형식이 올바르지 않습니다.".into());
        }
        *slot = part
            .parse()
            .map_err(|_| "릴리스 버전 숫자가 너무 큽니다.")?;
    }
    if parts.next().is_some() {
        return Err("정식 릴리스 버전 형식이 올바르지 않습니다.".into());
    }
    Ok(number)
}

fn newer(tag: &str, current: &str) -> Result<bool, String> {
    let remote = tag
        .strip_prefix('v')
        .ok_or("릴리스 태그는 v로 시작해야 합니다.")?;
    Ok(version(remote)? > version(current)?)
}

pub(crate) fn check() -> Result<Option<String>, String> {
    check_with(
        installer(installed_bundle().as_deref())?,
        env!("CARGO_PKG_VERSION"),
    )
}

fn check_with(mut command: Command, current: &str) -> Result<Option<String>, String> {
    command.arg("--check-only");
    let mut helper = Helper::spawn(command)?;
    let output = helper.receive(None, CHECK_TIMEOUT)?;
    let tag = std::str::from_utf8(&output)
        .ok()
        .and_then(|line| line.strip_suffix('\n'))
        .ok_or("업데이트 확인 응답을 읽을 수 없습니다.")?;
    Ok(newer(tag, current)?.then(|| tag.to_owned()))
}

pub(crate) fn prepare(tag: &str) -> Result<Prepared, String> {
    if !newer(tag, env!("CARGO_PKG_VERSION"))? {
        return Err("현재 버전보다 새로운 릴리스만 설치할 수 있습니다.".into());
    }
    let bundle = installed_bundle().ok_or("설치된 Forkarak.app에서 업데이트할 수 있습니다.")?;
    let command = update_command(&bundle, tag, crate::prefs::explicit_session_dir())?;
    Prepared::start(command, PREPARE_TIMEOUT)
}

fn update_command(bundle: &Path, tag: &str, session_dir: Option<&Path>) -> Result<Command, String> {
    let mut command = installer(Some(bundle))?;
    command
        .arg("--update-running-app")
        .arg(bundle)
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .arg("--version")
        .arg(tag);
    if let Some(dir) = session_dir {
        command
            .arg("--relaunch-session-dir")
            .arg(std::path::absolute(dir).map_err(|e| e.to_string())?);
    }
    Ok(command)
}

pub(crate) struct Prepared(Helper);

impl Prepared {
    fn start(command: Command, timeout: Duration) -> Result<Self, String> {
        let mut helper = Helper::spawn(command)?;
        helper.receive(Some(READY), timeout)?;
        Ok(Self(helper))
    }

    pub(crate) fn restart(mut self) -> Result<(), String> {
        let input = self
            .0
            .child
            .as_mut()
            .and_then(|c| c.stdin.as_mut())
            .ok_or("업데이트 도우미 연결이 종료되었습니다.")?;
        input
            .write_all(b"restart\n")
            .and_then(|_| input.flush())
            .map_err(|e| format!("업데이트 재시작 요청을 보낼 수 없습니다. {e}"))?;
        self.0.receive(Some(RESTARTING), ACK_TIMEOUT)?;
        // Ownership transfers to the installer before the caller asks GPUI to
        // quit. The helper waits for this process and redirects its own output.
        let mut child = self.0.child.take().expect("live update helper");
        child.stdin.take();
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

// Nonblocking output sockets keep stdout markers separate from diagnostics,
// even when the installer or one of its tools fills a pipe without a newline.
struct Helper {
    child: Option<Child>,
    stdout: UnixStream,
    stderr: UnixStream,
    diagnostics: Vec<u8>,
}

fn output_pipe() -> io::Result<(UnixStream, Stdio)> {
    let (read, write) = UnixStream::pair()?;
    read.set_nonblocking(true)?;
    Ok((read, Stdio::from(OwnedFd::from(write))))
}

// Bound work per poll so an output flood cannot starve stderr or the deadline.
fn drain(stream: &mut UnixStream, mut consume: impl FnMut(&[u8])) -> io::Result<bool> {
    let mut bytes = [0; 8192];
    for _ in 0..8 {
        match stream.read(&mut bytes) {
            Ok(0) => return Ok(true),
            Ok(n) => consume(&bytes[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

impl Helper {
    fn spawn(mut command: Command) -> Result<Self, String> {
        let (stdout, out) = output_pipe().map_err(|e| e.to_string())?;
        let (stderr, err) = output_pipe().map_err(|e| e.to_string())?;
        let child = command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(out)
            .stderr(err)
            .spawn()
            .map_err(|e| format!("업데이트 도우미를 실행할 수 없습니다. {e}"))?;
        Ok(Self {
            child: Some(child),
            stdout,
            stderr,
            diagnostics: vec![],
        })
    }

    fn receive(&mut self, marker: Option<&[u8]>, timeout: Duration) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + timeout;
        let mut line = Vec::new();
        let mut seen = false;
        loop {
            let stdout_done = drain(&mut self.stdout, |chunk| {
                if let Some(marker) = marker {
                    for &b in chunk {
                        if b == b'\n' {
                            seen |= line == marker;
                            line.clear();
                        } else if line.len() <= marker.len() {
                            line.push(b);
                        }
                    }
                } else {
                    // A version response is exactly one short line. Truncate
                    // oversized output with one extra byte to reject it below.
                    let remaining = 129usize.saturating_sub(line.len());
                    line.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                }
            })
            .map_err(|e| e.to_string())?;
            let stderr_done = drain(&mut self.stderr, |chunk| {
                let tail = &chunk[chunk.len().saturating_sub(DIAGNOSTIC_LIMIT)..];
                let discard =
                    (self.diagnostics.len() + tail.len()).saturating_sub(DIAGNOSTIC_LIMIT);
                self.diagnostics.drain(..discard);
                self.diagnostics.extend_from_slice(tail);
            })
            .map_err(|e| e.to_string())?;
            if seen {
                return Ok(vec![]);
            }
            if stdout_done && stderr_done && marker.is_some() {
                // Leave the child waitable for cancellation, even if only its
                // descendants are still running after a failed handshake.
                return Err(self.error("업데이트 도우미가 확인 응답 없이 연결을 종료했습니다."));
            }
            if stdout_done
                && stderr_done
                && let Some(status) = self
                    .child
                    .as_mut()
                    .expect("live helper")
                    .try_wait()
                    .map_err(|e| e.to_string())?
            {
                self.child.take();
                if status.success() && line.len() <= 128 {
                    return Ok(line);
                }
                return Err(self.error(&format!(
                    "업데이트 도우미가 응답을 완료하지 못했습니다 ({status})."
                )));
            }
            if Instant::now() >= deadline {
                return Err(self.error("업데이트 도우미 응답 시간이 초과되었습니다."));
            }
            thread::sleep(POLL);
        }
    }

    fn error(&self, description: &str) -> String {
        let details = String::from_utf8_lossy(&self.diagnostics);
        if details.trim().is_empty() {
            description.to_owned()
        } else {
            format!("{description}\n{}", details.trim())
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // EOF lets a staged helper run its cleanup trap without blocking the UI.
        child.stdin.take();
        let mut stdout = self.stdout.try_clone().ok();
        let mut stderr = self.stderr.try_clone().ok();
        thread::spawn(move || {
            let mut drain_for = |grace: Duration| {
                let deadline = Instant::now() + grace;
                while Instant::now() < deadline {
                    for socket in [&mut stdout, &mut stderr].into_iter().flatten() {
                        let _ = drain(socket, |_| {});
                    }
                    thread::sleep(POLL);
                }
            };
            drain_for(Duration::from_secs(1));
            #[cfg(target_os = "macos")]
            if let Some(group) = rustix::process::Pid::from_raw(child.id() as i32) {
                use rustix::process::{Signal, kill_process_group};
                // spawn placed this child in its own process group. Keep the
                // leader unreaped until both signals have been sent: its PID
                // reserves the group ID even if it exits at EOF or on TERM.
                let _ = kill_process_group(group, Signal::TERM);
                drain_for(Duration::from_secs(1));
                let _ = kill_process_group(group, Signal::KILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    use tempfile::tempdir;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/bash");
        command.args(["-c", script, "fixture"]);
        command
    }

    fn wait_for(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            thread::sleep(POLL);
        }
        assert!(path.exists(), "missing {}", path.display());
    }

    #[test]
    fn stable_versions_use_numeric_order_and_reject_non_releases() {
        assert!(newer("v10.2.4", "9.99.100").unwrap());
        assert!(newer("v0.10.1", "0.9.99").unwrap());
        assert!(!newer("v1.2.1", "1.10.0").unwrap());
        assert!(!newer("v1.0.0", "1.0.0").unwrap());
        for tag in [
            "1.2.3",
            "v1.0.0-beta.1",
            "v3.2",
            "v3.2.1.0",
            "v01.2.3",
            "v1.+2.0",
            "v1.2.3\n",
            "v1.2.3/../../x",
        ] {
            assert!(newer(tag, "1.0.0").is_err(), "{tag}");
        }
    }

    #[test]
    fn check_accepts_one_short_tag_line_and_successful_exit() {
        let command = shell(r#"test "$#" = 1 && test "$1" = --check-only && printf 'v2.10.0\n'"#);
        assert_eq!(
            check_with(command, "2.9.9").unwrap().as_deref(),
            Some("v2.10.0")
        );
        assert_eq!(check_with(shell("echo v1.2.3"), "1.2.3").unwrap(), None);
        for script in [
            "echo v1.2.3; echo v1.2.4",
            "printf v1.2.3",
            "echo v1.2.3; exit 1",
            "printf '%100000s' x",
        ] {
            assert!(check_with(shell(script), "1.0.0").is_err(), "{script}");
        }
    }

    #[test]
    fn helper_uses_running_bundle_and_forwards_session_as_an_argument() {
        let temp = tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let app = root.join("포카락 사용자 버전.app");
        let macos = app.join("Contents/MacOS");
        let resources = app.join("Contents/Resources");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(&resources).unwrap();
        fs::write(macos.join("forkarak"), "").unwrap();
        fs::write(app.join("Contents/Info.plist"), "").unwrap();
        assert_eq!(
            bundle_for_executable(&macos.join("forkarak")).as_deref(),
            Some(app.as_path())
        );
        let other_executable = macos.join("pokarak");
        fs::write(&other_executable, "").unwrap();
        assert!(bundle_for_executable(&other_executable).is_none());
        assert!(bundle_for_executable(&env::current_exe().unwrap()).is_none());
        let script = resources.join("install.sh");
        fs::write(
            &script,
            r#"
set -eu
[[ "$PWD" == / && "$PATH" == /usr/bin:/bin:/usr/sbin:/sbin && -z "${BASH_ENV-}" ]]
[[ "$#" == 8 && "$1" == --update-running-app && "$0" == "$2/Contents/Resources/install.sh" ]]
[[ "$3" == --wait-pid && "$4" == "$PPID" && "$5" == --version && "$6" == v5.0.0 ]]
[[ "$7" == --relaunch-session-dir && "$8" == "${2%/*}/session dir" ]]
exec 3>&1
exec > /dev/null 2>&1
printf 'POKARAK_UPDATE_READY\n' >&3
IFS= read -r command && [[ "$command" == restart ]]
printf 'POKARAK_UPDATE_RESTARTING\n' >&3
exec 3>&-
/bin/sleep 0.05
printf done > "$8"
"#,
        )
        .unwrap();
        let session = root.join("session dir");
        let command = update_command(&app, "v5.0.0", Some(&session)).unwrap();
        Prepared::start(command, Duration::from_secs(5))
            .unwrap()
            .restart()
            .unwrap();
        wait_for(&session);
        fs::remove_file(&script).unwrap();
        symlink("/bin/bash", &script).unwrap();
        assert!(installer(Some(&app)).is_err());
    }

    #[test]
    fn eof_cancels_before_restart_and_drains_diagnostics() {
        let temp = tempdir().unwrap();
        let canceled = temp.path().join("canceled");
        let mut command = shell(
            r#"
echo POKARAK_UPDATE_READY
if ! IFS= read -r reply; then
    printf '%180000s\n' cleanup >&2
    touch "$1"
fi"#,
        );
        command.arg(&canceled);
        drop(Prepared::start(command, Duration::from_secs(5)).unwrap());
        wait_for(&canceled);
    }

    #[test]
    fn markers_require_exact_stdout_lines_and_diagnostics_are_bounded() {
        let command = shell(
            r#"
echo POKARAK_UPDATE_READY >&2
echo NOT_POKARAK_UPDATE_READY
printf '%180000s\n' error >&2
echo 'verification failed' >&2
exit 7"#,
        );
        let error = Prepared::start(command, Duration::from_secs(5))
            .err()
            .unwrap();
        assert!(error.contains("verification failed"));
        assert!(error.len() < DIAGNOSTIC_LIMIT + 300);
        let command = shell("echo POKARAK_UPDATE_READY; read -r input; echo BROKEN; exit 1");
        assert!(
            Prepared::start(command, Duration::from_secs(5))
                .unwrap()
                .restart()
                .is_err()
        );
        let command = shell("printf POKARAK_UPDATE_READY");
        assert!(Prepared::start(command, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn hung_preparation_times_out_and_cancels_the_helper() {
        let temp = tempdir().unwrap();
        let canceled = temp.path().join("cancel-after-timeout");
        let mut command = shell(r#"read -r reply || touch "$1""#);
        command.arg(&canceled);
        let error = Prepared::start(command, Duration::from_millis(100))
            .err()
            .unwrap();
        assert!(error.contains("시간이 초과"));
        wait_for(&canceled);
    }
    #[cfg(target_os = "macos")]
    fn assert_cancellation_stops_group(leader_behavior: &str) {
        let temp = tempdir().unwrap();
        let ready = temp.path().join("child-pid");
        let mut command = shell(
            r#"
            if [[ "$2" == exit ]]; then
                trap 'echo TERM > "$1/terminated"; exit 0' TERM
            else
                trap '' TERM
            fi
            /bin/bash -c '
                trap "" TERM
                echo "$$" > "$1/pending"
                /bin/mv "$1/pending" "$1/child-pid"
                exec /bin/sleep 40
            ' child "$1" </dev/null >/dev/null 2>&1 &
            if [[ "$2" == fail ]]; then
                while [[ ! -e "$1/fail" ]]; do /bin/sleep 0.05; done
                exit 1
            fi
            wait "$!"
        "#,
        );
        command.arg(temp.path()).arg(leader_behavior);
        let mut helper = Helper::spawn(command).unwrap();
        let leader = helper.child.as_ref().unwrap().id();
        wait_for(&ready);
        let descendant: u32 = fs::read_to_string(ready).unwrap().trim().parse().unwrap();
        let pids = format!("{leader},{descendant}");
        let alive = || {
            let output = Command::new("/bin/ps")
                .args(["-p", &pids, "-o", "pgid=,state="])
                .output()
                .unwrap();
            assert!(
                output.status.success() || output.status.code() == Some(1),
                "ps failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter(|line| {
                    let mut columns = line.split_whitespace();
                    let group: u32 = columns.next().unwrap().parse().unwrap();
                    // launchd may retain an exited orphan as a zombie in CI.
                    group == leader && !columns.next().unwrap().starts_with('Z')
                })
                .count()
        };
        assert_eq!(
            alive(),
            2,
            "both fixture processes must be live in the helper group"
        );
        if leader_behavior == "fail" {
            fs::write(temp.path().join("fail"), "").unwrap();
            let error = helper
                .receive(Some(READY), Duration::from_secs(5))
                .unwrap_err();
            assert!(error.contains("확인 응답 없이"), "{error}");
        } else {
            assert!(
                helper
                    .receive(Some(READY), Duration::from_millis(50))
                    .is_err()
            );
        }
        drop(helper);
        let deadline = Instant::now() + Duration::from_secs(8);
        while alive() > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(alive(), 0, "a live fixture process survived cancellation");
        if leader_behavior == "exit" {
            assert!(
                temp.path().join("terminated").exists(),
                "leader should exit on TERM"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn timeout_kills_a_term_resistant_child_even_after_its_leader_exits() {
        assert_cancellation_stops_group("exit");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn timeout_stops_a_term_resistant_leader_and_child() {
        assert_cancellation_stops_group("ignore");
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn pre_handshake_exit_stops_descendants_with_closed_output() {
        assert_cancellation_stops_group("fail");
    }
}
