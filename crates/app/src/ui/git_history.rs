//! Dedicated immutable history view. Its editors never enter working-tree
//! documents/navigation, and writes use the existing Git job lifecycle.
mod columns;
mod context_actions;
mod home;
mod lanes;
mod navigation;
mod rows;
use super::*;
use context_actions::commit_context_menu;
use lanes::{BranchTrace, graph_view, reference_name, track_color};
use pokarak_core::history::{self, Commit, CommitDetail, GraphRow, HistoryCursor, HistoryEntry};
use pokarak_core::operations::history_actions::{HistoryAction, HistoryTarget};

#[derive(Default)]
pub(super) struct HistoryUi {
    pub(super) active: bool,
    nav: navigation::NavigationUi,
    root: Option<PathBuf>,
    request: u64,
    loading: bool,
    entries: Arc<Vec<HistoryEntry>>,
    hovered_tip: Option<String>,
    hover_anchor: Option<String>,
    hover_lookup: Option<Task<()>>,
    in_neighborhood: bool,
    cursor: Option<HistoryCursor>,
    limit_reached: bool,
    shallow: bool,
    detail: Option<CommitDetail>,
    selected: Option<PathBuf>,
    document: Option<ReviewDiff>,
    editor: Option<Entity<EditorState>>,
    code: bool,
    scroll: UniformListScrollHandle,
    files_scroll: UniformListScrollHandle,
    table_scroll: ScrollHandle,
    table_width: Option<Pixels>,
    column_drag: Option<columns::ColumnDrag>,
    error: Option<String>,
}

impl HistoryUi {
    fn reset(&mut self, active: bool) {
        let request = self.request + 1;
        *self = Self {
            active,
            request,
            ..Self::default()
        };
    }

    fn accepts_read(&self, request: u64, root: &Path) -> bool {
        self.active && self.request == request && self.root.as_deref() == Some(root)
    }
}
impl Pokarak {
    pub(super) fn graph_page_loaded(&self) -> bool {
        self.git_history.nav.catalog.is_some()
    }

