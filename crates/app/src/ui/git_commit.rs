//! A separate index reader and native commit form. No semantic navigation here.
//! Every write is owned by the existing git_actions runner and cancellation.
mod partial;
use super::*;
use pokarak_core::index::partial::{self as partial_index, PartialPreview, Side};
use pokarak_core::{
    index::{self, IndexSnapshot},
    operations::commit_actions::{CommitMode, StageScope},
};

actions!(pokarak_commit, [SubmitCommit]);

#[derive(Default)]
pub(super) struct CommitUi {
    open: bool,
    root: Option<PathBuf>,
    request: u64,
    preview_request: u64,
    reading: bool,
    snapshot: Option<Arc<IndexSnapshot>>,
    selected: Option<PathBuf>,
    preview: Option<Entity<EditorState>>,
    side: Option<Side>,
    partial: Option<Arc<PartialPreview>>,
    split: Option<Entity<split_diff::SplitDiff>>,
    marked: std::collections::BTreeSet<usize>,
    messages: HashMap<PathBuf, Entity<EditorState>>,
    error: Option<String>,
    clear_message_on_success: Option<PathBuf>,
}

impl Pokarak {
    pub(super) fn open_git_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy() || self.loading || self.opening_path.is_some() {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        window.close_all_dialogs(cx);
        if !self.commit_ui.messages.contains_key(&root) {
            let editor = commit_editor("", false, window, cx);
            self.commit_ui.messages.insert(root.clone(), editor);
        }
        self.commit_ui.open = true;
        self.commit_ui.root = Some(root);
        self.commit_ui.snapshot = None;
        self.commit_ui.selected = None;
        self.commit_ui.preview = None;
        self.commit_ui.partial = None;
        self.commit_ui.split = None;
        self.commit_ui.marked.clear();
        self.commit_ui.side = None;
        self.read_commit_index(window, cx);
        self.show_commit_dialog(window, cx);
    }

