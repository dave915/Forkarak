//! One explicit workflow entry; every write uses git_actions' shared job guard.
mod status;
use super::*;
use pokarak_core::operations::workflow::{
    ConflictContent, ConflictFile, ConflictSnapshot, InProgress, ProgressKind, Resolution,
    StashEntry, WorkflowState,
};
use pokarak_core::operations::{GitOperations, OperationOutput};

#[derive(Default)]
pub(super) struct WorkflowUi {
    request: u64,
    include_untracked: bool,
    saving_resolution: bool,
    draft: Option<ResolutionDraft>,
    retained: HashMap<(PathBuf, PathBuf), ResolutionDraft>,
    status: HashMap<PathBuf, Result<WorkflowState, String>>,
    status_request: u64,
}
struct ResolutionDraft {
    root: PathBuf,
    snapshot: ConflictSnapshot,
    sources: [Entity<EditorState>; 3],
    result: Entity<EditorState>,
    show_base: bool,
    kind: Option<ProgressKind>,
    side: Option<u8>,
    can_edit: bool,
    manual: Option<String>,
    _result_subscription: Subscription,
}
impl ResolutionDraft {
    fn key(&self) -> (PathBuf, PathBuf) {
        (self.root.clone(), self.snapshot.file.path.clone())
    }
    fn dirty(&self, cx: &App) -> bool {
        let original = self
            .snapshot
            .working
            .as_ref()
            .and_then(ConflictContent::text)
            .unwrap_or("");
        self.side.is_some()
            || (self.can_edit
                && (self.result.read(cx).value().as_ref() != original
                    || self.manual.as_deref().is_some_and(|text| text != original)))
    }
    fn side_content(&self, stage: u8) -> Option<&ConflictContent> {
        self.snapshot
            .stages
            .iter()
            .find(|s| s.entry.stage == stage)
            .map(|s| &s.content)
    }
    fn result_readonly(&self) -> bool {
        !self.can_edit
            || self
                .side
                .is_some_and(|side| self.side_content(side).is_none())
    }
    fn resolution(&self, cx: &App) -> Resolution {
        if let Some(side) = self.side {
            let unchanged = self
                .side_content(side)
                .and_then(ConflictContent::text)
                .is_none_or(|text| self.result.read(cx).value().as_ref() == text);
            if unchanged {
                return if side == 2 {
                    Resolution::Ours
                } else {
                    Resolution::Theirs
                };
            }
        }
        Resolution::Edited(self.result.read(cx).value().to_string())
    }
}
#[derive(Clone)]
enum WorkflowWrite {
    SaveStash {
        message: String,
        untracked: bool,
    },
    Apply(StashEntry),
    Pop(StashEntry),
    Drop(StashEntry),
    Resolve {
        snapshot: ConflictSnapshot,
        resolution: Resolution,
    },
    Finish {
        progress: InProgress,
        abort: bool,
    },
}
impl WorkflowWrite {
    fn run(self, operations: GitOperations) -> anyhow::Result<OperationOutput> {
        match self {
            Self::SaveStash { message, untracked } => operations.stash_save(&message, untracked),
            Self::Apply(entry) => operations.stash_apply(&entry),
            Self::Pop(entry) => operations.stash_pop(&entry),
            Self::Drop(entry) => operations.stash_drop(&entry),
            Self::Resolve {
                snapshot,
                resolution,
            } => operations.resolve_conflict(&snapshot, &resolution),
            Self::Finish { progress, abort } => operations.finish_operation(&progress, abort),
        }
    }
}
impl Pokarak {
    pub(in crate::ui) fn invalidate_workflow_read(&mut self) {
        self.workflow.request += 1;
    }
    pub(super) fn finish_resolution_save(&mut self, success: bool) {
        if std::mem::take(&mut self.workflow.saving_resolution) && success {
            self.workflow.draft = None;
        }
    }

    pub(super) fn has_unsaved_conflict(&self, cx: &App) -> bool {
        self.workflow
            .draft
            .as_ref()
            .is_some_and(|draft| draft.dirty(cx))
            || self.workflow.retained.values().any(|draft| draft.dirty(cx))
    }

    fn retain_active_conflict(&mut self) {
        if let Some(draft) = self.workflow.draft.take() {
            self.workflow.retained.insert(draft.key(), draft);
        }
    }

