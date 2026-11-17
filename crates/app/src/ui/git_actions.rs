//! Explicit Git actions; branch-sidebar selection remains comparison-only.
mod worktree_manager;
use super::*;
pub(super) mod integration;
use integration::IntegrationKind;
use gpui_kit::component::{
    IndexPath,
    select::{SearchableVec, Select, SelectState},
};
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
}

#[derive(Clone)]
struct GitCompletion {
    status: String,
    error: Option<String>,
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
enum Page {
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

    pub(super) fn git_is_busy(&self) -> bool {
        self.git_ui.busy.is_some() || self.git_ui.loading
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
        self.stop_analysis(cx);
        self.request += 1;
        self.file_request += 1;
        self.file_loading = false;
        let token = self.request;
        let compare = self
            .session()
            .map(|s| s.saved.comparison.clone())
            .unwrap_or_default();
        self.git_ui.completions.remove(&root);
        self.git_ui.busy = Some((root.clone(), label.clone()));
        self.error = None;
        self.status = format!("{label} 진행 중 — {}", root.display());
        let cancellation = OperationCancellation::default();
        self.git_ui.cancellation = Some(cancellation.clone());
        let job_root = root.clone();
        let mode = compare.clone();
        let task = cx.background_spawn(async move {
            let result = GitOperations::open(&job_root)
                .map(|git| git.with_cancellation(cancellation.clone()))
                .and_then(job);
            // Refresh even on failure: a fetch, hook, or timed-out operation may
            // have completed part of its work. Always inspect the original root.
            let snapshot = if cancellation.is_cancelled() {
                Err(anyhow::anyhow!(
                    "Git 작업이 취소되었습니다. 저장소를 다시 열어 상태를 확인하세요."
                ))
            } else {
                pokarak_core::snapshot(&job_root, &mode)
            };
            (result, snapshot)
        });
        cx.spawn_in(window, async move |view, cx| {
            let (result, snapshot) = task.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.git_ui.busy = None;
                this.git_ui.cancellation = None;
                let succeeded = result.is_ok();
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
                        error: Some(format!("{label}\n워크트리: {}\n{error:#}", root.display())),
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
                this.commit_job_finished(&root, succeeded, window, cx);
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
        self.load_git_page(None,window,cx);
    }

    fn load_git_page(&mut self,page:Option<Page>,window:&mut Window,cx:&mut Context<Self>){
        if self.git_is_busy() || self.loading {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        self.git_ui.loading = true;
        self.git_ui.load_request += 1;
        let token = self.git_ui.load_request;
        let load_root = root.clone();
        let task = cx.background_spawn(async move { GitOperations::open(&load_root)?.state() });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |this, window, cx| {
                if this.git_ui.load_request != token {
                    return;
                }
                this.git_ui.loading = false;
                if this.current.as_ref() != Some(&root) || this.opening_path.is_some() {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(state) => match page {Some(page)=>this.show_git_page(Arc::new(state),page,window,cx),None=>this.show_git_home(Arc::new(state),window,cx)},
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
        window.open_dialog(cx, move |dialog, _, _| {
            let mut sections = div().id("git-action-sections").max_h(px(355.)).overflow_y_scroll().v_flex().gap_3().child(scope(&state.root, state.current_branch.as_deref()));
            for (heading, actions) in [
                ("브랜치", vec![("만들기…", Page::Create), ("체크아웃…", Page::Checkout), ("삭제…", Page::Delete)]),
                ("브랜치 통합", vec![("Merge…",Page::Merge), ("Rebase…",Page::Rebase), ("Stash / 충돌 해결…",Page::Conflicts)]),
                ("원격 동기화", vec![("Fetch…", Page::Fetch), ("Push…", Page::Push), ("Pull (ff-only)…", Page::Pull)]),
                ("워크트리", vec![("워크트리 관리…", Page::Worktrees)]),
                ("저장소 Git 설정", vec![("원격 저장소 관리…", Page::Remotes), ("작성자 이름·이메일…", Page::Identity)]),
            ] {
                let mut row = div().flex().flex_wrap().gap_2();
                for (index, (label, page)) in actions.into_iter().enumerate() {
                    let weak = weak.clone();
                    let state = state.clone();
                    row = row.child(Button::new((SharedString::from(heading), index)).label(label).on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            window.close_dialog(cx);
                            app.show_git_page(state.clone(), page, window, cx);
                        });
                    }));
                }
                sections = sections.child(div().v_flex().gap_2().child(div().font_weight(FontWeight::MEDIUM).child(heading)).child(row));
            }
            dialog.title("Git 작업").width(px(600.)).margin_top(px(16.)).child(sections).footer(close_row())
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
            Page::Worktrees => self.show_worktree_manager(state,window,cx),
            Page::Create | Page::Checkout | Page::Delete => {
                self.show_branch_form(state, page, window, cx)
            }
            Page::Fetch | Page::Push | Page::Pull => {
                self.show_network_form(state, page, window, cx)
            }
            Page::Merge => self.open_integration(IntegrationKind::Merge,None,window,cx),
            Page::Rebase => self.open_integration(IntegrationKind::Rebase,None,window,cx),
            Page::Conflicts => self.open_git_workflow(window,cx),
            Page::Remotes => self.show_remote_settings(state, window, cx),
            Page::Identity => self.show_identity_form(state, window, cx),
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
        window.open_dialog(cx, move |dialog, _, _| {
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
                buttons = buttons.child(Button::new("git-force-delete").label("병합 안 된 브랜치 강제 삭제…").danger().disabled(empty).on_click(move |_, window, cx| {
                    let Some(name) = selected.read(cx).selected_value().cloned() else { return; };
                    let operation = Operation::DeleteBranch { name: name.clone(), mode: DeleteMode::ForceUnmerged };
                    let detail = format!("로컬 브랜치: {name}\n병합되지 않은 커밋이 있어도 브랜치 참조를 삭제합니다 (-D). 해당 커밋을 찾기 어려워질 수 있습니다. 원격 브랜치는 삭제하지 않습니다.");
                    let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: root.clone(), label: "브랜치 강제 삭제 (-D)".into(), detail, operation, destructive: true }, window, cx));
                }));
            }
            buttons = buttons.child(Button::new("git-branch-continue").label(if delete { "병합된 브랜치 삭제…" } else if create { "만들기…" } else { "체크아웃…" }).primary().disabled(!create && empty).on_click(move |_, window, cx| {
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
            dialog.title(title).width(px(640.)).child(body.child(buttons))
        });
    }

    fn show_network_form(
        &mut self,
        state: Arc<RepositoryState>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fetch = matches!(page, Page::Fetch);
        let push = matches!(page, Page::Push);
        let title = if fetch {
            "Fetch"
        } else if push {
            "Push"
        } else {
            "Pull (ff-only)"
        };
        let remote = choice(
            state.remotes.iter().map(|r| r.name.clone()).collect(),
            None,
            window,
            cx,
        );
        let branch = choice(
            state.local_branches.clone(),
            state.current_branch.as_deref(),
            window,
            cx,
        );
        let remote_branch = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(state.current_branch.clone().unwrap_or_default())
                .placeholder("원격 브랜치 이름")
        });
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let mut body = div().v_flex().gap_4().child(scope(&state.root, state.current_branch.as_deref())).child(select_field("원격 저장소", &remote));
            if push { body = body.child(select_field("보낼 로컬 브랜치", &branch)).child(note("선택한 로컬 브랜치만 같은 이름의 원격 브랜치로 보냅니다. 강제 푸시와 태그 전송은 하지 않습니다.")); }
            else if !fetch { body = body.child(field("가져올 원격 브랜치", &remote_branch)).child(note("위의 현재 브랜치에 fast-forward만 적용합니다. 작업 파일이 변경되었거나 이력이 갈라졌으면 중단합니다. 자동 stash와 rebase는 하지 않습니다.")); }
            else { body = body.child(note("원격 브랜치 정보를 가져옵니다. 작업 파일과 현재 브랜치는 변경하지 않습니다. 기본 Fetch는 사라진 원격 추적 브랜치를 정리하지 않습니다.")); }
            if state.remotes.is_empty() { body = body.child(note("등록된 원격 저장소가 없습니다. Git 작업의 ‘원격 저장소 관리’에서 추가하세요.")); }
            if !fetch && !push && state.current_branch.is_none() { body = body.child(note("Detached HEAD에서는 Pull할 수 없습니다. 먼저 로컬 브랜치를 체크아웃하세요.")); }
            let mut buttons = div().flex().justify_end().gap_2().child(close_button());
            for prune in if fetch { vec![true, false] } else { vec![false] } {
                let target = weak.clone();
                let state = state.clone();
                let remote = remote.clone();
                let branch = branch.clone();
                let remote_branch = remote_branch.clone();
                buttons = buttons.child(Button::new(if prune { "git-fetch-prune" } else { "git-network-next" })
                    .label(if prune { "Fetch + 원격 추적 정리…" } else { title })
                    .disabled(state.remotes.is_empty() || (!fetch && !push && state.current_branch.is_none()) || (push && state.local_branches.is_empty()))
                    .on_click(move |_, window, cx| {
                        let Some(remote_name) = remote.read(cx).selected_value().cloned() else { return; };
                        let Some(config) = state.remotes.iter().find(|r| r.name == remote_name) else { return; };
                        let urls = if push { &config.push_urls } else { &config.fetch_urls };
                        let destination = urls.join("\n");
                        let (operation, label, detail) = if fetch {
                            (Operation::Fetch { remote: remote_name.clone(), prune }, if prune { "Fetch 및 원격 추적 정리" } else { "Fetch" }.to_owned(), format!("원격: {remote_name}\nURL: {destination}\n{}", if prune { "원격에서 사라진 브랜치의 로컬 추적 참조도 삭제합니다 (--prune)." } else { "원격 추적 참조만 갱신합니다. Prune은 실행하지 않습니다." }))
                        } else if push {
                            let Some(branch) = branch.read(cx).selected_value().cloned() else { return; };
                            (Operation::Push { remote: remote_name.clone(), branch: branch.clone() }, "Push".into(), format!("원격: {remote_name}\nPush URL: {destination}\n로컬 {branch} → 원격 {branch}\n이 브랜치 하나만 보냅니다. 강제 푸시는 하지 않습니다."))
                        } else {
                            let Some(local) = state.current_branch.clone() else { return; };
                            let branch = remote_branch.read(cx).value().trim().to_owned();
                            if branch.is_empty() { return; }
                            (Operation::Pull { remote: remote_name.clone(), remote_branch: branch.clone(), local_branch: local.clone() }, "Pull (ff-only)".into(), format!("원격: {remote_name}\nFetch URL: {destination}\n원격 {branch} → 현재 로컬 {local}\nFast-forward만 적용합니다."))
                        };
                        let _ = target.update(cx, |app, cx| app.confirm_git_operation(GitConfirmation { root: state.root.clone(), label, detail, operation, destructive: prune }, window, cx));
                    }));
            }
            dialog.title(title).width(px(620.)).child(body.child(buttons))
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
        window.close_dialog(cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let root_for_action = root.clone();
            let operation = operation.clone();
            let label_for_action = label.clone();
            let action = Button::new("git-confirm").label(label.clone()).when(destructive, |b| b.danger()).when(!destructive, |b| b.primary()).on_click(move |_, window, cx| {
                let _ = weak.update(cx, |app, cx| {
                    window.close_dialog(cx);
                    let operation = operation.clone();
                    app.run_git_job(root_for_action.clone(), label_for_action.clone(), move |git| git.execute(&operation), window, cx);
                });
            });
            dialog.title(format!("{label} 확인")).width(px(640.)).child(div().v_flex().gap_4()
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
        window.open_dialog(cx, move |dialog, _, cx| {
            let palette = Palette::new(cx);
            let mut rows = div().id("git-remotes-list").v_flex().gap_4().max_h(px(380.)).overflow_y_scroll();
            if state.remotes.is_empty() { rows = rows.child(note("등록된 원격 저장소가 없습니다.")); }
            for (index, remote) in state.remotes.iter().enumerate() {
                let mut actions = div().flex().flex_wrap().gap_2();
                for (kind, label) in [(RemoteUrlKind::Fetch, "Fetch URL 수정…"), (RemoteUrlKind::Push, "Push URL 수정…")] {
                    let target = weak.clone();
                    let root = state.root.clone();
                    let remote = remote.clone();
                    actions = actions.child(Button::new((SharedString::from(label), index)).label(label).small().on_click(move |_, window, cx| {
                        let _ = target.update(cx, |app, cx| {
                            window.close_dialog(cx);
                            app.show_remote_url_form(root.clone(), Some(remote.clone()), kind, window, cx);
                        });
                    }));
                }
                let rename_target = weak.clone();
                let rename_root = state.root.clone();
                let rename_name = remote.name.clone();
                actions = actions.child(Button::new(("git-remote-rename", index)).label("이름 변경…").small().on_click(move |_, window, cx| {
                    let _ = rename_target.update(cx, |app, cx| {
                        window.close_dialog(cx);
                        app.show_remote_rename_form(rename_root.clone(), rename_name.clone(), window, cx);
                    });
                }));
                let target = weak.clone();
                let root = state.root.clone();
                let name = remote.name.clone();
                actions = actions.child(Button::new(("git-remote-remove", index)).label("제거…").small().danger().on_click(move |_, window, cx| {
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
            dialog.title("원격 저장소 설정").width(px(680.)).child(div().v_flex().gap_4()
                .child(scope(&state.root, None))
                .child(note("이 저장소와 연결된 모든 워크트리가 공유하는 Git 설정입니다."))
                .child(rows)
                .child(div().flex().justify_between().child(Button::new("git-add-remote").label("원격 저장소 추가…").on_click(move |_, window, cx| {
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
        window.open_dialog(cx, move |dialog, _, _| {
            let target = weak.clone();
            let name_for_action = name.clone();
            let url_for_action = url.clone();
            let root_for_action = root.clone();
            let initial_name = initial_name.clone();
            dialog.title(title).width(px(660.)).child(div().v_flex().gap_4().child(scope(&root, None))
                .child(if is_new { field("원격 이름", &name) } else { div().child(format!("원격: {initial_name}")) })
                .child(field(if kind == RemoteUrlKind::Push { "Push URL" } else { "Fetch URL" }, &url))
                .child(note(if multiple { "URL이 여러 개 설정되어 있습니다. 다른 URL을 덮어쓰지 않도록 외부 Git에서 편집하세요." } else { "저장소 로컬 설정만 변경합니다. 이 작업은 서버에 연결하지 않습니다." }))
                .child(div().flex().justify_end().gap_2().child(close_button()).child(Button::new("git-remote-save").label("변경 확인…").primary().disabled(multiple).on_click(move |_, window, cx| {
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
        window.open_dialog(cx, move |dialog, _, _| {
            let target = weak.clone();
            let root_for_action = root.clone();
            let old_for_action = old_name.clone();
            let name_for_action = name.clone();
            dialog.title("원격 이름 변경").width(px(600.)).child(div().v_flex().gap_4().child(scope(&root, None))
                .child(format!("현재 원격 이름: {old_name}")).child(field("새 이름", &name))
                .child(div().flex().justify_end().gap_2().child(close_button()).child(Button::new("git-remote-rename-confirm").label("변경 확인…").on_click(move |_, window, cx| {
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
        window.open_dialog(cx, move |dialog, _, _| {
            let target = weak.clone();
            let name_for_action = name.clone();
            let email_for_action = email.clone();
            let root = state.root.clone();
            dialog.title("Git 작성자 설정 — 이 저장소만").width(px(640.)).child(div().v_flex().gap_4()
                .child(scope(&state.root, None))
                .child(note("커밋에 기록되는 작성자 정보입니다. 서버 로그인·인증 설정과는 별개입니다. 현재 유효한 값을 표시하며, 저장하면 이 저장소와 연결된 워크트리에 적용됩니다."))
                .child(note(&format!("현재 저장소 로컬 값: 이름 {} / 이메일 {}", state.local_identity.name.as_deref().unwrap_or("미설정 (상위 설정 상속)"), state.local_identity.email.as_deref().unwrap_or("미설정 (상위 설정 상속)"))))
                .child(field("이름 (user.name)", &name)).child(field("이메일 (user.email)", &email))
                .child(div().flex().justify_end().gap_2().child(close_button()).child(Button::new("git-identity-save").label("이 저장소에 저장…").primary().on_click(move |_, window, cx| {
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
            .text_size(px(12.))
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
        .child(Input::new(input).id(label))
}

fn select_field(label: &'static str, input: &Choice) -> Div {
    div()
        .v_flex()
        .gap_2()
        .child(div().font_weight(FontWeight::MEDIUM).child(label))
        .child(Select::new(input).id(label).w_full())
}

fn note(text: &str) -> Div {
    div().text_size(px(12.)).child(text.to_owned())
}
fn close_button() -> Button {
    Button::new("git-dialog-close")
        .label("취소")
        .ghost()
        .on_click(|_, window, cx| window.close_dialog(cx))
}
fn close_row() -> Div {
    div().flex().justify_end().child(
        Button::new("git-home-close")
            .label("닫기")
            .on_click(|_, window, cx| window.close_dialog(cx)),
    )
}