    fn read_commit_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.commit_ui.root.clone() else {
            return;
        };
        if self.git_is_busy() || self.current.as_ref() != Some(&root) {
            return;
        }
        self.commit_ui.request += 1;
        self.commit_ui.preview_request += 1;
        self.commit_ui.reading = true;
        self.commit_ui.error = None;
        let token = self.commit_ui.request;
        let navigation = self.request;
        let target = root.clone();
        let task = cx.background_spawn(async move { index::snapshot(&target) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if app.commit_ui.request != token {
                    return;
                }
                app.commit_ui.reading = false;
                if !app.commit_ui.open
                    || app.commit_ui.root.as_ref() != Some(&root)
                    || app.current.as_ref() != Some(&root)
                    || app.request != navigation
                    || app.git_is_busy()
                {
                    app.commit_ui.snapshot = None;
                    app.commit_ui.error =
                        Some("작업 대상이 바뀌었습니다. 커밋 화면을 다시 여세요.".into());
                    cx.notify();
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        let side = app.commit_ui.side.unwrap_or(Side::Staged);
                        let files = if side==Side::Staged {&snapshot.staged} else {&snapshot.unstaged};
                        let selected = app.commit_ui.selected.clone()
                            .filter(|path| files.iter().any(|f| &f.path == path))
                            .map(|path|(path, side))
                            .or_else(||snapshot.staged.first().map(|f|(f.path.clone(),Side::Staged)))
                            .or_else(||snapshot.unstaged.first().map(|f|(f.path.clone(),Side::Unstaged)));
                        app.commit_ui.snapshot = Some(Arc::new(snapshot));
                        app.commit_ui.selected=None;
                        app.commit_ui.side=None;
                        app.commit_ui.preview=None;
                        app.commit_ui.partial=None;
                        app.commit_ui.split=None;
                        app.commit_ui.marked.clear();
                        if let Some((path,side))=selected {app.select_index_preview(path,side,window,cx);}
                    }
                    Err(error) => {
                        app.commit_ui.snapshot = None;
                        app.commit_ui.error = Some(format!("인덱스 읽기 실패: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_index_preview(&mut self, path: PathBuf, side:Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.commit_ui.snapshot.clone() else {
            return;
        };
        self.commit_ui.preview_request += 1;
        let token = self.commit_ui.preview_request;
        let generation = self.commit_ui.request;
        self.commit_ui.selected = Some(path.clone());
        self.commit_ui.side=Some(side);
        self.commit_ui.partial=None;
        self.commit_ui.split=None;
        self.commit_ui.marked.clear();
        self.commit_ui.preview = None;
        let root = snapshot.root.clone();
        let task = cx.background_spawn(async move {
            match partial_index::preview(&snapshot,&path,side) {
                Ok(preview)=>Ok(preview),
                Err(error)=>{
                    let patch=if side==Side::Staged {index::diff(&snapshot,&path).ok().map(|doc|doc.patch).unwrap_or_default()} else {String::new()};
                    Err(format!("부분 선택을 사용할 수 없습니다. {error:#}\n{patch}"))
                }
            }
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if !preview_is_current(&app.commit_ui, &root, token, generation)
                    || app.current.as_ref() != Some(&root)
                {
                    return;
                }
                match result {
                    Ok(preview)=>app.commit_ui.partial=Some(Arc::new(preview)),
                    Err(text)=>app.commit_ui.preview=Some(commit_editor(&text,true,window,cx)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Small completion hook in the shared runner, after its original-root
    /// refresh. This is view refresh only, never a second job/busy lifecycle.
    pub(super) fn commit_job_finished(
        &mut self,
        root: &Path,
        success: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_ui.clear_message_on_success.as_deref() == Some(root) {
            self.commit_ui.clear_message_on_success = None;
            if success && let Some(message) = self.commit_ui.messages.get(root) {
                message.update(cx, |editor, cx| editor.set_value("", window, cx));
            }
        }
        if self.commit_ui.open
            && self.commit_ui.root.as_deref() == Some(root)
            && self.current.as_deref() == Some(root)
        {
            self.read_commit_index(window, cx);
        }
    }

    fn change_commit_index(
        &mut self,
        stage: bool,
        scope: StageScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_disabled() {
            return;
        }
        let Some(snapshot) = self.commit_ui.snapshot.clone() else {
            return;
        };
        self.commit_ui.request += 1;
        self.commit_ui.preview_request += 1;
        self.run_git_job(
            snapshot.root.clone(),
            if stage { "Stage" } else { "Unstage" }.into(),
            move |operations| {
                if stage {
                    operations.stage(&snapshot, &scope)
                } else {
                    operations.unstage(&snapshot, &scope)
                }
            },
            window,
            cx,
        );
    }

    fn close_commit_view(&mut self, cx: &mut Context<Self>) {
        self.commit_ui.open = false;
        self.commit_ui.request += 1;
        self.commit_ui.preview_request += 1;
        cx.notify();
    }

    fn commit_disabled(&self) -> bool {
        self.git_is_busy()
            || self.loading
            || self.opening_path.is_some()
            || self.commit_ui.reading
            || !self.commit_ui.open
            || self.commit_ui.root != self.current
            || self.commit_ui.snapshot.is_none()
    }

    fn submit_index_commit(
        &mut self,
        mode: CommitMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_disabled() {
            return;
        }
        let Some(snapshot) = self.commit_ui.snapshot.clone() else {
            return;
        };
        let Some(editor) = self.commit_ui.messages.get(&snapshot.root) else {
            return;
        };
        let message = editor.read(cx).value().to_string();
        if message.trim().is_empty() {
            self.commit_ui.error = Some("커밋 메시지를 입력하세요.".into());
            editor.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        self.commit_ui.request += 1;
        self.commit_ui.preview_request += 1;
        self.commit_ui.clear_message_on_success = Some(snapshot.root.clone());
        let label = if matches!(mode, CommitMode::New) {
            "커밋"
        } else {
            "이전 커밋 수정"
        };
        self.run_git_job(
            snapshot.root.clone(),
            label.into(),
            move |operations| operations.commit_index(&snapshot, &message, &mode),
            window,
            cx,
        );
    }

    fn confirm_index_amend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_disabled() {
            return;
        }
        let Some(snapshot) = self.commit_ui.snapshot.clone() else {
            return;
        };
        let Some(previous) = snapshot.previous.clone() else {
            return;
        };
        let Some(message) = self.commit_ui.messages.get(&snapshot.root).cloned() else {
            return;
        };
        if message.read(cx).value().trim().is_empty() {
            message.update(cx, |editor, cx| {
                editor.set_value(previous.message.clone(), window, cx)
            });
        }
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let confirmed = previous.oid.clone();
            let expected = snapshot.clone();
            let disabled = weak.upgrade().is_none_or(|app| app.read(cx).commit_disabled());
            dialog.title("이전 커밋 수정 확인").width(px(660.)).margin_top(px(24.)).overlay_closable(false).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_2()
                    .child(commit_note(format!("작업 대상: {}", snapshot.root.display())))
                    .child(commit_note(format!("{}\n{} <{}>\n{}", previous.oid, previous.author, previous.email, previous.date)))
                    .child(div().id("amend-old-message").max_h(px(80.)).overflow_y_scroll().child(previous.message.clone()))
                    .child(commit_note(format!("현재 index의 {}개 변경과 아래 메시지로 이 커밋을 교체합니다. 이미 공유한 커밋을 수정하면 다른 사람의 이력과 달라집니다.", snapshot.staged.len())))
                    .child(Editor::new(&message).aria_label("수정할 커밋 메시지").h(px(82.)).font_family("IBM Plex Sans").text_size(px(13.)))
                    .child(commit_note("작업 파일은 자동 stage하지 않습니다. 기존 작성자와 Git hooks를 사용합니다.")))
                .footer(div().flex().justify_end().gap_2()
                    .child(Button::new("amend-cancel").label("취소").small().on_click(|_,w,cx|w.close_dialog(cx)))
                    .child(commit_button(&weak,"amend-confirm","이 커밋 수정",disabled,move |app,w,cx| {
                        // Confirmation binds the displayed prior commit and index;
                        // a refresh while this sheet is open must not retarget it.
                        if app.commit_ui.snapshot.as_ref().is_none_or(|current| current.guard != expected.guard) {
                            app.commit_ui.error = Some("인덱스가 바뀌었습니다. 수정 내용을 다시 확인하세요.".into());
                            w.close_dialog(cx); cx.notify(); return;
                        }
                        w.close_dialog(cx);
                        app.submit_index_commit(CommitMode::Amend { confirmed_head: confirmed.clone() },w,cx);
                    }).danger()))
        });
    }

    fn show_commit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let close = weak.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let Some(view) = weak.upgrade() else { return dialog; };
            let app = view.read(cx);
            let disabled = app.commit_disabled();
            let snapshot = app.commit_ui.snapshot.clone();
            let root = app.commit_ui.root.as_deref().unwrap_or(Path::new(""));
            let branch = snapshot.as_ref().and_then(|snapshot| snapshot.branch.as_deref())
                .map(|name| name.strip_prefix("refs/heads/").unwrap_or(name)).unwrap_or("detached HEAD");
            let mut body = div().v_flex().gap_2().key_context("CommitDialog")
                .on_action({ let weak = weak.clone(); move |_: &SubmitCommit, w, cx| {
                    let _ = weak.update(cx, |app,cx|app.submit_index_commit(CommitMode::New,w,cx));
                }})
                .on_action(|_: &FindDefinition, _, cx| cx.stop_propagation())
                .on_action(|_: &FindReferences, _, cx| cx.stop_propagation())
                .child(div().text_size(px(12.)).text_ellipsis().child(format!("{}  ·  {}", branch, root.display())));
            let mut lists = div().v_flex().w(px(310.)).flex_shrink_0().gap_1();
            for stage in [false, true] {
                let files = snapshot.as_ref().map(|s| if stage { &s.unstaged } else { &s.staged });
                let count = files.map_or(0, Vec::len);
                lists = lists.child(div().flex().items_center().justify_between()
                    .child(div().text_size(px(12.)).child(format!("{} ({count})", if stage { "작업 파일" } else { "Staged · 커밋에 포함" })))
                    .child(commit_button(&weak,if stage { "stage-all" } else { "unstage-all" },if stage { "모두 stage" } else { "모두 unstage" },disabled || count==0,move |app,w,cx|app.change_commit_index(stage,StageScope::All,w,cx))));
                let mut rows = div().id(if stage { "unstaged-files" } else { "staged-files" }).h(px(84.)).overflow_y_scroll().v_flex();
                if let Some(files) = files {
                    for (ix,file) in files.iter().enumerate() {
                        let path = file.path.clone();
                        let paths: Vec<PathBuf> = file.old_path.iter().cloned().chain(std::iter::once(file.path.clone())).collect();
                        let status = match file.status { FileStatus::Added|FileStatus::Untracked=>"A",FileStatus::Deleted=>"D",FileStatus::Renamed=>"R",FileStatus::Conflicted=>"U",_=>"M" };
                        // Debug spelling escapes newlines and invalid UTF-8. The
                        // original PathBuf, never this label, reaches Git argv.
                        let label = format!("{status} {}", display_path(&file.path));
                        let tip = file.old_path.as_ref().map(|old|format!("{} → {}",display_path(old),display_path(&file.path))).unwrap_or_else(||label.clone());
                        let side=if stage {Side::Unstaged}else{Side::Staged};
                        let selected = app.commit_ui.selected.as_ref()==Some(&file.path) && app.commit_ui.side==Some(side);
                        rows = rows.child(div().flex().items_center().gap_1()
                            .child(commit_button(&weak,(if stage {"work-file"}else{"index-file"},ix),label,disabled,move |app,w,cx|app.select_index_preview(path.clone(),side,w,cx))
                                .selected(selected).tooltip(tip).flex_1().min_w_0().justify_start().overflow_hidden())
                            .child(commit_button(&weak,(if stage {"stage-file"}else{"unstage-file"},ix),if stage {"Stage"}else{"Unstage"},disabled,move |app,w,cx|app.change_commit_index(stage,StageScope::Files(paths.clone()),w,cx))));
                    }
                }
                if count==0 { rows=rows.child(commit_note(if app.commit_ui.reading {"읽는 중…"}else if stage {"작업 파일 변경이 없습니다."}else{"Staged 변경이 없습니다."})); }
                lists=lists.child(rows);
            }
            let mut preview = div().v_flex().flex_1().min_w_0().gap_1()
                .child(div().flex().items_center().justify_between()
                    .child(div().text_size(px(12.)).child(if app.commit_ui.side == Some(Side::Unstaged) {"Index → 작업 파일"} else {"HEAD → Index"}))
                    .child(commit_button(&weak,"index-split",if app.commit_ui.split.is_some() {"줄 선택"} else {"좌우"},disabled || app.commit_ui.partial.is_none(),|app,_,cx|app.toggle_partial_split(cx)))
                    .child(commit_button(&weak,"apply-selected-lines",format!("선택 {}줄 {}",app.commit_ui.marked.len(),if app.commit_ui.side==Some(Side::Unstaged) {"Stage"} else {"Unstage"}),disabled || app.commit_ui.marked.is_empty() || app.commit_ui.partial.is_none(),|app,w,cx|app.apply_line_selection(w,cx)))
                    .child(commit_button(&weak,"index-refresh","새로고침",app.git_is_busy() || app.commit_ui.reading,|app,w,cx|app.read_commit_index(w,cx))));
            preview = if let Some(split)=&app.commit_ui.split {
                preview.child(div().h(px(204.)).min_w_0().child(split.clone()))
            } else if let Some(partial)=&app.commit_ui.partial {
                preview.child(app.partial_rows(partial.clone(),weak.clone(),Palette::new(cx)))
            } else if let Some(editor)=&app.commit_ui.preview {
                preview.child(Editor::new(editor).aria_label("실제 Git index diff 미리보기").readonly(true).h(px(204.)).font_family("JetBrains Mono").text_size(px(12.)))
            } else { preview.child(div().h(px(204.)).p_2().child(commit_note(if app.commit_ui.selected.is_some(){"파일 변경을 읽는 중…"}else{"파일을 선택하면 변경 내용을 표시합니다."}))) };
            body=body.child(div().flex().gap_3().child(lists).child(preview));
            if let Some(message)=app.commit_ui.messages.get(root) {
                body=body.child(div().text_size(px(12.)).child("커밋 메시지"))
                    .child(Editor::new(message).aria_label("커밋 메시지").disabled(app.git_is_busy()).h(px(64.)).font_family("IBM Plex Sans").text_size(px(13.)));
            }
            let status = app.commit_ui.error.as_deref().or(app.error.as_deref());
            body=body.child(div().id("commit-feedback").h(px(34.)).overflow_y_scroll().text_size(px(11.))
                .when_some(status, |div,error|div.text_color(Palette::new(cx).rgb(RED)).child(error.to_owned()))
                .when(status.is_none(), |div|div.child(if app.git_is_busy(){app.status.clone()}else if snapshot.as_ref().is_some_and(|s|s.has_conflicts){"미해결 충돌이 있습니다. Stash / 충돌 해결에서 먼저 해결하세요.".into()}else{"Index만 커밋합니다. 파일 자동 stage나 push는 실행하지 않습니다.  ⌘↵ 커밋".into()})));
            let conflicts=snapshot.as_ref().is_some_and(|s|s.has_conflicts);
            dialog.title("Stage / 커밋").width(px(860.)).margin_top(px(12.)).overlay_closable(false).on_ok(|_,_,_|false)
                .on_close({let close=close.clone();move |_,_,cx|{let _=close.update(cx,|app,cx|{app.close_commit_view(cx);});}})
                .child(body)
                .footer(div().flex().justify_end().gap_2()
                    .child(commit_button(&weak,"commit-close","닫기",false,|app,w,cx|{app.close_commit_view(cx);w.close_dialog(cx);}))
                    .child(commit_button(&weak,"commit-amend","이전 커밋 수정…",disabled || conflicts || snapshot.as_ref().is_none_or(|s|s.previous.is_none()),|app,w,cx|app.confirm_index_amend(w,cx)))
                    .child(commit_button(&weak,"commit-create","Staged 커밋",disabled || conflicts || snapshot.as_ref().is_none_or(|s|s.staged.is_empty()),|app,w,cx|app.submit_index_commit(CommitMode::New,w,cx)).primary()))
        });
    }
}

fn preview_is_current(ui: &CommitUi, root: &Path, token: u64, generation: u64) -> bool {
    ui.open
        && ui.root.as_deref() == Some(root)
        && ui.preview_request == token
        && ui.request == generation
}
fn display_path(path: &Path) -> String {
    match path
        .to_str()
        .filter(|text| !text.chars().any(char::is_control))
    {
        Some(text) => text.into(),
        None => format!("{:?}", path.as_os_str()),
    }
}
fn commit_button(
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
fn commit_note(text: impl Into<String>) -> Div {
    div().text_size(px(12.)).child(text.into())
}
fn commit_editor(
    text: &str,
    readonly: bool,
    window: &mut Window,
    cx: &mut Context<Pokarak>,
) -> Entity<EditorState> {
    let editor = cx.new(|cx| {
        EditorState::new(window, cx)
            .language(if readonly { "diff" } else { "plaintext" })
            .line_number(false)
            .soft_wrap(!readonly)
            .default_value(text.to_owned())
    });
    editor.update(cx, |editor, cx| editor.set_readonly(readonly, cx));
    editor
}

#[cfg(test)]
mod tests {
    use super::{CommitUi, preview_is_current};
    use std::path::Path;
    #[test]
    fn index_preview_rejects_stale_root_request_refresh_and_close() {
        let mut ui = CommitUi {
            open: true,
            root: Some("/repo/a".into()),
            request: 4,
            preview_request: 7,
            ..Default::default()
        };
        assert!(preview_is_current(&ui, Path::new("/repo/a"), 7, 4));
        assert!(!preview_is_current(&ui, Path::new("/repo/b"), 7, 4));
        assert!(!preview_is_current(&ui, Path::new("/repo/a"), 6, 4));
        assert!(!preview_is_current(&ui, Path::new("/repo/a"), 7, 3));
        ui.open = false;
        assert!(!preview_is_current(&ui, Path::new("/repo/a"), 7, 4));
    }
}