    fn history_read<T: Send + 'static>(
        &mut self,
        job: impl FnOnce(&Path) -> anyhow::Result<T> + Send + 'static,
        ready: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.git_history.root.clone() else {
            return;
        };
        if self.active_worktree_has_git_job()
            || self.current.as_ref() != Some(&root)
            || self.loading
        {
            return;
        }
        self.git_history.request += 1;
        let token = self.git_history.request;
        let navigation_generation = self.request;
        self.git_history.loading = true;
        self.git_history.error = None;
        let worktree = root.clone();
        let task = cx.background_spawn(async move { job(&worktree) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if app.git_history.request != token {
                    return;
                }
                app.git_history.loading = false;
                if !app.git_history.accepts_read(token, &root)
                    || app.current.as_ref() != Some(&root)
                    || app.request != navigation_generation
                    || app.active_worktree_has_git_job()
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(data) => ready(app, data, window, cx),
                    Err(error) => app.git_history.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn open_git_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_worktree_has_git_job() || self.opening_path.is_some() {
            return;
        }
        if self.loading {
            if self.git_history.active {
                return;
            }
            // Returning to the graph supersedes an in-flight code snapshot.
            self.request += 1;
            self.loading = false;
            self.checking_changes = false;
            self.count_pending = false;
            self.status = "준비됨".into();
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        self.navigation_request += 1;
        self.navigating = false;
        self.git_history.request += 1;
        self.git_history.root = Some(root);
        self.git_history.entries = Arc::default();
        self.clear_graph_hover();
        self.git_history.in_neighborhood = false;
        self.git_history.nav.catalog = None;
        self.git_history.cursor = None;
        self.git_history.detail = None;
        self.git_history.selected = None;
        self.git_history.document = None;
        self.git_history.editor = None;
        self.git_history.scroll = UniformListScrollHandle::new();
        self.git_history.files_scroll = UniformListScrollHandle::new();
        self.git_history.error = None;
        self.git_history.limit_reached = false;
        self.git_history.shallow = false;
        self.show_history_list(window, cx);
        self.load_history_page(true, window, cx);
    }
    fn load_history_page(&mut self, first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.git_history.cursor.clone();
        self.history_read(
            move |root| {
                if first {
                    history::navigation::open(root, 100)
                        .map(|(page, catalog)| (page, Some(catalog)))
                } else {
                    history::next_page(
                        root,
                        cursor
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("No further page"))?,
                        100,
                    )
                    .map(|page| (page, None))
                }
            },
            move |app, (page, catalog), window, cx| {
                if let Some(catalog) = catalog {
                    app.git_history.nav.catalog = Some(Arc::new(catalog));
                }
                Arc::make_mut(&mut app.git_history.entries).extend(page.entries);
                app.refresh_graph_hover(cx);
                app.git_history.cursor = page.next;
                app.git_history.limit_reached = page.limit_reached;
                app.git_history.shallow = page.shallow;
                if first && app.graph_fingerprint.is_none() {
                    app.check_graph_changes(cx);
                }
                if first && !app.inline_commit_active() {
                    let head = app.active_worktree().map(|tree| tree.head.as_str());
                    let selected = app
                        .git_history
                        .entries
                        .iter()
                        .find(|entry| Some(entry.commit.oid.as_str()) == head)
                        .or_else(|| app.git_history.entries.first())
                        .map(|entry| entry.commit.oid.clone());
                    if let Some(oid) = selected {
                        app.load_history_detail(oid, 0, window, cx);
                    }
                }
            },
            window,
            cx,
        );
    }
    fn show_history_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_history.active = true;
        self.results_open = false;
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn load_history_detail(
        &mut self,
        oid: String,
        parent: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_is_busy() || self.loading {
            return;
        }
        self.git_history.detail = None;
        self.git_history.document = None;
        self.git_history.selected = None;
        self.git_history.editor = None;
        self.history_read(
            move |root| history::commit_detail(root, &oid, parent),
            |app, detail, w, cx| {
                app.git_history.detail = Some(detail);
                app.git_history.selected = None;
                app.git_history.document = None;
                app.git_history.editor = None;
                app.git_history.code = false;
                app.show_history_detail(w, cx);
                if let Some(file) = app
                    .git_history
                    .detail
                    .as_ref()
                    .and_then(|d| d.files.first())
                    .cloned()
                {
                    app.load_history_file(file.path, w, cx);
                }
            },
            window,
            cx,
        );
    }
    fn load_history_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.git_history.detail.clone() else {
            return;
        };
        self.git_history.selected = Some(path.clone());
        self.git_history.document = None;
        self.git_history.editor = None;
        self.history_read(
            move |root| history::historical_diff(root, &detail, &path),
            |app, doc, w, cx| {
                app.git_history.document = Some(doc.into());
                app.update_history_editor(w, cx);
            },
            window,
            cx,
        );
    }
    fn update_history_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = &self.git_history.document else {
            return;
        };
        let text = if self.git_history.code && !doc.binary {
            doc.new_text.clone()
        } else {
            doc.patch.clone()
        };
        let lang = if self.git_history.code {
            language(&doc.path)
        } else {
            "diff"
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(lang)
                .line_number(true)
                .soft_wrap(false)
                .default_value(text)
        });
        editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
        self.git_history.editor = Some(editor);
        cx.notify();
    }
    fn show_history_detail(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.git_history.active = true;
        cx.notify();
    }
    fn confirm_history_action(
        &mut self,
        action: HistoryAction,
        commit: Commit,
        target: HistoryTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_all_dialogs(cx);
        self.git_history.active = true;
        let weak = cx.entity().downgrade();
        window.open_dialog(cx,move |dialog,_,cx| {
            let busy=weak.upgrade().is_none_or(|v|history_disabled(v.read(cx)));
            let destination=target.clone(); let source=commit.oid.clone();
            dialog.title(format!("{} 확인",action.label())).width(px(650.)).margin_top(px(20.)).overlay_closable(false).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_3().text_size(px(13.))
                    .child(format!("원본 커밋: {}\n{}",commit.oid,visible(&commit.subject)))
                    .child(format!("대상 브랜치: {}\n대상 HEAD: {}",target.branch,target.head))
                    .child(format!("대상 워크트리: {}",target.root.display()))
                    .child(if action==HistoryAction::CherryPick {"이 커밋 하나의 변경을 대상 브랜치에 새 커밋으로 적용합니다."}else{"이 커밋 하나의 변경을 되돌리는 새 커밋을 대상 브랜치에 만듭니다."})
                    .child("작업 트리와 index가 깨끗해야 합니다. 충돌 발생 시 상태를 보존하며, Stash / 충돌 해결에서 계속 또는 중단할 수 있습니다. 저장소에 설정된 Git hook이 실행될 수 있습니다."))
                .footer(div().flex().gap_2().justify_end()
                    .child(history_button(&weak,"history-confirm-back","취소",false,|_,w,cx| w.close_dialog(cx)))
                    .child(history_button(&weak,"history-confirm",format!("{} 실행",action.label()),busy,move |a,w,cx| {
                        a.git_history.request+=1;
                        w.close_all_dialogs(cx);
                        let source=source.clone(); let target=destination.clone();
                        a.run_git_job(target.root.clone(),action.label().into(),move |ops|ops.history_action(action,&source,&target),w,cx);
                    }).primary()))
        });
    }
}
fn history_disabled(app: &Pokarak) -> bool {
    app.git_is_busy()
        || app.loading
        || app.git_history.loading
        || app.current != app.git_history.root
}
fn history_button(
    weak: &WeakEntity<Pokarak>,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
    action: impl Fn(&mut Pokarak, &mut Window, &mut Context<Pokarak>) + 'static,
) -> Button {
    let weak = weak.clone();
    Button::new(id)
        .label(label)
        .small()
        .disabled(disabled)
        .on_click(move |_, w, cx| {
            let _ = weak.update(cx, |a, cx| action(a, w, cx));
        })
}

fn history_error(error: String) -> impl IntoElement {
    div()
        .id("history-error")
        .max_h(px(54.))
        .overflow_y_scroll()
        .text_size(px(12.))
        .child(error)
}
fn short(oid: &str) -> &str {
    oid.get(..12).unwrap_or(oid)
}
fn visible(text: &str) -> String {
    text.replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
#[cfg(test)]
mod tests {
    use super::HistoryUi;
    use std::path::PathBuf;

    #[test]
    fn reopening_a_worktree_does_not_accept_its_previous_read() {
        let root = PathBuf::from("/tmp/pokarak-history/main");
        let mut ui = HistoryUi {
            active: true,
            root: Some(root.clone()),
            request: 7,
            ..Default::default()
        };
        assert!(ui.accepts_read(7, &root));
        ui.reset(true);
        ui.root = Some(root.clone());
        assert!(!ui.accepts_read(7, &root));
        assert!(ui.accepts_read(ui.request, &root));
    }

    #[test]
    fn worktree_switch_and_code_view_reject_late_history() {
        let main = PathBuf::from("/tmp/pokarak-history/main");
        let feature = PathBuf::from("/tmp/pokarak-history/feature");
        let mut ui = HistoryUi {
            active: true,
            root: Some(feature.clone()),
            request: 8,
            ..Default::default()
        };
        assert!(!ui.accepts_read(8, &main));
        assert!(ui.accepts_read(8, &feature));
        ui.active = false;
        assert!(!ui.accepts_read(8, &feature));
    }
}
