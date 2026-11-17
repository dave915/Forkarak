//! Tags are explicit operations, independent of branch Push and application releases.
use super::*;
use std::{cell::Cell, rc::Rc};
impl Pokarak {
    pub(super) fn show_tag_create(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("태그 이름"));
        let target_ref = cx.new(|cx| InputState::new(window, cx).default_value("HEAD"));
        let message = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .line_number(false)
        });
        let annotated = Rc::new(Cell::new(false));
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_dialog(cx,move |dialog,_,cx| {
            let flag = annotated.clone(); let notify = weak.clone();
            let check = gpui_kit::component::checkbox::Checkbox::new("tag-annotated").label("주석 태그 · 메시지 포함").checked(annotated.get()).on_click(move |checked,_,cx| { flag.set(*checked); let _ = notify.update(cx, |_,cx| cx.notify()); });
            let mut body = div().v_flex().gap_3().child(scope(&root,None)).child(field("새 태그 이름",&name)).child(field("대상 커밋 / 태그 / 브랜치",&target_ref)).child(check);
            if annotated.get() { body = body.child(Editor::new(&message).aria_label("주석 태그 메시지").h(ui_px(110.))); }
            body = body.child(note("로컬 태그만 만듭니다. 원격 전송은 ‘태그 하나 Push’에서 별도로 선택하세요. 기존 태그는 덮어쓰지 않습니다. 주석 태그는 서명 없이 작성합니다."));
            let disabled = name.read(cx).value().trim().is_empty() || target_ref.read(cx).value().trim().is_empty() || (annotated.get() && message.read(cx).value().trim().is_empty());
            let (weak,root,name,target_ref,message,annotated) = (weak.clone(),root.clone(),name.clone(),target_ref.clone(),message.clone(),annotated.clone());
            dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title("태그 만들기").width(ui_px(680.)).child(body).footer(div().flex().justify_end().gap_2().child(super::recovery::review_close(&weak)).child(theme::action_button("tag-create-review",disabled).label("대상 확인…").on_click(move |_,w,cx| {
                let name = name.read(cx).value().trim().to_owned(); let reference = target_ref.read(cx).value().trim().to_owned();
                let message = annotated.get().then(|| message.read(cx).value().to_string());
                let read_root = root.clone(); let root = root.clone();
                let _ = weak.update(cx, |app,cx| app.workflow_read(root.clone(),move |_| pokarak_core::comparison::compare_refs(&read_root,&reference,&reference),move |app,comparison,w,cx| {
                    let oid = comparison.right_oid;
                    let detail = format!("새 태그: {name}\n확정한 대상 커밋: {oid}\n종류: {}\n{}\n원격 전송과 기존 태그 덮어쓰기는 하지 않습니다.", if message.is_some() { "주석 태그" } else { "경량 태그" }, message.as_deref().unwrap_or("메시지 없음"));
                    app.confirm_git_extra(GitExtra { root,label: "태그 만들기".into(),detail,destructive: false },move |ops| ops.create_tag(&name,&oid,message.as_deref()),w,cx);
                },w,cx));
            })))
        });
    }
    pub(super) fn open_tag_push(
        &mut self,
        state: Arc<RepositoryState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = state.root.clone();
        let read_root = root.clone();
        self.workflow_read(
            root,
            move |_| pokarak_core::history::navigation::open(&read_root, 1),
            move |app, (_, catalog), w, cx| {
                let tags: Vec<String> = catalog
                    .group(pokarak_core::history::navigation::RefGroup::Tag)
                    .iter()
                    .map(|r| r.label.clone())
                    .collect();
                app.show_tag_push(state, tags, w, cx);
            },
            window,
            cx,
        );
    }
    fn show_tag_push(
        &mut self,
        state: Arc<RepositoryState>,
        tags: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tag = choice(tags, None, window, cx);
        let remote = choice(
            state.remotes.iter().map(|r| r.name.clone()).collect(),
            None,
            window,
            cx,
        );
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_dialog(cx,move |dialog,_,cx| {
            let disabled = tag.read(cx).selected_value().is_none() || remote.read(cx).selected_value().is_none();
            let (weak,state,tag,remote) = (weak.clone(),state.clone(),tag.clone(),remote.clone());
            dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title("태그 하나 Push").width(ui_px(660.)).child(div().v_flex().gap_3().child(scope(&state.root,None)).child(select_field("전송할 로컬 태그",&tag)).child(select_field("보낼 원격 저장소",&remote)).child(note("선택한 태그 하나만 전송합니다. 서버 상태는 확인하지 않았으며 실행 시 기존 태그와 충돌하면 실패합니다. 강제 전송은 하지 않습니다.")))
                .footer(div().flex().justify_end().gap_2().child(super::recovery::review_close(&weak)).child(theme::action_button("tag-push-review",disabled).label("대상 확인…").on_click(move |_,w,cx| {
                    let Some(tag) = tag.read(cx).selected_value().cloned() else { return; };
                    let Some(remote_name) = remote.read(cx).selected_value().cloned() else { return; };
                    let Some(config) = state.remotes.iter().find(|r|r.name == remote_name).cloned() else { return; };
                    let root = state.root.clone(); let read_root = root.clone(); let reference = format!("refs/tags/{tag}");
                    let _ = weak.update(cx, |app,cx| app.workflow_read(root.clone(),move |_| pokarak_core::comparison::compare_refs(&read_root,&reference,&reference),move |app,comparison,w,cx| {
                        let oid = comparison.right_oid;
                        let detail = format!("태그: refs/tags/{tag}\n대상 커밋: {oid}\n원격: {remote_name}\nPush URL:\n{}\n이 태그 하나만 전송합니다. 브랜치, 다른 태그와 강제 Push는 포함하지 않습니다.",config.push_urls.join("\n"));
                        app.confirm_git_extra(GitExtra { root,label: "태그 하나 Push".into(),detail,destructive: false },move |ops| {
                            anyhow::ensure!(ops.remotes()?.iter().any(|r|r == &config),"원격 설정이 바뀌었습니다. 다시 확인하세요.");
                            let current = pokarak_core::comparison::compare_refs(&comparison.root,&format!("refs/tags/{tag}"),&format!("refs/tags/{tag}"))?;
                            anyhow::ensure!(current.right_oid == oid,"태그 대상이 바뀌었습니다. 다시 확인하세요.");
                            ops.push_tag(&remote_name,&tag)
                        },w,cx);
                    },w,cx));
                })))
        });
    }
}
