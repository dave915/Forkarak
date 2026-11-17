mod analysis_feedback;
mod auto_refresh;
mod branch_review;
mod cli_control;
mod code_edit;
mod code_workspace;
mod diff_tools;
mod draft_reconciliation;
mod file_icons;
mod file_inspector;
mod file_tree;
mod git_actions;
mod git_commit;
mod git_history;
mod git_workflow;
mod pane_visibility;
mod patch;
mod reader_navigation;
mod reviewed_files;
mod settings;
mod split_diff;
mod syntax;
#[cfg(target_os = "macos")]
mod terminal_dock;
#[cfg(target_os = "macos")]
mod terminal_sessions;
mod toolbar;
mod update_workspace;
mod updates;
use analysis_feedback::AnalysisFeedback;
use file_tree::FileRow;
use split_diff::ReviewDiff;

use crate::prefs::{Preferences, SavedPosition, SavedProject, SavedView};
use crate::theme::{self, Palette, ThemeChoice, ui_px, ui_text};
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::{
    Disableable, Icon, Selectable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Editor, EditorState, Input, InputEvent, InputState, Position, RopeExt},
    resizable::{h_resizable, resizable_panel},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use pokarak_core::{
    ChangedFile, CompareMode, DiffDocument, DiffKind, FileStatus, Snapshot, SourceLocation,
    Worktree,
};
use pokarak_navigation::{NavigationKind, NavigationResult, Navigator};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

const BG: u32 = 0x20252c;
const SIDE: u32 = 0x1b2026;
const PANEL: u32 = 0x282f38;
const LINE: u32 = 0x343e49;
const TEXT: u32 = 0xe5eaf0;
const MUTED: u32 = 0xa4b0be;
const BLUE: u32 = 0x7fb8e2;
const GREEN: u32 = 0x83c4a1;
const RED: u32 = 0xe5a0a5;
type FileRevision = (u64, SystemTime);

struct ResultExcerpt {
    location: SourceLocation,
    // Search providers return a bounded preview. Each visible fragment has its
    // own parse context; unrelated hits and files must not share parser state.
    syntax: std::sync::OnceLock<syntax::CodeSyntax>,
}

impl ResultExcerpt {
    fn styled(&self, cx: &App) -> StyledText {
        let hit = &self.location;
        self.syntax
            .get_or_init(|| syntax::CodeSyntax::new(&hit.path, &hit.preview))
            .styled_line(0, &hit.preview, true, cx)
    }
}

fn file_revision(path: &Path) -> Option<FileRevision> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}
fn result_revisions(locations: &[SourceLocation]) -> HashMap<PathBuf, FileRevision> {
    locations
        .iter()
        .filter_map(|hit| file_revision(&hit.path).map(|revision| (hit.path.clone(), revision)))
        .collect()
}

actions!(
    pokarak,
    [
        Quit,
        OpenSettings,
        OpenWorktreeTerminal,
        OpenGitActions,
        OpenGitCommit,
        OpenGitWorkflow,
        OpenGitHistory,
        SearchGitGraph,
        CloseFile,
        CloseProject,
        CloseAllProjects,
        NextWorktree,
        PreviousWorktree,
        NextProject,
        PreviousProject,
        OpenRepository,
        Refresh,
        Back,
        Forward,
        FindFile,
        FindText,
        FindReferences,
        FindDefinition,
        ToggleCode,
        ToggleSidebar,
        ToggleFiles,
        SaveCodeFile
    ]
);

