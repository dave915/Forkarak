//! Explicit Git actions; sidebar selection navigates without mutating Git.
mod activity;
mod branch_forms;
mod compact;
mod contextual;
mod history;
mod ignore;
mod network;
mod recovery;
mod repository;
mod review;
mod source;
mod tags;
mod worktree_manager;
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;
pub(in crate::ui) use contextual::branch_context_menu;
pub(super) mod integration;
use gpui_kit::component::{
    IndexPath,
    select::{SearchableVec, Select, SelectState},
};
use integration::IntegrationKind;
use pokarak_core::operations::{
    DeleteMode, GitOperations, Operation, OperationCancellation, OperationOutput, Remote,
    RemoteUrlKind, RepositoryState,
};

type Choice = Entity<SelectState<SearchableVec<String>>>;

#[derive(Default)]
pub(super) struct GitUi {
    busy: Option<(PathBuf, String)>,
    cancellation: Option<OperationCancellation>,
    loading: bool,
    load_request: u64,
    completions: HashMap<PathBuf, GitCompletion>,
    history: HashMap<PathBuf, VecDeque<history::GitRecord>>,
    job_id: u64,
    started: Option<std::time::Instant>,
    progress: Option<pokarak_core::operations::OperationProgress>,
    live_editor: Option<(PathBuf, u64, Entity<EditorState>)>,
    setup_job: bool,
    setup_result: Option<repository::SetupResult>,
    sync: HashMap<PathBuf, network::SyncRead>,
    sync_request: u64,
}

#[derive(Clone)]
struct GitCompletion {
    status: String,
    error: Option<String>,
}

struct GitExtra {
    root: PathBuf,
    label: String,
    detail: String,
    destructive: bool,
}

struct GitConfirmation {
    root: PathBuf,
    label: String,
    detail: String,
    operation: Operation,
    destructive: bool,
}

struct GitQuitOwner(WeakEntity<Pokarak>);
impl Global for GitQuitOwner {}

pub(super) fn request_app_quit(_: &Quit, cx: &mut App) {
    let owner = cx.try_global::<GitQuitOwner>().map(|owner| owner.0.clone());
    let blocked = owner
        .and_then(|view| view.update(cx, |app, cx| app.block_git_close(cx)).ok())
        .unwrap_or(false);
    if !blocked {
        cx.quit();
    }
}

#[derive(Clone, Copy)]
pub(super) enum Page {
    Log,
    Worktrees,
    Create,
    Checkout,
    Delete,
    Fetch,
    Push,
    Pull,
    Remotes,
    Identity,
    Merge,
    Rebase,
    Conflicts,
    CompareRefs,
    Reflog,
    Undo,
    Ignore,
    CreateTag,
    PushTag,
    Clone,
    Init,
}

