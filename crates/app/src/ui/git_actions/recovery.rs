//! Explicit exact-endpoint comparison, reflog and reference-only recovery.
use super::review::{Review, ReviewSource};
use super::*;
use pokarak_core::operations::recovery::ReflogEntry;
impl Pokarak {
    pub(super) fn confirm_git_extra(
        &mut self,
        confirmation: GitExtra,
        job: impl Fn(GitOperations) -> anyhow::Result<OperationOutput> + Send + Sync + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let GitExtra {
            root,
            label,
            detail,
            destructive,
        } = confirmation;
        let job = Arc::new(job);
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let target = weak.clone();
            let root_for_job = root.clone();
            let title = label.clone();
            let job = job.clone();
            let disabled = weak.upgrade().is_none_or(|view| {
                let app = view.read(cx);
                app.git_is_busy()
                    || app.current.as_ref() != Some(&root)
                    || app.opening_path.is_some()
            });
            dialog
                .on_close({
                    let owner = weak.clone();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |app, _| app.invalidate_workflow_read());
                    }
                })
                .title(format!("{label} 확인"))
                .width(ui_px(700.))
                .margin_top(ui_px(16.))
                .child(
                    div()
                        .id("git-extra-confirm")
                        .v_flex()
                        .gap_3()
                        .max_h(ui_px(340.))
                        .overflow_y_scroll()
                        .child(scope(&root, None))
                        .child(detail.clone()),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(super::recovery::review_close(&weak))
                        .child(
                            theme::action_button("git-extra-execute", disabled)
                                .label(label.clone())
                                .when(destructive, |b| b.danger())
                                .when(!destructive, |b| b.primary())
                                .on_click(move |_, w, cx| {
                                    let job = job.clone();
                                    let _ = target.update(cx, |app, cx| {
                                        w.close_dialog(cx);
                                        app.run_git_job(
                                            root_for_job.clone(),
                                            title.clone(),
                                            move |ops| job(ops),
                                            w,
                                            cx,
                                        );
                                    });
                                }),
                        ),
                )
        });
    }
    pub(super) fn show_ref_comparison(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let left = cx.new(|cx| InputState::new(window, cx).default_value("HEAD^"));
        let right = cx.new(|cx| InputState::new(window, cx).default_value("HEAD"));
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_dialog(cx,move |dialog,_,cx| {
            let (target,root,left,right) = (weak.clone(),root.clone(),left.clone(),right.clone());
            let disabled = left.read(cx).value().trim().is_empty() || right.read(cx).value().trim().is_empty();
            dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title("두 참조 직접 비교").width(ui_px(640.)).child(div().v_flex().gap_3().child(scope(&root,None))
                .child(note("커밋 해시, 태그 또는 브랜치를 입력하세요. 선택한 두 커밋 자체를 비교합니다. ‘브랜치 비교’의 공통 조상 기준 비교와 별개이며 현재 작업 파일은 포함하지 않습니다."))
                .child(field("이전 참조",&left)).child(field("이후 참조",&right)))
                .footer(div().flex().justify_end().gap_2().child(super::recovery::review_close(&weak)).child(theme::action_button("git-ref-compare",disabled).label("직접 비교").primary().on_click(move |_,w,cx| {
                    let left = left.read(cx).value().trim().to_owned(); let right = right.read(cx).value().trim().to_owned(); let read_root = root.clone(); let show_root = root.clone();
                    let description = format!("{left} → {right}");
                    let _ = target.update(cx, |app,cx| app.workflow_read(root.clone(),move |_| pokarak_core::comparison::compare_refs(&read_root,&left,&right),move |app,comparison,w,cx| app.show_git_review(Review { root: show_root, title:"두 참조 직접 비교".into(), description, source:ReviewSource::Comparison(comparison),write:None },w,cx),w,cx));
                })))
        });
    }
    pub(super) fn open_reflog(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_read(root.clone(), |ops| ops.reflog(200), move |app,entries,w,cx| {
            let weak = cx.entity().downgrade();
            w.close_dialog(cx);
            w.open_dialog(cx,move |dialog,_,_| {
                let mut body = div().id("reflog-list").v_flex().gap_2().max_h(ui_px(390.)).overflow_y_scroll().child(scope(&root,None))
                    .child(note("이 워크트리 HEAD의 최근 Reflog 최대 200개입니다. 항목을 선택해 커밋 코드와 첫 부모 기준 diff를 확인하세요. 만료되거나 삭제된 객체는 읽을 수 없습니다."));
                if entries.is_empty() { body = body.child(note("기록된 HEAD Reflog가 없습니다.")); }
                for (index,entry) in entries.iter().enumerate() {
                    let target = weak.clone(); let entry = entry.clone();
                    body = body.child(Button::new(("reflog-entry",index)).label(format!("{} · {} · {}",entry.timestamp,&entry.oid[..8],entry.subject)).small().on_click(move |_,w,cx| {
                        let entry = entry.clone(); let read_root = entry.root.clone(); let oid = entry.oid.clone();
                        let _ = target.update(cx, |app,cx| app.workflow_read(entry.root.clone(),move |_| pokarak_core::history::commit_detail(&read_root,&oid,0),move |app,detail,w,cx| app.show_git_review(Review { root:entry.root.clone(),title:"Reflog 커밋".into(),description:entry.subject.clone(),source:ReviewSource::Reflog { entry,detail },write:None },w,cx),w,cx));
                    }));
                }
                dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title("Reflog / 이력 복구").width(ui_px(790.)).margin_top(ui_px(12.)).child(body).footer(close_row())
            });
            let _ = app;
        },window,cx);
    }
    pub(super) fn show_recovery_branch(
        &mut self,
        entry: ReflogEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("recovered-work"));
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_dialog(cx,move |dialog,_,cx| {
            let (target,entry,name) = (weak.clone(),entry.clone(),name.clone());
            let disabled = name.read(cx).value().trim().is_empty();
            dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title("복구 브랜치 만들기").width(ui_px(650.)).child(div().v_flex().gap_3().child(scope(&entry.root,None)).child(format!("{}\n{}\n{}",entry.selector,entry.oid,entry.subject)).child(field("새 로컬 브랜치",&name)).child(note("선택한 커밋에 새 브랜치만 만듭니다. 현재 브랜치 전환, index 또는 작업 파일 변경은 하지 않습니다.")))
                .footer(div().flex().justify_end().gap_2().child(super::recovery::review_close(&weak)).child(theme::action_button("recovery-branch-confirm",disabled).label("대상 확인…").on_click(move |_,w,cx| {
                    let name = name.read(cx).value().trim().to_owned(); let entry = entry.clone();
                    let detail = format!("새 브랜치: {name}\n시작 커밋: {}\n현재 브랜치, index와 작업 파일은 유지합니다. 기존 브랜치를 덮어쓰지 않습니다.",entry.oid);
                    let _ = target.update(cx, |app,cx| app.confirm_git_extra(GitExtra { root: entry.root.clone(),label: "복구 브랜치 만들기".into(),detail,destructive: false },move |ops| ops.create_recovery_branch(&name,&entry),w,cx));
                })))
        });
    }
    pub(super) fn open_undo_last_commit(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_read(root,|ops| ops.undo_last_commit_target(),move |app,target,w,cx| {
            let detail = format!("브랜치: {}\n취소할 마지막 커밋: {}\n이동할 첫 부모 커밋: {}\n브랜치 참조만 이전으로 이동합니다. 현재 index와 작업 파일의 모든 변경은 그대로 남습니다. 마지막 커밋의 변경도 다시 검토 / 커밋할 수 있습니다. 병합 커밋은 첫 부모를 사용합니다. 공유된 이력을 바꿀 수 있으므로 정확한 대상을 확인하세요.",target.current.branch,target.current.head,target.parent_oid);
            app.confirm_git_extra(GitExtra { root: target.current.root.clone(),label: "마지막 커밋 취소 · 변경 보존".into(),detail,destructive: true },move |ops| ops.undo_last_commit(&target),w,cx);
        },window,cx);
    }
}

pub(super) fn review_close(weak: &WeakEntity<Pokarak>) -> Button {
    let owner = weak.clone();
    theme::action_button("git-review-close", false)
        .label("취소")
        .on_click(move |_, w, cx| {
            let _ = owner.update(cx, |app, _| app.invalidate_workflow_read());
            w.close_dialog(cx);
        })
}
