//! Bounded, in-memory diagnostics belong to the exact worktree that ran the job.
use super::*;

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
    id: u64,
    label: String,
    started: SystemTime,
    finished: Option<SystemTime>,
    status: Status,
    stdout: String,
    stderr: String,
    truncated: bool,
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
            (String::new(), format!("{error:#}"), false)
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
    None,
}

pub(super) struct ErrorHelp {
    pub message: &'static str,
    action: HelpAction,
}

pub(super) fn error_help(error: &str) -> ErrorHelp {
    let text = error.to_lowercase();
    let (message, action) = if [
        "authentication",
        "permission denied",
        "could not read username",
        "terminal prompts disabled",
        "publickey",
        "credential",
        "http 401",
        "http 403",
        "returned error: 403",
        "returned error: 401",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        (
            "인증에 실패했습니다. 워크트리 터미널에서 Git 인증 또는 SSH 키를 확인한 뒤 작업을 다시 열어 실행하세요.",
            HelpAction::Terminal,
        )
    } else if [
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

fn timestamp(time: SystemTime) -> String {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(value) => format!("Unix {}", value.as_secs()),
        Err(_) => "시간 알 수 없음".into(),
    }
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
                smol::Timer::after(Duration::from_secs(1)).await;
                let keep = view
                    .update_in(cx, |app, _, cx| {
                        if app.git_ui.job_id != id || app.git_ui.busy.is_none() {
                            return false;
                        }
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
        window.open_dialog(cx, move |dialog, _, cx| {
            let records = weak.upgrade().filter(|view| view.read(cx).current.as_ref() == Some(&root)).and_then(|view| view.read(cx).git_ui.history.get(&root).cloned()).unwrap_or_default();
            let mut body = div().id("git-log-records").v_flex().gap_3().max_h(ui_px(345.)).overflow_y_scroll().child(scope(&root, None)).child(note("워크트리별 최근 20개 작업 · 앱 종료 시 지워집니다. 각 출력은 최대 32 KiB입니다."));
            if records.is_empty() { body = body.child(note("이 워크트리에서 실행한 Git 작업이 없습니다.")); }
            for record in records {
                let status = match record.status { Status::Running => "실행 중 · Git 결과 대기", Status::Succeeded => "완료", Status::Failed => "실패" };
                let elapsed = record.finished.unwrap_or_else(SystemTime::now).duration_since(record.started).unwrap_or_default().as_secs();
                let target = weak.clone(); let root = root.clone(); let item = record.clone();
                body = body.child(div().v_flex().gap_1().child(format!("{} · {status} · {elapsed}초", record.label)).child(note(&timestamp(record.started))).when(record.status == Status::Failed, |row| row.child(note(error_help(&record.stderr).message))).child(theme::action_button(("git-log-output", record.id as usize), record.status == Status::Running).label("출력 및 도움말…").small().on_click(move |_, w, cx| {
                    let _ = target.update(cx, |app, cx| { if app.current.as_ref() == Some(&root) { w.close_dialog(cx); app.show_git_output(root.clone(), item.clone(), w, cx); } });
                })));
            }
            dialog.title("Git 작업 기록").width(ui_px(680.)).margin_top(ui_px(16.)).child(body).footer(close_row())
        });
    }

    fn show_git_output(
        &mut self,
        root: PathBuf,
        record: GitRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = format!(
            "{}\n워크트리: {}\n시작: {}\n종료: {}\n\nstdout{}\n{}\n\n{}\n{}{}",
            record.label,
            root.display(),
            timestamp(record.started),
            record
                .finished
                .map(timestamp)
                .unwrap_or_else(|| "실행 중".into()),
            if record.status == Status::Failed {
                " (실패 시 별도 제공되지 않음)"
            } else {
                ""
            },
            record.stdout,
            if record.status == Status::Failed {
                "Git 오류 진단 (stderr 포함)"
            } else {
                "stderr"
            },
            record.stderr,
            if record.truncated {
                "\n\n[출력 일부 생략]"
            } else {
                ""
            }
        );
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .soft_wrap(true)
                .default_value(text)
        });
        editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let help = error_help(&record.stderr);
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
            if record.status == Status::Failed && help.action != HelpAction::None {
                let target = weak.clone();
                let root = root.clone();
                let action = help.action;
                let label = match action {
                    HelpAction::Fetch => "Fetch 대상 확인…",
                    HelpAction::Changes => "Stage / 커밋…",
                    HelpAction::Integrate => "브랜치 통합…",
                    HelpAction::Terminal => "워크트리 터미널",
                    HelpAction::None => "",
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
                        .when(record.status == Status::Failed, |body| {
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
    use super::{begin, finish, bounded, error_help, HelpAction, Status, OUTPUT_LIMIT};
    use std::{collections::HashMap, path::Path};
    use pokarak_core::operations::OperationOutput;
    #[test]
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
    #[test]
    fn output_limit_is_utf8_safe() {
        let (text, cut) = bounded(&"가".repeat(OUTPUT_LIMIT));
        assert!(cut);
        assert!(text.len() <= OUTPUT_LIMIT);
        assert!(text.ends_with('가'));
    }
    #[test]
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
}