impl Pokarak {
    pub(super) fn install_git_lifecycle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        cx.set_global(GitQuitOwner(weak.clone()));
        window.on_window_should_close(cx, move |_, cx| {
            !weak
                .update(cx, |app, cx| app.block_git_close(cx))
                .unwrap_or(false)
        });
    }

    fn block_git_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.block_code_close(cx) {
            return true;
        }
        if self.updates.restarting() {
            return true;
        }
        if let Some((root, label)) = &self.git_ui.busy {
            self.status = format!(
                "{label} 진행 중 — 완료 후 창을 닫거나 종료하세요. {}",
                root.display()
            );
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Native OS/Dock quit bypasses menu actions and cannot be vetoed by GPUI.
    /// Synchronously reap our owned mutating process before its task is dropped.
    pub(super) fn cancel_git_job(&mut self) {
        if let Some(cancellation) = self.git_ui.cancellation.take() {
            cancellation.cancel();
        }
    }

    pub(super) fn invalidate_git_page(&mut self) {
        self.git_ui.load_request += 1;
        self.git_ui.loading = false;
    }

    pub(super) fn active_worktree_has_git_job(&self) -> bool {
        self.git_ui
            .busy
            .as_ref()
            .is_some_and(|(root, _)| self.current.as_ref() == Some(root))
    }

    pub(super) fn git_is_busy(&self) -> bool {
        self.git_ui.busy.is_some() || self.git_ui.loading || self.updates.restarting()
    }

    pub(super) fn run_git_job(
        &mut self,
        root: PathBuf,
        label: String,
        job: impl FnOnce(GitOperations) -> anyhow::Result<OperationOutput> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() {
            self.error = Some("다른 Git 작업이 진행 중입니다. 완료 후 다시 실행하세요.".into());
            cx.notify();
            return;
        }
        if self.current.as_ref() != Some(&root) || self.loading || self.opening_path.is_some() {
            self.error = Some(
                "작업 대상 워크트리가 바뀌었습니다. Git 작업을 다시 열어 대상을 확인하세요.".into(),
            );
            self.status = "Git 작업 대상 확인 필요".into();
            cx.notify();
            return;
        }
        self.persist(cx);
        // Invalidate in-flight navigation, retaining an explicitly enabled
        // server until actual working-source identities have been compared.
        self.navigation_request += 1;
        if let Some(cancel) = self.cancellation.take() {
            let _ = cancel.cancel();
        }
        self.navigating = false;
        let had_analysis = self.navigation.is_some();
        self.request += 1;
        self.checking_changes = false;
        self.count_pending = false;
        self.file_request += 1;
        self.file_loading = false;
        let token = self.request;
        let compare = self
            .session()
            .map(|s| s.saved.comparison.clone())
            .unwrap_or_default();
        self.git_ui.sync_request += 1;
        self.git_ui.sync.remove(&root);
        self.git_ui.completions.remove(&root);
        self.git_ui.job_id += 1;
        let job_id = self.git_ui.job_id;
        self.git_ui.started = Some(std::time::Instant::now());
        history::begin(&mut self.git_ui.history, &root, job_id, &label);
        self.git_ui.busy = Some((root.clone(), label.clone()));
        self.error = None;
        self.status = format!("{label} 진행 중 — {}", root.display());
        self.tick_git_elapsed(job_id, window, cx);
        let cancellation = OperationCancellation::default();
        self.git_ui.cancellation = Some(cancellation.clone());
        let progress = pokarak_core::operations::OperationProgress::default();
        self.git_ui.progress = Some(progress.clone());
        let job_root = root.clone();
        let mode = compare.clone();
        let task = cx.background_spawn(async move {
            let before = had_analysis.then(|| source::stamp(&job_root)).transpose();
            let result = GitOperations::open(&job_root)
                .map(|git| {
                    git.with_cancellation(cancellation.clone())
                        .with_progress(progress)
                })
                .and_then(job);
            // Refresh even on failure: a fetch, hook, or timed-out operation may
            // have completed part of its work. Always inspect the original root.
            // A cancelled command's handle cannot perform inspection. These
            // independent reads always run, including partial failures.
            let snapshot = pokarak_core::snapshot_current_inventory(&job_root, &mode);
            let unchanged = !had_analysis || source::unchanged(before, source::stamp(&job_root));
            (result, snapshot, unchanged)
        });
        cx.spawn_in(window, async move |view, cx| {
            let (result, snapshot, unchanged) = task.await;
            let _ = view.update_in(cx, |this, window, cx| {
                if !activity::job_matches(&this.git_ui, &root, job_id) { return; }
                this.drain_git_progress(window, cx);
                this.git_ui.progress = None;
                this.git_ui.busy = None;
                this.git_ui.started = None;
                history::finish(&mut this.git_ui.history, &root, job_id, &result);
                this.git_ui.cancellation = None;
                if !unchanged && this.current.as_ref() == Some(&root) {
                    this.stop_analysis(cx);
                    this.results = Arc::from([]);
                    this.result_revisions.clear();
                    this.results_open = false;
                }
                let succeeded = result.is_ok();
                let sync_needs_refresh = snapshot.is_err() || this.request != token;
                let mut completion = match result {
                    Ok(output) => GitCompletion {
                        status: format!(
                            "{label} 완료 — {}{}",
                            root.display(),
                            if output.truncated {
                                " (Git 출력 일부 생략)"
                            } else {
                                ""
                            }
                        ),
                        error: None,
                    },
                    Err(error) => GitCompletion {
                        status: format!("{label} 실패 — {}", root.display()),
                        error: Some(format!("{label}\n워크트리: {}\n{error:#}\n\n{}\nGit 작업 기록에서 전체 출력을 확인하세요.", root.display(), history::classified_help(pokarak_core::operations::errors::classify(&error)).message)),
                    },
                };
                if let Err(error) = &snapshot {
                    let message = format!(
                        "작업 후 새로고침 실패: {error:#}\n비교 기준을 확인하고 새로고침하세요."
                    );
                    completion.error = Some(match completion.error.take() {
                        Some(previous) => format!("{previous}\n\n{message}"),
                        None => message,
                    });
                    completion.status.push_str("; 새로고침 필요");
                }
                this.git_ui.completions.insert(root.clone(), completion);
                if this.current.as_ref() == Some(&root) {
                    this.capture_positions(cx);
                    if snapshot.is_err() {
                        this.external_change = true;
                    }
                }
                if let Some(session) = this.sessions.get_mut(&root) {
                    session.diffs.clear();
                    session.documents.clear();
                }
                if this.current.as_ref() == Some(&root) && this.opening_path.is_none() {
                    if this.request == token {
                        if let Ok(snapshot) = snapshot {
                            this.external_change = false;
                            this.install_snapshot(snapshot, window, cx);
                        }
                        this.restore_git_completion(cx);
                    } else {
                        // The reader/comparison changed during the operation.
                        // A new snapshot uses that current comparison instead.
                        this.refresh(window, cx);
                        if let Some(completion) = this.git_ui.completions.get(&root) {
                            this.status = completion.status.clone();
                            if let Some(error) = &completion.error {
                                this.error = Some(error.clone());
                            }
                        }
                    }
                } else if let Some(session) = this.sessions.get_mut(&root)
                    && session.saved.comparison == compare
                    && let Ok(snapshot) = snapshot
                {
                    session.snapshot = Some(snapshot);
                }
                if sync_needs_refresh && this.current.as_ref() == Some(&root) {
                    this.refresh_branch_sync(window, cx);
                }
                this.finish_resolution_save(succeeded);
                this.commit_job_finished(&root, succeeded, window, cx);
                this.refresh_workflow_status(window, cx);
                this.update_git_live_editor(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Called after a snapshot/file installs, including returning to a worktree
    /// whose operation finished while another project was visible.
    pub(super) fn restore_git_completion(&mut self, cx: &mut Context<Self>) {
        let Some(root) = &self.current else {
            return;
        };
        if let Some(completion) = self.git_ui.completions.get(root) {
            self.status = completion.status.clone();
            if let Some(error) = &completion.error {
                self.error = Some(error.clone());
            }
            if !self.file_loading {
                self.git_ui.completions.remove(root);
            }
            cx.notify();
        }
    }

    pub(super) fn open_git_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_git_page(None, window, cx);
    }

    pub(super) fn load_git_page(
        &mut self,
        page: Option<Page>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() || self.loading {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        self.git_ui.loading = true;
        self.git_ui.load_request += 1;
        let token = self.git_ui.load_request;
        let generation = self.request;
        let load_root = root.clone();
        let task = cx.background_spawn(async move { GitOperations::open(&load_root)?.state() });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |this, window, cx| {
                if this.git_ui.load_request != token {
                    return;
                }
                this.git_ui.loading = false;
                if this.current.as_ref() != Some(&root)
                    || this.request != generation
                    || this.opening_path.is_some()
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(state) => match page {
                        Some(page) => this.show_git_page(Arc::new(state), page, window, cx),
                        None => this.show_git_home(Arc::new(state), window, cx),
                    },
                    Err(error) => {
                        this.error = Some(format!(
                            "Git 작업 정보 읽기 실패\n{}\n{error:#}",
                            root.display()
                        ));
                        this.status = "Git 설정을 읽을 수 없습니다".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn show_git_home(
        &mut self,
        state: Arc<RepositoryState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, _| {
            let mut sections = div()
                .id("git-action-sections")
                .max_h(ui_px(355.))
                .overflow_y_scroll()
                .v_flex()
                .gap_3()
                .child(scope(&state.root, state.current_branch.as_deref()));
            for (heading, actions) in [
                (
                    "브랜치",
                    vec![
                        ("만들기…", Page::Create),
                        ("체크아웃…", Page::Checkout),
                        ("삭제…", Page::Delete),
                    ],
                ),
                (
                    "브랜치 통합",
                    vec![
                        ("Merge…", Page::Merge),
                        ("Rebase…", Page::Rebase),
                        ("Stash / 충돌 해결…", Page::Conflicts),
                    ],
                ),
                (
                    "원격 동기화",
                    vec![
                        ("Fetch…", Page::Fetch),
                        ("Push…", Page::Push),
                        ("Pull (ff-only)…", Page::Pull),
                    ],
                ),
                ("워크트리", vec![("워크트리 관리…", Page::Worktrees)]),
                (
                    "이력과 비교",
                    vec![
                        ("두 참조 직접 비교…", Page::CompareRefs),
                        ("Reflog / 이력 복구…", Page::Reflog),
                        ("마지막 커밋 취소…", Page::Undo),
                    ],
                ),
                ("진단", vec![("Git 작업 기록…", Page::Log)]),
                (
                    "제외 규칙",
                    vec![("미추적 파일에서 .gitignore 편집…", Page::Ignore)],
                ),
                (
                    "태그",
                    vec![
                        ("태그 만들기…", Page::CreateTag),
                        ("태그 하나 Push…", Page::PushTag),
                    ],
                ),
                (
                    "저장소 시작",
                    vec![("Clone…", Page::Clone), ("Init…", Page::Init)],
                ),
                (
                    "저장소 Git 설정",
                    vec![
                        ("원격 저장소 관리…", Page::Remotes),
                        ("작성자 이름·이메일…", Page::Identity),
                    ],
                ),
            ] {
                let mut row = div().flex().flex_wrap().gap_2();
                for (index, (label, page)) in actions.into_iter().enumerate() {
                    let weak = weak.clone();
                    let state = state.clone();
                    row = row.child(
                        theme::action_button((SharedString::from(heading), index), false)
                            .label(label)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    window.close_dialog(cx);
                                    app.show_git_page(state.clone(), page, window, cx);
                                });
                            }),
                    );
                }
                sections = sections.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(div().font_weight(FontWeight::MEDIUM).child(heading))
                        .child(row),
                );
            }
            dialog
                .title("Git 작업")
                .width(ui_px(600.))
                .margin_top(ui_px(16.))
                .child(sections)
                .footer(close_row())
        });
    }

    fn show_git_page(
        &mut self,
        state: Arc<RepositoryState>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match page {
            Page::Log => self.open_git_log(window, cx),
            Page::Worktrees => self.show_worktree_manager(state, window, cx),
            Page::Create => self.open_branch_create(None, window, cx),
            Page::Checkout | Page::Delete => self.show_branch_form(state, page, window, cx),
            Page::Fetch | Page::Push | Page::Pull => {
                self.show_network_form(state, page, window, cx)
            }
            Page::Merge => self.open_integration(IntegrationKind::Merge, None, window, cx),
            Page::Rebase => self.open_integration(IntegrationKind::Rebase, None, window, cx),
            Page::Conflicts => self.open_git_workflow(window, cx),
            Page::Remotes => self.show_remote_settings(state, window, cx),
            Page::Identity => self.show_identity_form(state, window, cx),
            Page::CompareRefs => self.show_ref_comparison(state.root.clone(), window, cx),
            Page::Reflog => self.open_reflog(state.root.clone(), window, cx),
            Page::Undo => self.open_undo_last_commit(state.root.clone(), window, cx),
            Page::Ignore => self.open_ignore_picker(state.root.clone(), window, cx),
            Page::CreateTag => self.show_tag_create(state.root.clone(), window, cx),
            Page::PushTag => self.open_tag_push(state, window, cx),
            Page::Clone => self.open_repository_setup(true, window, cx),
            Page::Init => self.open_repository_setup(false, window, cx),
        }
    }

    fn show_branch_form(
        &mut self,
        state: Arc<RepositoryState>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let create = matches!(page, Page::Create);
        let delete = matches!(page, Page::Delete);
        let title = if create {
            "브랜치 만들기"
        } else if delete {
            "브랜치 삭제"
        } else {
            "브랜치 체크아웃"
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("새 로컬 브랜치 이름"));
        let candidates: Vec<_> = state
            .local_branches
            .iter()
            .filter(|b| !state.checked_out_branches.contains(b))
            .cloned()
            .collect();
        let empty = candidates.is_empty();
        let branch = choice(candidates, None, window, cx);
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, _| {
            let mut body = div().v_flex().gap_4().child(scope(&state.root, state.current_branch.as_deref()));
            body = if create { body.child(field("이름", &name)).child(note("현재 HEAD에서 브랜치만 만듭니다. 체크아웃은 별도로 실행합니다.")) }
            else { body.child(select_field("로컬 브랜치", &branch)).child(note(if empty { "선택할 브랜치가 없습니다. 체크아웃된 브랜치는 제외됩니다." } else { "이 워크트리나 다른 워크트리에서 체크아웃된 브랜치는 제외됩니다." })) };
            let target = weak.clone();
            let state_for_action = state.clone();
            let name = name.clone();
            let selected = branch.clone();
            let mut buttons = div().flex().justify_end().gap_2().child(close_button());
            if delete {
                let target = weak.clone();
                let root = state.root.clone();
                let selected = branch.clone();
                buttons = buttons.child(theme::action_button("git-force-delete", empty).label("병합 안 된 브랜치 강제 삭제…").danger().on_click(move |_, window, cx| {
                    let Some(name) = selected.read(cx).selected_value().cloned() else { return; };
                    let operation = Operation::DeleteBranch { name: name.clone(), mode: DeleteMode::ForceUnmerged };
                    let detail = format!("로컬 브랜치: {name}\n병합되지 않은 커밋이 있어도 브랜치 참조를 삭제합니다 (-D). 해당 커밋을 찾기 어려워질 수 있습니다. 원격 브랜치는 삭제하지 않습니다.");
                    let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: root.clone(), label: "브랜치 강제 삭제 (-D)".into(), detail, operation, destructive: true }, window, cx));
                }));
            }
            buttons = buttons.child(theme::action_button("git-branch-continue", !create && empty).label(if delete { "병합된 브랜치 삭제…" } else if create { "만들기…" } else { "체크아웃…" }).primary().on_click(move |_, window, cx| {
                let selected_name = if create { Some(name.read(cx).value().trim().to_owned()) } else { selected.read(cx).selected_value().cloned() };
                let Some(name) = selected_name.filter(|name| !name.is_empty()) else { return; };
                let (operation, detail) = if create {
                    (Operation::CreateBranch { name: name.clone() }, format!("새 로컬 브랜치: {name}\n시작점: 실행 시 이 워크트리의 HEAD\n생성 후 현재 브랜치를 유지합니다."))
                } else if delete {
                    (Operation::DeleteBranch { name: name.clone(), mode: DeleteMode::MergedOnly }, format!("삭제할 로컬 브랜치: {name}\nGit의 병합 여부 검사(-d)를 통과한 브랜치만 삭제합니다. 원격 브랜치는 삭제하지 않습니다."))
                } else {
                    (Operation::CheckoutBranch { name: name.clone() }, format!("전환할 로컬 브랜치: {name}\n이 워크트리의 파일이 변경됩니다. 덮어쓸 로컬 변경이나 파일이 있으면 중단합니다."))
                };
                let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: state_for_action.root.clone(), label: title.into(), detail, operation, destructive: delete }, window, cx));
            }));
            dialog.title(title).width(ui_px(640.)).child(body.child(buttons))
        });
    }

    fn confirm_git_operation(
        &mut self,
        confirmation: GitConfirmation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let GitConfirmation {
            root,
            label,
            detail,
            operation,
            destructive,
        } = confirmation;
        let checkout_source = matches!(operation, Operation::CheckoutBranch { .. }).then(|| {
            self.active_worktree()
                .and_then(|tree| (!tree.is_detached).then(|| tree.branch.clone()))
        });
        window.close_dialog(cx);
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let root_for_action = root.clone();
            let checkout_source = checkout_source.clone();
            let operation = operation.clone();
            let label_for_action = label.clone();
            let action = theme::action_button("git-confirm", false).label(label.clone()).when(destructive, |b| b.danger()).when(!destructive, |b| b.primary()).on_click(move |_, window, cx| {
                let _ = weak.update(cx, |app, cx| {
                    window.close_dialog(cx);
                    let operation = operation.clone();
                    let checkout_source = checkout_source.clone();
                    app.run_git_job(root_for_action.clone(), label_for_action.clone(), move |git| {
                        if let Some(source) = checkout_source {
                            anyhow::ensure!(git.state()?.current_branch == source, "현재 브랜치가 변경되었습니다. 체크아웃 대상을 다시 확인하세요.");
                            anyhow::ensure!(git.in_progress()?.is_none(), "진행 중인 Git 작업을 먼저 완료하거나 중단하세요.");
                        }
                        git.execute(&operation)
                    }, window, cx);
                });
            });
            dialog.title(format!("{label} 확인")).width(ui_px(640.)).child(div().v_flex().gap_4()
                .child(scope(&root, None)).child(div().child(detail.clone()))
                .child(note("설정된 Git hooks와 기존 인증·SSH 설정을 사용합니다. 터미널 입력이 필요한 인증은 외부 터미널에서 준비한 뒤 다시 시도하세요."))
                .child(div().flex().justify_end().gap_2().child(close_button()).child(action)))
        });
    }

    fn show_remote_settings(
        &mut self,
        state: Arc<RepositoryState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let palette = Palette::new(cx);
            let mut rows = div().id("git-remotes-list").v_flex().gap_4().max_h(ui_px(380.)).overflow_y_scroll();
            if state.remotes.is_empty() { rows = rows.child(note("등록된 원격 저장소가 없습니다.")); }
            for (index, remote) in state.remotes.iter().enumerate() {
                let mut actions = div().flex().flex_wrap().gap_2();
                for (kind, label) in [(RemoteUrlKind::Fetch, "Fetch URL 수정…"), (RemoteUrlKind::Push, "Push URL 수정…")] {
                    let target = weak.clone();
                    let root = state.root.clone();
                    let remote = remote.clone();
                    actions = actions.child(theme::action_button((SharedString::from(label), index), false).label(label).small().on_click(move |_, window, cx| {
                        let _ = target.update(cx, |app, cx| {
                            window.close_dialog(cx);
                            app.show_remote_url_form(root.clone(), Some(remote.clone()), kind, window, cx);
                        });
                    }));
                }
                let rename_target = weak.clone();
                let rename_root = state.root.clone();
                let rename_name = remote.name.clone();
                actions = actions.child(theme::action_button(("git-remote-rename", index), false).label("이름 변경…").small().on_click(move |_, window, cx| {
                    let _ = rename_target.update(cx, |app, cx| {
                        window.close_dialog(cx);
                        app.show_remote_rename_form(rename_root.clone(), rename_name.clone(), window, cx);
                    });
                }));
                let target = weak.clone();
                let root = state.root.clone();
                let name = remote.name.clone();
                actions = actions.child(theme::action_button(("git-remote-remove", index), false).label("제거…").small().danger().on_click(move |_, window, cx| {
                    let detail = format!("제거할 원격 설정: {name}\n이 저장소의 원격 설정과 해당 원격 추적 참조가 제거됩니다. 서버의 저장소는 삭제하지 않습니다.");
                    let operation = Operation::RemoveRemote { name: name.clone() };
                    let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: root.clone(), label: "원격 설정 제거".into(), detail, operation, destructive: true }, window, cx));
                }));
                rows = rows.child(div().v_flex().gap_2().pb_3().border_b_1().border_color(palette.rgb(LINE))
                    .child(div().font_weight(FontWeight::MEDIUM).child(remote.name.clone()))
                    .child(note(&format!("Fetch: {}\nPush: {}", remote.fetch_urls.join(", "), remote.push_urls.join(", ")))).child(actions));
            }
            let target = weak.clone();
            let root = state.root.clone();
            dialog.title("원격 저장소 설정").width(ui_px(680.)).child(div().v_flex().gap_4()
                .child(scope(&state.root, None))
                .child(note("이 저장소와 연결된 모든 워크트리가 공유하는 Git 설정입니다."))
                .child(rows)
                .child(div().flex().justify_between().child(theme::action_button("git-add-remote", false).label("원격 저장소 추가…").on_click(move |_, window, cx| {
                    let _ = target.update(cx, |app, cx| {
                        window.close_dialog(cx);
                        app.show_remote_url_form(root.clone(), None, RemoteUrlKind::Fetch, window, cx);
                    });
                })).child(close_button())))
        });
    }

    fn show_remote_url_form(
        &mut self,
        root: PathBuf,
        remote: Option<Remote>,
        kind: RemoteUrlKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_new = remote.is_none();
        let title = if is_new {
            "원격 저장소 추가"
        } else if kind == RemoteUrlKind::Push {
            "Push URL 수정"
        } else {
            "Fetch URL 수정"
        };
        let initial_name = remote.as_ref().map(|r| r.name.clone()).unwrap_or_default();
        let urls = remote.as_ref().map(|r| {
            if kind == RemoteUrlKind::Push {
                &r.push_urls
            } else {
                &r.fetch_urls
            }
        });
        let multiple = urls.is_some_and(|urls| urls.len() != 1);
        let initial_url = urls
            .and_then(|urls| urls.first())
            .cloned()
            .unwrap_or_default();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial_name.clone())
                .placeholder("origin")
        });
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial_url)
                .placeholder("https://host/repo.git, git@host:repo.git, /local/path")
        });
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, _| {
            let target = weak.clone();
            let name_for_action = name.clone();
            let url_for_action = url.clone();
            let root_for_action = root.clone();
            let initial_name = initial_name.clone();
            dialog.title(title).width(ui_px(660.)).child(div().v_flex().gap_4().child(scope(&root, None))
                .child(if is_new { field("원격 이름", &name) } else { div().child(format!("원격: {initial_name}")) })
                .child(field(if kind == RemoteUrlKind::Push { "Push URL" } else { "Fetch URL" }, &url))
                .child(note(if multiple { "URL이 여러 개 설정되어 있습니다. 다른 URL을 덮어쓰지 않도록 외부 Git에서 편집하세요." } else { "저장소 로컬 설정만 변경합니다. 이 작업은 서버에 연결하지 않습니다." }))
                .child(div().flex().justify_end().gap_2().child(close_button()).child(theme::action_button("git-remote-save", multiple).label("변경 확인…").primary().on_click(move |_, window, cx| {
                    let name = if is_new { name_for_action.read(cx).value().trim().to_owned() } else { initial_name.clone() };
                    let url = url_for_action.read(cx).value().trim().to_owned();
                    if name.is_empty() || url.is_empty() { return; }
                    let operation = if is_new { Operation::AddRemote { name: name.clone(), url: url.clone() } } else { Operation::SetRemoteUrl { name: name.clone(), url: url.clone(), kind } };
                    let detail = format!("원격: {name}\n{}: {url}", if kind == RemoteUrlKind::Push { "Push URL" } else { "Fetch URL" });
                    let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: root_for_action.clone(), label: title.into(), detail, operation, destructive: false }, window, cx));
                }))))
        });
    }

    fn show_remote_rename_form(
        &mut self,
        root: PathBuf,
        old_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(old_name.clone()));
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, _| {
            let target = weak.clone();
            let root_for_action = root.clone();
            let old_for_action = old_name.clone();
            let name_for_action = name.clone();
            dialog.title("원격 이름 변경").width(ui_px(600.)).child(div().v_flex().gap_4().child(scope(&root, None))
                .child(format!("현재 원격 이름: {old_name}")).child(field("새 이름", &name))
                .child(div().flex().justify_end().gap_2().child(close_button()).child(theme::action_button("git-remote-rename-confirm", false).label("변경 확인…").on_click(move |_, window, cx| {
                    let new_name = name_for_action.read(cx).value().trim().to_owned();
                    if new_name.is_empty() { return; }
                    let operation = Operation::RenameRemote { name: old_for_action.clone(), new_name: new_name.clone() };
                    let detail = format!("원격 이름: {old_for_action} → {new_name}\n원격 추적 참조와 관련 설정 이름도 Git이 함께 변경합니다.");
                    let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: root_for_action.clone(), label: "원격 이름 변경".into(), detail, operation, destructive: false }, window, cx));
                }))))
        });
    }

    fn show_identity_form(
        &mut self,
        state: Arc<RepositoryState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(state.effective_identity.name.clone().unwrap_or_default())
                .placeholder("커밋 작성자 이름")
        });
        let email = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(state.effective_identity.email.clone().unwrap_or_default())
                .placeholder("author@example.com")
        });
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, _| {
            let target = weak.clone();
            let name_for_action = name.clone();
            let email_for_action = email.clone();
            let root = state.root.clone();
            dialog.title("Git 작성자 설정 — 이 저장소만").width(ui_px(640.)).child(div().v_flex().gap_4()
                .child(scope(&state.root, None))
                .child(note("커밋에 기록되는 작성자 정보입니다. 서버 로그인·인증 설정과는 별개입니다. 현재 유효한 값을 표시하며, 저장하면 이 저장소와 연결된 워크트리에 적용됩니다."))
                .child(note(&format!("현재 저장소 로컬 값: 이름 {} / 이메일 {}", state.local_identity.name.as_deref().unwrap_or("미설정 (상위 설정 상속)"), state.local_identity.email.as_deref().unwrap_or("미설정 (상위 설정 상속)"))))
                .child(field("이름 (user.name)", &name)).child(field("이메일 (user.email)", &email))
                .child(div().flex().justify_end().gap_2().child(close_button()).child(theme::action_button("git-identity-save", false).label("이 저장소에 저장…").primary().on_click(move |_, window, cx| {
                    let name = name_for_action.read(cx).value().trim().to_owned();
                    let email = email_for_action.read(cx).value().trim().to_owned();
                    if name.is_empty() || email.is_empty() { return; }
                    let detail = format!("작성자 이름: {name}\n작성자 이메일: {email}\n범위: 이 저장소의 로컬 Git 설정 (--local). 글로벌 설정과 인증 정보는 변경하지 않습니다.");
                    let operation = Operation::SetIdentity { name, email };
                    let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: root.clone(), label: "Git 작성자 설정 저장".into(), detail, operation, destructive: false }, window, cx));
                }))))
        });
    }
}

