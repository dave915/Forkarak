//! Bounded, in-memory diagnostics belong to the exact worktree that ran the job.
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;

const HISTORY_LIMIT: usize = 20;
const OUTPUT_LIMIT: usize = 32 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone)]
pub(super) struct GitRecord {
    pub(super) id: u64,
    label: String,
    started: SystemTime,
    finished: Option<SystemTime>,
    status: Status,
    stdout: String,
    stderr: String,
    truncated: bool,
    live_stdout: Vec<u8>,
    live_stderr: Vec<u8>,
    dropped: usize,
    kind: Option<pokarak_core::operations::errors::ErrorKind>,
}

pub(super) fn begin(
    history: &mut HashMap<PathBuf, VecDeque<GitRecord>>,
    root: &Path,
    id: u64,
    label: &str,
) {
    let records = history.entry(root.to_owned()).or_default();
    records.push_front(GitRecord {
        id,
        label: label.into(),
        started: SystemTime::now(),
        finished: None,
        status: Status::Running,
        stdout: String::new(),
        stderr: String::new(),
        truncated: false,
        live_stdout: vec![],
        live_stderr: vec![],
        dropped: 0,
        kind: None,
    });
    records.truncate(HISTORY_LIMIT);
}

fn bounded(text: &str) -> (String, bool) {
    if text.len() <= OUTPUT_LIMIT {
        return (text.into(), false);
    }
    let mut end = OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

pub(super) fn finish(
    history: &mut HashMap<PathBuf, VecDeque<GitRecord>>,
    root: &Path,
    id: u64,
    result: &anyhow::Result<OperationOutput>,
) {
    let Some(record) = history
        .get_mut(root)
        .and_then(|records| records.iter_mut().find(|r| r.id == id))
    else {
        return;
    };
    record.finished = Some(SystemTime::now());
    let (stdout, stderr, truncated) = match result {
        Ok(output) => {
            record.status = Status::Succeeded;
            (
                output.stdout.clone(),
                output.stderr.clone(),
                output.truncated,
            )
        }
        // The operations API returns failed command diagnostics as one error
        // chain. Keep it explicitly labelled; do not invent a stdout split.
        Err(error) => {
            record.status = Status::Failed;
            record.kind = Some(pokarak_core::operations::errors::classify(error));
            (
                String::from_utf8_lossy(&record.live_stdout).into_owned(),
                format!("{error:#}"),
                false,
            )
        }
    };
    let (stdout, stdout_cut) = bounded(&stdout);
    let (stderr, stderr_cut) = bounded(&stderr);
    record.stdout = stdout;
    record.stderr = stderr;
    record.truncated = truncated || stdout_cut || stderr_cut;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HelpAction {
    Fetch,
    Changes,
    Integrate,
    Terminal,
    Workflow,
    None,
}

pub(super) struct ErrorHelp {
    pub message: &'static str,
    action: HelpAction,
}

pub(super) fn classified_help(kind: pokarak_core::operations::errors::ErrorKind) -> ErrorHelp {
    use pokarak_core::operations::errors::ErrorKind;
    let action = match kind {
        ErrorKind::Authentication | ErrorKind::FilesystemPermission | ErrorKind::RepositoryLock => {
            HelpAction::Terminal
        }
        ErrorKind::RejectedUpdate => HelpAction::Fetch,
        ErrorKind::Conflict => HelpAction::Workflow,
        ErrorKind::Cancelled | ErrorKind::Timeout | ErrorKind::Other => HelpAction::None,
    };
    ErrorHelp {
        message: pokarak_core::operations::errors::guidance(kind),
        action,
    }
}

pub(super) fn error_help(error: &str) -> ErrorHelp {
    let kind = pokarak_core::operations::errors::classify(&anyhow::anyhow!(error.to_owned()));
    if kind != pokarak_core::operations::errors::ErrorKind::Other {
        return classified_help(kind);
    }
    let diagnostic = error.split("\nFor authentication failures, configure your existing credential helper or SSH agent in a terminal, then retry.").next().unwrap_or(error);
    let text = diagnostic.to_lowercase();
    let (message, action) = if [
        "diverg",
        "not possible to fast-forward",
        "fast-forward is not possible",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        (
            "로컬과 원격 이력이 갈라졌습니다. Fetch 후 그래프에서 비교하고 Merge 또는 Rebase를 직접 선택하세요.",
            HelpAction::Integrate,
        )
    } else if [
        "non-fast-forward",
        "[rejected]",
        "fetch first",
        "remote rejected",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        (
            "Push가 거절되었습니다. Fetch 후 원격 변경과 서버 정책을 확인하세요. 필요한 통합 작업은 직접 선택하세요.",
            HelpAction::Fetch,
        )
    } else if [
        "local changes",
        "working tree",
        "working-tree",
        "untracked",
        "overwritten",
        "index contains",
        "not clean",
        "dirty",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        (
            "로컬 변경 때문에 중단했습니다. Stage / 커밋에서 변경을 확인하고 보관 또는 커밋 여부를 직접 결정하세요.",
            HelpAction::Changes,
        )
    } else {
        (
            "아래 Git 진단을 확인하고 대상을 다시 선택하세요. 자동 재시도는 하지 않습니다.",
            HelpAction::None,
        )
    };
    ErrorHelp { message, action }
}

pub(super) fn relative_time(time: SystemTime, now: SystemTime) -> String {
    let Ok(age) = now.duration_since(time) else {
        return "방금 (시계 차이)".into();
    };
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}초 전")
    } else if seconds < 3600 {
        format!("{}분 전", seconds / 60)
    } else if seconds < 86400 {
        format!("{}시간 전", seconds / 3600)
    } else {
        format!("{}일 전", seconds / 86400)
    }
}

