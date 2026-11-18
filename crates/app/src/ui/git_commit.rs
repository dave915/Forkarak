//! Worktree-scoped index reader and commit form, shared by graph and dialog.
//! Every write is owned by the existing git_actions runner and cancellation.
mod panel;
mod partial;
mod selection;
use super::*;
use pokarak_core::index::partial::{self as partial_index, PartialPreview, Side};
use pokarak_core::{
    index::{self, IndexSnapshot},
    operations::commit_actions::{CommitMode, StageScope},
};

actions!(pokarak_commit, [SubmitCommit]);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Surface {
    #[default]
    Closed,
    Dialog,
    Inline,
}

#[derive(Default)]
pub(super) struct CommitUi {
    surface: Surface,
    navigation: u64,
    root: Option<PathBuf>,
    request: u64,
    preview_request: u64,
    reading: bool,
    snapshot: Option<Arc<IndexSnapshot>>,
    selected: Option<PathBuf>,
    preview: Option<Entity<EditorState>>,
    side: Option<Side>,
    partial: Option<partial::PartialDiff>,
    split: Option<Entity<split_diff::SplitDiff>>,
    marked: std::collections::BTreeSet<usize>,
    files: selection::FileSelection,
    messages: HashMap<PathBuf, Entity<EditorState>>,
    error: Option<String>,
    submitted_message: Option<(PathBuf, String)>,
    progress: Option<pokarak_core::operations::workflow::ProgressKind>,
}

fn commit_message_editor(
    text: String,
    window: &mut Window,
    cx: &mut Context<Pokarak>,
) -> Entity<EditorState> {
    cx.new(|cx| {
        EditorState::new(window, cx)
            .language("plaintext")
            .line_number(false)
            .soft_wrap(true)
            .default_value(text)
    })
}

impl CommitUi {
    pub(super) fn draft_messages(&self, cx: &App) -> HashMap<PathBuf, String> {
        self.messages
            .iter()
            .filter_map(|(root, editor)| {
                let message = editor.read(cx).value();
                (!message.is_empty()).then(|| (root.clone(), message.to_string()))
            })
            .collect()
    }

    pub(super) fn restore_messages(
        &mut self,
        messages: HashMap<PathBuf, String>,
        window: &mut Window,
        cx: &mut Context<Pokarak>,
    ) {
        self.messages.extend(
            messages
                .into_iter()
                .map(|(root, text)| (root, commit_message_editor(text, window, cx))),
        );
    }

    fn next_selection(
        &self,
        staged: &[ChangedFile],
        unstaged: &[ChangedFile],
    ) -> Option<(PathBuf, Side)> {
        let prefer = self.side.unwrap_or(if self.surface == Surface::Inline {
            Side::Unstaged
        } else {
            Side::Staged
        });
        let lists = match prefer {
            Side::Staged => [(staged, Side::Staged), (unstaged, Side::Unstaged)],
            Side::Unstaged => [(unstaged, Side::Unstaged), (staged, Side::Staged)],
        };
        if let Some(path) = &self.selected {
            for (files, side) in lists {
                if files.iter().any(|file| &file.path == path) {
                    return Some((path.clone(), side));
                }
            }
        }
        lists
            .into_iter()
            .find_map(|(files, side)| files.first().map(|file| (file.path.clone(), side)))
    }

    fn clear_preview(&mut self) {
        self.preview = None;
        self.partial = None;
        self.split = None;
        self.marked.clear();
    }

    fn close(&mut self) {
        self.request += 1;
        self.preview_request += 1;
        self.surface = Surface::Closed;
        self.root = None;
        self.reading = false;
        self.snapshot = None;
        self.selected = None;
        self.side = None;
        self.files.clear();
        self.error = None;
        self.clear_preview();
    }
}

impl Pokarak {
    pub(super) fn commit_defers_auto_refresh(&self, window: &Window, cx: &App) -> bool {
        self.commit_ui.reading
            || !self.commit_ui.marked.is_empty()
            || self.commit_ui.files.defers_refresh()
            || (self.inline_commit_active()
                && self
                    .current
                    .as_ref()
                    .and_then(|root| self.commit_ui.messages.get(root))
                    .is_some_and(|editor| {
                        let editor = editor.read(cx);
                        !editor.value().is_empty() && editor.focus_handle(cx).is_focused(window)
                    }))
    }

