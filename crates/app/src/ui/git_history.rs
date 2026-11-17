//! Dedicated immutable history view. Its editors never enter working-tree
//! documents/navigation, and writes use the existing Git job lifecycle.
mod branch_filter;
mod columns;
mod context_actions;
mod home;
mod lanes;
mod navigation;
mod rows;
use super::*;
use context_actions::commit_context_menu;
use lanes::{BranchTrace, graph_view, reference_name, track_color};
use pokarak_core::history::{
    self, Commit, CommitDetail, GraphRow, HistoryCursor, HistoryEntry, HistoryPage,
};
use pokarak_core::operations::history_actions::{HistoryAction, HistoryTarget};

#[derive(Default)]
enum NextPage {
    #[default]
    Idle,
    Loading,
    Failed(String),
}

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
    next_page: NextPage,
    limit_reached: bool,
    shallow: bool,
    detail: Option<CommitDetail>,
    selected: Option<PathBuf>,
    document: Option<ReviewDiff>,
    editor: Option<Entity<EditorState>>,
    code: bool,
    resume: Option<crate::prefs::SavedCommit>,
    scroll: UniformListScrollHandle,
    files_scroll: UniformListScrollHandle,
    table_scroll: ScrollHandle,
    table_width: Option<Pixels>,
    column_drag: Option<columns::ColumnDrag>,
    error: Option<String>,
}

impl HistoryUi {
    pub(super) fn review_data(&self) -> Option<(&CommitDetail, Option<&Arc<DiffDocument>>)> {
        Some((
            self.detail.as_ref()?,
            self.document.as_ref().map(|doc| &doc.document),
        ))
    }

    pub(super) fn read_pending(&self) -> bool {
        self.loading || matches!(self.next_page, NextPage::Loading)
    }

    pub(super) fn scroll_y(&self) -> f32 {
        self.scroll.0.borrow().base_handle.offset().y.into()
    }

    pub(super) fn selection(&self) -> Option<crate::prefs::SavedCommit> {
        let detail = self.detail.as_ref()?;
        let parent = detail
            .commit
            .parents
            .iter()
            .position(|oid| Some(oid) == detail.parent.as_ref())
            .unwrap_or(0);
        Some(crate::prefs::SavedCommit {
            oid: detail.commit.oid.clone(),
            parent,
            file: self.selected.clone(),
            code: self.code,
        })
    }

    pub(super) fn resume_selection(&mut self, selection: Option<crate::prefs::SavedCommit>) {
        self.resume = selection;
    }

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

    fn accepts_code_sidebar(&self, request: u64, root: &Path) -> bool {
        !self.active && self.request == request && self.root.as_deref() == Some(root)
    }
}

fn extend_refresh_to_anchor(
    root: &Path,
    page: &mut HistoryPage,
    wanted: usize,
    anchor: Option<&str>,
) -> anyhow::Result<()> {
    let wanted = wanted.min(history::HISTORY_LIMIT);
    while page.entries.len() < history::HISTORY_LIMIT
        && (page.entries.len() < wanted
            || anchor.is_some_and(|oid| !page.entries.iter().any(|row| row.commit.oid == oid)))
    {
        let Some(cursor) = page.next.as_ref() else {
            break;
        };
        let count = if page.entries.len() < wanted {
            (wanted - page.entries.len()).min(100)
        } else {
            100
        };
        let next = history::next_page(root, cursor, count)?;
        let added = next.entries.len();
        page.entries.extend(next.entries);
        page.next = next.next;
        page.loaded = next.loaded;
        page.limit_reached = next.limit_reached;
        page.shallow = next.shallow;
        if added == 0 {
            break;
        }
    }
    Ok(())
}

impl Pokarak {
    /// Reload refs/rows in place. Immutable selected commit readers, split diff
    /// entities and both file/editor scroll handles remain untouched.
    pub(in crate::ui) fn refresh_git_history_preserving(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let branch = self.graph_branch().map(str::to_owned);
        let wanted = self.git_history.entries.len().max(100);
        let offset = self.git_history.scroll.0.borrow().base_handle.offset();
        let row_height: f32 = ui_px(30.).into();
        let y: f32 = offset.y.into();
        let top = (-y / row_height).max(0.).floor() as usize;
        let anchor = self
            .git_history
            .entries
            .get(top)
            .map(|entry| entry.commit.oid.clone());
        let remainder = (-y).max(0.) % row_height;
        let refresh_anchor = anchor.clone();
        self.history_read(
            move |root| {
                let scoped = branch
                    .as_ref()
                    .map(|name| history::navigation::open_branch(root, name, 100))
                    .transpose()?
                    .flatten();
                let (mut page, catalog, missing) = match scoped {
                    Some((page, catalog)) => (page, catalog, None),
                    None => {
                        let (page, catalog) = history::navigation::open(root, 100)?;
                        (page, catalog, branch)
                    }
                };
                extend_refresh_to_anchor(root, &mut page, wanted, refresh_anchor.as_deref())?;
                Ok((page, catalog, missing))
            },
            move |app, (page, catalog, missing), window, cx| {
                if missing.is_some() {
                    app.save_graph_branch(None, cx);
                }
                app.git_history.entries = Arc::default();
                app.git_history.nav.catalog = Some(Arc::new(catalog));
                app.git_history.in_neighborhood = false;
                app.git_history.next_page = NextPage::Idle;
                app.append_history_page(page, cx);
                let row = anchor
                    .as_ref()
                    .and_then(|oid| {
                        app.git_history
                            .entries
                            .iter()
                            .position(|entry| &entry.commit.oid == oid)
                    })
                    .unwrap_or(top.min(app.git_history.entries.len().saturating_sub(1)));
                app.git_history
                    .scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(point(offset.x, px(-(row as f32 * row_height + remainder))));
                app.refresh_inline_commit(window, cx);
            },
            window,
            cx,
        );
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
                    Ok(data) => {
                        ready(app, data, window, cx);
                        if !app.git_history.read_pending() {
                            app.auto_refresh.installed(navigation_generation);
                        }
                    }
                    Err(error) => {
                        if app.auto_refresh.failed(navigation_generation) {
                            app.external_change = true;
                        }
                        app.git_history.error = Some(format!("{error:#}"));
                    }
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
        self.git_history.next_page = NextPage::Idle;
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
        self.load_first_history_page(window, cx);
    }

