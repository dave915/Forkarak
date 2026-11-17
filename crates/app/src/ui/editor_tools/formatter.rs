//! Explicit trusted formatter execution: argv + stdin, bounded output, no shell.
use super::*;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
struct Formatter {
    path: String,
    args: Vec<String>,
}
fn run(config: Formatter, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    run_with_timeout(config, root, file, text, Duration::from_secs(15))
}
fn run_with_timeout(
    config: Formatter,
    root: &Path,
    file: &Path,
    text: &str,
    timeout: Duration,
) -> Result<String, String> {
    let args: Vec<_> = config
        .args
        .iter()
        .map(|arg| {
            arg.replace("{file}", &root.join(file).to_string_lossy())
                .replace("{path}", &root.join(file).to_string_lossy())
                .replace("{root}", &root.to_string_lossy())
        })
        .collect();
    let mut command = Command::new(&config.path);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("포맷터 실행 실패: {e}"))?;
    #[cfg(target_os = "macos")]
    let group = rustix::process::Pid::from_raw(child.id() as i32);
    let mut input = child.stdin.take().ok_or("포맷터 입력 없음")?;
    let text = text.to_owned();
    let writer = std::thread::spawn(move || input.write_all(text.as_bytes()));
    let stdout = child.stdout.take().ok_or("포맷터 출력 없음")?;
    let stderr = child.stderr.take().ok_or("포맷터 오류 출력 없음")?;
    let out = std::thread::spawn(move || {
        let mut bytes = vec![];
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = vec![];
        stderr
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
            result => {
                #[cfg(target_os = "macos")]
                if let Some(group) = group {
                    let _ =
                        rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => format!("포맷터 상태 확인 실패: {e}"),
                    _ => "포맷터가 15초 안에 완료되지 않았습니다. 초안은 유지했습니다.".into(),
                });
            }
        }
    };
    // A formatter that exits with a background child must not retain our pipes.
    #[cfg(target_os = "macos")]
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let output = out
        .join()
        .map_err(|_| "포맷터 출력 오류")?
        .map_err(|e| e.to_string())?;
    let error = err
        .join()
        .map_err(|_| "포맷터 오류 출력 실패")?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "포맷터 종료 {status}: {}",
            String::from_utf8_lossy(&error)
        ));
    }
    writer
        .join()
        .map_err(|_| "포맷터 입력 오류")?
        .map_err(|e| e.to_string())?;
    if output.len() > 4 * 1024 * 1024 {
        return Err("포맷터 출력이 4MB를 초과했습니다.".into());
    }
    String::from_utf8(output).map_err(|_| "포맷터 출력이 UTF-8이 아닙니다.".into())
}
impl Pokarak {
    pub(in crate::ui) fn format_code_file(
        &mut self,
        save_after: bool,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.git_is_busy() {
            return;
        }
        let Some((root, path, editor)) = self.code_tools_target(cx) else {
            return;
        };
        let Some(key) = FileKey::new(&root, &path) else {
            return;
        };
        if !self.code_edits.editing(&root, &path) {
            self.start_code_edit(window, cx);
        }
        if !self.code_edits.editing(&root, &path) || self.code_edits.tools.formatting.contains(&key)
        {
            return;
        }
        let project = self.current_project.as_deref();
        if !self.prefs.trusted.contains(&root)
            && !project.is_some_and(|p| self.prefs.trusted.iter().any(|t| t == p))
        {
            self.status =
                "포맷터는 프로젝트 신뢰를 허용한 뒤 실행할 수 있습니다. 초안과 파일은 유지됩니다."
                    .into();
            self.trust_prompt = true;
            cx.notify();
            return;
        }
        let language = language(&path);
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let configured = self
            .prefs
            .option(project, "formatters.config")
            .and_then(|v| v.get(language).or_else(|| v.get(extension)));
        let config = configured.and_then(|v| {
            let path = v.get("path")?.as_str()?.to_owned();
            if path.trim().is_empty() {
                return None;
            }
            let args = v
                .get("args")
                .and_then(|v| v.as_array())
                .map(|values| {
                    values
                        .iter()
                        .map(|v| v.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
                .unwrap_or(Some(vec![]))?;
            Some(Formatter { path, args })
        });
        let Some(config) = config else {
            self.status = format!(
                "{language} 포맷터 실행 경로와 인수를 설정하세요. 입력은 stdin, 결과는 stdout입니다. 파일은 저장하지 않았습니다."
            );
            cx.notify();
            return;
        };
        let text = editor.read(cx).value().to_string();
        let expected = text.clone();
        let id = editor.entity_id();
        let target = key.clone();
        self.code_edits.tools.formatting.insert(key.clone());
        self.status = "포맷터 실행 중… 입력을 계속하면 오래된 결과는 적용하지 않습니다.".into();
        let task =
            cx.background_spawn(async move { run(config, &target.root, &target.path, &text) });
        cx.spawn_in(window, async move |app, cx| {
            let result = task.await;
            let _ = app.update_in(cx, |app, window, cx| {
                app.code_edits.tools.formatting.remove(&key);
                if app.current.as_ref() != Some(&root) || app.selected().as_ref() != Some(&path) {
                    return;
                }
                if !app.prefs.trusted.contains(&root)
                    && !app
                        .current_project
                        .as_ref()
                        .is_some_and(|p| app.prefs.trusted.contains(p))
                {
                    app.status = "신뢰가 해제되어 포맷 결과를 적용하지 않았습니다.".into();
                    cx.notify();
                    return;
                }
                match result.and_then(|replacement| {
                    app.apply_editor_draft(&root, &path, id, &expected, replacement, window, cx)
                }) {
                    Ok(()) => {
                        app.status = "포맷 결과를 초안에 반영했습니다. ⌘S로 저장하세요.".into();
                        if save_after {
                            app.save_code_file_after_format(commit, window, cx);
                        }
                    }
                    Err(error) => app.status = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
#[cfg(test)]
mod tests {
    use super::{Formatter, run, run_with_timeout};
    use std::path::Path;
    use std::time::{Duration, Instant};
    #[test]
    fn formatter_reads_stdin_and_returns_output_without_writing_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "disk unchanged").unwrap();
        let formatted = run(
            Formatter {
                path: "/usr/bin/tr".into(),
                args: vec!["a-z".into(), "A-Z".into()],
            },
            root.path(),
            Path::new("a.txt"),
            "draft hello",
        )
        .unwrap();
        assert_eq!(formatted, "DRAFT HELLO");
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "disk unchanged"
        );
    }
    #[test]
    fn formatter_arguments_are_not_interpreted_by_a_shell() {
        let root = tempfile::tempdir().unwrap();
        let formatted = run(
            Formatter {
                path: "/usr/bin/printf".into(),
                args: vec!["%s".into(), "$(touch must-not-exist); {file}".into()],
            },
            root.path(),
            Path::new("--name.txt"),
            "",
        )
        .unwrap();
        assert!(formatted.starts_with("$(touch must-not-exist); "));
        assert!(!root.path().join("must-not-exist").exists());
    }

    #[test]
    fn formatter_deadline_stops_owned_process() {
        let root = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = run_with_timeout(
            Formatter {
                path: "/bin/sleep".into(),
                args: vec!["30".into()],
            },
            root.path(),
            Path::new("a.txt"),
            "",
            Duration::from_millis(20),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
