//! One ephemeral test run per worktree. Saved tasks have their own runtime and namespace.
mod results;
#[cfg(test)]
mod tests;
use super::*;
use crate::task_runtime::{RunStatus, TaskRun};
use crate::test_report::{TestSummary, read_junit_reports};
use crate::test_target::TestTarget;
use language_tools::RequestIdentity;

#[derive(Clone, Copy)]
pub(in crate::ui) enum TestCommand {
    Current,
    File,
    Rerun,
}

#[derive(Clone)]
struct Request {
    generation: u64,
    workspace: u64,
    navigation: u64,
    root: PathBuf,
    project: PathBuf,
    origin: Option<RequestIdentity>,
}

/// Only target discovery depends on a live language snapshot. An approved
/// Save All can replace its text and temporarily make the editor read-only.
struct CaretSource<'a> {
    view: u64,
    cursor: usize,
    text: &'a str,
}

impl Request {
    fn matches_reader(
        &self,
        path: &Path,
        editor: EntityId,
        navigation: u64,
        caret: Option<CaretSource<'_>>,
    ) -> bool {
        self.origin.as_ref().is_some_and(|origin| {
            origin.key.path == path
                && origin.editor == editor
                && self.navigation == navigation
                && caret.is_none_or(|source| {
                    origin.view == source.view
                        && origin.snapshot.is_current()
                        && origin.cursor == source.cursor
                        && origin.snapshot.text() == source.text
                })
        })
    }
}

#[derive(Clone)]
enum Selection {
    Editor { line: u32, file_scope: bool },
    Previous(TestTarget),
}

struct Launch {
    request: Request,
    target: TestTarget,
    require_clean: bool,
}

struct ResultView {
    id: RunId,
    project: PathBuf,
    target: TestTarget,
    started: SystemTime,
    logs: SelectedLogs,
    status: RunStatus,
    cancelled: bool,
    summary: Option<TestSummary>,
    revisions: HashMap<PathBuf, FileRevision>,
    report_error: Option<String>,
}

impl ResultView {
    fn label(&self) -> String {
        if let Some(summary) = &self.summary {
            return format!(
                "테스트 {}개 중 {}개 실패",
                summary.total,
                summary.failures.len()
            );
        }
        let status = if self.cancelled && !self.status.active() {
            "중지됨".to_owned()
        } else {
            self.status.label()
        };
        format!("테스트 {status} · {}", self.target.label)
    }
}

#[derive(Default)]
pub(super) struct TestRuns {
    runtime: TaskRuntime,
    results: HashMap<PathBuf, ResultView>,
    generation: u64,
    awaiting_trust: Option<(Request, Selection)>,
    after_stop: HashMap<PathBuf, (Launch, LogOptions)>,
}

impl TestRuns {
    fn next_request(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.awaiting_trust = None;
        self.generation
    }

    pub(super) fn trust_generation(&self) -> Option<u64> {
        self.awaiting_trust.as_ref().map(|(r, _)| r.generation)
    }

    pub(super) fn leave_trust_screen(&mut self) {
        self.awaiting_trust = None;
    }

    pub(super) fn shutdown_root(&mut self, root: &Path) {
        if self
            .awaiting_trust
            .as_ref()
            .is_some_and(|(r, _)| r.root == root)
        {
            self.awaiting_trust = None;
        }
        self.after_stop.remove(root);
        self.results.remove(root);
        self.runtime.shutdown_root(root);
    }

    pub(super) fn shutdown(&mut self) {
        self.next_request();
        self.after_stop.clear();
        // Preserve the runtime's monotonically increasing RunIds across Close All / reopen.
        for root in std::mem::take(&mut self.results).into_keys() {
            self.runtime.shutdown_root(&root);
        }
    }

    fn run(&self, root: &Path, id: RunId) -> Option<&TaskRun> {
        let view = self.results.get(root).filter(|v| v.id == id)?;
        self.runtime
            .run(root, &view.target.profile.name)
            .filter(|r| r.id == id)
    }

    pub(super) fn logs(&self, root: &Path, id: RunId) -> Option<SelectedLogs> {
        self.results
            .get(root)
            .filter(|view| view.id == id)
            .map(|view| view.logs.clone())
    }

    fn stop(&mut self, root: &Path, id: RunId) -> bool {
        let Some(view) = self.results.get_mut(root).filter(|v| v.id == id) else {
            return false;
        };
        if !self
            .runtime
            .run(root, &view.target.profile.name)
            .is_some_and(|r| r.id == id && r.process.status().active())
        {
            return false;
        }
        view.cancelled = self.runtime.stop(root, &view.target.profile.name, id);
        view.cancelled
    }