fn choice(
    values: Vec<String>,
    selected: Option<&str>,
    window: &mut Window,
    cx: &mut Context<Pokarak>,
) -> Choice {
    let index = selected
        .and_then(|s| values.iter().position(|v| v == s))
        .or_else(|| (!values.is_empty()).then_some(0))
        .map(IndexPath::new);
    cx.new(|cx| SelectState::new(SearchableVec::new(values), index, window, cx).searchable(true))
}

fn scope(root: &Path, branch: Option<&str>) -> Div {
    let mut content = div().v_flex().gap_1().child(
        div()
            .text_size(ui_text(12.))
            .child(format!("워크트리: {}", root.display())),
    );
    if let Some(branch) = branch {
        content = content.child(format!("현재 로컬 브랜치: {branch}"));
    }
    content
}

fn field(label: &'static str, input: &Entity<InputState>) -> Div {
    div()
        .v_flex()
        .gap_2()
        .child(div().font_weight(FontWeight::MEDIUM).child(label))
        .child(Input::new(input).id(label).aria_label(label))
}

fn select_field(label: &'static str, input: &Choice) -> Div {
    div()
        .v_flex()
        .gap_2()
        .child(div().font_weight(FontWeight::MEDIUM).child(label))
        .child(Select::new(input).id(label).w_full().cursor_pointer())
}

fn note(text: &str) -> Div {
    div().text_size(ui_text(12.)).child(text.to_owned())
}
fn close_button() -> Button {
    theme::action_button("git-dialog-close", false)
        .label("취소")
        .on_click(|_, window, cx| window.close_dialog(cx))
}
fn close_row() -> Div {
    div().flex().justify_end().child(
        theme::action_button("git-home-close", false)
            .label("닫기")
            .on_click(|_, window, cx| window.close_dialog(cx)),
    )
}