fn timestamp(time: SystemTime) -> String {
    relative_time(time, SystemTime::now())
}

impl Pokarak {
    pub(in crate::ui) fn git_running_status(&self) -> Option<String> {
        let (root, label) = self.git_ui.busy.as_ref()?;
        if self.current.as_ref() != Some(root) {
            return None;
        }
        let elapsed = self
            .git_ui
            .started
            .map(|t| t.elapsed().as_secs())
            .unwrap_or_default();
        Some(format!("{label} · Git 실행 / 결과 대기 · {elapsed}초"))
    }

    pub(super) fn tick_git_elapsed(&self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |view, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(100)).await;
                let keep = view
                    .update_in(cx, |app, window, cx| {
                        if app.git_ui.job_id != id || app.git_ui.busy.is_none() {
                            return false;
                        }
                        app.drain_git_progress(window, cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::ui) fn open_git_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let records = weak.upgrade().filter(|view| view.read(cx).current.as_ref() == Some(&root)).and_then(|view| view.read(cx).git_ui.history.get(&root).cloned()).unwrap_or_default();
            let mut body = div().id("git-log-records").v_flex().gap_3().max_h(ui_px(345.)).overflow_y_scroll().child(scope(&root, None)).child(note("워크트리별 최근 20개 작업 · 앱 종료 시 지워집니다. 각 출력은 최대 32 KiB입니다."));
            if records.is_empty() { body = body.child(note("이 워크트리에서 실행한 Git 작업이 없습니다.")); }
            for record in records {
                let status = match record.status { Status::Running => "실행 중 · Git 결과 대기", Status::Succeeded => "완료", Status::Failed => "실패" };
                let elapsed = record.finished.unwrap_or_else(SystemTime::now).duration_since(record.started).unwrap_or_default().as_secs();
                let target = weak.clone(); let root = root.clone(); let item = record.clone();
                body = body.child(div().v_flex().gap_1().child(format!("{} · {status} · {elapsed}초", record.label)).child(note(&timestamp(record.started))).when(record.status == Status::Failed, |row| row.child(note(error_help(&record.stderr).message))).child(theme::action_button(("git-log-output", record.id as usize), false).label("출력 및 도움말…").small().on_click(move |_, w, cx| {
                    let _ = target.update(cx, |app, cx| { if app.current.as_ref() == Some(&root) { w.close_dialog(cx); app.show_git_output(root.clone(), item.clone(), w, cx); } });
                })));
            }
            dialog.title("Git 작업 기록").width(ui_px(680.)).margin_top(ui_px(16.)).child(body).footer(close_row())
        });
    }

    pub(super) fn show_git_output(
        &mut self,
        root: PathBuf,
        record: GitRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = record.text(&root);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .soft_wrap(true)
                .default_value(text)
        });
        editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
        self.git_ui.live_editor = Some((root.clone(), record.id, editor.clone()));
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let current = weak
                .upgrade()
                .and_then(|view| {
                    view.read(cx)
                        .git_ui
                        .history
                        .get(&root)
                        .and_then(|records| records.iter().find(|r| r.id == record.id))
                        .cloned()
                })
                .unwrap_or_else(|| record.clone());
            let help = match current.kind {
                Some(kind) if kind != pokarak_core::operations::errors::ErrorKind::Other => {
                    classified_help(kind)
                }
                _ => error_help(&current.stderr),
            };
            let mut buttons = div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap_2()
                .child(close_button());
            let target = weak.clone();
            let back_root = root.clone();
            buttons = buttons.child(
                theme::action_button("git-log-back", false)
                    .label("기록 목록")
                    .on_click(move |_, w, cx| {
                        let _ = target.update(cx, |app, cx| {
                            if app.current.as_ref() == Some(&back_root) {
                                w.close_dialog(cx);
                                app.open_git_log(w, cx);
                            }
                        });
                    }),
            );
            if current.status == Status::Running {
                let target = weak.clone();
                let root = root.clone();
                let id = record.id;
                buttons = buttons.child(
                    theme::action_button("git-output-cancel", false)
                        .label("작업 취소")
                        .on_click(move |_, _, cx| {
                            let _ =
                                target.update(cx, |app, cx| app.cancel_git_action(&root, id, cx));
                        }),
                );
            }
            if current.status == Status::Failed && help.action != HelpAction::None {
                let target = weak.clone();
                let root = root.clone();
                let action = help.action;
                let label = match action {
                    HelpAction::Fetch => "Fetch 대상 확인…",
                    HelpAction::Changes => "Stage / 커밋…",
                    HelpAction::Integrate => "브랜치 통합…",
                    HelpAction::Terminal => "워크트리 터미널",
                    HelpAction::None => "",
                    HelpAction::Workflow => "진행 중 작업 / 충돌 해결…",
                };
                buttons = buttons.child(
                    theme::action_button("git-log-help", false)
                        .label(label)
                        .on_click(move |_, w, cx| {
                            let _ = target.update(cx, |app, cx| {
                                if app.current.as_ref() != Some(&root) {
                                    return;
                                }
                                w.close_dialog(cx);
                                match action {
                                    HelpAction::Fetch => {
                                        app.load_git_page(Some(Page::Fetch), w, cx)
                                    }
                                    HelpAction::Changes => app.open_git_commit(w, cx),
                                    HelpAction::Workflow => app.open_git_workflow(w, cx),
                                    HelpAction::Integrate => {
                                        app.load_git_page(Some(Page::Merge), w, cx)
                                    }
                                    HelpAction::Terminal => {
                                        #[cfg(target_os = "macos")]
                                        app.open_worktree_terminal(w, cx);
                                    }
                                    HelpAction::None => {}
                                }
                            });
                        }),
                );
            }
            dialog
                .title(format!("{} — Git 출력", record.label))
                .width(ui_px(760.))
                .margin_top(ui_px(16.))
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .when(current.status == Status::Failed, |body| {
                            body.child(note(help.message))
                        })
                        .child(
                            Editor::new(&editor)
                                .readonly(true)
                                .bordered(true)
                                .h(ui_px(280.)),
                        ),
                )
                .footer(buttons)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{HelpAction, OUTPUT_LIMIT, Status, begin, bounded, error_help, finish};
    use pokarak_core::operations::OperationOutput;
    use std::{collections::HashMap, path::Path};
    #[::core::prelude::v1::test]
    fn history_is_bounded_and_worktree_scoped_and_keeps_success_output() {
        let mut history = HashMap::new();
        let a = Path::new("/fixture/a");
        let b = Path::new("/fixture/b");
        begin(&mut history, b, 100, "other");
        for id in 0..25 {
            begin(&mut history, a, id, "Push");
        }
        assert_eq!(history[a].len(), 20);
        assert_eq!(history[b].len(), 1);
        finish(
            &mut history,
            a,
            24,
            &Ok(OperationOutput {
                root: a.into(),
                stdout: "sent".into(),
                stderr: "upstream set".into(),
                truncated: false,
            }),
        );
        assert!(history[a][0].status == Status::Succeeded);
        assert_eq!(history[a][0].stdout, "sent");
        assert_eq!(history[a][0].stderr, "upstream set");
        assert!(history[a][0].finished.is_some());
        finish(&mut history, a, 0, &Err(anyhow::anyhow!("evicted")));
        assert!(history[b][0].status == Status::Running);
    }
    #[::core::prelude::v1::test]
    fn output_limit_is_utf8_safe() {
        let (text, cut) = bounded(&"가".repeat(OUTPUT_LIMIT));
        assert!(cut);
        assert!(text.len() <= OUTPUT_LIMIT);
        assert!(text.ends_with('가'));
    }
    #[::core::prelude::v1::test]
    fn errors_offer_specific_explicit_actions() {
        assert!(error_help("Permission denied (publickey)").action == HelpAction::Terminal);
        assert!(
            error_help("! [rejected] main -> main (non-fast-forward)").action == HelpAction::Fetch
        );
        assert!(
            error_help("fatal: Not possible to fast-forward, aborting").action
                == HelpAction::Integrate
        );
        assert!(error_help("local changes would be overwritten").action == HelpAction::Changes);
    }
    #[::core::prelude::v1::test]
    fn wrapped_errors_ignore_core_generic_auth_advice() {
        let footer = "\nFor authentication failures, configure your existing credential helper or SSH agent in a terminal, then retry. For rejected updates, fetch and review the branch first. No force or automatic stash is used.";
        for (diagnostic, expected) in [
            (
                "fatal: Not possible to fast-forward, aborting",
                HelpAction::Integrate,
            ),
            (
                "! [rejected] main -> main (non-fast-forward)",
                HelpAction::Fetch,
            ),
            ("local changes would be overwritten", HelpAction::Changes),
            ("Permission denied (publickey)", HelpAction::Terminal),
        ] {
            let full = format!("Git operation failed in /fixture:\n{diagnostic}\n{footer}");
            assert!(error_help(&full).action == expected, "{full}");
        }
    }
    #[::core::prelude::v1::test]
    fn timestamps_are_readable_relative_ages() {
        use std::time::{Duration, SystemTime};
        let then = SystemTime::UNIX_EPOCH;
        assert_eq!(
            super::relative_time(then, then + Duration::from_secs(10)),
            "10초 전"
        );
        assert_eq!(
            super::relative_time(then, then + Duration::from_secs(120)),
            "2분 전"
        );
        assert_eq!(
            super::relative_time(then, then + Duration::from_secs(7200)),
            "2시간 전"
        );
    }
}

