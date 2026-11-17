//! Explicit stdin/stdout formatter processes; never writes a source file.
use crate::{
    CancellationHandle, DocumentSnapshot, FormatterConfig, Position, TextEdit, TextRange,
    config::discover_program, paths::end_position,
};
use anyhow::{Context, Result, bail};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(crate) fn format(
    snapshot: &DocumentSnapshot,
    config: &FormatterConfig,
    cancellation: &CancellationHandle,
    timeout: Duration,
) -> Result<Vec<TextEdit>> {
    snapshot.check()?;
    cancellation.check()?;
    let program = discover_program(&config.program)
        .context("Configured formatter executable was not found")?;
    let args: Vec<_> = config
        .args
        .iter()
        .map(|arg| {
            arg.replace("{root}", &snapshot.root().to_string_lossy())
                .replace("{path}", &snapshot.path().to_string_lossy())
        })
        .collect();
    let mut command = Command::new(program);
    command
        .args(&args)
        .current_dir(snapshot.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let process = cancellation.spawn(&mut command)?;
    let result = (|| -> Result<String> {
        let (mut stdin, stdout, stderr) = process.take_stdio()?;
        let source = snapshot.text().to_owned();
        let (tx, rx) = mpsc::channel();
        let writer = tx.clone();
        thread::spawn(move || {
            let _ = writer.send((0, stdin.write_all(source.as_bytes()).map(|_| vec![])));
        });
        for (kind, mut pipe, limit) in [
            (1, Box::new(stdout) as Box<dyn Read + Send>, 8 * 1024 * 1024),
            (2, Box::new(stderr) as Box<dyn Read + Send>, 16 * 1024),
        ] {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut result = Vec::new();
                let mut buffer = [0; 4096];
                let output = (|| -> std::io::Result<Vec<u8>> {
                    loop {
                        let count = pipe.read(&mut buffer)?;
                        if count == 0 {
                            return Ok(result);
                        }
                        result.extend_from_slice(&buffer[..count]);
                        if kind == 1 && result.len() > limit {
                            return Err(std::io::Error::other("Formatter output exceeds 8 MiB"));
                        }
                        if kind == 2 && result.len() > limit {
                            result.drain(..result.len() - limit);
                        }
                    }
                })();
                let _ = tx.send((kind, output));
            });
        }
        drop(tx);
        let deadline = Instant::now() + timeout;
        let mut output = None;
        let mut stderr = String::new();
        let mut done = 0;
        while done < 3 || process.exit_status().is_none() {
            cancellation.check()?;
            snapshot.check()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("Formatter timed out")?;
            match rx.recv_timeout(remaining.min(Duration::from_millis(20))) {
                Ok((kind, value)) => {
                    let value = value.context("Formatter I/O failed")?;
                    done += 1;
                    if kind == 1 {
                        output = Some(
                            String::from_utf8(value).context("Formatter output is not UTF-8")?,
                        );
                    } else if kind == 2 {
                        stderr = String::from_utf8_lossy(&value).into_owned();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) if done == 3 => {
                    thread::sleep(remaining.min(Duration::from_millis(10)));
                }
                Err(_) => bail!("Formatter I/O worker exited"),
            }
        }
        let status = process.exit_status().context("Missing formatter status")?;
        if !matches!(
            status.as_str(),
            "Language server exited: exit code 0" | "Language server exited: exit status: 0"
        ) {
            bail!("Formatter failed ({status}): {stderr}");
        }
        output.context("Formatter returned no output")
    })();
    // Retain numeric ownership until group termination, including on successful exit.
    let _ = process.terminate();
    process.reap();
    let output = result?;
    snapshot.check()?;
    cancellation.check()?;
    if output == snapshot.text() {
        return Ok(vec![]);
    }
    let (line, character) = end_position(snapshot.text());
    Ok(vec![TextEdit {
        range: TextRange {
            start: Position::default(),
            end: Position { line, character },
        },
        new_text: output,
    }])
}