    fn cancel(&mut self, root: &Path, id: RunId) {
        if self.results.get(root).is_some_and(|run| run.id == id) {
            self.next_request();
            self.after_stop.remove(root);
            self.stop(root, id);
        }
    }

    fn submit(
        &mut self,
        launch: Launch,
        trusted: bool,
        options: LogOptions,
    ) -> anyhow::Result<Option<RunId>> {
        let root = &launch.request.root;
        anyhow::ensure!(trusted, "이 워크트리의 실행을 먼저 허용하세요");
        launch.target.profile.validate(root)?;
        self.after_stop.remove(root);
        if let Some(id) = self.results.get(root).map(|v| v.id)
            && self.stop(root, id)
        {
            self.after_stop.insert(root.clone(), (launch, options));
            return Ok(None);
        }
        let previous_status = self
            .results
            .get(root)
            .and_then(|view| self.run(root, view.id))
            .map(|run| run.process.status());
        self.runtime.shutdown_root(root);
        let started = SystemTime::now();
        let id = match self
            .runtime
            .start(root, launch.target.profile.clone(), trusted, options)
        {
            Ok(id) => id,
            Err(error) => {
                if let Some(view) = self.results.get_mut(root)
                    && let Some(status) = previous_status
                {
                    view.status = status;
                }
                return Err(error);
            }
        };
        let run = self.runtime.run(root, &launch.target.profile.name).unwrap();
        let logs = SelectedLogs {
            buffer: run.logs.clone(),
            cwd: run.cwd.clone(),
        };
        self.results.insert(
            root.clone(),
            ResultView {
                id,
                project: launch.request.project,
                target: launch.target,
                started,
                logs,
                status: RunStatus::Running,
                cancelled: false,
                summary: None,
                revisions: HashMap::new(),
                report_error: None,
            },
        );
        Ok(Some(id))
    }

    fn ready_replacements(&mut self) -> Vec<(Launch, LogOptions)> {
        let ready: Vec<_> = self
            .after_stop
            .keys()
            .filter(|root| {
                self.results.get(*root).is_some_and(|v| {
                    !self
                        .run(root, v.id)
                        .is_some_and(|run| run.process.status().active())
                })
            })
            .cloned()
            .collect();
        ready
            .into_iter()
            .filter_map(|root| self.after_stop.remove(&root))
            .collect()
    }

    fn finish_report(
        &mut self,
        root: &Path,
        id: RunId,
        report: anyhow::Result<Option<TestSummary>>,
        revisions: HashMap<PathBuf, FileRevision>,
    ) -> bool {
        let Some(view) = self
            .results
            .get_mut(root)
            .filter(|v| v.id == id && !v.cancelled)
        else {
            return false;
        };
        match report {
            Ok(summary) => {
                view.summary = summary;
                view.revisions = revisions;
            }
            Err(error) => view.report_error = Some(format!("테스트 보고서 읽기 실패 · {error}")),
        }
        true
    }
}

