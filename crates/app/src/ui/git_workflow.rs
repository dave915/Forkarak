//! One explicit workflow entry; every write uses git_actions' shared job guard.
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
    draft: Option<ResolutionDraft>,
}
struct ResolutionDraft {
    root: PathBuf,
    snapshot: ConflictSnapshot,
    source: Entity<EditorState>,
    result: Entity<EditorState>,
    source_stage: u8,
    side: Option<u8>,
    can_edit: bool,
}
impl ResolutionDraft {
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
    pub(super) fn workflow_read<T: Send + 'static>(
        &mut self,
        root: PathBuf,
        job: impl FnOnce(GitOperations) -> anyhow::Result<T> + Send + 'static,
        ready: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() || self.current.as_ref() != Some(&root) {
            return;
        }
        self.workflow.request += 1;
        let token = self.workflow.request;
        let navigation_generation = self.request;
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
                    .child(div().flex_1().child("Stash · 이 저장소의 모든 워크트리가 공유"))
                    .child(wf_action(&weak, "stash-new", "변경사항 보관…", busy, move |app,w,cx| app.show_stash_save(save_root.clone(),w,cx)))
                    .child(wf_action(&weak, "workflow-refresh", "새로고침", busy, |app,w,cx| app.open_git_workflow(w,cx))));
            let mut stashes = div().id("workflow-stashes").h(px(116.)).overflow_y_scroll().v_flex().gap_1();
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
                body = body.child(workflow_note("진행 중인 merge/rebase가 없습니다. Stash 적용 충돌은 파일만 해결하세요. 보관된 stash는 유지됩니다."));
            }
            let mut conflicts = div().id("workflow-conflicts").h(px(112.)).overflow_y_scroll().v_flex().gap_1();
            if state.conflicts.is_empty() { conflicts = conflicts.child(workflow_note("미해결 index stage가 없습니다.")); }
            for (index, file) in state.conflicts.iter().enumerate() {
                let root = state.root.clone(); let file = file.clone();
                let label = format!("{}  · stages {}", file.path.display(), file.stages.iter().map(|s|s.stage.to_string()).collect::<Vec<_>>().join("/"));
                conflicts = conflicts.child(wf_action(&weak,("conflict-entry", index),label,busy,move |app,w,cx| app.open_conflict(root.clone(),file.clone(),w,cx)).w_full().justify_start());
            }
            dialog.title("Stash / 충돌 해결").width(px(740.)).margin_top(px(24.)).on_ok(|_,_,_|false)
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
            dialog.title("변경사항을 stash에 보관").width(px(620.)).margin_top(px(36.)).on_ok(|_,_,_|false)
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
            move |ops| ops.stash_patch(&selected),
            move |app, patch, w, cx| app.show_stash(target, entry, patch, w, cx),
            window,
            cx,
        );
    }
    fn show_stash(
        &mut self,
        root: PathBuf,
        entry: StashEntry,
        patch: OperationOutput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = format!(
            "{}{}",
            patch.stdout,
            if patch.truncated {
                "\n[Patch 일부 생략 — 출력 제한]"
            } else {
                ""
            }
        );
        let preview = workflow_editor(text, Path::new("stash.diff"), true, window, cx);
        let path =
            cx.new(|cx| InputState::new(window, cx).placeholder("내용을 볼 저장소 상대 파일 경로"));
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_dialog(cx,move |dialog,_,cx| {
            let busy=workflow_disabled(&weak,&root,cx);
            let mut inspect=div().flex().gap_2().child(Input::new(&path).flex_1());
            for (untracked,label,id) in [(false,"파일 내용","stash-tracked-content"),(true,"미추적 내용","stash-untracked-content")] {
                let input=path.clone();let target=root.clone();let selected=entry.clone();
                inspect=inspect.child(wf_action(&weak,id,label,busy,move |app,w,cx| {
                    let path=PathBuf::from(input.read(cx).value().as_ref());let filename=path.display().to_string();
                    app.workflow_read(target.clone(),{let entry=selected.clone();move |ops|ops.stash_content(&entry,&path,untracked)},move |app,content,w,cx|app.show_workflow_content(filename,content,w,cx),w,cx);
                }));
            }
            let mut actions=div().flex().gap_2().justify_end().child(workflow_close());
            for (id,label,action) in [("stash-apply","적용",WorkflowWrite::Apply(entry.clone())),("stash-pop","적용 후 제거",WorkflowWrite::Pop(entry.clone())),("stash-drop","삭제…",WorkflowWrite::Drop(entry.clone()))] {
                let target=root.clone();let selected=entry.clone();
                actions=actions.child(wf_action(&weak,id,label,busy,move |app,w,cx| {
                    let detail=match &action {
                        WorkflowWrite::Drop(_) => format!("공유 stash를 삭제합니다. 이 항목의 보관 내용을 잃을 수 있습니다.\n{}\n{}\n{}",selected.selector,selected.oid,selected.message),
                        WorkflowWrite::Pop(_) => format!("이 워크트리에 적용하고 성공하면 공유 stash에서 제거합니다. 충돌이나 실패 시 stash를 유지합니다.\n{}\n{}",selected.oid,selected.message),
                        _ => format!("이 워크트리에 적용합니다. Stash는 유지됩니다.\n{}\n{}",selected.oid,selected.message),
                    };
                    app.confirm_workflow(target.clone(),format!("Stash {label}"),detail,action.clone(),w,cx);
                }));
            }
            dialog.title(format!("{} · {}",entry.selector,short_oid(&entry.oid))).width(px(780.)).margin_top(px(24.)).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_2().child(workflow_scope(&root))
                    .child(div().text_ellipsis().child(entry.message.clone()))
                    .child(workflow_note(&format!("원본 브랜치 기록: {} · 저장소 전체 공유",entry.source_branch.as_deref().unwrap_or("확인 불가"))))
                    .child(editor_view(&preview,true,214.))
                    .child(inspect))
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
            Path::new(&title),
            true,
            window,
            cx,
        );
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(title.clone())
                .width(px(740.))
                .margin_top(px(36.))
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
                .width(px(640.))
                .margin_top(px(36.))
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .id("workflow-confirm-detail")
                        .max_h(px(290.))
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
        let source = workflow_editor(
            describe_content(
                snapshot
                    .stages
                    .iter()
                    .find(|s| s.entry.stage == 1)
                    .map(|s| &s.content),
            ),
            &snapshot.file.path,
            true,
            window,
            cx,
        );
        let result = workflow_editor(
            snapshot
                .working
                .as_ref()
                .map(|c| describe_content(Some(c)))
                .unwrap_or_default(),
            &snapshot.file.path,
            !can_edit,
            window,
            cx,
        );
        self.workflow.draft = Some(ResolutionDraft {
            root: root.clone(),
            snapshot,
            source,
            result,
            source_stage: 1,
            side: None,
            can_edit,
        });
        let weak = cx.entity().downgrade();
        let (ours, theirs) = kind.unwrap_or(ProgressKind::Merge).side_labels();
        window.close_all_dialogs(cx);
        window.open_dialog(cx,move |dialog,_,cx| {
            let Some(view)=weak.upgrade() else {return dialog;};
            let app=view.read(cx);let Some(draft)=&app.workflow.draft else {return dialog;};
            let busy=workflow_disabled(&weak,&root,cx);let mut tabs=div().flex().gap_2();
            for (stage,label,id) in [(1,"Base","conflict-base"),(2,"Ours","conflict-ours"),(3,"Theirs","conflict-theirs")] {
                tabs=tabs.child(wf_action(&weak,id,label,busy,move |app,w,cx|app.workflow_source(stage,w,cx)).selected(draft.source_stage==stage));
            }
            let stage_info=draft.snapshot.stages.iter().find(|s|s.entry.stage==draft.source_stage).map(|s|format!("stage {} · {} · {}",s.entry.stage,s.entry.mode,s.entry.oid)).unwrap_or_else(||"이 stage는 없습니다 (추가/삭제 충돌)".into());
            let mut choices=div().flex().gap_2();
            for (side,label,id) in [(2,"Ours 사용","choose-ours"),(3,"Theirs 사용","choose-theirs")] {
                let content=draft.side_content(side);let supported=!matches!(content,Some(ConflictContent::Unsupported(_)));
                let label=if content.is_none(){format!("{label} (파일 삭제)")}else{label.into()};
                choices=choices.child(wf_action(&weak,id,label,busy||!supported,move |app,w,cx|app.workflow_choose_side(side,w,cx)).selected(draft.side==Some(side)));
            }
            if draft.can_edit { choices=choices.child(wf_action(&weak,"conflict-edit","결과 편집",busy,|app,w,cx|app.workflow_edit(w,cx)).selected(draft.side.is_none())); }
            let readonly=draft.result_readonly();
            let save_label=if draft.side.is_some_and(|s|draft.side_content(s).is_none()){ "삭제하고 해결 표시" } else { "저장하고 해결 표시" };
            let disabled=busy||(!draft.can_edit&&draft.side.is_none());
            let title=format!("충돌 해결 · {}",draft.snapshot.file.path.display());
            dialog.title(div().text_ellipsis().child(title)).width(px(820.)).margin_top(px(16.)).overlay_closable(false).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_1().child(workflow_scope(&root))
                    .child(workflow_note(&format!("{ours}\n{theirs}")))
                    .child(tabs).child(div().text_size(px(11.)).text_ellipsis().child(stage_info))
                    .child(editor_view(&draft.source,true,86.))
                    .child(choices).child(editor_view(&draft.result,readonly,130.))
                    .child(workflow_note("결과만 저장·stage 합니다. 커밋/푸시는 별도입니다. 외부 변경이 있으면 다시 열어 비교하세요.")))
                .footer(div().flex().gap_2().justify_end().child(workflow_close()).child(wf_action(&weak,"resolve-save",save_label,disabled,|app,w,cx|app.save_workflow_resolution(w,cx)).primary()))
        });
    }
    fn workflow_source(&mut self, stage: u8, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.workflow.draft.as_mut() else {
            return;
        };
        draft.source_stage = stage;
        let text = describe_content(draft.side_content(stage));
        draft
            .source
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        cx.notify();
    }
    fn workflow_choose_side(&mut self, side: u8, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.workflow.draft.as_mut() else {
            return;
        };
        draft.side = Some(side);
        let text = describe_content(draft.side_content(side));
        let readonly = draft.result_readonly();
        draft.result.update(cx, |editor, cx| {
            editor.set_value(text, window, cx);
            editor.set_readonly(readonly, cx);
        });
        cx.notify();
    }
    fn workflow_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                editor.set_value("", window, cx);
            }
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
    Button::new(id)
        .label(label)
        .small()
        .disabled(disabled)
        .on_click(move |_, w, cx| {
            let _ = target.update(cx, |app, cx| action(app, w, cx));
        })
}
fn workflow_close() -> Button {
    Button::new("workflow-close")
        .label("닫기")
        .small()
        .on_click(|_, w, cx| w.close_dialog(cx))
}
fn workflow_scope(root: &Path) -> Div {
    div()
        .text_size(px(12.))
        .text_ellipsis()
        .child(format!("작업 대상: {}", root.display()))
}
fn workflow_note(text: &str) -> Div {
    div().text_size(px(12.)).child(text.to_owned())
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
fn workflow_editor(
    text: String,
    path: &Path,
    readonly: bool,
    window: &mut Window,
    cx: &mut Context<Pokarak>,
) -> Entity<EditorState> {
    let editor = cx.new(|cx| {
        EditorState::new(window, cx)
            .language(language(path))
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
        .h(px(height))
        .font_family("JetBrains Mono")
        .text_size(px(12.))
}