pub fn bind_keys(cx: &mut App) {
    diff_tools::bind_keys(cx);
    #[cfg(target_os = "macos")]
    crate::terminal::bind_keys(cx);
    #[cfg(target_os = "macos")]
    terminal_sessions::bind_keys(cx);
    cx.on_action(git_actions::request_app_quit);
    cx.set_menus([
        Menu::new("Forkarak").items([
            MenuItem::action("설정…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Forkarak 종료", Quit),
        ]),
        Menu::new("파일").items([
            MenuItem::action("저장소 열기…", OpenRepository),
            #[cfg(target_os = "macos")]
            MenuItem::action("워크트리 터미널 표시 / 숨기기", OpenWorktreeTerminal),
            MenuItem::action("새로고침", Refresh),
            MenuItem::action("파일 저장", SaveCodeFile),
            MenuItem::action("Stash / 충돌 해결…", OpenGitWorkflow),
            MenuItem::separator(),
            MenuItem::action("파일 닫기", CloseFile),
            MenuItem::action("프로젝트 닫기", CloseProject),
            MenuItem::action("모든 프로젝트 닫기", CloseAllProjects),
        ]),
        Menu::new("Git").items([
            MenuItem::action("Stage / 커밋…", OpenGitCommit),
            MenuItem::action("Git 그래프", OpenGitHistory),
            MenuItem::action("커밋 검색…", SearchGitGraph),
            MenuItem::action("Git 작업 및 저장소 설정…", OpenGitActions),
        ]),
        Menu::new("탐색").items([
            MenuItem::action("파일 찾기", FindFile),
            MenuItem::action("본문 검색", FindText),
            MenuItem::separator(),
            MenuItem::action("정의로 이동", FindDefinition),
            MenuItem::action("사용처 찾기", FindReferences),
            MenuItem::separator(),
            MenuItem::action("뒤로", Back),
            MenuItem::action("앞으로", Forward),
        ]),
        Menu::new("보기").items([
            MenuItem::action("사이드바 표시 / 숨기기", ToggleSidebar),
            MenuItem::action("파일 목록 표시 / 숨기기 (코드 보기)", ToggleFiles),
        ]),
    ]);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-shift-k", OpenGitCommit, None),
        KeyBinding::new("cmd-enter", git_commit::SubmitCommit, Some("CommitDialog")),
        KeyBinding::new("cmd-shift-g", OpenGitWorkflow, None),
        KeyBinding::new("cmd-alt-h", OpenGitHistory, None),
        KeyBinding::new("cmd-f", SearchGitGraph, Some("GitGraph")),
        KeyBinding::new("cmd-w", CloseFile, None),
        KeyBinding::new("cmd-shift-w", CloseProject, None),
        KeyBinding::new("cmd-alt-down", NextWorktree, None),
        KeyBinding::new("cmd-alt-up", PreviousWorktree, None),
        KeyBinding::new("ctrl-tab", NextProject, None),
        KeyBinding::new("ctrl-shift-tab", PreviousProject, None),
        KeyBinding::new("cmd-o", OpenRepository, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-`", OpenWorktreeTerminal, None),
        KeyBinding::new("cmd-r", Refresh, None),
        KeyBinding::new("cmd-s", SaveCodeFile, None),
        KeyBinding::new("cmd-[", Back, None),
        KeyBinding::new("cmd-]", Forward, None),
        KeyBinding::new("cmd-p", FindFile, None),
        KeyBinding::new("cmd-shift-f", FindText, None),
        KeyBinding::new("alt-f7", FindReferences, None),
        KeyBinding::new("cmd-b", FindDefinition, None),
        KeyBinding::new("cmd-alt-b", ToggleSidebar, None),
        KeyBinding::new("cmd-shift-b", ToggleFiles, None),
        KeyBinding::new("cmd-d", ToggleCode, None),
    ]);
}

#[derive(Clone)]
struct ReadingPoint {
    path: PathBuf,
    code: bool,
    row: usize,
    line: u32,
    column: u32,
    scroll_x: f32,
    scroll_y: f32,
}

struct Session {
    #[cfg(target_os = "macos")]
    terminal: Option<Entity<terminal_sessions::TerminalSessions>>,
    #[cfg(target_os = "macos")]
    terminal_visible: bool,
    snapshot: Option<Snapshot>,
    saved: SavedView,
    documents: HashMap<PathBuf, Entity<EditorState>>,
    diffs: HashMap<PathBuf, ReviewDiff>,
    split_positions: HashMap<PathBuf, (usize, f32)>,
    scroll: UniformListScrollHandle,
    history: Vec<ReadingPoint>,
    forward: Vec<ReadingPoint>,
    restoring: bool,
}
impl Session {
    fn new(saved: SavedView) -> Self {
        let restoring = saved.selected.is_some();
        Self {
            #[cfg(target_os = "macos")]
            terminal: None,
            #[cfg(target_os = "macos")]
            terminal_visible: false,
            snapshot: None,
            saved,
            documents: HashMap::new(),
            diffs: HashMap::new(),
            split_positions: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            history: vec![],
            forward: vec![],
            restoring,
        }
    }
}

pub struct Pokarak {
    reviewed_files: reviewed_files::ReviewedFiles,
    auto_refresh: auto_refresh::AutoRefresh,
    code_edits: code_edit::CodeEdits,
    git_history: git_history::HistoryUi,
    workflow: git_workflow::WorkflowUi,
    focus: FocusHandle,
    prefs: Preferences,
    current: Option<PathBuf>,
    current_project: Option<PathBuf>,
    opening_path: Option<PathBuf>,
    inventory: Vec<Worktree>,
    change_count: Option<usize>,
    graph_fingerprint: Option<String>,
    count_pending: bool,
    projects: Vec<SavedProject>,
    pending_history: Option<ReadingPoint>,
    pending_projects: VecDeque<PathBuf>,
    update_view: Option<(PathBuf, crate::prefs::UpdateView)>,
    sessions: HashMap<PathBuf, Session>,
    search_input: Entity<InputState>,
    filter: String,
    text_search: bool,
    loading: bool,
    file_loading: bool,
    request: u64,
    file_request: u64,
    navigation_request: u64,
    git_ui: git_actions::GitUi,
    commit_ui: git_commit::CommitUi,
    updates: updates::Updates,
    error: Option<String>,
    analysis_feedback: AnalysisFeedback,
    status: String,
    results: Arc<[ResultExcerpt]>,
    result_revisions: HashMap<PathBuf, FileRevision>,
    pending_location_revision: Option<FileRevision>,
    result_title: String,
    result_semantic: bool,
    results_open: bool,
    result_scroll: UniformListScrollHandle,
    file_scroll: UniformListScrollHandle,
    file_cursor: usize,
    file_tree_focus: FocusHandle,
    result_cursor: usize,
    results_focus: FocusHandle,
    navigation: Option<Arc<Mutex<Navigator>>>,
    cancellation: Option<pokarak_navigation::CancellationHandle>,
    navigating: bool,
    trust_prompt: bool,
    watcher: Option<RecommendedWatcher>,
    changed: Arc<AtomicBool>,
    external_change: bool,
    checking_changes: bool,
    cli_control: Option<crate::control::Server>,
    pending_cli_file: Option<code_workspace::PendingFile>,
    _subscriptions: Vec<Subscription>,
}

impl Pokarak {
    fn set_theme(&mut self, choice: ThemeChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.theme = choice;
        self.prefs.save();
        theme::apply(choice, self.prefs.display, window, cx);
        cx.notify();
    }

    fn cycle_worktree(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let trees: Vec<_> = self
            .inventory
            .iter()
            .filter(|t| !t.is_prunable)
            .map(|t| t.path.clone())
            .collect();
        if trees.is_empty() {
            return;
        }
        let index = trees
            .iter()
            .position(|p| Some(p) == self.opening_path.as_ref().or(self.current.as_ref()))
            .unwrap_or(0);
        let index = if forward {
            (index + 1) % trees.len()
        } else {
            (index + trees.len() - 1) % trees.len()
        };
        self.open_repository(trees[index].clone(), window, cx);
    }

    fn active_project_index(&self) -> Option<usize> {
        if let Some(path) = &self.opening_path {
            return project_index_for_path(&self.projects, path);
        }
        self.projects
            .iter()
            .position(|p| Some(&p.root) == self.current_project.as_ref())
    }

    fn cycle_project(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.projects.is_empty() {
            return;
        }
        let index = self.active_project_index().unwrap_or(0);
        let index = if forward {
            (index + 1) % self.projects.len()
        } else {
            (index + self.projects.len() - 1) % self.projects.len()
        };
        self.open_repository(self.projects[index].last_worktree.clone(), window, cx);
    }
    fn activate_row(&mut self, row: FileRow, window: &mut Window, cx: &mut Context<Self>) {
        if row.directory {
            self.toggle_folder(&row.path, cx);
            self.file_tree_focus.focus(window, cx);
        } else {
            self.open_file(row.path, true, None, window, cx);
        }
    }

    fn handle_list_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if self.search_input.focus_handle(cx).is_focused(window) && !self.text_search {
            let rows = self.file_rows();
            match key {
                "down" if !rows.is_empty() => {
                    self.file_cursor = (self.file_cursor + 1).min(rows.len() - 1);
                }
                "up" => {
                    self.file_cursor = self.file_cursor.saturating_sub(1);
                }
                "escape" => {
                    self.focus.focus(window, cx);
                }
                _ => return,
            }
            self.file_scroll
                .scroll_to_item(self.file_cursor, ScrollStrategy::Nearest);
            cx.stop_propagation();
            cx.notify();
        } else if self.results_focus.is_focused(window) {
            match key {
                "down" if !self.results.is_empty() => {
                    self.result_cursor = (self.result_cursor + 1).min(self.results.len() - 1)
                }
                "up" => self.result_cursor = self.result_cursor.saturating_sub(1),
                "enter" => {
                    if let Some(hit) = self.results.get(self.result_cursor) {
                        self.open_location(hit.location.clone(), window, cx);
                    }
                }
                "escape" => {
                    self.results_open = false;
                    self.navigation_request += 1;
                    self.focus.focus(window, cx);
                }
                _ => return,
            }
            self.result_scroll
                .scroll_to_item(self.result_cursor, ScrollStrategy::Nearest);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn close_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_code_close(cx) {
            return;
        }
        self.capture_positions(cx);
        self.file_request += 1;
        self.navigation_request += 1;
        self.navigating = false;
        let Some(root) = self.current.clone() else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&root) else {
            return;
        };
        session.saved.tabs.retain(|p| p != &path);
        session.documents.remove(&path);
        session.diffs.remove(&path);
        session.restoring = false;
        if session.saved.selected.as_ref() == Some(&path) {
            session.saved.selected = session.saved.tabs.last().cloned();
            if let Some(path) = session.saved.selected.clone() {
                self.open_file(path, false, None, window, cx);
            }
        }
        self.persist(cx);
        cx.notify();
    }

    fn close_project(
        &mut self,
        project_root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_code_close(cx) {
            return;
        }
        self.persist(cx);
        let Some(index) = self.projects.iter().position(|p| p.root == project_root) else {
            return;
        };
        let closing_pending = self
            .opening_path
            .as_ref()
            .is_some_and(|path| project_index_for_path(&self.projects, path) == Some(index));
        let project = self.projects.remove(index);
        if closing_pending {
            self.request += 1;
            self.loading = false;
            self.opening_path = None;
        }
        for root in project.worktrees {
            #[cfg(target_os = "macos")]
            self.close_worktree_terminals(&root, cx);
            self.sessions.remove(&root);
        }
        if self.current_project.as_ref() == Some(&project_root) {
            self.reset_active(cx);
            if let Some(project) = self
                .projects
                .get(index.min(self.projects.len().saturating_sub(1)))
                .cloned()
            {
                self.open_repository(project.last_worktree, window, cx);
            }
        } else if closing_pending {
            self.pending_projects.clear();
            self.resume_current_reader(window, cx);
        }
        self.persist(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn reset_active(&mut self, cx: &mut Context<Self>) {
        self.auto_refresh.reset(None);
        self.reset_git_history();
        self.invalidate_git_page();
        self.changed = Arc::new(AtomicBool::new(false));
        self.opening_path = None;
        self.inventory.clear();
        self.change_count = None;
        self.graph_fingerprint = None;
        self.count_pending = false;
        self.analysis_feedback = AnalysisFeedback::default();
        self.pending_projects.clear();
        self.stop_analysis(cx);
        self.request += 1;
        self.file_request += 1;
        self.current = None;
        self.current_project = None;
        self.watcher = None;
        self.external_change = false;
        self.loading = false;
        self.file_loading = false;
        self.results_open = false;
        self.results = Arc::default();
        self.error = None;
        self.status = "프로젝트를 열어 시작하세요".into();
        self.filter.clear();
        self.text_search = false;
    }

    fn close_all_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_code_close(cx) {
            return;
        }
        self.persist(cx);
        self.reset_active(cx);
        self.projects.clear();
        #[cfg(target_os = "macos")]
        self.shutdown_terminals(cx);
        self.sessions.clear();
        self.persist(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn new(initial: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut prefs = Preferences::load();
        prefs.display = prefs.display.validated();
        let resume = prefs.update_workspace.take();
        if resume.is_some() {
            prefs.save();
        }
        let (mut commit_ui, mut update_view) = (git_commit::CommitUi::default(), None);
        if let Some(resume) = resume {
            commit_ui.restore_messages(resume.messages, window, cx);
            if initial.is_empty() {
                update_view = resume.root.map(|root| (root, resume.view));
            }
        }
        let projects = prefs.open_projects.clone();
        let mut pending_projects: VecDeque<_> = initial.into_iter().collect();
        let initial = pending_projects
            .pop_front()
            .or_else(|| prefs.last_root.clone());
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("파일 이름 검색   ⌘P"));
        let mut subscriptions = vec![cx.subscribe_in(
            &search_input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.filter = input.read(cx).value().to_string();
                    this.navigation_request += 1;
                    this.navigating = false;
                    this.file_cursor = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } if this.text_search => {
                    this.search_contents(window, cx)
                }
                InputEvent::PressEnter { .. } => {
                    if let Some(row) = this.file_rows().get(this.file_cursor).cloned() {
                        this.activate_row(row, window, cx);
                    }
                }
                _ => {}
            },
        )];
        subscriptions.push(cx.observe_window_appearance(window, |this, window, cx| {
            if this.prefs.theme == ThemeChoice::System {
                theme::apply(this.prefs.theme, this.prefs.display, window, cx);
                cx.notify();
            }
        }));
        let mut this = Self {
            reviewed_files: reviewed_files::ReviewedFiles::load(),
            auto_refresh: auto_refresh::AutoRefresh::default(),
            code_edits: code_edit::CodeEdits::load(),
            workflow: git_workflow::WorkflowUi::default(),
            git_history: git_history::HistoryUi::default(),
            focus: cx.focus_handle(),
            prefs,
            current: None,
            current_project: None,
            opening_path: None,
            inventory: Vec::new(),
            change_count: None,
            graph_fingerprint: None,
            count_pending: false,
            projects,
            pending_history: None,
            pending_projects,
            sessions: HashMap::new(),
            search_input,
            filter: String::new(),
            text_search: false,
            loading: false,
            file_loading: false,
            request: 0,
            file_request: 0,
            navigation_request: 0,
            git_ui: git_actions::GitUi::default(),
            commit_ui,
            update_view,
            updates: updates::Updates::new(cx),
            error: None,
            analysis_feedback: AnalysisFeedback::default(),
            status: "저장소를 열어 시작하세요".into(),
            results: Arc::default(),
            result_revisions: HashMap::new(),
            pending_location_revision: None,
            result_title: String::new(),
            result_semantic: false,
            results_open: false,
            result_scroll: UniformListScrollHandle::new(),
            file_scroll: UniformListScrollHandle::new(),
            file_cursor: 0,
            file_tree_focus: cx.focus_handle(),
            result_cursor: 0,
            results_focus: cx.focus_handle(),
            navigation: None,
            cancellation: None,
            navigating: false,
            trust_prompt: false,
            watcher: None,
            changed: Arc::new(AtomicBool::new(false)),
            external_change: false,
            checking_changes: false,
            cli_control: None,
            pending_cli_file: None,
            _subscriptions: subscriptions,
        };
        this.reset_git_history();
        this.install_git_lifecycle(window, cx);
        this.install_cli_control(window, cx);
        cx.on_release(|this, cx| {
            this.cancel_git_job();
            #[cfg(target_os = "macos")]
            this.shutdown_terminals(cx);
            this.persist(cx);
            if let Some(cancel) = this.cancellation.take() {
                let _ = cancel.cancel();
            }
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.cancel_git_job();
            this.persist(cx);
            if let Some(cancel) = this.cancellation.take() {
                let _ = cancel.cancel();
            }
            #[cfg(target_os = "macos")]
            let terminals = this.shutdown_terminals(cx);
            async move {
                #[cfg(target_os = "macos")]
                for terminal in terminals {
                    terminal.wait().await;
                }
            }
        })
        .detach();
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |view, cx| {
            loop {
                executor.timer(Duration::from_millis(200)).await;
                if view
                    .update_in(cx, |this, window, cx| this.auto_refresh_tick(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        theme::apply(this.prefs.theme, this.prefs.display, window, cx);
        this.focus.focus(window, cx);
        if let Some(path) = initial {
            this.begin_open_repository(path, window, cx);
        }
        this
    }

    fn session(&self) -> Option<&Session> {
        self.current
            .as_ref()
            .and_then(|root| self.sessions.get(root))
    }
    fn active_worktree(&self) -> Option<&Worktree> {
        self.inventory
            .iter()
            .find(|tree| Some(&tree.path) == self.current.as_ref())
    }

    fn snapshot(&self) -> Option<&Snapshot> {
        self.session().and_then(|s| s.snapshot.as_ref())
    }
    fn selected(&self) -> Option<PathBuf> {
        self.session().and_then(|s| s.saved.selected.clone())
    }
    fn active_editor(&self) -> Option<Entity<EditorState>> {
        self.session().and_then(|s| {
            s.saved
                .selected
                .as_ref()
                .and_then(|path| s.documents.get(path).cloned())
        })
    }

    fn reading_point(&self, cx: &App) -> Option<ReadingPoint> {
        let session = self.session()?;
        let path = session.saved.selected.clone()?;
        let position = session
            .documents
            .get(&path)
            .map(|doc| doc.read(cx).cursor_position())
            .unwrap_or(Position::new(session.saved.line, session.saved.column));
        let scroll = session
            .documents
            .get(&path)
            .map(|doc| doc.read(cx).scroll_offset())
            .unwrap_or_else(|| point(px(session.saved.scroll_x), px(session.saved.scroll_y)));
        Some(ReadingPoint {
            scroll_x: scroll.x.into(),
            scroll_y: scroll.y.into(),
            path,
            code: session.saved.code,
            row: if session.restoring {
                session.saved.diff_row
            } else {
                split_diff::scroll_row_at(&session.scroll, cx)
            },
            line: position.line,
            column: position.character,
        })
    }

    fn capture_positions(&mut self, cx: &App) {
        if let Some(session) = self
            .current
            .as_ref()
            .and_then(|root| self.sessions.get_mut(root))
        {
            if session.restoring {
                return;
            }
            for (path, diff) in &session.diffs {
                if let Some(position) = diff.split_position(cx) {
                    session.split_positions.insert(path.clone(), position);
                }
            }
            for (path, doc) in &session.documents {
                let editor = doc.read(cx);
                let p = editor.cursor_position();
                let offset = editor.scroll_offset();
                let saved = session.saved.positions.entry(path.clone()).or_default();
                saved.line = p.line;
                saved.column = p.character;
                saved.scroll_x = offset.x.into();
                saved.scroll_y = offset.y.into();
            }
            if let Some(path) = &session.saved.selected {
                let saved = session.saved.positions.entry(path.clone()).or_default();
                saved.diff_row = split_diff::scroll_row_at(&session.scroll, cx);
                session.saved.diff_row = saved.diff_row;
                session.saved.line = saved.line;
                session.saved.column = saved.column;
                session.saved.scroll_x = saved.scroll_x;
                session.saved.scroll_y = saved.scroll_y;
            }
        }
    }

    fn remember_view(&mut self, cx: &App) {
        self.capture_positions(cx);
        if let Some(root) = &self.current
            && let Some(session) = self.sessions.get_mut(root)
        {
            session.saved.filter = self.filter.clone();
            session.saved.text_search = self.text_search;
            self.prefs
                .worktrees
                .insert(root.clone(), session.saved.clone());
        }
    }

    fn persist(&mut self, cx: &App) {
        self.retain_code_drafts(cx);
        self.remember_view(cx);
        if self.current.is_some() || self.opening_path.is_none() {
            self.prefs.last_root = self.current.clone();
        }
        self.prefs.open_projects = self.projects.clone();
        self.prefs.save();
    }

    fn choose_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("프로젝트 열기".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.pending_projects = paths.into_iter().collect();
                    if let Some(path) = this.pending_projects.pop_front() {
                        this.begin_open_repository(path, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn open_repository(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_projects.clear();
        self.begin_open_repository(path, window, cx);
    }

    fn begin_open_repository(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current.is_some() {
            self.remember_view(cx);
        }
        self.stop_analysis(cx);
        self.analysis_feedback = AnalysisFeedback::default();
        self.invalidate_git_page();
        let update_view = self.update_view.take();
        self.opening_path = Some(path.clone());
        self.request += 1;
        self.file_request += 1;
        let token = self.request;
        self.loading = true;
        self.file_loading = false;
        self.checking_changes = false;
        self.count_pending = false;
        self.pending_location_revision = None;
        self.pending_history = None;
        self.error = None;
        self.status = "워크트리 목록을 읽는 중…".into();
        self.results_open = false;
        let task = cx.background_spawn(async move {
            let root = pokarak_core::repository_root(&path)?;
            let trees = pokarak_core::worktree_inventory(&root)?;
            anyhow::Ok((root, trees))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.request != token {
                    return;
                }
                this.loading = false;
                let opening_path = this.opening_path.take();
                match result {
                    Ok((root, trees)) => {
                        this.reset_git_history();
                        this.inventory = trees;
                        this.change_count = None;
                        this.graph_fingerprint = None;
                        this.current = Some(root.clone());
                        this.file_cursor = 0;
                        let project_root = this
                            .inventory
                            .iter()
                            .find(|t| t.is_main)
                            .map(|t| t.path.clone())
                            .unwrap_or_else(|| root.clone());
                        this.current_project = Some(project_root.clone());
                        let project = SavedProject {
                            root: project_root.clone(),
                            last_worktree: root.clone(),
                            worktrees: this.inventory.iter().map(|t| t.path.clone()).collect(),
                        };
                        if let Some(existing) =
                            this.projects.iter_mut().find(|p| p.root == project_root)
                        {
                            *existing = project;
                        } else {
                            this.projects.push(project);
                        }
                        this.prefs.recent.retain(|p| p != &project_root);
                        this.prefs.recent.insert(0, project_root);
                        this.prefs.recent.truncate(8);
                        let saved = this.prefs.worktrees.get(&root).cloned().unwrap_or_default();
                        let session = this
                            .sessions
                            .entry(root.clone())
                            .or_insert_with(|| Session::new(saved));
                        session.snapshot = None;
                        session.restoring = session.saved.selected.is_some();
                        let saved = session.saved.clone();
                        this.filter = saved.filter.clone();
                        this.text_search = saved.text_search;
                        this.search_input.update(cx, |input, cx| {
                            input.set_value(saved.filter, window, cx);
                            input.set_placeholder(
                                if saved.text_search {
                                    "본문 검색 후 Enter"
                                } else {
                                    "파일 이름 검색   ⌘P"
                                },
                                window,
                                cx,
                            );
                        });
                        this.external_change = false;
                        this.status = "준비됨".into();
                        if this.pending_projects.is_empty() {
                            this.watch(root.clone(), cx);
                            let view = update_view
                                .filter(|(path, _)| *path == root)
                                .map(|(_, view)| view);
                            this.open_restored_view(view, window, cx);
                            this.refresh_branch_sync(window, cx);
                            this.restore_git_completion(cx);
                        }
                        this.prefs.last_root = Some(root);
                        this.prefs.open_projects = this.projects.clone();
                        this.prefs.save();
                    }
                    Err(error) => {
                        this.resume_current_reader(window, cx);
                        this.error = Some(match opening_path {
                            Some(path) => format!("{}\n{error:#}", path.display()),
                            None => format!("{error:#}"),
                        });
                        this.status = "저장소를 열 수 없습니다".into();
                    }
                }
                if let Some(path) = this.pending_projects.pop_front() {
                    this.begin_open_repository(path, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn resume_current_reader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            self.status = "프로젝트를 열어 시작하세요".into();
            return;
        };
        self.status = "준비됨".into();
        self.watch(root, cx);
        self.changed.store(true, Ordering::Relaxed);
        if self.git_history.active {
            self.open_git_history(window, cx);
        } else if self.snapshot().is_none() {
            self.refresh(window, cx);
        } else if let Some(path) = self.selected() {
            self.open_file(path, false, None, window, cx);
        }
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening_path.is_some() {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        self.checking_changes = false;
        self.reviewed_files.invalidate_working(&root);
        self.count_pending = false;
        if self.git_history.active {
            self.refresh_graph_inventory(root, window, cx);
            return;
        }
        if let Some(session) = self.sessions.get_mut(&root) {
            session.prepare_code_snapshot();
        }
        let mode = self
            .session()
            .map(|session| session.saved.comparison.clone())
            .unwrap_or_default();
        self.persist(cx);
        self.request += 1;
        let token = self.request;
        self.loading = true;
        self.error = None;
        self.status = "변경사항을 읽는 중…".into();
        let task = cx.background_spawn(async move { pokarak_core::snapshot_current(&root, &mode) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if token != this.request {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(snapshot) => {
                        this.external_change = false;
                        this.install_snapshot(snapshot, window, cx);
                        if !this.file_loading {
                            this.auto_refresh.installed(token);
                        }
                    }
                    Err(error) => {
                        if this.auto_refresh.failed(token) {
                            this.external_change = true;
                        }
                        this.error = Some(format!("{error:#}"));
                        this.status = "변경사항을 읽을 수 없습니다".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn refresh_graph_inventory(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request += 1;
        let token = self.request;
        self.loading = true;
        self.error = None;
        self.status = "워크트리 목록을 읽는 중…".into();
        let task = cx.background_spawn(async move { pokarak_core::worktree_inventory(&root) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if app.request != token {
                    return;
                }
                app.loading = false;
                match result {
                    Ok(trees) => {
                        if let Some(project) = app
                            .projects
                            .iter_mut()
                            .find(|p| Some(&p.root) == app.current_project.as_ref())
                        {
                            project.worktrees =
                                trees.iter().map(|tree| tree.path.clone()).collect();
                        }
                        app.inventory = trees;
                        if let Some(session) = app
                            .current
                            .as_ref()
                            .and_then(|root| app.sessions.get_mut(root))
                        {
                            session.snapshot = None;
                        }
                        app.external_change = false;
                        app.status = "준비됨".into();
                        app.sync_git_history(window, cx);
                        app.refresh_branch_sync(window, cx);
                    }
                    Err(error) => {
                        if app.auto_refresh.failed(token) {
                            app.external_change = true;
                        }
                        app.error = Some(format!("{error:#}"));
                        app.status = "워크트리를 갱신할 수 없습니다".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn install_snapshot(
        &mut self,
        snapshot: Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending_file = self.take_pending_cli_file();
        self.capture_code_buffers(cx);
        self.inventory = snapshot.worktrees.clone();
        self.change_count = Some(snapshot.worktree.changed_files);
        self.graph_fingerprint = None;
        self.count_pending = false;
        let root = self.current.clone().unwrap();
        let session = self.sessions.get_mut(&root).unwrap();
        let code_edits = &self.code_edits;
        let selection = session.snapshot_selection(&snapshot);
        session.saved.comparison = snapshot.compare.clone();
        if pending_file.is_none() {
            session.saved.selected = selection.clone();
        }
        session.saved.tabs.retain(|path| {
            if session.saved.selected.as_ref() == Some(path) || code_edits.dirty(&root, path) {
                true
            } else if path.is_absolute() {
                path.is_file()
            } else {
                snapshot.files.contains(path) || snapshot.changes.iter().any(|f| &f.path == path)
            }
        });
        session
            .documents
            .retain(|path, _| session.saved.tabs.contains(path));
        session
            .diffs
            .retain(|path, _| session.saved.tabs.contains(path));
        session.snapshot = Some(snapshot);
        session.diffs.clear();
        self.verify_reviewed_files(cx);
        self.status = "준비됨".into();
        if let Some(pending) = pending_file {
            self.pending_history = pending.history_restore;
            self.pending_location_revision = pending.target_revision;
            self.open_file(pending.path, pending.history, pending.position, window, cx);
        } else if let Some(path) = selection.filter(|_| !self.git_history.active) {
            self.open_file(path, false, None, window, cx);
        }
        self.sync_git_history(window, cx);
        self.refresh_branch_sync(window, cx);
        self.refresh_workflow_status(window, cx);
        self.restore_git_completion(cx);
        self.finish_code_save_commit(window, cx);
    }

    fn open_file(
        &mut self,
        path: PathBuf,
        history: bool,
        position: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_git_graph();
        let Some(root) = self.current.clone() else {
            return;
        };
        self.capture_positions(cx);
        self.capture_code_buffers(cx);
        if self
            .sessions
            .get_mut(&root)
            .is_some_and(Session::prepare_code_snapshot)
        {
            self.file_request += 1;
            self.navigation_request += 1;
            self.file_loading = false;
            self.navigating = false;
            self.refresh(window, cx);
            self.pending_cli_file = Some(code_workspace::PendingFile {
                root,
                snapshot_request: self.request,
                file_request: self.file_request,
                path,
                position,
                history,
                history_restore: self.pending_history.take(),
                target_revision: self.pending_location_revision.take(),
            });
            return;
        }
        self.pending_cli_file = None;
        let history_restore = self.pending_history.take();
        let target_revision = self.pending_location_revision.take();
        let previous = if history {
            self.reading_point(cx)
        } else {
            None
        };
        let Some(session) = self.sessions.get_mut(&root) else {
            return;
        };
        if let Some(point) = previous {
            session.history.push(point);
            if session.history.len() > 100 {
                session.history.remove(0);
            }
            session.forward.clear();
        }
        let restore = session
            .saved
            .positions
            .get(&path)
            .cloned()
            .unwrap_or_else(|| {
                if session.saved.selected.as_ref() == Some(&path) {
                    SavedPosition {
                        diff_row: session.saved.diff_row,
                        line: session.saved.line,
                        column: session.saved.column,
                        scroll_x: session.saved.scroll_x,
                        scroll_y: session.saved.scroll_y,
                    }
                } else {
                    SavedPosition::default()
                }
            });
        if history && session.saved.selected.as_ref() != Some(&path) {
            session.saved.diff_row = 0;
        }
        session.saved.selected = Some(path.clone());
        if !session.saved.tabs.contains(&path) {
            session.saved.tabs.push(path.clone());
        }
        if position.is_some() {
            session.saved.code = true;
        }
        let file = session
            .snapshot
            .as_ref()
            .and_then(|s| s.changes.iter().find(|f| f.path == path))
            .cloned();
        if file.is_none() {
            session.saved.code = true;
        }
        let mode = session.saved.comparison.clone();
        let previous_position = session
            .documents
            .get(&path)
            .map(|doc| (doc.read(cx).cursor_position(), doc.read(cx).scroll_offset()));
        let previous_selection = session.documents.get(&path).map(|doc| {
            let editor = doc.read(cx);
            let range = editor.selected_range();
            (
                editor.text().offset_to_position(range.start),
                editor.text().offset_to_position(range.end),
                editor.is_selection_reversed(),
            )
        });
        if let Some(point) = &history_restore {
            session.saved.code = point.code;
        }
        session.restoring = true;
        self.file_request += 1;
        self.navigation_request += 1;
        self.navigating = false;
        let token = self.file_request;
        self.file_loading = true;
        self.error = None;
        let load_root = root.clone();
        let load_path = path.clone();
        self.capture_code_buffers(cx);
        let retained = self.code_edits.retained_text(&root, &path);
        let preserve_focus = self.auto_refresh.preserves_focus(self.request);
        let load_request = self.request;
        let live = retained.is_some()
            || !matches!(file.as_ref().map(|f| &f.status), Some(FileStatus::Deleted));
        let task = cx.background_spawn(async move {
            let result = (|| {
                if let Some(expected) = target_revision {
                    anyhow::ensure!(
                        file_revision(&load_root.join(&load_path)) == Some(expected),
                        "탐색 결과의 파일이 변경되었습니다. 다시 검색하세요."
                    );
                }
                let diff = file
                    .as_ref()
                    .map(|file| pokarak_core::diff(&load_root, file, &mode))
                    .transpose()?;
                let deleted = matches!(file.as_ref().map(|f| &f.status), Some(FileStatus::Deleted));
                let binary = diff.as_ref().is_some_and(|diff| diff.binary);
                let text = if let Some(diff) = &diff {
                    if diff.binary {
                        String::new()
                    } else if deleted {
                        diff.old_text.clone()
                    } else {
                        pokarak_core::read_file(&load_root, &load_path)?
                    }
                } else if load_path.is_absolute() && !load_path.starts_with(&load_root) {
                    let metadata = std::fs::metadata(&load_path)?;
                    anyhow::ensure!(
                        metadata.len() < 4 * 1024 * 1024,
                        "외부 소스가 4MB를 초과합니다"
                    );
                    std::fs::read_to_string(&load_path)?
                } else {
                    pokarak_core::read_file(&load_root, &load_path)?
                };
                if let Some(expected) = target_revision {
                    anyhow::ensure!(
                        file_revision(&load_root.join(&load_path)) == Some(expected),
                        "파일을 여는 중 내용이 변경되었습니다. 다시 검색하세요."
                    );
                }
                anyhow::Ok((
                    text,
                    diff.map(split_diff::PreparedDiff::new),
                    !deleted,
                    !binary,
                ))
            })();
            result.or_else(|error| match retained {
                Some(text) => Ok((text, None, false, true)),
                None => Err(error),
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.current.as_ref() != Some(&root) || this.file_request != token {
                    return;
                }
                this.file_loading = false;
                this.capture_code_buffers(cx);
                // A user may have typed after this request began, including
                // after a refresh that now finds the file missing/unreadable.
                let result =
                    result.or_else(|error| match this.code_edits.retained_text(&root, &path) {
                        Some(text) => Ok((text, None, false, true)),
                        None => Err(error),
                    });
                match result {
                    Ok((text, diff, available, text_file)) => {
                        let has_draft = this.code_edits.dirty(&root, &path);
                        let text = this.prepare_code_buffer(
                            &root,
                            &path,
                            text,
                            available,
                            (live && text_file) || has_draft,
                        );
                        let editing = this.code_edits.editing(&root, &path);
                        let review = diff.map(ReviewDiff::from);
                        if let Some(review) = &review
                            && let Some(position) = this
                                .sessions
                                .get(&root)
                                .and_then(|session| session.split_positions.get(&path))
                                .copied()
                        {
                            review.restore_split_position(this, position, cx);
                        }
                        let session = this.sessions.get_mut(&root).unwrap();
                        if let Some(review) = review {
                            session.diffs.insert(path.clone(), review);
                        }
                        let editor = session
                            .documents
                            .entry(path.clone())
                            .or_insert_with(|| {
                                cx.new(|cx| {
                                    EditorState::new(window, cx)
                                        .language(language(&path))
                                        .line_number(true)
                                        .soft_wrap(false)
                                        .default_value(text.clone())
                                })
                            })
                            .clone();
                        editor.update(cx, |editor, cx| {
                            editor.set_readonly(!editing, cx);
                            if editor.value().as_ref() != text {
                                editor.set_value(text, window, cx);
                            }
                            if preserve_focus
                                && history_restore.is_none()
                                && position.is_none()
                                && let Some((cursor, offset)) = previous_position
                            {
                                if let Some((start, end, reversed)) = previous_selection {
                                    let start = editor.text().position_to_offset(&start);
                                    let end = editor.text().position_to_offset(&end);
                                    editor.set_selected_range_with_direction(
                                        start..end,
                                        reversed,
                                        cx,
                                    );
                                } else {
                                    let offset = editor.text().position_to_offset(&cursor);
                                    editor.set_selected_range(offset..offset, cx);
                                }
                                editor.set_scroll_offset(offset, cx);
                            } else if let Some(point) = &history_restore {
                                editor.set_cursor_position(
                                    Position::new(point.line, point.column),
                                    window,
                                    cx,
                                );
                                editor.set_scroll_offset(
                                    gpui_kit::point(px(point.scroll_x), px(point.scroll_y)),
                                    cx,
                                );
                            } else if let Some(position) = position {
                                // Navigation/search locations use UTF-16; saved reading
                                // positions above and below remain editor-native columns.
                                if let Some(position) =
                                    reader_navigation::editor_position(editor.text(), position)
                                {
                                    editor.set_cursor_position(position, window, cx);
                                }
                            } else if let Some((position, offset)) = previous_position {
                                editor.set_cursor_position(position, window, cx);
                                editor.set_scroll_offset(offset, cx);
                            } else {
                                editor.set_cursor_position(
                                    Position::new(restore.line, restore.column),
                                    window,
                                    cx,
                                );
                                editor.set_scroll_offset(
                                    point(px(restore.scroll_x), px(restore.scroll_y)),
                                    cx,
                                );
                            }
                        });
                        let diff_row = history_restore
                            .as_ref()
                            .map(|point| point.row)
                            .unwrap_or(restore.diff_row);
                        session.saved.diff_row = diff_row;
                        session
                            .scroll
                            .scroll_to_item_strict(diff_row, ScrollStrategy::Top);
                        let code = session.saved.code;
                        this.bind_code_editor(&root, &path, &editor, cx);
                        if !code && !preserve_focus && !this.file_tree_focus.is_focused(window) {
                            this.focus.focus(window, cx);
                        }
                        this.status = if this.navigation.is_some() {
                            "코드 탐색 준비됨".into()
                        } else {
                            "파일 열람 중".into()
                        };
                        let weak = cx.entity().downgrade();
                        let restore_root = root.clone();
                        window.on_next_frame(move |window, _| {
                            window.on_next_frame(move |_, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.file_request == token {
                                        if let Some(session) = this.sessions.get_mut(&restore_root)
                                        {
                                            session.restoring = false;
                                        }
                                        this.persist(cx);
                                        this.auto_refresh.installed(load_request);
                                    }
                                });
                            });
                        });
                    }
                    Err(error) => {
                        if this.auto_refresh.failed(load_request) {
                            this.external_change = true;
                        }
                        if let Some(s) = this.sessions.get_mut(&root) {
                            s.documents.remove(&path);
                            s.diffs.remove(&path);
                            s.restoring = false;
                        }
                        this.error = Some(format!("{error:#}"));
                        this.status = "파일을 읽을 수 없습니다".into();
                    }
                }
                this.restore_git_completion(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn navigate_history(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let current = self.reading_point(cx);
        let session = self.sessions.get_mut(&root).unwrap();
        let target = if backward {
            session.history.pop()
        } else {
            session.forward.pop()
        };
        if let Some(target) = target {
            if let Some(point) = current {
                if backward {
                    session.forward.push(point);
                } else {
                    session.history.push(point);
                }
            }
            session.saved.code = target.code;
            session.saved.diff_row = target.row;
            session.saved.line = target.line;
            session.saved.column = target.column;
            self.pending_history = Some(target.clone());
            self.open_file(target.path, false, None, window, cx);
        }
    }

    fn set_split_diff(&mut self, split: bool) {
        self.prefs.split_diff = split;
        self.prefs.save();
    }

    fn toggle_code(&mut self, code: bool, cx: &mut Context<Self>) {
        self.navigation_request += 1;
        self.navigating = false;
        if let Some(root) = &self.current
            && let Some(s) = self.sessions.get_mut(root)
        {
            s.saved.code = code;
        }
        self.persist(cx);
        cx.notify();
    }

    fn select_reader_mode(&mut self, code: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_code(code, cx);
        if !code {
            // Explicit Diff/side-by-side selection must leave the hidden code
            // editor's focus path so DiffReview shortcuts reach the root.
            self.focus.focus(window, cx);
        }
    }

    fn stop_analysis(&mut self, cx: &mut Context<Self>) {
        self.navigation_request += 1;
        if let Some(cancel) = self.cancellation.take() {
            let _ = cancel.cancel();
        }
        if let Some(nav) = self.navigation.take() {
            cx.background_spawn(async move {
                if let Ok(mut nav) = nav.lock() {
                    nav.shutdown();
                }
            })
            .detach();
        }
        self.navigating = false;
        self.trust_prompt = false;
    }

    fn enable_analysis(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        if !self.prefs.trusted.contains(&root) {
            self.prefs.trusted.push(root.clone());
            self.prefs.save();
        }
        let navigator = Navigator::with_options(
            root,
            pokarak_navigation::default_configs(),
            pokarak_navigation::NavigatorOptions {
                allow_project_imports: true,
                ..Default::default()
            },
        );
        self.cancellation = Some(navigator.cancellation_handle());
        self.navigation = Some(Arc::new(Mutex::new(navigator)));
        self.trust_prompt = false;
        self.status = "탐색할 심볼을 ⌘클릭하세요. 서버는 첫 요청 때 시작합니다.".into();
        cx.notify();
    }

    fn request_navigation(
        &mut self,
        kind: NavigationKind,
        position: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Dialog readers (index/history/conflict) are not current working code.
        if self.navigating
            || self.git_history.active
            || self.active_worktree_has_git_job()
            || window.has_active_dialog(cx)
        {
            return;
        }
        let (Some(root), Some(path)) = (self.current.clone(), self.selected()) else {
            return;
        };
        if self.code_edits.dirty(&root, &path) {
            self.status =
                "심볼 탐색 전에 파일을 저장하세요. 미저장 초안을 디스크 소스로 탐색하지 않습니다."
                    .into();
            cx.notify();
            return;
        }
        let current_source = self.session().map(|s| s.saved.code).unwrap_or(false)
            && !self
                .snapshot()
                .and_then(|s| s.changes.iter().find(|f| f.path == path))
                .map(|f| f.status == FileStatus::Deleted)
                .unwrap_or(false);
        if !current_source {
            self.status =
                "현재 코드 탭에서 심볼을 선택하세요. 변경 전 코드는 탐색하지 않습니다.".into();
            cx.notify();
            return;
        }
        if self.navigation.is_none() {
            if self.prefs.trusted.contains(&root) {
                self.enable_analysis(cx);
            } else {
                self.trust_prompt = true;
                cx.notify();
                return;
            }
        }
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(target) = reader_navigation::navigation_target(
            editor.read(cx).text(),
            editor.read(cx).cursor(),
            position,
        ) else {
            return;
        };
        let position = target.position;
        let displayed_text = editor.read(cx).value().to_string();
        let symbol_start = target.symbol_start;
        let absolute = root.join(&path);
        let navigator = self.navigation.as_ref().unwrap().clone();
        let retry_kind = kind.clone();
        let retry_path = path.clone();
        let is_definition = matches!(kind, NavigationKind::Definition);
        let title = match kind {
            NavigationKind::Definition => "정의",
            NavigationKind::References => "사용처",
            NavigationKind::Implementation => "구현체",
        }
        .to_string();
        self.navigating = true;
        self.navigation_request += 1;
        let token = self.navigation_request;
        self.status = if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("kt" | "kts")
        ) {
            format!("Kotlin {title} 찾는 중… 첫 탐색 시 Gradle/Maven 프로젝트를 동기화합니다.")
        } else {
            format!("{title} 찾는 중…")
        };
        let source = absolute.clone();
        let expected_text = displayed_text.clone();
        let task = cx.background_spawn(async move {
            let mut output = String::new();
            let result = (|| {
                let mut nav = navigator
                    .lock()
                    .map_err(|_| anyhow::anyhow!("분석 서버 상태를 읽을 수 없습니다"))?;
                let result = (|| {
                    anyhow::ensure!(
                        std::fs::read_to_string(&absolute)? == displayed_text,
                        "파일이 외부에서 변경되었습니다. 새로고침 후 다시 탐색하세요."
                    );
                    let mut result =
                        nav.navigate(&absolute, position.line, position.character, kind)?;
                    let mut declaration = false;
                    if is_definition
                        && result.semantic
                        && result.locations.len() == 1
                        && result.locations[0].path == absolute
                        && result.locations[0].line == position.line
                        && result.locations[0].column == symbol_start.character
                    {
                        result = nav.navigate(
                            &absolute,
                            position.line,
                            position.character,
                            NavigationKind::References,
                        )?;
                        declaration = true;
                    }
                    anyhow::ensure!(
                        std::fs::read_to_string(&absolute)? == displayed_text,
                        "탐색 중 파일이 변경되었습니다. 새로고침 후 다시 탐색하세요."
                    );
                    let revisions = result_revisions(&result.locations);
                    anyhow::Ok((result, declaration, revisions))
                })();
                output = format!("{}\n{}", nav.server_messages(), nav.stderr());
                result
            })();
            (result, output)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (result, output) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.navigation_request != token
                    || this.current.as_ref() != Some(&root)
                    || this.selected().map(|p| root.join(p)) != Some(source)
                {
                    return;
                }
                this.navigating = false;
                if !this.session().map(|s| s.saved.code).unwrap_or(false)
                    || this.active_editor().map(|e| e.read(cx).value().to_string())
                        != Some(expected_text)
                {
                    return;
                }
                this.observe_feedback();
                this.analysis_feedback.append(&output);
                match result {
                    Ok((result, declaration, revisions)) => {
                        this.error = None;
                        this.result_revisions = revisions;
                        this.status = result.status.clone();
                        if is_definition
                            && !declaration
                            && result.semantic
                            && result.locations.len() == 1
                        {
                            this.open_location(result.locations[0].clone(), window, cx);
                        } else {
                            this.show_results(
                                if declaration {
                                    "사용처".into()
                                } else {
                                    title
                                },
                                result,
                                window,
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        let error = format!("{error:#}");
                        this.record_analysis_failure(
                            root.clone(),
                            retry_path,
                            retry_kind,
                            position,
                            error.clone(),
                        );
                        this.error = Some(error);
                        this.status = "분석을 완료하지 못했습니다. Details에서 확인하세요.".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn show_results(
        &mut self,
        title: String,
        result: NavigationResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.results = result
            .locations
            .into_iter()
            .map(|location| ResultExcerpt {
                location,
                syntax: Default::default(),
            })
            .collect();
        self.result_semantic = result.semantic;
        self.result_title = title;
        self.results_open = true;
        self.analysis_feedback.output_open = false;
        self.result_cursor = 0;
        self.results_focus.focus(window, cx);
        self.result_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn open_location(
        &mut self,
        location: SourceLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(expected) = self.result_revisions.get(&location.path).copied() {
            if file_revision(&location.path) != Some(expected) {
                self.error = Some(
                    "탐색 결과의 파일이 변경되었습니다. 사용처 또는 본문 검색을 다시 실행하세요."
                        .into(),
                );
                cx.notify();
                return;
            }
            self.pending_location_revision = Some(expected);
        }
        let Some(root) = &self.current else {
            return;
        };
        let path = location
            .path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or(location.path);
        self.open_file(
            path,
            true,
            Some(Position::new(location.line, location.column)),
            window,
            cx,
        );
    }

    fn search_contents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let query = self.filter.trim().to_string();
        if query.len() < 2 {
            self.status = "본문 검색어를 두 글자 이상 입력하세요".into();
            cx.notify();
            return;
        }
        self.status = "본문 검색 중…".into();
        self.navigation_request += 1;
        self.navigating = false;
        let token = self.navigation_request;
        let expected_query = query.clone();
        let search_root = root.clone();
        let task = cx.background_spawn(async move {
            let locations = pokarak_core::search(&search_root, &query, 300)?;
            let revisions = result_revisions(&locations);
            anyhow::Ok((locations, revisions))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.navigation_request != token
                    || this.current.as_ref() != Some(&root)
                    || this.filter.trim() != expected_query
                {
                    return;
                }
                match result {
                    Ok((locations, revisions)) => {
                        this.result_revisions = revisions;
                        this.show_results(
                            format!("본문 검색: {expected_query}"),
                            NavigationResult {
                                locations,
                                semantic: false,
                                status: String::new(),
                            },
                            window,
                            cx,
                        );
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
                this.status = "본문 검색 완료".into();
                cx.notify();
            });
        })
        .detach();
    }

    fn focus_search(&mut self, text: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening_path.is_some() {
            return;
        }
        self.show_code_view(window, cx);
        self.text_search = text;
        if !text && let Some(root) = &self.current {
            self.sessions.get_mut(root).unwrap().saved.all_files = true;
        }
        self.search_input.update(cx, |input, cx| {
            input.set_placeholder(
                if text {
                    "본문 검색 후 Enter"
                } else {
                    "파일 이름 검색   ⌘P"
                },
                window,
                cx,
            );
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn render_worktrees(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_graph_sidebar(cx)
    }

    fn code_analysis_control(&self, cx: &mut Context<Self>) -> Button {
        Button::new("analysis")
            .label(if self.navigation.is_some() {
                "코드 탐색 켜짐"
            } else {
                "코드 탐색"
            })
            .icon(IconName::Waypoints)
            .ghost()
            .small()
            .disabled(self.current.is_none())
            .on_click(cx.listener(|this, _, _, cx| {
                if this.navigation.is_some() {
                    this.stop_analysis(cx);
                    this.status = "코드 탐색 중지됨".into();
                } else {
                    this.trust_prompt = true;
                }
                cx.notify();
            }))
    }

    fn render_reader(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let Some(session) = self.session() else {
            return self.render_welcome(cx);
        };
        let Some(path) = &session.saved.selected else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette.rgb(MUTED))
                .child("파일을 선택하면 변경사항과 코드를 볼 수 있습니다.")
                .into_any_element();
        };
        let code = session.saved.code;
        let diff = session.diffs.get(path);
        let editor = session.documents.get(path).cloned();
        let deleted = session
            .snapshot
            .as_ref()
            .and_then(|s| s.changes.iter().find(|f| &f.path == path))
            .map(|f| f.status == FileStatus::Deleted)
            .unwrap_or(false);
        let dirty = self
            .current
            .as_ref()
            .is_some_and(|root| self.code_edits.dirty(root, path));
        let editing = self
            .current
            .as_ref()
            .is_some_and(|root| self.code_edits.editing(root, path))
            && (!deleted || dirty);
        let mut tabs = div()
            .id("file-tabs")
            .flex()
            .items_center()
            .h(ui_px(34.))
            .flex_none()
            .overflow_x_scroll()
            .border_b_1()
            .border_color(palette.rgb(LINE));
        for (i, tab) in session.saved.tabs.iter().enumerate() {
            let target = tab.clone();
            let close = tab.clone();
            tabs = tabs.child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        file_icons::file_button(
                            ("tab", i),
                            tab,
                            format!(
                                "{}{}",
                                if self
                                    .current
                                    .as_ref()
                                    .is_some_and(|root| self.code_edits.dirty(root, tab))
                                {
                                    "● "
                                } else {
                                    ""
                                },
                                tab.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            cx,
                        )
                        .ghost()
                        .small()
                        .selected(tab == path)
                        .tooltip(tab.to_string_lossy().to_string())
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.open_file(target.clone(), true, None, w, cx)
                        })),
                    )
                    .child(
                        Button::new(("close-tab", i))
                            .icon(IconName::X)
                            .ghost()
                            .small()
                            .tooltip("파일 닫기  ⌘W")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_file(close.clone(), window, cx)
                            })),
                    ),
            );
        }
        let head = div()
            .id("reader-controls")
            .overflow_x_scroll()
            .h(ui_px(40.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(palette.rgb(LINE))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .font_family("JetBrains Mono")
                    .text_size(ui_text(11.))
                    .child(path.to_string_lossy().to_string()),
            )
            .child(self.code_analysis_control(cx))
            .child(self.reviewed_file_button(cx))
            .child(
                Button::new("diff")
                    .label("변경")
                    .ghost()
                    .small()
                    .selected(!code && !self.prefs.split_diff)
                    .disabled(diff.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_split_diff(false);
                        this.select_reader_mode(false, window, cx)
                    })),
            )
            .child(
                Button::new("work-split")
                    .label("좌우")
                    .tooltip("변경 전후를 좌우로 비교")
                    .small()
                    .ghost()
                    .selected(!code && self.prefs.split_diff)
                    .disabled(diff.is_none_or(|d| d.binary))
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.set_split_diff(true);
                        app.select_reader_mode(false, window, cx)
                    })),
            )
            .child(
                Button::new("code")
                    .label("코드")
                    .ghost()
                    .small()
                    .selected(code)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_code(true, cx))),
            )
            .when_some(
                self.current.as_ref().and_then(|root| {
                    let relative = if path.is_absolute() {
                        path.strip_prefix(root).ok()?
                    } else {
                        path
                    };
                    Some(file_inspector::Target {
                        path: relative.to_owned(),
                        revision: None,
                        blame_revision: session
                            .snapshot
                            .as_ref()
                            .filter(|_| deleted)
                            .map(|s| s.worktree.head.clone())
                            .filter(|s| !s.is_empty()),
                    })
                }),
                |bar, target| bar.child(self.file_inspection_buttons(target, cx)),
            )
            .when(code, |bar| {
                bar.child(
                    Button::new("refs")
                        .label("사용처")
                        .ghost()
                        .small()
                        .disabled(!code || deleted || dirty || self.navigating)
                        .tooltip("선택한 심볼의 사용처  ⌥F7")
                        .on_click(cx.listener(|this, _, w, cx| {
                            this.request_navigation(NavigationKind::References, None, w, cx)
                        })),
                )
            })
            .when(code, |bar| {
                bar.child(
                    Button::new("impl")
                        .label("구현체")
                        .ghost()
                        .small()
                        .disabled(!code || deleted || dirty || self.navigating)
                        .on_click(cx.listener(|this, _, w, cx| {
                            this.request_navigation(NavigationKind::Implementation, None, w, cx)
                        })),
                )
            });
        let content = if diff.map(|d| d.binary).unwrap_or(false) && !dirty {
            div()
                .p_6()
                .text_color(palette.rgb(MUTED))
                .child("바이너리 파일입니다. 텍스트 비교를 제공하지 않습니다.")
                .into_any_element()
        } else if code {
            if let Some(editor) = editor {
                let clicked_editor = editor.clone();
                div()
                    .size_full()
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            if event.modifiers.platform && !deleted && !dirty {
                                this.request_clicked_definition(
                                    &clicked_editor,
                                    event.position,
                                    window,
                                    cx,
                                );
                            }
                        }),
                    )
                    .child(
                        Editor::new(&editor)
                            .readonly(!editing)
                            .bordered(false)
                            .h(relative(1.)),
                    )
                    .into_any_element()
            } else {
                div()
                    .p_4()
                    .text_color(palette.rgb(MUTED))
                    .child(if self.file_loading {
                        "파일을 읽는 중…"
                    } else {
                        "파일을 표시할 수 없습니다. 새로고침하거나 다른 파일을 선택하세요."
                    })
                    .into_any_element()
            }
        } else if let Some(diff) = diff.filter(|_| self.prefs.split_diff) {
            diff.split(self, cx)
        } else if let Some(diff) = diff {
            diff.unified(self, Some(&session.scroll), false, cx)
        } else {
            div()
                .p_4()
                .text_color(palette.rgb(MUTED))
                .child(if self.file_loading {
                    "변경사항을 읽는 중…"
                } else {
                    "이 파일에는 비교할 변경사항이 없습니다."
                })
                .into_any_element()
        };
        let _ = window;
        div()
            .size_full()
            .v_flex()
            .overflow_hidden()
            .child(tabs)
            .child(head)
            .when(!code, |reader| {
                reader.children(diff.and_then(|doc| self.diff_reading_controls(doc, false, cx)))
            })
            .when(code, |d| d.child(self.code_edit_controls(path, cx)))
            .children(self.code_draft_notice(path, cx))
            .when(deleted && code && !dirty, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(ui_text(11.))
                        .text_color(palette.rgb(RED))
                        .child(
                            "삭제 전 코드입니다. 현재 작업 트리 기준 탐색은 사용할 수 없습니다.",
                        ),
                )
            })
            .when(diff.map(|d| d.truncated).unwrap_or(false), |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(ui_text(11.))
                        .text_color(palette.rgb(0xe4bb79))
                        .child("큰 파일의 일부만 표시합니다."),
                )
            })
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
            .into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let results = self.results.clone();
        let weak = cx.entity().downgrade();
        let root = self.current.clone().unwrap_or_default();
        let cursor = self.result_cursor;
        div()
            .id("results-panel")
            .max_h(relative(0.3))
            .min_h_0()
            .track_focus(&self.results_focus)
            .h(ui_px(210.))
            .flex_none()
            .v_flex()
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .bg(palette.rgb(SIDE))
            .child(
                div()
                    .h(ui_px(34.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(format!("{}  {}", self.result_title, self.results.len()))
                    .child(
                        div()
                            .text_size(ui_text(11.))
                            .text_color(palette.rgb(MUTED))
                            .child(if self.result_semantic {
                                "언어 분석 결과"
                            } else {
                                "텍스트 검색 결과"
                            }),
                    )
                    .child(div().flex_1())
                    .child(self.output_button("results-output", cx))
                    .child(
                        Button::new("close-results")
                            .icon(IconName::X)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.results_open = false;
                                this.navigation_request += 1;
                                this.navigating = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(if results.is_empty() {
                div()
                    .p_3()
                    .text_color(palette.rgb(MUTED))
                    .child("검색 결과가 없습니다.")
                    .into_any_element()
            } else {
                uniform_list("results", results.len(), move |range, _, cx| {
                    range
                        .map(|ix| {
                            let preview = &results[ix];
                            let hit = preview.location.clone();
                            let target = weak.clone();
                            let accessible_target = weak.clone();
                            let accessible_hit = hit.clone();
                            let label = format!(
                                "{}:{}",
                                hit.path.strip_prefix(&root).unwrap_or(&hit.path).display(),
                                hit.line + 1
                            );
                            div()
                                .id(("hit", ix))
                                .role(accesskit::Role::Button)
                                .aria_label(label.clone())
                                .bg(if ix == cursor {
                                    palette.rgb(PANEL)
                                } else {
                                    palette.rgb(SIDE)
                                })
                                .on_a11y_action(accesskit::Action::Click, move |_, window, cx| {
                                    let _ = accessible_target.update(cx, |this, cx| {
                                        this.open_location(accessible_hit.clone(), window, cx)
                                    });
                                })
                                .h(ui_px(40.))
                                .px_3()
                                .cursor_pointer()
                                .hover(|s| s.bg(palette.rgb(PANEL)))
                                .child(
                                    div()
                                        .text_size(ui_text(11.))
                                        .text_color(palette.rgb(BLUE))
                                        .text_ellipsis()
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(ui_text(11.))
                                        .text_color(palette.rgb(MUTED))
                                        .text_ellipsis()
                                        .child(preview.styled(cx)),
                                )
                                .on_click(move |_, w, cx| {
                                    let _ = target.update(cx, |this, cx| {
                                        this.open_location(hit.clone(), w, cx)
                                    });
                                })
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(&self.result_scroll)
                .h_full()
                .flex_1()
                .into_any_element()
            })
            .into_any_element()
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let mut content = div()
            .size_full()
            .v_flex()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Icon::new(IconName::Utensils)
                    .data(include_bytes!("../../../assets/icons/spork.svg").as_slice())
                    .size(ui_px(52.))
                    .text_color(palette.rgb(BLUE)),
            )
            .child(
                div()
                    .text_size(ui_text(24.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("포카락"),
            )
            .child(
                div()
                    .text_color(palette.rgb(MUTED))
                    .child("워크트리의 커밋 그래프에서 변경사항을 비교하고, 필요한 줄이나 블록을 골라 Stage하세요."),
            )
            .child(
                Button::new("welcome-open")
                    .label("프로젝트 열기")
                    .icon(IconName::FolderOpen)
                    .on_click(cx.listener(|this, _, w, cx| this.choose_repository(w, cx))),
            )
            .child(div().flex().gap_2()
                .child(Button::new("welcome-clone").label("Clone…").on_click(cx.listener(|app,_,w,cx| app.open_repository_setup(true,w,cx))))
                .child(Button::new("welcome-init").label("Init…").on_click(cx.listener(|app,_,w,cx| app.open_repository_setup(false,w,cx)))));
        let mut recent = div()
            .id("recent-projects")
            .w(ui_px(620.))
            .max_w(relative(0.9))
            .max_h(ui_px(200.))
            .v_flex()
            .gap_1()
            .overflow_y_scroll();
        for (index, path) in self.prefs.recent.iter().enumerate() {
            let target = path.clone();
            recent = recent.child(
                Button::new(("recent", index))
                    .label(
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                    )
                    .icon(IconName::FolderGit2)
                    .tooltip(path.to_string_lossy().to_string())
                    .ghost()
                    .small()
                    .w_full()
                    .h(ui_px(40.))
                    .flex_none()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .text_size(ui_text(11.))
                            .text_color(palette.rgb(MUTED))
                            .child(path.parent().unwrap_or(path).to_string_lossy().to_string()),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.open_repository(target.clone(), w, cx)
                    })),
            );
        }
        if !self.prefs.recent.is_empty() {
            content = content
                .child(
                    div()
                        .mt_3()
                        .text_size(ui_text(12.))
                        .text_color(palette.rgb(MUTED))
                        .child("최근 프로젝트"),
                )
                .child(recent);
        }
        content.into_any_element()
    }
}

impl Render for Pokarak {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.observe_feedback();
        let palette = Palette::new(cx);
        let pending = self
            .opening_path
            .as_deref()
            .map(|p| (p, "워크트리 목록을 읽는 중…"))
            .or_else(|| {
                self.current
                    .as_deref()
                    .filter(|_| {
                        self.loading && !self.git_history.active && self.snapshot().is_none()
                    })
                    .map(|p| (p, "파일 목록과 변경사항을 읽는 중…"))
            });
        let body = if let Some((path, phase)) = pending {
            div()
                .id("opening-project")
                .size_full()
                .v_flex()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().font_weight(FontWeight::MEDIUM).child(phase))
                .child(
                    div()
                        .text_color(palette.rgb(MUTED))
                        .child(path.display().to_string()),
                )
                .child(
                    Button::new("switch-opening-project")
                        .ghost()
                        .small()
                        .label("다른 프로젝트 열기…")
                        .on_click(
                            cx.listener(|app, _, window, cx| app.choose_repository(window, cx)),
                        ),
                )
                .into_any_element()
        } else if let Some(root) = &self.current {
            self.render_workspace_panes(root, window, cx)
        } else {
            self.render_welcome(cx)
        };
        let context = self
            .opening_path
            .as_ref()
            .or(self.current.as_ref())
            .map(|p| {
                let path = p.to_string_lossy();
                if self.opening_path.is_none() && !self.git_history.active {
                    format!("{path}  ·  {}", self.applied_comparison_label())
                } else {
                    path.to_string()
                }
            })
            .unwrap_or_else(|| "로컬 Git 프로젝트".into());
        #[cfg(target_os = "macos")]
        let body = self.with_terminal_dock(body, window);
        let root = div().id("pokarak");
        #[cfg(target_os = "macos")]
        let root = root.on_action(cx.listener(|app, _: &OpenWorktreeTerminal, window, cx| {
            app.open_worktree_terminal(window, cx)
        }));
        // Register only available panel actions so native menu entries reflect
        // the same enabled state as the toolbar (files are a Code-only panel).
        let root = root.when(
            self.current.is_some() && self.opening_path.is_none(),
            |root| {
                root.on_action(cx.listener(|app, _: &ToggleSidebar, w, cx| {
                    app.toggle_pane(pane_visibility::Pane::Sidebar, w, cx)
                }))
                .when(!self.git_history.active, |root| {
                    root.on_action(cx.listener(|app, _: &ToggleFiles, w, cx| {
                        app.toggle_pane(pane_visibility::Pane::Files, w, cx)
                    }))
                })
            },
        );
        root.key_context(self.diff_key_context()).track_focus(&self.focus).size_full().overflow_hidden().v_flex().bg(palette.rgb(BG)).text_color(palette.rgb(TEXT)).text_size(ui_text(13.)).font_family("IBM Plex Sans")
            .capture_key_down(cx.listener(Self::handle_list_key))
            .on_action(cx.listener(|this,_:&SearchGitGraph,w,cx|this.open_graph_search(w,cx)))
            .on_action(cx.listener(|this,_:&OpenGitHistory,w,cx|this.open_git_history(w,cx)))
            .on_action(cx.listener(|this,_:&OpenGitWorkflow,w,cx|this.open_git_workflow(w,cx)))
            .on_action(cx.listener(|this,_:&OpenSettings,w,cx|this.open_settings(w,cx)))
            .on_action(cx.listener(|this,_:&OpenGitActions,w,cx|this.open_git_actions(w,cx)))
            .on_action(cx.listener(|this,_:&OpenGitCommit,w,cx|this.open_git_commit(w,cx)))
            .on_action(cx.listener(|this,_:&OpenRepository,w,cx|this.choose_repository(w,cx)))
            .on_action(cx.listener(|this,_:&CloseFile,w,cx|{if !this.git_history.active && let Some(path)=this.selected(){this.close_file(path,w,cx)}else if let Some(project)=this.current_project.clone(){this.close_project(project,w,cx)}}))
            .on_action(cx.listener(|this,_:&CloseProject,w,cx|{if let Some(project)=this.current_project.clone(){this.close_project(project,w,cx)}}))
            .on_action(cx.listener(|this,_:&CloseAllProjects,w,cx|this.close_all_projects(w,cx)))
            .on_action(cx.listener(|this,_:&NextWorktree,w,cx|this.cycle_worktree(true,w,cx)))
            .on_action(cx.listener(|this,_:&PreviousWorktree,w,cx|this.cycle_worktree(false,w,cx)))
            .on_action(cx.listener(|this,_:&NextProject,w,cx|this.cycle_project(true,w,cx)))
            .on_action(cx.listener(|this,_:&PreviousProject,w,cx|this.cycle_project(false,w,cx)))
            .on_action(cx.listener(|this,_:&Refresh,w,cx|this.refresh(w,cx)))
            .on_action(cx.listener(|this,_:&diff_tools::NextDiffHunk,w,cx|this.jump_diff_hunk(true,w,cx)))
            .on_action(cx.listener(|this,_:&diff_tools::PreviousDiffHunk,w,cx|this.jump_diff_hunk(false,w,cx)))
            .on_action(cx.listener(|this,_:&SaveCodeFile,w,cx|this.save_code_file(false,w,cx)))
            .on_action(cx.listener(|this,_:&Back,w,cx|{if !this.git_history.active {this.navigate_history(true,w,cx)}}))
            .on_action(cx.listener(|this,_:&Forward,w,cx|{if !this.git_history.active {this.navigate_history(false,w,cx)}}))
            .on_action(cx.listener(|this,_:&FindFile,w,cx|this.focus_search(false,w,cx)))
            .on_action(cx.listener(|this,_:&FindText,w,cx|this.focus_search(true,w,cx)))
            .on_action(cx.listener(|this,_:&FindReferences,w,cx|{if !w.has_active_dialog(cx) {this.request_navigation(NavigationKind::References,None,w,cx)}}))
            .on_action(cx.listener(|this,_:&FindDefinition,w,cx|{if !w.has_active_dialog(cx) {this.request_navigation(NavigationKind::Definition,None,w,cx)}}))
            .on_action(cx.listener(|this,_:&ToggleCode,w,cx|{if this.git_history.active {this.toggle_history_code(w,cx)}else{let code=!this.session().map(|s|s.saved.code).unwrap_or(false);this.select_reader_mode(code,w,cx)}}))
            .child(self.render_toolbar(window, cx))
            .children(self.render_git_activity(cx))
            .children(self.render_repository_result(cx))
            .children(self.render_workflow_status(cx))
            .children(self.branch_review_controls(cx))
            .children(self.update_banner(cx))
            .children(self.code_drafts_banner(cx))
            .children(self.reviewed_files_notice(cx))
            .when(self.opening_path.is_none() && self.auto_refresh.show_manual_notice(self.external_change),|d|d.child(div().h(ui_px(32.)).flex_none().px_3().flex().items_center().gap_3().bg(palette.rgb(0x3b3326)).text_color(palette.rgb(0xe4c28c)).text_size(ui_text(12.))
                .child("새 변경사항이 있습니다. 읽던 위치를 유지한 채 갱신할 수 있습니다.")
                .child(Button::new("apply-refresh").label("갱신").ghost().small().on_click(cx.listener(|this,_,w,cx|this.refresh(w,cx))))))
            .when(self.trust_prompt,|d|d.child(div().p_3().v_flex().gap_2().bg(palette.rgb(0x293848)).border_b_1().border_color(palette.rgb(LINE))
                .child(div().font_weight(FontWeight::MEDIUM).child("이 워크트리에서 코드 분석을 시작할까요?"))
                .child(div().text_size(ui_text(12.)).text_color(palette.rgb(MUTED)).child("언어 서버가 Gradle/Maven 설정을 읽거나 실행할 수 있습니다. 신뢰하는 프로젝트에서만 시작하세요. 파일과 diff는 분석 없이도 읽을 수 있습니다."))
                .child(div().flex().gap_2()
                    .child(Button::new("trust-start").label("신뢰하고 분석 허용").small().on_click(cx.listener(|this,_,_,cx|this.enable_analysis(cx))))
                    .child(Button::new("trust-cancel").label("파일만 보기").ghost().small().on_click(cx.listener(|this,_,_,cx|{this.trust_prompt=false;cx.notify();}))))))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .when(self.error.is_some(), |d| d.child(self.render_error_summary(cx)))
            .when(self.analysis_feedback.output_open, |d| d.child(self.render_output(cx)))
            .when(self.results_open && !self.analysis_feedback.output_open,|d|d.child(self.render_results(cx)))
            .child(div().h(ui_px(27.)).flex_none().px_3().flex().items_center().gap_4().border_t_1().border_color(palette.rgb(LINE)).bg(palette.rgb(SIDE)).text_size(ui_text(11.)).text_color(palette.rgb(MUTED))
                .child(div().flex_1().min_w_0().text_ellipsis().child(context))
                .when(self.current.is_some(), |row| row.child(self.git_sync_button(cx)))
                .child(self.output_button("status-output", cx))
                .child(div().max_w(ui_px(400.)).min_w_0().text_ellipsis().child(self.git_running_status().unwrap_or_else(|| self.status.clone())))
                .child(if self.current.is_none() { "⌘O 프로젝트 열기" } else if self.git_history.active { "⌘⌥H Git 그래프  /  ⌘P 파일 찾기" } else { "⌘클릭 정의  /  ⌥F7 사용처" }))
    }
}

// A nested independent repository may also be inside another project's worktree.
fn project_index_for_path(projects: &[SavedProject], path: &Path) -> Option<usize> {
    projects
        .iter()
        .enumerate()
        .flat_map(|(index, project)| {
            project
                .worktrees
                .iter()
                .chain(std::iter::once(&project.root))
                .filter(|root| path.starts_with(root))
                .map(move |root| (root.components().count(), index))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, index)| index)
}

fn language(path: &Path) -> &'static str {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return "text";
    };
    let name = name.to_ascii_lowercase();
    if name == "dockerfile"
        || name.starts_with("dockerfile.")
        || name == "containerfile"
        || name.starts_with("containerfile.")
    {
        return "dockerfile";
    }
    if name == ".env" || name.starts_with(".env.") {
        return "dotenv";
    }
    match name.as_str() {
        "gradlew" | "mvnw" | ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" => return "bash",
        "makefile" | "gnumakefile" => return "make",
        "cargo.lock" => return "toml",
        "gemfile" | "rakefile" => return "ruby",
        ".editorconfig" => return "ini",
        ".gitignore" | ".ignore" => return "gitignore",
        _ => {}
    }
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("rs") => "rust",
        Some("java") => "java",
        Some("kt" | "kts" | "ktm") => "kotlin",
        Some("gradle" | "groovy") => "groovy",
        Some("properties") => "properties",
        Some("js" | "mjs" | "cjs") => "javascript",
        // The bundled Syntect set provides JSX markup via TypeScriptReact.
        Some("jsx" | "tsx") => "tsx",
        Some("ts" | "mts" | "cts") => "typescript",
        Some("html" | "htm") => "html",
        Some("vue") => "vue",
        Some("svelte") => "svelte",
        Some("css") => "css",
        Some("scss") => "scss",
        Some("json" | "jsonc") => "json",
        Some("yml" | "yaml") => "yaml",
        Some("toml") => "toml",
        Some("md" | "mdx") => "markdown",
        Some("sh" | "bash" | "zsh") => "bash",
        Some("bat" | "cmd") => "bat",
        Some("xml" | "xhtml" | "svg") => "xml",
        Some("sql") => "sql",
        Some("py" | "pyi") => "python",
        Some("h") => "c",
        Some("txt" | "log") => "text",
        _ => syntax::embedded_language(path).unwrap_or("text"),
    }
}
fn file_status(status: &FileStatus) -> (&'static str, u32) {
    match status {
        FileStatus::Added => ("A", GREEN),
        FileStatus::Deleted => ("D", RED),
        FileStatus::Renamed => ("R", BLUE),
        FileStatus::Untracked => ("?", GREEN),
        FileStatus::Conflicted => ("!", 0xe4bb79),
        FileStatus::Modified => ("M", 0xe4bb79),
    }
}

#[cfg(test)]
mod project_selection_tests {
    use super::{SavedProject, project_index_for_path};
    use std::path::Path;

    #[test]
    fn pending_paths_select_their_nested_project_or_linked_worktree() {
        let projects = [
            SavedProject {
                root: "/top".into(),
                worktrees: vec!["/top".into(), "/linked".into()],
                ..Default::default()
            },
            SavedProject {
                root: "/top/nested".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            project_index_for_path(&projects, Path::new("/top/nested/src")),
            Some(1)
        );
        assert_eq!(
            project_index_for_path(&projects, Path::new("/linked/src")),
            Some(0)
        );
        assert_eq!(
            project_index_for_path(&projects, Path::new("/outside")),
            None
        );
    }
}

#[cfg(test)]
mod language_tests {
    use super::language;
    use std::path::Path;

    #[test]
    fn code_views_share_file_names_aliases_and_embedded_grammar_fallback() {
        for (path, expected) in [
            ("src/한글/Main.KT", "kotlin"),
            ("settings.gradle.kts", "kotlin"),
            ("app/build.gradle", "groovy"),
            ("gradlew", "bash"),
            ("gradle/wrapper/gradle-wrapper.properties", "properties"),
            ("mvnw", "bash"),
            ("pom.xml", "xml"),
            ("src/Widget.jsx", "tsx"),
            ("src/example.d.mts", "typescript"),
            ("src/main.rs", "rust"),
            ("Cargo.lock", "toml"),
            ("Containerfile.dev", "dockerfile"),
            (".env.local", "dotenv"),
            ("main.go", "Go"),
            ("README.txt", "text"),
            ("events.log", "text"),
            ("artifact.unknown-pokarak", "text"),
            ("", "text"),
        ] {
            assert_eq!(language(Path::new(path)), expected, "{path}");
        }
    }
}