impl Pokarak {
    pub(in crate::ui) fn has_last_test(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|root| self.execution.tests.results.contains_key(root))
    }

    pub(in crate::ui) fn run_editor_test(
        &mut self,
        command: TestCommand,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading
            || self.git_is_busy()
            || self.opening_path.is_some()
            || w.has_active_dialog(cx)
        {
            return;
        }
        let (Some(root), Some(project)) = (self.current.clone(), self.current_project.clone())
        else {
            return;
        };
        let (origin, selection) = match command {
            TestCommand::Rerun => {
                let Some(last) = self.execution.tests.results.get(&root) else {
                    return;
                };
                (None, Selection::Previous(last.target.clone()))
            }
            TestCommand::Current | TestCommand::File => {
                let Ok(origin) = self.language_request_identity(cx) else {
                    return;
                };
                if self
                    .session()
                    .is_none_or(|s| s.historical_documents.contains(&origin.key.path))
                {
                    return;
                }
                let line = self
                    .active_editor()
                    .unwrap()
                    .read(cx)
                    .cursor_position()
                    .line;
                (
                    Some(origin),
                    Selection::Editor {
                        line,
                        file_scope: matches!(command, TestCommand::File),
                    },
                )
            }
        };
        let request = Request {
            generation: self.execution.tests.next_request(),
            workspace: self.request,
            navigation: self.navigation_request,
            root: root.clone(),
            project,
            origin,
        };
        if !self.execution_trusted(&root) {
            self.open_tasks(w, cx);
            self.execution.tests.awaiting_trust = Some((request, selection));
        } else {
            self.resolve_test(request, selection, w, cx);
        }
    }

    // Save All may format this same editor. Its completion still requires the original reader and worktree.
    fn test_request_current(&self, request: &Request, check_text: bool, cx: &App) -> bool {
        self.current.as_ref() == Some(&request.root)
            && self.current_project.as_ref() == Some(&request.project)
            && self.request == request.workspace
            && self.execution.tests.generation == request.generation
            && self.opening_path.is_none()
            && !self.loading
            && !self.git_is_busy()
            && request.origin.as_ref().is_none_or(|origin| {
                !self.git_history.active
                    && !self.file_loading
                    && self.session().is_some_and(|s| {
                        s.saved.code && !s.historical_documents.contains(&origin.key.path)
                    })
                    && self
                        .selected()
                        .zip(self.active_editor())
                        .is_some_and(|(path, editor)| {
                            let state = editor.read(cx);
                            let text = check_text.then(|| state.value());
                            let caret = text.as_ref().map(|text| CaretSource {
                                view: self.language_tools.view,
                                cursor: state.cursor(),
                                text: text.as_ref(),
                            });
                            request.matches_reader(
                                &path,
                                editor.entity_id(),
                                self.navigation_request,
                                caret,
                            )
                        })
            })
    }

    pub(super) fn resume_trusted_test(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some((request, selection)) = self.execution.tests.awaiting_trust.take()
            && self.test_request_current(&request, true, cx)
        {
            self.execution.visible = false;
            self.focus_addressed_reader(w, cx);
            self.resolve_test(request, selection, w, cx);
        }
    }

    fn resolve_test(
        &mut self,
        request: Request,
        selection: Selection,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.test_request_current(&request, true, cx) || !self.execution_trusted(&request.root)
        {
            return;
        }
        let (line, file_scope) = match selection {
            Selection::Previous(target) => {
                self.confirm_test(request, target, w, cx);
                return;
            }
            Selection::Editor { line, file_scope } => (line, file_scope),
        };
        let origin = request.origin.as_ref().unwrap().clone();
        let settings = match self.execution.registry.get(SPRING_ID) {
            Ok(extension) => self.extension_settings(&extension, &request.root),
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
                return;
            }
        };
        let navigator = self.navigation.clone().filter(|_| !file_scope);
        let detect = cx.background_spawn(async move {
            let symbols = navigator
                .and_then(|nav| {
                    let mut nav = nav.lock().ok()?;
                    if nav.root() != origin.key.root || !origin.snapshot.is_current() {
                        return None;
                    }
                    nav.document_symbols(&origin.snapshot).ok().map(|r| r.value)
                })
                .unwrap_or_default();
            crate::test_target::detect(
                &origin.key.root,
                &origin.key.path,
                origin.snapshot.text(),
                line,
                file_scope,
                &symbols,
                &settings,
            )
        });
        cx.spawn_in(w, async move |view, cx| {
            let result = detect.await;
            let _ = view.update_in(cx, |app, w, cx| {
                if !app.test_request_current(&request, true, cx)
                    || !app.execution_trusted(&request.root)
                    || w.has_active_dialog(cx)
                {
                    return;
                }
                match result {
                    Ok(Some(target)) => app.confirm_test(request, target, w, cx),
                    Ok(None) => {
                        app.open_tasks(w, cx);
                        app.execution.message = Some("이 파일의 테스트 실행 방법을 찾지 못했습니다 · 실행 프로필에서 직접 추가".into());
                    }
                    Err(error) => app.status = format!("테스트 대상 확인 실패 · {error:#}"),
                }
                cx.notify();
            });
        }).detach();
    }

    fn confirm_test(
        &mut self,
        request: Request,
        target: TestTarget,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capture_code_buffers(cx);
        let count = self.code_edits.dirty_count(&request.root);
        if count == 0 {
            self.launch_editor_test(
                Launch {
                    request,
                    target,
                    require_clean: true,
                },
                cx,
            );
            return;
        }
        let owner = cx.weak_entity();
        w.open_app_dialog(cx, move |dialog, _, _| {
            let disk_owner = owner.clone();
            let disk_request = request.clone();
            let disk_target = target.clone();
            let save_owner = owner.clone();
            let save_request = request.clone();
            let save_target = target.clone();
            dialog
                .title(format!("미저장 파일 {count}개"))
                .child(format!(
                    "{} 테스트는 디스크에 저장된 내용으로 실행합니다.",
                    target.label
                ))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("test-run-cancel")
                                .label("취소")
                                .on_click(|_, w, cx| w.close_dialog(cx)),
                        )
                        .child(Button::new("test-run-disk").label("그대로 실행").on_click(
                            move |_, w, cx| {
                                w.close_dialog(cx);
                                let _ = disk_owner.update(cx, |app, cx| {
                                    if app.test_request_current(&disk_request, true, cx) {
                                        app.launch_editor_test(
                                            Launch {
                                                request: disk_request.clone(),
                                                target: disk_target.clone(),
                                                require_clean: false,
                                            },
                                            cx,
                                        );
                                    }
                                });
                            },
                        ))
                        .child(
                            Button::new("test-run-save-all")
                                .primary()
                                .label("모두 저장 후 실행")
                                .on_click(move |_, w, cx| {
                                    w.close_dialog(cx);
                                    let _ = save_owner.update(cx, |app, cx| {
                                        app.save_before_test(
                                            save_request.clone(),
                                            save_target.clone(),
                                            w,
                                            cx,
                                        );
                                    });
                                }),
                        ),
                )
        });
    }

    fn save_before_test(
        &mut self,
        request: Request,
        target: TestTarget,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.test_request_current(&request, true, cx) || !self.execution_trusted(&request.root)
        {
            return;
        }
        let started = self.save_all_code_files_then(w, cx, move |app, root, report, _, cx| {
            if root != request.root || !app.test_request_current(&request, false, cx) {
                return;
            }
            if !report.failures.is_empty() || report.remaining_dirty > 0 {
                app.status = format!(
                    "테스트 실행 보류 · 저장 실패 {}개 · 미저장 파일 {}개",
                    report.failures.len(),
                    report.remaining_dirty
                );
                return;
            }
            app.launch_editor_test(
                Launch {
                    request,
                    target,
                    require_clean: true,
                },
                cx,
            );
        });
        if let Err(error) = started {
            self.status = error;
            cx.notify();
        }
    }

    fn launch_editor_test(&mut self, launch: Launch, cx: &mut Context<Self>) {
        let root = &launch.request.root;
        if !self.test_request_current(&launch.request, false, cx) || !self.execution_trusted(root) {
            return;
        }
        self.capture_code_buffers(cx);
        let options = self.execution_options();
        self.start_accepted_test(launch, options, cx);
    }

    fn start_accepted_test(&mut self, launch: Launch, options: LogOptions, cx: &mut Context<Self>) {
        let root = launch.request.root.clone();
        if launch.require_clean && self.code_edits.dirty_count(&root) > 0 {
            if self.current.as_ref() == Some(&root) {
                self.status = "미저장 파일이 변경되었습니다. 테스트를 다시 실행하세요.".into();
            }
            cx.notify();
            return;
        }
        let trusted = self.execution_trusted(&root);
        match self.execution.tests.submit(launch, trusted, options) {
            Ok(None) => {}
            Ok(Some(id)) => {
                self.execution
                    .views
                    .entry(root.clone())
                    .or_default()
                    .selected = Some(LogSource::Test(id));
                if self.current.as_ref() == Some(&root) {
                    self.execution.message = None;
                }
            }
            Err(error) => {
                if self.current.as_ref() == Some(&root) {
                    self.status = format!("테스트 실행 실패 · {error:#}");
                } else if let Some(view) = self.execution.tests.results.get_mut(&root) {
                    view.report_error = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    pub(super) fn tick_editor_tests(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.execution.tests.results.is_empty() {
            return;
        }
        let mut finished = Vec::new();
        let mut removed = Vec::new();
        for (root, view) in &mut self.execution.tests.results {
            if self.current_project.as_ref() == Some(&view.project)
                && !self.loading
                && self.opening_path.is_none()
                && !self
                    .inventory
                    .iter()
                    .any(|tree| tree.path == *root && !tree.is_prunable)
            {
                removed.push(root.clone());
                continue;
            }
            let Some(run) = self
                .execution
                .tests
                .runtime
                .run(root, &view.target.profile.name)
            else {
                continue;
            };
            let status = run.process.status();
            if status == view.status {
                continue;
            }
            let ended = view.status.active() && !status.active();
            view.status = status;
            if ended && !view.cancelled && !view.target.report_dirs.is_empty() {
                finished.push((
                    root.clone(),
                    view.id,
                    view.started,
                    view.target.report_dirs.clone(),
                ));
            }
            if self.current.as_ref() == Some(root) {
                cx.notify();
            }
        }
        for root in removed {
            self.execution.shutdown_root(&root);
        }
        for (launch, options) in self.execution.tests.ready_replacements() {
            self.start_accepted_test(launch, options, cx);
        }
        for (root, id, started, dirs) in finished {
            let base = root.clone();
            let read = cx.background_spawn(async move {
                let report = read_junit_reports(&base, &dirs, started);
                let revisions = report
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .into_iter()
                    .flat_map(|s| &s.failures)
                    .filter_map(|f| f.location.as_ref())
                    .filter_map(|loc| Some((loc.path.clone(), file_revision(&loc.path)?)))
                    .collect();
                (report, revisions)
            });
            cx.spawn_in(w, async move |view, cx| {
                let (report, revisions) = read.await;
                let _ = view.update(cx, |app, cx| {
                    if app
                        .execution
                        .tests
                        .finish_report(&root, id, report, revisions)
                        && app.current.as_ref() == Some(&root)
                    {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}