    pub(super) fn workflow_read<T: Send + 'static>(
        &mut self,
        root: PathBuf,
        job: impl FnOnce(GitOperations) -> anyhow::Result<T> + Send + 'static,
        ready: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy()
            || self.loading
            || self.opening_path.is_some()
            || self.current.as_ref() != Some(&root)
        {
            return;
        }
        self.workflow.request += 1;
        let token = self.workflow.request;
        let navigation_generation = self.request;
        let dialog_bound = window.has_active_dialog(cx);
        self.error = None;
        self.status = "Git 작업 정보를 읽는 중…".into();
        let target = root.clone();
        let task = cx.background_spawn(async move { GitOperations::open(&target).and_then(job) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workflow.request != token {
                    return;
                }
                if this.current.as_ref() != Some(&root)
                    || this.request != navigation_generation
                    || this.git_is_busy()
                    || this.opening_path.is_some()
                    || (dialog_bound && !window.has_active_dialog(cx))
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(data) => {
                        this.status = "Git 작업 정보 준비됨".into();
                        ready(this, data, window, cx);
                    }
                    Err(error) => {
                        this.error = Some(format!("Git 작업 — {}\n{error:#}", root.display()));
                        this.status = "Git workflow 읽기 실패".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn open_git_workflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        self.workflow_read(
            root,
            |ops| ops.workflow_state(),
            Self::show_workflow,
            window,
            cx,
        );
    }
    /// Stashes are shared across worktrees: always pick an explicit identity.
    pub(super) fn open_stash_pop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        self.workflow_read(
            root,
            |ops| ops.workflow_state(),
            Self::show_stash_pop_picker,
            window,
            cx,
        );
    }

    fn show_stash_pop_picker(
        &mut self,
        state: WorkflowState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_dialog(cx, move |dialog,_,cx| {
            let busy = workflow_disabled(&weak, &state.root, cx);
            let mut entries = div().id("stash-pop-targets").max_h(ui_px(240.)).overflow_y_scroll().v_flex().gap_2();
            if state.stashes.is_empty() { entries = entries.child(workflow_note("보관된 stash가 없습니다.")); }
            for (index, entry) in state.stashes.iter().enumerate() {
                let target = state.root.clone(); let selected = entry.clone();
                let preview_root = state.root.clone(); let preview_entry = entry.clone();
                entries = entries.child(div().v_flex().gap_1()
                    .child(div().text_ellipsis().child(format!("{} · {} · {}", entry.selector, short_oid(&entry.oid), entry.message)))
                    .child(div().flex().gap_2()
                        .child(wf_action(&weak,("stash-pop-preview",index),"내용 보기…",busy,move |app,w,cx|app.open_stash(preview_root.clone(),preview_entry.clone(),w,cx)))
                        .child(wf_action(&weak,("stash-pop-pick",index),"Pop · 적용 후 제거…",busy,move |app,w,cx| {
                            let job = WorkflowWrite::Pop(selected.clone());
                            app.confirm_workflow(target.clone(),"Stash Pop · 적용 후 제거".into(),stash_write_detail(&selected,&job),job,w,cx);
                        }))));
            }
            dialog.title(format!("Pop 대상 선택 · Stash {}개", state.stashes.len())).width(ui_px(740.)).margin_top(ui_px(24.)).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_3().child(workflow_scope(&state.root))
                    .child(workflow_note("Stash는 이 저장소의 모든 워크트리가 공유합니다. 현재 워크트리에 적용할 항목을 선택하고 확인하세요. 충돌이나 실패 시 stash는 보관됩니다."))
                    .child(entries)).footer(workflow_close())
        });
    }
    fn workflow_write(
        &mut self,
        root: PathBuf,
        label: &str,
        job: WorkflowWrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() || self.current.as_ref() != Some(&root) {
            return;
        }
        self.workflow.saving_resolution = matches!(job, WorkflowWrite::Resolve { .. });
        self.workflow.request += 1; // Invalidate any older workflow read.
        window.close_all_dialogs(cx);
        self.run_git_job(root, label.into(), move |ops| job.run(ops), window, cx);
    }
    fn show_workflow(&mut self, state: WorkflowState, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let busy = workflow_disabled(&weak, &state.root, cx);
            let save_root = state.root.clone();
            let mut body = div().v_flex().gap_2().child(workflow_scope(&state.root))
                .child(div().flex().gap_2().items_center()
                    .child(div().flex_1().child(format!("Stash {}개 · 모든 워크트리 공유",state.stashes.len())))
                    .child(wf_action(&weak, "stash-new", "변경사항 보관…", busy, move |app,w,cx| app.show_stash_save(save_root.clone(),w,cx)))
                    .child(wf_action(&weak,"workflow-pop","Pop…",busy,|app,w,cx|app.open_stash_pop(w,cx)))
                    .child(wf_action(&weak, "workflow-refresh", "새로고침", busy, |app,w,cx| app.open_git_workflow(w,cx))));
            let mut stashes = div().id("workflow-stashes").h(ui_px(116.)).overflow_y_scroll().v_flex().gap_1();
            if state.stashes.is_empty() { stashes = stashes.child(workflow_note("보관된 stash가 없습니다.")); }
            for (index, entry) in state.stashes.iter().enumerate() {
                let label = format!("{}  {}  {}", entry.selector, short_oid(&entry.oid), entry.message);
                let root = state.root.clone(); let entry = entry.clone();
                stashes = stashes.child(wf_action(&weak, ("stash-entry", index), label, busy, move |app,w,cx| app.open_stash(root.clone(), entry.clone(), w,cx)).w_full().justify_start());
            }
            body = body.child(stashes).child(div().border_t_1().border_color(Palette::new(cx).rgb(LINE)).pt_2().child("충돌 · 현재 워크트리"));
            if let Some(progress) = &state.progress {
                let continue_progress = progress.clone(); let abort_progress = progress.clone();
                let root = state.root.clone(); let abort_root = root.clone();
                body = body.child(div().flex().items_center().gap_2()
                    .child(div().flex_1().child(format!("{} 진행 중", progress.kind.label())))
                    .child(wf_action(&weak,"workflow-continue","계속…",busy || !state.conflicts.is_empty(), move |app,w,cx| {
                        app.confirm_workflow(root.clone(), format!("{} 계속", continue_progress.kind.label()), "스테이징된 결과로 Git 작업을 계속합니다. 커밋이 만들어질 수 있으며 기존 메시지를 사용합니다. 기존 Git hooks가 실행됩니다.".into(), WorkflowWrite::Finish { progress:continue_progress.clone(),abort:false }, w,cx);
                    }))
                    .child(wf_action(&weak,"workflow-abort","중단…",busy,move |app,w,cx| {
                        app.confirm_workflow(abort_root.clone(), format!("{} 중단", abort_progress.kind.label()), "이 워크트리의 진행 중 작업을 중단하고 Git이 이전 상태로 복원합니다. 충돌 해결 중 작성한 내용이 사라질 수 있습니다. Stash 항목은 삭제하지 않습니다.".into(), WorkflowWrite::Finish { progress:abort_progress.clone(),abort:true }, w,cx);
                    })));
            } else if !state.conflicts.is_empty() {
                body = body.child(workflow_note("진행 중인 Merge / Rebase / Cherry-pick / Revert가 없습니다. Stash 적용 충돌은 파일만 해결하세요. 보관된 stash는 유지됩니다."));
            }
            let mut conflicts = div().id("workflow-conflicts").h(ui_px(112.)).overflow_y_scroll().v_flex().gap_1();
            if state.conflicts.is_empty() { conflicts = conflicts.child(workflow_note("미해결 index stage가 없습니다.")); }
            for (index, file) in state.conflicts.iter().enumerate() {
                let root = state.root.clone(); let file = file.clone();
                let label = format!("{}  · stages {}", file.path.display(), file.stages.iter().map(|s|s.stage.to_string()).collect::<Vec<_>>().join("/"));
                conflicts = conflicts.child(wf_action(&weak,("conflict-entry", index),label,busy,move |app,w,cx| app.open_conflict(root.clone(),file.clone(),w,cx)).w_full().justify_start());
            }
            if let Some(view)=weak.upgrade() {
                let app=view.read(cx);
                for (index,draft) in app.workflow.draft.iter().chain(app.workflow.retained.values()).filter(|draft|draft.root==state.root&&draft.dirty(cx)).enumerate() {
                    let root=draft.root.clone();let file=draft.snapshot.file.clone();
                    conflicts=conflicts.child(wf_action(&weak,("retained-conflict",index),format!("작성한 결과로 돌아가기 · {}",file.path.display()),busy,move |app,w,cx|app.open_conflict(root.clone(),file.clone(),w,cx)).w_full().justify_start());
                }
            }
            dialog.title("Stash / 충돌 해결").width(ui_px(740.)).margin_top(ui_px(24.)).on_ok(|_,_,_|false)
                .child(body.child(conflicts)).footer(workflow_close())
        });
    }
    fn show_stash_save(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let message =
            cx.new(|cx| InputState::new(window, cx).placeholder("변경사항을 설명하는 메시지"));
        self.workflow.include_untracked = false;
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog,_,cx| {
            let selected = weak.upgrade().is_some_and(|view| view.read(cx).workflow.include_untracked); let input = message.clone();
            let target = root.clone();
            dialog.title("변경사항을 stash에 보관").width(ui_px(620.)).margin_top(ui_px(36.)).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_3().child(workflow_scope(&root))
                    .child(workflow_note("추적 중인 변경사항을 보관하고 작업 파일을 되돌립니다. 이 저장소의 모든 워크트리에 stash가 표시됩니다."))
                    .child(Input::new(&message))
                    .child(wf_action(&weak,"stash-untracked","추적 안 된 파일도 포함",false,|app,_,cx|{app.workflow.include_untracked = !app.workflow.include_untracked;cx.notify();}).selected(selected))
                    .child(workflow_note("무시된 파일 (.gitignore)은 포함하지 않습니다.")))
                .footer(div().flex().justify_end().gap_2().child(workflow_close()).child(wf_action(&weak,"stash-save-confirm","Stash에 보관",workflow_disabled(&weak,&root,cx),move |app,w,cx| {
                    let text=input.read(cx).value().to_string();
                    if text.trim().is_empty() { app.error=Some("Stash 메시지를 입력하세요.".into());cx.notify();return; }
                    app.workflow_write(target.clone(),"Stash 보관",WorkflowWrite::SaveStash{message:text,untracked:app.workflow.include_untracked},w,cx);
                }).primary()))
        });
    }
    fn open_stash(
        &mut self,
        root: PathBuf,
        entry: StashEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = entry.clone();
        let target = root.clone();
        self.workflow_read(
            root,
            move |ops| {
                let output = ops.stash_patch(&selected)?;
                let mut text = output.stdout;
                if output.truncated {
                    text.push_str("\n[Patch 일부 생략 — 출력 제한]");
                }
                Ok(patch::Prepared::new(text, None))
            },
            move |app, patch, w, cx| app.show_stash(target, entry, patch, w, cx),
            window,
            cx,
        );
    }
    fn show_stash(
        &mut self,
        root: PathBuf,
        entry: StashEntry,
        patch: patch::Prepared,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let preview = patch::editor(patch, true, window, cx);
        let path =
            cx.new(|cx| InputState::new(window, cx).placeholder("내용을 볼 저장소 상대 파일 경로"));
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let busy = workflow_disabled(&weak, &root, cx);
            let mut inspect = div().flex().gap_2().child(Input::new(&path).flex_1());
            for (untracked, label, id) in [
                (false, "파일 내용", "stash-tracked-content"),
                (true, "미추적 내용", "stash-untracked-content"),
            ] {
                let input = path.clone();
                let target = root.clone();
                let selected = entry.clone();
                inspect = inspect.child(wf_action(&weak, id, label, busy, move |app, w, cx| {
                    let path = PathBuf::from(input.read(cx).value().as_ref());
                    let filename = path.display().to_string();
                    app.workflow_read(
                        target.clone(),
                        {
                            let entry = selected.clone();
                            move |ops| ops.stash_content(&entry, &path, untracked)
                        },
                        move |app, content, w, cx| {
                            app.show_workflow_content(filename, content, w, cx)
                        },
                        w,
                        cx,
                    );
                }));
            }
            let mut actions = div().flex().gap_2().justify_end().child(workflow_close());
            for (id, label, action) in [
                (
                    "stash-apply",
                    "Apply · 보관 유지",
                    WorkflowWrite::Apply(entry.clone()),
                ),
                (
                    "stash-pop",
                    "Pop · 적용 후 제거",
                    WorkflowWrite::Pop(entry.clone()),
                ),
                ("stash-drop", "삭제…", WorkflowWrite::Drop(entry.clone())),
            ] {
                let target = root.clone();
                let selected = entry.clone();
                actions = actions.child(wf_action(&weak, id, label, busy, move |app, w, cx| {
                    let detail = stash_write_detail(&selected, &action);
                    app.confirm_workflow(
                        target.clone(),
                        format!("Stash {label}"),
                        detail,
                        action.clone(),
                        w,
                        cx,
                    );
                }));
            }
            dialog
                .title(format!("{} · {}", entry.selector, short_oid(&entry.oid)))
                .width(ui_px(780.))
                .margin_top(ui_px(24.))
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(workflow_scope(&root))
                        .child(div().text_ellipsis().child(entry.message.clone()))
                        .child(workflow_note(&format!(
                            "원본 브랜치 기록: {} · 저장소 전체 공유",
                            entry.source_branch.as_deref().unwrap_or("확인 불가")
                        )))
                        .child(editor_view(&preview, true, 214.))
                        .child(inspect),
                )
                .footer(actions)
        });
    }
    fn show_workflow_content(
        &mut self,
        title: String,
        content: ConflictContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = workflow_editor(
            describe_content(Some(&content)),
            content_language(Path::new(&title), Some(&content)),
            true,
            window,
            cx,
        );
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(title.clone())
                .width(ui_px(740.))
                .margin_top(ui_px(36.))
                .on_ok(|_, _, _| false)
                .child(editor_view(&editor, true, 300.))
                .footer(workflow_close())
        });
    }
    fn confirm_workflow(
        &mut self,
        root: PathBuf,
        title: String,
        detail: String,
        job: WorkflowWrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let target = root.clone();
            let job = job.clone();
            let label = title.clone();
            dialog
                .title(title.clone())
                .width(ui_px(640.))
                .margin_top(ui_px(36.))
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .id("workflow-confirm-detail")
                        .max_h(ui_px(290.))
                        .overflow_y_scroll()
                        .v_flex()
                        .gap_3()
                        .child(workflow_scope(&root))
                        .child(detail.clone()),
                )
                .footer(
                    div()
                        .flex()
                        .gap_2()
                        .justify_end()
                        .child(workflow_close())
                        .child(
                            wf_action(
                                &weak,
                                "workflow-confirm",
                                format!("{title} 확인"),
                                workflow_disabled(&weak, &root, cx),
                                move |app, w, cx| {
                                    app.workflow_write(target.clone(), &label, job.clone(), w, cx)
                                },
                            )
                            .primary(),
                        ),
                )
        });
    }
    fn open_conflict(
        &mut self,
        root: PathBuf,
        file: ConflictFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() || self.current.as_ref() != Some(&root) {
            return;
        }
        let key = (root.clone(), file.path.clone());
        if self
            .workflow
            .draft
            .as_ref()
            .is_some_and(|draft| draft.key() == key)
        {
            self.workflow.request += 1;
            self.present_conflict(window, cx);
            return;
        }
        if let Some(draft) = self.workflow.retained.remove(&key) {
            self.workflow.request += 1;
            self.retain_active_conflict();
            self.workflow.draft = Some(draft);
            self.present_conflict(window, cx);
            return;
        }
        let target = root.clone();
        self.workflow_read(
            root,
            move |ops| {
                Ok((
                    ops.inspect_conflict(&file)?,
                    ops.in_progress()?.map(|p| p.kind),
                ))
            },
            move |app, (snapshot, kind), w, cx| app.show_conflict(target, snapshot, kind, w, cx),
            window,
            cx,
        );
    }
    fn show_conflict(
        &mut self,
        root: PathBuf,
        snapshot: ConflictSnapshot,
        kind: Option<ProgressKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_edit = snapshot
            .stages
            .iter()
            .all(|s| matches!(s.content, ConflictContent::Text(_)))
            && snapshot
                .working
                .as_ref()
                .is_none_or(|c| matches!(c, ConflictContent::Text(_)));
        let sources = [1, 2, 3].map(|stage| {
            let content = snapshot
                .stages
                .iter()
                .find(|s| s.entry.stage == stage)
                .map(|s| &s.content);
            workflow_editor(
                describe_content(content),
                content_language(&snapshot.file.path, content),
                true,
                window,
                cx,
            )
        });
        let result = workflow_editor(
            snapshot
                .working
                .as_ref()
                .map(|c| describe_content(Some(c)))
                .unwrap_or_default(),
            snapshot.working.as_ref().map_or_else(
                || language(&snapshot.file.path),
                |content| content_language(&snapshot.file.path, Some(content)),
            ),
            !can_edit,
            window,
            cx,
        );
        let subscription = cx.subscribe(&result, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        self.retain_active_conflict();
        self.workflow.draft = Some(ResolutionDraft {
            root,
            snapshot,
            sources,
            result,
            show_base: false,
            kind,
            side: None,
            can_edit,
            manual: None,
            _result_subscription: subscription,
        });
        self.present_conflict(window, cx);
    }
    fn present_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(view) = weak.upgrade() else { return dialog; };
            let app = view.read(cx);
            let Some(draft) = &app.workflow.draft else { return dialog; };
            let busy = workflow_disabled(&weak, &draft.root, cx);
            let viewport_height = f32::from(window.viewport_size().height);
            let (source_height, result_height) = conflict_heights(viewport_height);
            let (ours, theirs) = draft.kind.unwrap_or(ProgressKind::Merge).side_labels();
            let mut sources = div().flex().min_w_0().gap_2();
            if draft.show_base { sources = sources.child(conflict_source_pane(draft,1,"Base · common ancestor (stage 1)",source_height)); }
            sources = sources.child(conflict_source_pane(draft,2,ours,source_height))
                .child(conflict_source_pane(draft,3,theirs,source_height));
            let mut choices = div().flex().flex_wrap().items_center().gap_2();
            for (side,label,id) in [(2,"Ours 사용","choose-ours"),(3,"Theirs 사용","choose-theirs")] {
                let content = draft.side_content(side);
                let supported = !matches!(content,Some(ConflictContent::Unsupported(_)));
                let label = if content.is_none() { format!("{label} (파일 삭제)") } else { label.into() };
                choices = choices.child(wf_action(&weak,id,label,busy || !supported,move |app,w,cx|app.workflow_choose_side(side,w,cx)).selected(draft.side == Some(side)));
            }
            if draft.can_edit {
                choices = choices.child(wf_action(&weak,"conflict-edit","결과 편집",busy,|app,w,cx|app.workflow_edit(w,cx)).selected(draft.side.is_none()));
                if draft.manual.is_some() { choices = choices.child(wf_action(&weak,"conflict-restore-edit","이전 편집 복원",busy,|app,w,cx|app.workflow_restore_result(w,cx))); }
            }
            let marker_count = conflict_marker_lines(draft.result.read(cx).value().as_ref()).len();
            let save_label = if draft.side.is_some_and(|s|draft.side_content(s).is_none()) { "삭제하고 해결 표시" } else { "저장하고 해결 표시" };
            let title = format!("충돌 해결 · {}",draft.snapshot.file.path.display());
            dialog.title(div().text_ellipsis().child(title)).width(ui_px(880.)).margin_top(ui_px(12.)).overlay_closable(false).on_ok(|_,_,_|false)
                .child(div().id("conflict-body").v_flex().gap_1().max_h(ui_px((viewport_height-142.).max(300.))).overflow_y_scroll()
                    .child(workflow_scope(&draft.root))
                    .child(div().flex().items_center().gap_2()
                        .child(div().flex_1().text_size(ui_text(11.)).child("원본 · 읽기 전용"))
                        .child(wf_action(&weak,"conflict-base","Base 보기",busy,|app,_,cx| { if let Some(draft)=app.workflow.draft.as_mut() { draft.show_base = !draft.show_base; } cx.notify(); }).selected(draft.show_base))
                        .child(wf_action(&weak,"conflict-next-file","다음 충돌 파일 →",busy,|app,w,cx|app.open_next_conflict(w,cx))))
                    .child(sources).child(choices)
                    .child(div().flex().items_center().gap_2()
                        .child(div().flex_1().text_size(ui_text(11.)).child(format!("결과 · 충돌 마커 {marker_count}개")))
                        .child(wf_action(&weak,"conflict-marker-prev","이전 마커 ↑",busy || marker_count == 0,|app,w,cx|app.navigate_conflict_marker(false,w,cx)))
                        .child(wf_action(&weak,"conflict-marker-next","다음 마커 ↓",busy || marker_count == 0,|app,w,cx|app.navigate_conflict_marker(true,w,cx))))
                    .child(Editor::new(&draft.result).aria_label("충돌 해결 결과").readonly(draft.result_readonly()).bordered(true).h(ui_px(result_height)))
                    .child(workflow_note("작성한 결과는 파일·워크트리별로 유지됩니다. 저장 시 원래 디스크·index 상태를 검증하고 결과만 저장·stage 합니다.")))
                .footer(div().flex().gap_2().justify_end()
                    .child(wf_action(&weak,"conflict-discard","초안 버리기…",busy,|app,w,cx|app.confirm_discard_conflict(w,cx)))
                    .child(wf_action(&weak,"conflict-back","목록 · 결과 보관",busy,|app,w,cx|app.open_git_workflow(w,cx)))
                    .child(workflow_close())
                    .child(wf_action(&weak,"resolve-save",save_label,busy || (!draft.can_edit && draft.side.is_none()),|app,w,cx|app.save_workflow_resolution(w,cx)).primary()))
        });
    }
    fn open_next_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.workflow.draft else {
            return;
        };
        let root = draft.root.clone();
        let path = draft.snapshot.file.path.clone();
        self.workflow_read(
            root,
            |ops| ops.workflow_state(),
            move |app, state, w, cx| {
                if let Some(file) = next_conflict_file(&state.conflicts, &path) {
                    app.open_conflict(state.root, file.clone(), w, cx);
                } else {
                    app.status = "다른 미해결 충돌 파일이 없습니다. 현재 결과는 유지됩니다.".into();
                    cx.notify();
                }
            },
            window,
            cx,
        );
    }
    fn navigate_conflict_marker(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = &self.workflow.draft else {
            return;
        };
        if self.git_is_busy() || self.current.as_ref() != Some(&draft.root) {
            return;
        }
        let editor = draft.result.clone();
        let state = editor.read(cx);
        let lines = conflict_marker_lines(state.value().as_ref());
        let Some(line) = next_marker_line(&lines, state.cursor_position().line, forward) else {
            return;
        };
        // EditorState moves and scrolls its own caret; do not guess pixel offsets.
        editor.update(cx, |editor, cx| {
            editor.set_cursor_position(Position::new(line, 0), window, cx);
            let start = editor.text().position_to_offset(&Position::new(line, 0));
            let end = start
                + editor
                    .value()
                    .lines()
                    .nth(line as usize)
                    .map_or(0, str::len);
            editor.set_selected_range(start..end, cx);
        });
        cx.notify();
    }
    fn confirm_discard_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.workflow.draft else {
            return;
        };
        let key = draft.key();
        let weak = cx.entity().downgrade();
        window.open_dialog(cx,move |dialog,_,cx| {
            let target=key.clone();
            dialog.title("작성한 충돌 해결 초안 버리기").width(ui_px(620.)).overlay_closable(false).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_2().child(workflow_scope(&key.0)).child(workflow_note(&format!("{}의 보관 중인 편집 결과와 side 선택을 버립니다. 작업 파일과 index는 바꾸지 않습니다.",key.1.display()))))
                .footer(div().flex().justify_end().gap_2().child(workflow_close())
                    .child(wf_action(&weak,"conflict-discard-confirm","이 초안 버리기",workflow_disabled(&weak,&key.0,cx),move |app,w,cx| {
                        // A stale confirmation cannot discard a newly activated file.
                        if app.workflow.draft.as_ref().is_none_or(|draft|draft.key()!=target) { return; }
                        app.workflow.draft=None;
                        app.workflow.retained.remove(&target);
                        app.open_git_workflow(w,cx);
                    }).danger()))
        });
    }
    fn workflow_choose_side(&mut self, side: u8, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy()
            || self
                .workflow
                .draft
                .as_ref()
                .is_none_or(|d| self.current.as_ref() != Some(&d.root))
        {
            return;
        }
        let Some(draft) = self.workflow.draft.as_mut() else {
            return;
        };
        let current = draft.result.read(cx).value().to_string();
        let selected_text = draft
            .side
            .and_then(|s| draft.side_content(s))
            .and_then(ConflictContent::text);
        let manual = retained_manual_result(draft.can_edit, draft.side, &current, selected_text);
        if let Some(manual) = manual {
            draft.manual = Some(manual);
        }
        draft.side = Some(side);
        let content = draft.side_content(side);
        let text = describe_content(content);
        let lang = content_language(&draft.snapshot.file.path, content);
        let readonly = draft.result_readonly();
        draft.result.update(cx, |editor, cx| {
            editor.set_highlighter(lang, cx);
            editor.set_value(text, window, cx);
            editor.set_readonly(readonly, cx);
        });
        cx.notify();
    }
    fn workflow_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy()
            || self
                .workflow
                .draft
                .as_ref()
                .is_none_or(|d| self.current.as_ref() != Some(&d.root))
        {
            return;
        }
        let Some(draft) = self.workflow.draft.as_mut() else {
            return;
        };
        if !draft.can_edit {
            return;
        }
        let deleted = draft
            .side
            .is_some_and(|side| draft.side_content(side).is_none());
        draft.side = None;
        draft.result.update(cx, |editor, cx| {
            if deleted {
                editor.set_highlighter(language(&draft.snapshot.file.path), cx);
                editor.set_value("", window, cx);
            }
            editor.set_readonly(false, cx);
        });
        draft.result.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }
    fn workflow_restore_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy()
            || self
                .workflow
                .draft
                .as_ref()
                .is_none_or(|d| self.current.as_ref() != Some(&d.root))
        {
            return;
        }
        let Some(draft) = self.workflow.draft.as_mut() else {
            return;
        };
        if !draft.can_edit {
            return;
        }
        let Some(text) = draft.manual.take() else {
            return;
        };
        // Restoration is reversible too; absent-side explanatory text is not a draft.
        if !draft.result_readonly() {
            draft.manual = Some(draft.result.read(cx).value().to_string());
        }
        draft.side = None;
        draft.result.update(cx, |editor, cx| {
            editor.set_highlighter(language(&draft.snapshot.file.path), cx);
            editor.set_value(text, window, cx);
            editor.set_readonly(false, cx);
        });
        draft.result.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }
    fn save_workflow_resolution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.workflow.draft else {
            return;
        };
        let job = WorkflowWrite::Resolve {
            snapshot: draft.snapshot.clone(),
            resolution: draft.resolution(cx),
        };
        self.workflow_write(draft.root.clone(), "충돌 결과 저장·stage", job, window, cx);
    }
}
fn retained_manual_result(
    can_edit: bool,
    side: Option<u8>,
    current: &str,
    selected_text: Option<&str>,
) -> Option<String> {
    (can_edit && (side.is_none() || selected_text.is_some_and(|text| text != current)))
        .then(|| current.to_owned())
}
fn conflict_heights(viewport_height: f32) -> (f32, f32) {
    let available = (viewport_height - 320.).max(200.);
    (
        (available * 0.44).clamp(88., 180.),
        (available * 0.56).clamp(112., 260.),
    )
}
fn conflict_source_pane(
    draft: &ResolutionDraft,
    stage: u8,
    label: &'static str,
    height: f32,
) -> Div {
    let identity = draft
        .snapshot
        .stages
        .iter()
        .find(|s| s.entry.stage == stage)
        .map(|s| format!("{} · {}", s.entry.mode, short_oid(&s.entry.oid)))
        .unwrap_or_else(|| "stage 없음 · 선택 시 파일 삭제".into());
    div()
        .v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(div().h(ui_px(30.)).text_size(ui_text(11.)).child(label))
        .child(
            div()
                .text_size(ui_text(10.))
                .text_ellipsis()
                .child(identity),
        )
        .child(
            Editor::new(&draft.sources[(stage - 1) as usize])
                .aria_label(label)
                .readonly(true)
                .bordered(true)
                .h(ui_px(height)),
        )
}
fn conflict_marker_lines(text: &str) -> Vec<u32> {
    text.lines()
        .enumerate()
        .filter_map(|(line, text)| {
            let rest = text.strip_prefix("<<<<<<<")?.trim_start_matches('<');
            (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(line as u32)
        })
        .collect()
}
fn next_marker_line(lines: &[u32], current: u32, forward: bool) -> Option<u32> {
    if forward {
        lines
            .iter()
            .copied()
            .find(|line| *line > current)
            .or_else(|| lines.first().copied())
    } else {
        lines
            .iter()
            .rev()
            .copied()
            .find(|line| *line < current)
            .or_else(|| lines.last().copied())
    }
}
fn next_conflict_file<'a>(files: &'a [ConflictFile], current: &Path) -> Option<&'a ConflictFile> {
    let next = files
        .iter()
        .position(|file| file.path == current)
        .map_or(0, |i| i + 1);
    files
        .iter()
        .skip(next)
        .chain(files.iter().take(next))
        .find(|file| file.path != current)
}
fn stash_write_detail(entry: &StashEntry, action: &WorkflowWrite) -> String {
    let effect = match action {
        WorkflowWrite::Drop(_) => {
            "공유 stash를 삭제합니다. 이 항목의 보관 내용을 잃을 수 있습니다."
        }
        WorkflowWrite::Pop(_) => {
            "이 워크트리에 적용하고 성공하면 공유 stash에서 제거합니다. 충돌이나 실패 시 stash를 유지합니다."
        }
        _ => "이 워크트리에 적용합니다. Stash는 유지됩니다.",
    };
    format!(
        "{effect}\n{}\n{}\n{}",
        entry.selector, entry.oid, entry.message
    )
}
fn workflow_disabled(weak: &WeakEntity<Pokarak>, root: &Path, cx: &App) -> bool {
    weak.upgrade().is_none_or(|view| {
        let app = view.read(cx);
        app.git_is_busy() || app.loading || app.current.as_deref() != Some(root)
    })
}
fn wf_action(
    weak: &WeakEntity<Pokarak>,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
    action: impl Fn(&mut Pokarak, &mut Window, &mut Context<Pokarak>) + 'static,
) -> Button {
    let target = weak.clone();
    theme::action_button(id, disabled)
        .label(label)
        .small()
        .on_click(move |_, w, cx| {
            let _ = target.update(cx, |app, cx| action(app, w, cx));
        })
}
fn workflow_close() -> Button {
    theme::action_button("workflow-close", false)
        .label("닫기")
        .small()
        .on_click(|_, w, cx| w.close_dialog(cx))
}
fn workflow_scope(root: &Path) -> Div {
    div()
        .text_size(ui_text(12.))
        .text_ellipsis()
        .child(format!("작업 대상: {}", root.display()))
}
fn workflow_note(text: &str) -> Div {
    div().text_size(ui_text(12.)).child(text.to_owned())
}
fn short_oid(oid: &str) -> &str {
    oid.get(..12).unwrap_or(oid)
}
fn describe_content(content: Option<&ConflictContent>) -> String {
    match content {
        None => "[이 stage/파일은 없습니다. 해당 side 선택은 삭제를 뜻합니다.]".into(),
        Some(ConflictContent::Text(text)) => text.clone(),
        Some(ConflictContent::Binary(bytes)) => {
            format!("[Binary · {} bytes · 원본 side 선택만 지원]", bytes.len())
        }
        Some(ConflictContent::Unsupported(reason)) => {
            format!("[외부 Git 도구로 해결 필요: {reason}]")
        }
    }
}
fn content_language(path: &Path, content: Option<&ConflictContent>) -> &'static str {
    match content {
        Some(ConflictContent::Text(_)) => language(path),
        _ => "text",
    }
}
fn workflow_editor(
    text: String,
    lang: &'static str,
    readonly: bool,
    window: &mut Window,
    cx: &mut Context<Pokarak>,
) -> Entity<EditorState> {
    let editor = cx.new(|cx| {
        EditorState::new(window, cx)
            .language(lang)
            .line_number(true)
            .soft_wrap(false)
            .default_value(text)
    });
    editor.update(cx, |editor, cx| editor.set_readonly(readonly, cx));
    editor
}
fn editor_view(editor: &Entity<EditorState>, readonly: bool, height: f32) -> impl IntoElement {
    Editor::new(editor)
        .readonly(readonly)
        .bordered(true)
        .h(ui_px(height))
}