    fn load_first_history_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.graph_branch().map(str::to_owned);
        let resume = self.git_history.resume.take();
        self.history_read(
            move |root| {
                let scoped = branch
                    .as_ref()
                    .map(|name| history::navigation::open_branch(root, name, 100))
                    .transpose()?
                    .flatten();
                let (page, catalog, missing_branch) = match scoped {
                    Some((page, catalog)) => (page, catalog, None),
                    None => {
                        let (page, catalog) = history::navigation::open(root, 100)?;
                        (page, catalog, branch)
                    }
                };
                // The commit can have been pruned while the app was restarting.
                let restored = resume.and_then(|target| {
                    let detail = history::commit_detail(root, &target.oid, target.parent).ok()?;
                    Some((detail, target))
                });
                Ok((page, catalog, missing_branch, restored))
            },
            move |app, (page, catalog, missing_branch, restored), window, cx| {
                if let Some(name) = missing_branch {
                    app.save_graph_branch(None, cx);
                    app.status = format!(
                        "{} 브랜치가 없어 전체 그래프를 표시합니다",
                        branch_filter::branch_label(&name)
                    );
                }
                app.git_history.nav.catalog = Some(Arc::new(catalog));
                app.append_history_page(page, cx);
                if app.graph_fingerprint.is_none() {
                    app.check_graph_changes(cx);
                }
                if !app.inline_commit_active() {
                    if let Some((detail, target)) = restored {
                        if let Some(row) = app
                            .git_history
                            .entries
                            .iter()
                            .position(|e| e.commit.oid == target.oid)
                        {
                            app.git_history
                                .scroll
                                .scroll_to_item(row, ScrollStrategy::Top);
                        }
                        app.open_commit_detail(detail, target.file, target.code, window, cx);
                        return;
                    }
                    let head = app
                        .active_worktree()
                        .filter(|_| app.graph_branch().is_none())
                        .map(|tree| tree.head.as_str());
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
    fn append_history_page(&mut self, page: HistoryPage, cx: &mut Context<Self>) {
        Arc::make_mut(&mut self.git_history.entries).extend(page.entries);
        self.git_history.cursor = page.next;
        self.git_history.limit_reached = page.limit_reached;
        self.git_history.shallow = page.shallow;
        self.refresh_graph_hover(cx);
    }

    fn load_next_history_page(&mut self, cx: &mut Context<Self>) {
        if !self.git_history.active
            || self.loading
            || self.git_is_busy()
            || self.current != self.git_history.root
            || !matches!(self.git_history.next_page, NextPage::Idle)
        {
            return;
        }
        let (Some(root), Some(cursor)) = (self.current.clone(), self.git_history.cursor.clone())
        else {
            return;
        };
        // A detail selection may run concurrently. Bind pagination to these graph
        // rows instead: scope changes, refreshes and search neighborhoods replace them.
        let rows = Arc::downgrade(&self.git_history.entries);
        let generation = self.request;
        self.git_history.next_page = NextPage::Loading;
        let worktree = root.clone();
        let task = cx.background_spawn(async move { history::next_page(&worktree, &cursor, 100) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |app, cx| {
                if !rows.ptr_eq(&Arc::downgrade(&app.git_history.entries)) {
                    return;
                }
                app.git_history.next_page = NextPage::Idle;
                if app.git_history.active
                    && app.current.as_ref() == Some(&root)
                    && app.request == generation
                    && !app.active_worktree_has_git_job()
                {
                    match result {
                        Ok(page) => app.append_history_page(page, cx),
                        Err(error) => {
                            app.git_history.next_page = NextPage::Failed(format!("{error:#}"))
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
            |app, detail, w, cx| app.open_commit_detail(detail, None, false, w, cx),
            window,
            cx,
        );
    }
    fn open_commit_detail(
        &mut self,
        detail: CommitDetail,
        preferred: Option<PathBuf>,
        code: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = preferred
            .filter(|path| detail.files.iter().any(|f| &f.path == path))
            .or_else(|| detail.files.first().map(|file| file.path.clone()));
        self.git_history.detail = Some(detail);
        self.git_history.selected = None;
        self.git_history.document = None;
        self.git_history.editor = None;
        self.git_history.code = code;
        self.verify_reviewed_files(cx);
        self.show_history_detail(window, cx);
        if let Some(path) = file {
            self.load_history_file(path, window, cx);
        }
    }

    fn load_history_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.git_history.detail.clone() else {
            return;
        };
        self.git_history.selected = Some(path.clone());
        self.git_history.document = None;
        self.git_history.editor = None;
        self.history_read(
            move |root| {
                history::historical_diff(root, &detail, &path).map(split_diff::PreparedDiff::new)
            },
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
        let editor = if self.git_history.code && !doc.binary {
            cx.new(|cx| {
                let mut editor = EditorState::new(window, cx)
                    .language(language(&doc.path))
                    .line_number(true)
                    .soft_wrap(false)
                    .default_value(doc.new_text.clone());
                editor.set_readonly(true, cx);
                editor
            })
        } else {
            patch::editor(
                patch::Prepared::new(doc.patch.clone(), Some(doc.syntax.clone())),
                true,
                window,
                cx,
            )
        };
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
            dialog.title(format!("{} 확인",action.label())).width(ui_px(650.)).margin_top(ui_px(20.)).overlay_closable(false).on_ok(|_,_,_|false)
                .child(div().v_flex().gap_3().text_size(ui_text(13.))
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
        .max_h(ui_px(54.))
        .overflow_y_scroll()
        .text_size(ui_text(12.))
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
    use super::{HistoryUi, extend_refresh_to_anchor};
    use pokarak_core::history;
    use std::{
        path::{Path, PathBuf},
        process::Command,
    };

    fn fixture_git(root: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        let output = command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "--literal-pathspecs",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-C",
            ])
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .trim_end_matches('\n')
            .to_owned()
    }

    #[test]
    fn refresh_pages_past_the_old_count_to_find_a_shifted_visible_anchor() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fixture_git(root, &["init", "-b", "main"]);
        std::fs::write(root.join("source.rs"), "authored fixture\n").unwrap();
        fixture_git(root, &["add", "--", "source.rs"]);
        let tree = fixture_git(root, &["write-tree"]);
        let mut head = fixture_git(root, &["commit-tree", &tree, "-m", "seed"]);
        for number in 1..150 {
            head = fixture_git(
                root,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &head,
                    "-m",
                    &format!("old {number}"),
                ],
            );
        }
        fixture_git(root, &["update-ref", "refs/heads/main", &head]);
        let (old, _) = history::navigation::open_branch(root, "refs/heads/main", 100)
            .unwrap()
            .unwrap();
        let anchor = old.entries[40].commit.oid.clone();
        let excluded = fixture_git(
            root,
            &["commit-tree", &tree, "-p", &head, "-m", "unrelated branch"],
        );
        fixture_git(root, &["update-ref", "refs/heads/unrelated", &excluded]);
        for number in 0..120 {
            head = fixture_git(
                root,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &head,
                    "-m",
                    &format!("incoming {number}"),
                ],
            );
        }
        fixture_git(root, &["update-ref", "refs/heads/main", &head]);
        let (mut page, _) = history::navigation::open_branch(root, "refs/heads/main", 100)
            .unwrap()
            .unwrap();
        assert!(!page.entries.iter().any(|row| row.commit.oid == anchor));
        extend_refresh_to_anchor(root, &mut page, old.entries.len(), Some(&anchor)).unwrap();
        assert_eq!(
            page.entries.iter().position(|row| row.commit.oid == anchor),
            Some(160)
        );
        assert_eq!(page.entries.len(), 200);
        assert!(!page.entries.iter().any(|row| row.commit.oid == excluded));
        // A removed or branch-excluded anchor stops at exhausted scoped history.
        extend_refresh_to_anchor(root, &mut page, old.entries.len(), Some(&excluded)).unwrap();
        assert_eq!(page.entries.len(), 270);
        assert!(page.next.is_none());
    }

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

    #[test]
    fn shared_code_catalog_cannot_cross_worktree_refresh_or_graph_navigation() {
        let main = PathBuf::from("/fixture/main");
        let feature = PathBuf::from("/fixture/feature");
        let mut ui = HistoryUi {
            root: Some(feature.clone()),
            request: 8,
            ..Default::default()
        };
        assert!(ui.accepts_code_sidebar(8, &feature));
        assert!(!ui.accepts_code_sidebar(8, &main));
        assert!(!ui.accepts_read(8, &feature));
        // Reference navigation promotes Code's catalog to a Graph read scope.
        ui.active = true;
        assert!(ui.accepts_read(8, &feature));
        assert!(!ui.accepts_code_sidebar(8, &feature));
        ui.reset(false);
        ui.root = Some(feature.clone());
        assert!(!ui.accepts_code_sidebar(8, &feature));
        assert!(ui.accepts_code_sidebar(ui.request, &feature));
    }
}
