//! Terminal.app provides the PTY. Its login process can discard the environment
//! of the GUI that opened it, so hand the worktree and exported variables to a
//! one-shot helper over a socket accessible only to the current user.
use anyhow::{Context, Result, bail, ensure};
use smol::{io::AsyncWriteExt, net::unix::UnixListener};
use std::{
    env,
    ffi::OsStr,
    fs,
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{PermissionsExt, symlink},
        net::UnixStream,
        process::CommandExt,
    },
    path::Path,
    process::Command,
    time::Duration,
};

pub const HELPER_ARG: &str = "--worktree-zsh";
const LAUNCHER: &str =
    "#!/bin/sh\nexec \"${0%/*}/pokarak\" --worktree-zsh \"${0%/*}/session.sock\"\n";

pub async fn open(root: &Path) -> Result<()> {
    ensure!(root.is_dir(), "워크트리 폴더를 찾을 수 없습니다");
    // A short path fits the macOS sockaddr_un limit even with a long TMPDIR.
    let dir = tempfile::Builder::new()
        .prefix("pokarak-zsh-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")?;
    let listener = UnixListener::bind(dir.path().join("session.sock"))?;
    symlink(env::current_exe()?, dir.path().join("pokarak"))?;
    let launcher = dir.path().join("Pokarak.command");
    fs::write(&launcher, LAUNCHER)?;
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))?;
    let deliver = async {
        let result = smol::process::Command::new("/usr/bin/open")
            .args(["-b", "com.apple.Terminal"])
            .arg(&launcher)
            .output()
            .await?;
        ensure!(
            result.status.success(),
            "Terminal.app: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );

        let (mut client, _) = listener.accept().await?;
        let mut payload = root.as_os_str().as_bytes().to_vec();
        payload.push(0);
        for (key, value) in env::vars_os() {
            // The new terminal supplies its own PTY, dimensions, and session ID.
            if matches!(
                key.as_bytes(),
                b"TERM"
                    | b"TERM_PROGRAM"
                    | b"TERM_PROGRAM_VERSION"
                    | b"TERM_SESSION_ID"
                    | b"TTY"
                    | b"COLUMNS"
                    | b"LINES"
                    | b"SHLVL"
                    | b"PWD"
                    | b"OLDPWD"
                    | b"_"
            ) {
                continue;
            }
            payload.extend(key.as_bytes());
            payload.push(b'=');
            payload.extend(value.as_bytes());
            payload.push(0);
        }
        payload.push(0);
        client.write_all(&payload).await?;
        Ok(())
    };
    let timeout = async {
        smol::Timer::after(Duration::from_secs(90)).await;
        bail!("Terminal.app에서 zsh를 시작하지 못했습니다. 다시 시도해 주세요.")
    };
    smol::future::or(deliver, timeout).await
}

fn shell_command(payload: &[u8]) -> Result<Command> {
    let data = payload
        .strip_suffix(b"\0\0")
        .context("터미널 환경을 모두 받지 못했습니다")?;
    let mut fields = data.split(|&b| b == 0);
    let root = Path::new(OsStr::from_bytes(fields.next().unwrap_or_default()));
    ensure!(root.is_absolute(), "워크트리 경로가 올바르지 않습니다");
    let mut command = Command::new("/bin/zsh");
    command.arg("-il").current_dir(root);
    for field in fields {
        let eq = field
            .iter()
            .position(|&b| b == b'=')
            .context("터미널 환경 형식 오류")?;
        command.env(
            OsStr::from_bytes(&field[..eq]),
            OsStr::from_bytes(&field[eq + 1..]),
        );
    }
    command.env("SHELL", "/bin/zsh");
    Ok(command)
}

/// Runs inside the new Terminal.app PTY, before GPUI is initialized.
pub fn exec_shell(socket: &OsStr) -> Result<()> {
    let mut payload = Vec::new();
    UnixStream::connect(socket)?.read_to_end(&mut payload)?;
    Err(shell_command(&payload)?.exec()).context("워크트리에서 zsh를 실행할 수 없습니다")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_literal_paths_and_exported_bytes() {
        let command =
            shell_command(b"/tmp/a ' $(literal)\xff\0QA=one=two\n$HOME\xff\0EMPTY=\0\0").unwrap();
        assert_eq!(
            command.get_current_dir().unwrap().as_os_str().as_bytes(),
            b"/tmp/a ' $(literal)\xff"
        );
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["-il"]);
        let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            vars[OsStr::new("QA")].unwrap().as_bytes(),
            b"one=two\n$HOME\xff"
        );
        assert_eq!(vars[OsStr::new("EMPTY")], Some(OsStr::new("")));
    }

    #[test]
    fn incomplete_handoff_does_not_launch_a_shell() {
        for malformed in [
            b"/tmp\0KEY=part".as_slice(),
            b"relative\0\0",
            b"/tmp\0invalid\0\0",
        ] {
            assert!(shell_command(malformed).is_err());
        }
    }
}