#[cfg(test)]
mod tests {
    use super::{
        ConflictFile, ProgressKind, conflict_heights, conflict_marker_lines, next_conflict_file,
        next_marker_line, retained_manual_result,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn marker_navigation_uses_editor_lines_handles_unicode_and_wraps() {
        let text = "한글\n<<<<<<< HEAD\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> topic\n중간\n<<<<<<<<< custom\nend\n";
        let lines = conflict_marker_lines(text);
        assert_eq!(lines, vec![1, 9]);
        assert_eq!(next_marker_line(&lines, 0, true), Some(1));
        assert_eq!(next_marker_line(&lines, 1, true), Some(9));
        assert_eq!(next_marker_line(&lines, 9, true), Some(1));
        assert_eq!(next_marker_line(&lines, 9, false), Some(1));
        assert_eq!(next_marker_line(&lines, 1, false), Some(9));
        assert_eq!(next_marker_line(&[], 0, true), None);
        assert!(conflict_marker_lines("   <<<<<<< indented\n<<<<<<<operator\n=======").is_empty());
        assert_eq!(conflict_marker_lines("<<<<<<<\r\ntext\r\n"), vec![0]);
    }

    #[test]
    fn next_conflict_wraps_and_survives_resolution_or_disappearing_current_file() {
        let files: Vec<_> = ["a", "b", "c"]
            .map(|path| ConflictFile {
                path: PathBuf::from(path),
                stages: vec![],
            })
            .into();
        assert_eq!(
            next_conflict_file(&files, Path::new("a")).unwrap().path,
            Path::new("b")
        );
        assert_eq!(
            next_conflict_file(&files, Path::new("c")).unwrap().path,
            Path::new("a")
        );
        assert_eq!(
            next_conflict_file(&files, Path::new("removed"))
                .unwrap()
                .path,
            Path::new("a")
        );
        assert!(next_conflict_file(&files[..1], Path::new("a")).is_none());
        assert!(next_conflict_file(&[], Path::new("a")).is_none());
    }

    #[test]
    fn side_switch_preserves_manual_edits_without_replacing_them_with_unedited_side() {
        let edited = "my edited resolution\n";
        let mut retained = retained_manual_result(true, None, edited, None);
        assert_eq!(retained.as_deref(), Some(edited));
        // Switching Ours -> Theirs must keep the earlier direct edit.
        if let Some(text) = retained_manual_result(true, Some(2), "ours\n", Some("ours\n")) {
            retained = Some(text);
        }
        assert_eq!(retained.as_deref(), Some(edited));
        assert_eq!(
            retained_manual_result(true, Some(2), "edited ours\n", Some("ours\n")),
            Some("edited ours\n".into())
        );
        // Absent-side explanatory text and binary previews are never saved as text edits.
        assert!(retained_manual_result(true, Some(3), "absent side", None).is_none());
        assert!(retained_manual_result(false, None, "binary placeholder", None).is_none());
    }

    #[test]
    fn minimum_window_keeps_two_usable_source_views_and_editable_result() {
        let (source, result) = conflict_heights(580.);
        assert!(source >= 100. && result >= 140.);
        assert!(source + result <= 270.);
        let (source, result) = conflict_heights(900.);
        assert!(source >= 160. && result >= 240.);
    }

    #[test]
    fn rebase_source_roles_are_not_confused_with_original_checkout_branch() {
        let (ours, theirs) = ProgressKind::Rebase.side_labels();
        assert!(ours.contains("rebased-onto") && ours.contains("stage 2"));
        assert!(theirs.contains("replayed") && theirs.contains("stage 3"));
    }
}
