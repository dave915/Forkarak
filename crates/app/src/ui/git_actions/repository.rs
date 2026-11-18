//! Explicit Clone/Init with an exclusive new destination and owned cancellation.
//! No analysis is started by setup or by viewing the result.
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;
use pokarak_core::operations::{
    OperationProgress,
    repository::{clone_repository, init_repository_with_control},
};

#[derive(Clone)]
struct Setup {
    destination: PathBuf,
    source: String,
    clone: bool,
}
pub(super) struct SetupResult {
    root: PathBuf,
    id: u64,
    message: String,
    inspectable: bool,
}
fn destination(value: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(value.trim());
    anyhow::ensure!(
        path.is_absolute() && path.file_name().is_some(),
        "새 저장소의 절대 경로를 입력하세요."
    );
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("상위 폴더가 필요합니다."))?
        .canonicalize()?;
    anyhow::ensure!(parent.is_dir(), "기존 상위 폴더를 선택하세요.");
    let path = parent.join(path.file_name().unwrap());
    anyhow::ensure!(
        !path.try_exists()? && std::fs::symlink_metadata(&path).is_err(),
        "아직 존재하지 않는 새 폴더를 선택하세요. 기존 폴더를 덮어쓰지 않습니다."
    );
    Ok(path)
}
impl Pokarak {
    pub(in crate::ui) fn open_repository_setup(
        &mut self,
        clone: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() {
            return;
        }
        let default_parent = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let path = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                default_parent
                    .join("new-project")
                    .to_string_lossy()
                    .to_string(),
            )
        });
        let source = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(if clone { "" } else { "main" })
                .placeholder(if clone {
                    "https://… 또는 SSH URL / 로컬 저장소 경로"
                } else {
                    "초기 브랜치 이름"
                })
        });
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_app_dialog(cx,move |dialog,_,cx| {
            let title = if clone { "Clone · 저장소 복제" } else { "Init · 새 저장소 만들기" };
            let validation = destination(&path.read(cx).value()).map_err(|e|e.to_string());
            let mut body = div().v_flex().gap_3().child(field(if clone { "복제할 URL / 로컬 저장소" } else { "초기 브랜치" },&source)).child(field("새 저장소 폴더 · 절대 경로",&path))
                .child(note("상위 폴더가 이미 있고 대상 폴더는 없어야 합니다. Clone은 네트워크에 접속할 수 있습니다. 하위 모듈은 자동 복제하지 않습니다. 작업 실패 / 취소 후 남은 파일은 자동 삭제하지 않습니다."));
            if let Err(error) = &validation { body = body.child(note(error)); }
            let disabled = validation.is_err() || source.read(cx).value().trim().is_empty();
            let (weak,path,source) = (weak.clone(),path.clone(),source.clone());
            dialog.title(title).width(ui_px(700.)).child(body).footer(div().flex().justify_end().gap_2().child(close_button()).child(theme::action_button("repository-setup-review",disabled).label("대상 확인…").on_click(move |_,w,cx| {
                let Ok(destination) = destination(&path.read(cx).value()) else { return; };
                let setup = Setup { destination,source:source.read(cx).value().trim().to_owned(),clone };
                let _ = weak.update(cx, |app,cx| app.confirm_repository_setup(setup,w,cx));
            })))
        });
    }
    fn confirm_repository_setup(
        &mut self,
        setup: Setup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_app_dialog(cx,move |dialog,_,cx| {
            let target = weak.clone(); let selected = setup.clone();
            let disabled = weak.upgrade().is_none_or(|view| view.read(cx).git_is_busy());
            dialog.title(if setup.clone { "Clone 확인" } else { "Init 확인" }).width(ui_px(700.))
                .child(div().v_flex().gap_3().child(format!("대상 폴더: {}\n{}: {}",setup.destination.display(),if setup.clone { "복제 원본" } else { "초기 브랜치" },setup.source))
                    .child(note("이 새 폴더에 저장소를 작성합니다. 기존 저장소와 전역 Git 설정은 변경하지 않습니다. 취소 시 일부 데이터가 남을 수 있습니다. 완료 후 직접 열면 파일을 분석 없이 볼 수 있습니다.")))
                .footer(div().flex().justify_end().gap_2().child(close_button()).child(theme::action_button("repository-setup-execute",disabled).label(if setup.clone { "Clone 실행" } else { "Init 실행" }).primary().on_click(move |_,w,cx| {
                    let _ = target.update(cx, |app,cx| { w.close_dialog(cx); app.run_repository_setup(selected.clone(),w,cx); });
                })))
        });
    }
    fn run_repository_setup(&mut self, setup: Setup, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy() {
            return;
        }
        let root = setup.destination.clone();
        let label = if setup.clone { "Clone" } else { "Init" };
        let cancellation = OperationCancellation::default();
        let progress = OperationProgress::default();
        self.git_ui.job_id += 1;
        let id = self.git_ui.job_id;
        self.git_ui.setup_job = true;
        self.git_ui.setup_result = None;
        self.git_ui.busy = Some((root.clone(), label.into()));
        self.git_ui.started = Some(std::time::Instant::now());
        self.git_ui.cancellation = Some(cancellation.clone());
        self.git_ui.progress = Some(progress.clone());
        history::begin(&mut self.git_ui.history, &root, id, label);
        self.tick_git_elapsed(id, window, cx);
        let task = cx.background_spawn(async move {
            let result = if setup.clone {
                clone_repository(&setup.source, &setup.destination, cancellation, progress)
            } else {
                init_repository_with_control(
                    &setup.destination,
                    &setup.source,
                    cancellation,
                    progress,
                )
            };
            let inspection =
                pokarak_core::snapshot_current(&setup.destination, &CompareMode::Uncommitted);
            let exists = setup.destination.exists();
            (result, inspection, exists)
        });
        cx.spawn_in(window, async move |view, cx| {
            let (result, inspection, exists) = task.await;
            let _ = view.update_in(cx, |app, w, cx| {
                if !super::activity::job_matches(&app.git_ui, &root, id) {
                    return;
                }
                app.drain_git_progress(w, cx);
                history::finish(&mut app.git_ui.history, &root, id, &result);
                app.git_ui.busy = None;
                app.git_ui.started = None;
                app.git_ui.cancellation = None;
                app.git_ui.progress = None;
                app.git_ui.setup_job = false;
                let message = match result {
                    Ok(_) => format!("{label} 완료 · {}", root.display()),
                    Err(error) => format!(
                        "{label} 실패 / 취소 · {}\n{error:#}\n{}\n{}",
                        root.display(),
                        pokarak_core::operations::errors::guidance(
                            pokarak_core::operations::errors::classify(&error)
                        ),
                        if exists {
                            "대상 폴더에 남은 데이터를 보존했습니다."
                        } else {
                            "대상 폴더가 없습니다."
                        }
                    ),
                };
                let message = if let Err(error) = &inspection {
                    format!("{message}\n저장소 재확인: {error:#}")
                } else {
                    format!(
                        "{message}\n저장소 상태를 다시 확인했습니다. 분석은 시작하지 않았습니다."
                    )
                };
                app.git_ui.setup_result = Some(SetupResult {
                    root,
                    id,
                    message,
                    inspectable: inspection.is_ok(),
                });
                app.update_git_live_editor(w, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(in crate::ui) fn render_repository_result(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let result = self.git_ui.setup_result.as_ref()?;
        let root = result.root.clone();
        let output_root = root.clone();
        let id = result.id;
        Some(
            div()
                .flex_none()
                .v_flex()
                .gap_1()
                .px_3()
                .py_1()
                .child(
                    div()
                        .id("repository-result-message")
                        .max_h(ui_px(80.))
                        .overflow_y_scroll()
                        .child(result.message.clone()),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .when(result.inspectable, |d| {
                            d.child(
                                Button::new("repository-setup-open")
                                    .label("저장소 열기")
                                    .small()
                                    .on_click(cx.listener(move |app, _, w, cx| {
                                        app.open_repository(root.clone(), w, cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("repository-setup-output")
                                .label("출력 보기…")
                                .small()
                                .on_click(cx.listener(move |app, _, w, cx| {
                                    if let Some(record) = app
                                        .git_ui
                                        .history
                                        .get(&output_root)
                                        .and_then(|records| records.iter().find(|r| r.id == id))
                                        .cloned()
                                    {
                                        app.show_git_output(output_root.clone(), record, w, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("repository-setup-dismiss")
                                .label("닫기")
                                .small()
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.git_ui.setup_result = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::destination;
    #[test]
    fn setup_requires_new_absolute_destination_and_real_parent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("new");
        assert_eq!(
            destination(path.to_str().unwrap()).unwrap(),
            temp.path().canonicalize().unwrap().join("new")
        );
        std::fs::create_dir(&path).unwrap();
        assert!(destination(path.to_str().unwrap()).is_err());
        assert!(destination("relative").is_err());
        assert!(destination(temp.path().join("missing/new").to_str().unwrap()).is_err());
    }
}
