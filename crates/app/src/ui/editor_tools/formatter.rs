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
#[cfg(test)]
fn run(config: Formatter, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    run_with_timeout(config, root, file, text, Duration::from_secs(15))
}
#[cfg(test)]
fn run_with_timeout(
    config: Formatter,
    root: &Path,
    file: &Path,
    text: &str,
    timeout: Duration,
) -> Result<String, String> {
    run_controlled(config, root, file, text, timeout, None)
}
fn run_controlled(
    config: Formatter,
    root: &Path,
    file: &Path,
    text: &str,
    timeout: Duration,
    snapshot: Option<pokarak_navigation::DocumentSnapshot>,
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
            Ok(None)
                if start.elapsed() < timeout
                    && snapshot.as_ref().is_none_or(|s| s.is_current()) =>
            {
                std::thread::sleep(Duration::from_millis(15))
            }
            result => {
                #[cfg(target_os = "macos")]
                if let Some(group) = group {
                    let _ =
                        rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    _ if snapshot.as_ref().is_some_and(|s| !s.is_current()) => {
                        "소스·보기·신뢰가 변경되어 포맷터를 취소했습니다. 초안을 유지합니다.".into()
                    }
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
fn resolve_formatter(
    value: Option<&serde_json::Value>,
    path: &Path,
    registrations: Vec<crate::extension_host::ToolRegistration>,
) -> Result<Option<Formatter>, String> {
    let language = pokarak_navigation::language_for_path(path).unwrap_or_else(|| language(path));
    let family = pokarak_navigation::language_family(language);
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let configured = language_tools::formatter_configs(value)?;
    if let Some(config) = configured
        .iter()
        .rev()
        .find(|c| c.language == language || c.language == extension)
        .or_else(|| configured.iter().rev().find(|c| c.language == family))
    {
        return Ok(Some(Formatter {
            path: config.program.clone(),
            args: config.args.clone(),
        }));
    }
    Ok(registrations
        .into_iter()
        .rev()
        .find(|r| {
            r.language == language
                || r.language == family
                || r.file_extensions
                    .iter()
                    .any(|e| e.trim_start_matches('.') == extension)
        })
        .map(|r| Formatter {
            path: r.executable.to_string_lossy().into(),
            args: r.args,
        }))
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
        let language =
            pokarak_navigation::language_for_path(&path).unwrap_or_else(|| language(&path));
        if self
            .prefs
            .option(project, "language.enabled")
            .and_then(|v| {
                v.get(language)
                    .or_else(|| v.get(pokarak_navigation::language_family(language)))
            })
            .and_then(|v| v.as_bool())
            == Some(false)
        {
            self.status = format!("{language} 언어 지원이 꺼져 있습니다.");
            cx.notify();
            return;
        }
        let config = match resolve_formatter(
            self.prefs.option(project, "formatters.config"),
            &path,
            self.registered_extension_formatters(),
        ) {
            Ok(config) => config,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let config = if config.is_some() {
            config
        } else {
            let managed = match self.effective_managed_formatter_configs() {
                Ok(v) => v,
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            };
            let family = pokarak_navigation::language_family(language);
            managed
                .into_iter()
                .rev()
                .find(|c| c.language == language || c.language == family)
                .map(|c| Formatter {
                    path: c.program,
                    args: c.args,
                })
        };
        let Some(config) = config else {
            self.request_language(
                language_tools::Tool::Format { save_after, commit },
                None,
                window,
                cx,
            );
            return;
        };
        if !self.prefs.trusted.contains(&root)
            && !project.is_some_and(|p| self.prefs.trusted.iter().any(|t| t == p))
        {
            self.status =
                "포맷터는 프로젝트 신뢰를 허용한 뒤 실행할 수 있습니다. 초안과 파일은 유지됩니다."
                    .into();
            self.show_explicit_analysis_prompt();
            cx.notify();
            return;
        }
        let identity = match self.external_format_identity(cx) {
            Ok(v) => v,
            Err(e) => {
                self.status = e;
                cx.notify();
                return;
            }
        };
        let source = identity.snapshot.clone();
        let text = editor.read(cx).value().to_string();
        let expected = text.clone();
        let id = editor.entity_id();
        let target = key.clone();
        self.code_edits.tools.formatting.insert(key.clone());
        self.status = "포맷터 실행 중… 입력을 계속하면 오래된 결과는 적용하지 않습니다.".into();
        let task = cx.background_spawn(async move {
            run_controlled(
                config,
                &target.root,
                &target.path,
                &text,
                Duration::from_secs(15),
                Some(source),
            )
        });
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
                if !app.accepts_external_format(&identity, cx) {
                    app.status =
                        "선택·초안·설정이 변경되어 포맷 결과를 적용하지 않았습니다.".into();
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
    fn invalidated_snapshot_cancels_a_running_external_formatter() {
        let root = tempfile::tempdir().unwrap();
        let source = pokarak_navigation::DocumentSnapshot::new(
            root.path().canonicalize().unwrap(),
            "x.ts".into(),
            "draft".into(),
            1,
        )
        .unwrap();
        let cancel = source.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(35));
            cancel.invalidate();
        });
        let started = Instant::now();
        let result = super::run_controlled(
            Formatter {
                path: "/bin/sleep".into(),
                args: vec!["30".into()],
            },
            root.path(),
            Path::new("x.ts"),
            "draft",
            Duration::from_secs(15),
            Some(source),
        );
        thread.join().unwrap();
        assert!(result.unwrap_err().contains("취소"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn configured_formatter_overrides_enabled_extension_and_family_handles_react() {
        let extension = crate::extension_host::ToolRegistration {
            extension_id: "fixture".into(),
            language: "typescript".into(),
            executable: "/tmp/extension formatter".into(),
            args: vec!["--stdin".into()],
            file_extensions: vec!["tsx".into()],
        };
        let config = serde_json::json!({"typescript":{"path":"/tmp/custom formatter","args":["{path}","$(literal)"]}});
        let selected = super::resolve_formatter(
            Some(&config),
            Path::new("src/Component.tsx"),
            vec![extension.clone()],
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.path, "/tmp/custom formatter");
        assert_eq!(selected.args[1], "$(literal)");
        assert_eq!(
            super::resolve_formatter(None, Path::new("src/Component.tsx"), vec![extension])
                .unwrap()
                .unwrap()
                .path,
            "/tmp/extension formatter"
        );
        assert!(
            super::resolve_formatter(
                Some(&serde_json::json!({"rust":{"path":"rustfmt","args":[8]}})),
                Path::new("a.rs"),
                vec![]
            )
            .is_err()
        );
    }
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