pub(super) fn append(
    history: &mut HashMap<PathBuf, VecDeque<GitRecord>>,
    root: &Path,
    id: u64,
    events: Vec<pokarak_core::operations::OperationEvent>,
    dropped: usize,
) {
    let Some(record) = history.get_mut(root).and_then(|records| {
        records
            .iter_mut()
            .find(|r| r.id == id && r.status == Status::Running)
    }) else {
        return;
    };
    record.dropped = dropped;
    for event in events {
        if event.root != root {
            continue;
        }
        let buffer = match event.stream {
            pokarak_core::operations::OutputStream::Stdout => &mut record.live_stdout,
            pokarak_core::operations::OutputStream::Stderr => &mut record.live_stderr,
        };
        let retain = event
            .bytes
            .len()
            .min(OUTPUT_LIMIT.saturating_sub(buffer.len()));
        buffer.extend_from_slice(&event.bytes[..retain]);
        record.truncated |= retain < event.bytes.len();
    }
}

impl GitRecord {
    pub(super) fn text(&self, root: &Path) -> String {
        let running = self.status == Status::Running;
        let stdout = if running {
            String::from_utf8_lossy(&self.live_stdout).into_owned()
        } else {
            self.stdout.clone()
        };
        let stderr = if running {
            String::from_utf8_lossy(&self.live_stderr).into_owned()
        } else {
            self.stderr.clone()
        };
        format!(
            "{}\n워크트리: {}\n시작: {}\n상태: {}\n\nstdout\n{}\n\nstderr / Git 진단\n{}{}{}",
            self.label,
            root.display(),
            timestamp(self.started),
            if running {
                "실행 중 · 취소 후에도 일부 단계가 완료되어 있을 수 있습니다"
            } else if self.status == Status::Succeeded {
                "완료"
            } else {
                "실패 · 현재 상태 확인 필요"
            },
            stdout,
            stderr,
            if self.truncated {
                "\n[출력 보관 한도 32 KiB: 일부 생략]"
            } else {
                ""
            },
            if self.dropped > 0 {
                format!("\n[실시간 큐에서 {}바이트 생략]", self.dropped)
            } else {
                String::new()
            }
        )
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn live_chunks_concatenate_before_utf8_and_reject_foreign_roots() {
        use pokarak_core::operations::{OperationEvent, OutputStream};
        let mut history = HashMap::new();
        let root = Path::new("/a");
        begin(&mut history, root, 3, "Fetch");
        let bytes = "한글\r".as_bytes();
        for chunk in bytes.chunks(2) {
            append(
                &mut history,
                root,
                3,
                vec![OperationEvent {
                    root: root.into(),
                    stream: OutputStream::Stderr,
                    bytes: chunk.into(),
                }],
                0,
            );
        }
        append(
            &mut history,
            root,
            3,
            vec![OperationEvent {
                root: "/b".into(),
                stream: OutputStream::Stderr,
                bytes: b"wrong".into(),
            }],
            12,
        );
        append(
            &mut history,
            root,
            4,
            vec![OperationEvent {
                root: root.into(),
                stream: OutputStream::Stderr,
                bytes: b"stale".into(),
            }],
            0,
        );
        let record = history[root].front().unwrap();
        assert_eq!(record.live_stderr, bytes);
        let text = record.text(root);
        assert!(text.contains("한글\r"));
        assert!(!text.contains('\u{fffd}'));
        assert!(text.contains("12바이트"));
        assert_eq!(
            error_help("Unable to create '.git/index.lock': Permission denied").message,
            pokarak_core::operations::errors::guidance(
                pokarak_core::operations::errors::ErrorKind::FilesystemPermission
            )
        );
    }
}

#[cfg(test)]
mod classification_tests {
    use super::{HelpAction, error_help};
    use pokarak_core::operations::errors::{ErrorKind, guidance};
    #[test]
    fn local_paths_and_locks_never_invent_authentication_failures() {
        assert!(
            error_help("fatal: cannot read credential-cache/data: No such file or directory")
                .action
                == HelpAction::None
        );
        assert_eq!(
            error_help("fatal: Unable to create '.git/index.lock': Permission denied").message,
            guidance(ErrorKind::FilesystemPermission)
        );
        assert_eq!(
            error_help("fatal: Unable to create '.git/index.lock': File exists").message,
            guidance(ErrorKind::RepositoryLock)
        );
    }
}