    pub(super) fn inline_commit_active(&self) -> bool {
        self.commit_ui.surface == Surface::Inline && self.commit_ui.root == self.current
    }

    pub(super) fn leave_inline_commit(&mut self) {
        if self.commit_ui.surface == Surface::Inline {
            self.commit_ui.close();
        }
    }

    pub(super) fn refresh_inline_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inline_commit_active() {
            self.read_commit_index(window, cx);
        }
    }

    pub(super) fn open_inline_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inline_commit_active() {
            self.open_commit_surface(Surface::Inline, window, cx);
        }
    }

    pub(super) fn open_git_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inline_commit_active() {
            if let Some(editor) = self
                .current
                .as_ref()
                .and_then(|root| self.commit_ui.messages.get(root))
            {
                editor.read(cx).focus_handle(cx).focus(window, cx);
            }
            return;
        }
        self.open_commit_surface(Surface::Dialog, window, cx);
    }

    fn open_commit_surface(
        &mut self,
        surface: Surface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() || self.loading || self.opening_path.is_some() {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        window.close_all_dialogs(cx);
        self.commit_ui.close();
        if !self.commit_ui.messages.contains_key(&root) {
            let editor = commit_message_editor(String::new(), window, cx);
            self.commit_ui.messages.insert(root.clone(), editor);
        }
        self.commit_ui.surface = surface;
        self.commit_ui.root = Some(root);
        self.read_commit_index(window, cx);
        if surface == Surface::Dialog {
            self.show_commit_dialog(window, cx);
        }
    }

    fn read_commit_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.commit_ui.root.clone() else {
            return;
        };
        if self.git_is_busy()
            || self.current.as_ref() != Some(&root)
            || (self.commit_ui.reading && self.commit_ui.navigation == self.request)
        {
            return;
        }
        self.commit_ui.request += 1;
        self.commit_ui.preview_request += 1;
        self.commit_ui.reading = true;
        self.commit_ui.files.clear();
        self.commit_ui.navigation = self.request;
        self.commit_ui.error = None;
        let token = self.commit_ui.request;
        let navigation = self.request;
        let target = root.clone();
        let task = cx.background_spawn(async move {
            let snapshot = index::snapshot(&target)?;
            let progress = pokarak_core::operations::GitOperations::open(&target)?
                .in_progress()?
                .map(|p| p.kind);
            anyhow::Ok((snapshot, progress))
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if app.commit_ui.request != token {
                    return;
                }
                app.commit_ui.reading = false;
                if app.commit_ui.surface == Surface::Closed
                    || app.commit_ui.root.as_ref() != Some(&root)
                    || app.current.as_ref() != Some(&root)
                    || app.request != navigation
                    || app.git_is_busy()
                {
                    app.commit_ui.snapshot = None;
                    app.commit_ui.clear_preview();
                    app.commit_ui.error =
                        Some("작업 대상이 바뀌었습니다. 커밋 화면을 다시 여세요.".into());
                    cx.notify();
                    return;
                }
                match result {
                    Ok((snapshot, progress)) => {
                        app.commit_ui.progress = progress;
                        let selected = app
                            .commit_ui
                            .next_selection(&snapshot.staged, &snapshot.unstaged);
                        app.commit_ui.snapshot = Some(Arc::new(snapshot));
                        app.commit_ui.selected = None;
                        app.commit_ui.side = None;
                        app.commit_ui.clear_preview();
                        if let Some((path, side)) = selected {
                            app.select_index_preview(path, side, window, cx);
                        }
                    }
                    Err(error) => {
                        app.commit_ui.snapshot = None;
                        app.commit_ui.clear_preview();
                        app.commit_ui.error = Some(format!("인덱스 읽기 실패: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_index_preview(
        &mut self,
        path: PathBuf,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.commit_ui.snapshot.clone() else {
            return;
        };
        self.commit_ui.preview_request += 1;
        let token = self.commit_ui.preview_request;
        let generation = self.commit_ui.request;
        self.commit_ui.selected = Some(path.clone());
        self.commit_ui.side = Some(side);
        self.commit_ui.clear_preview();
        let root = snapshot.root.clone();
        let task = cx.background_spawn(async move {
            match partial_index::preview(&snapshot, &path, side) {
                Ok(preview) => Ok(partial::PartialDiff::new(&path, preview)),
                Err(error) => {
                    let mut message = format!("부분 선택을 사용할 수 없습니다. {error:#}\n");
                    let source = if side == Side::Staged {
                        index::diff(&snapshot, &path).ok().map(|doc| {
                            message.push_str(&doc.patch);
                            Arc::new(split_diff::DiffSyntax::new(&doc))
                        })
                    } else {
                        None
                    };
                    Err(patch::Prepared::new(message, source))
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
                    Ok(preview) => app.commit_ui.partial = Some(preview),
                    Err(patch) => {
                        app.commit_ui.preview = Some(patch::editor(patch, false, window, cx))
                    }
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
        if let Some((_, submitted)) = self
            .commit_ui
            .submitted_message
            .take_if(|(owner, _)| owner == root)
            && success
            && let Some(editor) = self.commit_ui.messages.get(root)
            && editor.read(cx).value().as_ref() == submitted.as_str()
        {
            editor.update(cx, |editor, cx| editor.set_value("", window, cx));
        }
        if self.commit_ui.surface != Surface::Closed
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
        self.commit_ui.files.clear();
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

    fn select_commit_file(
        &mut self,
        snapshot: &Arc<IndexSnapshot>,
        path: PathBuf,
        side: Side,
        gesture: selection::Gesture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.commit_snapshot_current(snapshot) {
            return;
        }
        self.commit_ui.files.select(
            &snapshot.root,
            self.commit_ui.request,
            side,
            selection::side_files(snapshot, side),
            &path,
            gesture,
        );
        self.select_index_preview(path, side, window, cx);
    }

    fn commit_snapshot_current(&self, snapshot: &Arc<IndexSnapshot>) -> bool {
        !self.commit_disabled()
            && self
                .commit_ui
                .snapshot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, snapshot))
            && self.current.as_ref() == Some(&snapshot.root)
    }

    fn change_selected_commit_files(
        &mut self,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.commit_ui.snapshot.as_ref() else {
            return;
        };
        let paths = self
            .commit_ui
            .files
            .paths(snapshot, self.commit_ui.request, side);
        if paths.is_empty() {
            return;
        }
        self.change_commit_index(side == Side::Unstaged, StageScope::Files(paths), window, cx);
    }

    fn close_commit_view(&mut self, cx: &mut Context<Self>) {
        if self.commit_ui.surface == Surface::Dialog {
            self.commit_ui.close();
        }
        cx.notify();
    }

    fn commit_disabled(&self) -> bool {
        self.git_is_busy()
            || self.loading
            || self.opening_path.is_some()
            || self.commit_ui.reading
            || self.commit_ui.surface == Surface::Closed
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
        if self.commit_ui.progress.is_some() {
            self.commit_ui.error = Some("진행 중인 Git 작업은 작업 상태의 ‘계속’ 또는 ‘중단’을 사용하세요. 일반 커밋과 이전 커밋 수정은 사용할 수 없습니다.".into());
            cx.notify();
            return;
        }
        let Some(snapshot) = self.commit_ui.snapshot.clone() else {
            return;
        };
        if snapshot.has_conflicts || (matches!(mode, CommitMode::New) && snapshot.staged.is_empty())
        {
            return;
        }
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
        self.commit_ui.submitted_message = Some((snapshot.root.clone(), message.clone()));
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
        use crate::ui::dialogs::DialogWindowExt as _;
        if self.commit_disabled() {
            return;
        }
        if self.commit_ui.progress.is_some() {
            self.commit_ui.error = Some("진행 중인 Git 작업은 작업 상태의 ‘계속’ 또는 ‘중단’을 사용하세요. 일반 커밋과 이전 커밋 수정은 사용할 수 없습니다.".into());
            cx.notify();
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
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let confirmed = previous.oid.clone();
            let expected = snapshot.clone();
            let disabled = weak.upgrade().is_none_or(|app| app.read(cx).commit_disabled());
            dialog.title("이전 커밋 수정 확인").width(ui_px(660.)).margin_top(ui_px(24.)).overlay_closable(false).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_2()
                    .child(commit_note(format!("작업 대상: {}", snapshot.root.display())))
                    .child(commit_note(format!("{}\n{} <{}>\n{}", previous.oid, previous.author, previous.email, previous.date)))
                    .child(div().id("amend-old-message").max_h(ui_px(80.)).overflow_y_scroll().child(previous.message.clone()))
                    .child(commit_note(format!("현재 index의 {}개 변경과 아래 메시지로 이 커밋을 교체합니다. 이미 공유한 커밋을 수정하면 다른 사람의 이력과 달라집니다.", snapshot.staged.len())))
                    .child(Editor::new(&message).aria_label("수정할 커밋 메시지").h(ui_px(82.)).font_family("IBM Plex Sans").text_size(ui_text(13.)))
                    .child(commit_note("작업 파일은 자동 stage하지 않습니다. 기존 작성자와 Git hooks를 사용합니다.")))
                .footer(div().flex().justify_end().gap_2()
                    .child(theme::action_button("amend-cancel", false).label("취소").small().on_click(|_,w,cx|w.close_dialog(cx)))
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
        use crate::ui::dialogs::DialogWindowExt as _;
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let Some(view) = weak.upgrade() else {
                return dialog;
            };
            let app = view.read(cx);
            let close = weak.clone();
            dialog
                .title("Stage / 커밋")
                .width(ui_px(860.))
                .margin_top(ui_px(12.))
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |app, cx| app.close_commit_view(cx));
                })
                .child(app.commit_panel(&weak, false, cx))
        });
    }
}

fn preview_is_current(ui: &CommitUi, root: &Path, token: u64, generation: u64) -> bool {
    ui.surface != Surface::Closed
        && ui.root.as_deref() == Some(root)
        && ui.preview_request == token
        && ui.request == generation
}
pub(in crate::ui) fn display_path(path: &Path) -> String {
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
    theme::action_button(id, disabled)
        .label(label)
        .small()
        .on_click(move |_, w, cx| {
            let _ = target.update(cx, |app, cx| action(app, w, cx));
        })
}
fn commit_note(text: impl Into<String>) -> Div {
    div().text_size(ui_text(12.)).child(text.into())
}
#[cfg(test)]
mod tests {
    use super::{ChangedFile, CommitUi, Side, Surface, preview_is_current};
    use std::path::Path;
    #[test]
    fn index_preview_rejects_stale_root_request_refresh_and_close() {
        let mut ui = CommitUi {
            surface: Surface::Dialog,
            root: Some("/repo/a".into()),
            request: 4,
            preview_request: 7,
            ..Default::default()
        };
        assert!(preview_is_current(&ui, Path::new("/repo/a"), 7, 4));
        assert!(!preview_is_current(&ui, Path::new("/repo/b"), 7, 4));
        assert!(!preview_is_current(&ui, Path::new("/repo/a"), 6, 4));
        assert!(!preview_is_current(&ui, Path::new("/repo/a"), 7, 3));
        ui.close();
        assert!(!preview_is_current(&ui, Path::new("/repo/a"), 7, 4));
    }

    #[test]
    fn inline_selects_working_changes_and_follows_the_file_across_index_writes() {
        let file = |name: &str| ChangedFile {
            path: name.into(),
            ..Default::default()
        };
        let mut ui = CommitUi {
            surface: Surface::Inline,
            ..Default::default()
        };
        let staged = [file("other"), file("edited")];
        let working = [file("edited")];
        assert_eq!(
            ui.next_selection(&staged, &working),
            Some(("edited".into(), Side::Unstaged))
        );
        ui.selected = Some("edited".into());
        ui.side = Some(Side::Unstaged);
        assert_eq!(
            ui.next_selection(&staged, &[]),
            Some(("edited".into(), Side::Staged))
        );
        ui.close();
        ui.surface = Surface::Dialog;
        assert_eq!(
            ui.next_selection(&staged, &working),
            Some(("other".into(), Side::Staged))
        );
    }

    #[test]
    fn leaving_inline_discards_late_reads_and_selection_on_same_root_reentry() {
        let root = Path::new("/repo/w");
        let mut ui = CommitUi {
            surface: Surface::Inline,
            root: Some(root.into()),
            request: 10,
            preview_request: 8,
            ..Default::default()
        };
        ui.marked.insert(3);
        ui.close();
        ui.root = Some(root.into());
        ui.surface = Surface::Inline;
        assert!(ui.marked.is_empty());
        assert!(!preview_is_current(&ui, root, 8, 10));
    }
}
