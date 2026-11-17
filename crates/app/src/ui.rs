mod analysis_feedback;
mod file_icons;
mod file_inspector;
mod git_actions;
mod git_commit;
mod git_history;
mod git_workflow;
mod reader_navigation;
mod split_diff;
mod toolbar;
use analysis_feedback::AnalysisFeedback;
use split_diff::ReviewDiff;

use crate::prefs::{Preferences, SavedPosition, SavedProject, SavedView};
use crate::theme::{self, Palette, ThemeChoice};
use git_actions::integration::IntegrationKind;
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::{
    Disableable, Icon, Selectable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Editor, EditorState, Input, InputEvent, InputState, Position},
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
        ToggleCode
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.on_action(git_actions::request_app_quit);
    cx.set_menus([
        Menu::new("Pokarak").items([
            MenuItem::action("설정…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Pokarak 종료", Quit),
        ]),
        Menu::new("파일").items([
            MenuItem::action("저장소 열기…", OpenRepository),
            #[cfg(target_os = "macos")]
            MenuItem::action("워크트리 터미널 열기", OpenWorktreeTerminal),
            MenuItem::action("새로고침", Refresh),
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
        KeyBinding::new("cmd-[", Back, None),
        KeyBinding::new("cmd-]", Forward, None),
        KeyBinding::new("cmd-p", FindFile, None),
        KeyBinding::new("cmd-shift-f", FindText, None),
        KeyBinding::new("alt-f7", FindReferences, None),
        KeyBinding::new("cmd-b", FindDefinition, None),
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
    snapshot: Option<Snapshot>,
    saved: SavedView,
    documents: HashMap<PathBuf, Entity<EditorState>>,
    diffs: HashMap<PathBuf, ReviewDiff>,
    scroll: UniformListScrollHandle,
    history: Vec<ReadingPoint>,
    forward: Vec<ReadingPoint>,
    collapsed: HashSet<PathBuf>,
    restoring: bool,
}
impl Session {
    fn new(saved: SavedView) -> Self {
        let restoring = saved.selected.is_some();
        Self {
            snapshot: None,
            saved,
            documents: HashMap::new(),
            diffs: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            history: vec![],
            forward: vec![],
            collapsed: HashSet::new(),
            restoring,
        }
    }
}

#[derive(Clone)]
struct FileRow {
    path: PathBuf,
    label: String,
    depth: usize,
    directory: bool,
    change: Option<ChangedFile>,
}

pub struct Pokarak {
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
    sessions: HashMap<PathBuf, Session>,
    search_input: Entity<InputState>,
    base_input: Entity<InputState>,
    filter: String,
    text_search: bool,
    loading: bool,
    file_loading: bool,
    request: u64,
    file_request: u64,
    navigation_request: u64,
    git_ui: git_actions::GitUi,
    commit_ui: git_commit::CommitUi,
    error: Option<String>,
    analysis_feedback: AnalysisFeedback,
    status: String,
    results: Vec<SourceLocation>,
    result_revisions: HashMap<PathBuf, FileRevision>,
    pending_location_revision: Option<FileRevision>,
    result_title: String,
    result_semantic: bool,
    results_open: bool,
    result_scroll: UniformListScrollHandle,
    file_scroll: UniformListScrollHandle,
    file_cursor: usize,
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
    _subscriptions: Vec<Subscription>,
}

impl Pokarak {
    fn set_theme(&mut self, choice: ThemeChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.theme = choice;
        self.prefs.save();
        theme::apply(choice, self.prefs.code_font_size, window, cx);
        cx.notify();
    }

    fn set_code_size(&mut self, value: f32, cx: &mut Context<Self>) {
        self.prefs.code_font_size = value.clamp(10., 24.);
        self.prefs.save();
        gpui_kit::component::Theme::update(cx, |theme| {
            theme.mono_font_size = px(self.prefs.code_font_size)
        });
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigation_request += 1;
        self.navigating = false;
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (choice, font_size) = weak
                .upgrade()
                .map(|view| {
                    let app = view.read(cx);
                    (app.prefs.theme, app.prefs.code_font_size)
                })
                .unwrap_or((ThemeChoice::Dark, 13.));
            let mut choices = div().flex().gap_2();
            for (index, (value, label, icon)) in [
                (ThemeChoice::System, "시스템", IconName::Monitor),
                (ThemeChoice::Light, "라이트", IconName::Sun),
                (ThemeChoice::Dark, "다크", IconName::Moon),
            ]
            .into_iter()
            .enumerate()
            {
                let target = weak.clone();
                choices = choices.child(
                    Button::new(("theme", index))
                        .label(label)
                        .icon(icon)
                        .selected(choice == value)
                        .on_click(move |_, window, cx| {
                            let _ = target.update(cx, |app, cx| app.set_theme(value, window, cx));
                        }),
                );
            }
            let palette = Palette::new(cx);
            let smaller = weak.clone();
            let larger = weak.clone();
            dialog.title("설정").width(px(430.)).child(
                div()
                    .v_flex()
                    .gap_5()
                    .child(
                        div()
                            .v_flex()
                            .gap_2()
                            .child(div().font_weight(FontWeight::MEDIUM).child("테마"))
                            .child(choices),
                    )
                    .child(
                        div()
                            .v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("코드·Diff 글자 크기"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        Button::new("font-smaller")
                                            .icon(IconName::Minus)
                                            .disabled(font_size <= 10.)
                                            .accessibility_label("코드 글자 작게")
                                            .tooltip("작게")
                                            .on_click(move |_, _, cx| {
                                                let _ = smaller.update(cx, |app, cx| {
                                                    app.set_code_size(
                                                        app.prefs.code_font_size - 1.,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    )
                                    .child(format!("{} pt", font_size as u32))
                                    .child(
                                        Button::new("font-larger")
                                            .icon(IconName::Plus)
                                            .disabled(font_size >= 24.)
                                            .accessibility_label("코드 글자 크게")
                                            .tooltip("크게")
                                            .on_click(move |_, _, cx| {
                                                let _ = larger.update(cx, |app, cx| {
                                                    app.set_code_size(
                                                        app.prefs.code_font_size + 1.,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(palette.rgb(MUTED))
                                    .child("파일 본문, 변경 비교, Blame의 코드에 적용됩니다."),
                            )
                            .child(
                                div()
                                    .id("code-font-preview")
                                    .v_flex()
                                    .rounded_md()
                                    .overflow_hidden()
                                    .font_family("JetBrains Mono")
                                    .text_size(px(font_size))
                                    .line_height(theme::code_row_height(px(font_size)))
                                    .child(
                                        div()
                                            .px_2()
                                            .bg(palette.rgb(0x3c2a30))
                                            .text_color(palette.rgb(RED))
                                            .child("− val count = 1"),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .bg(palette.rgb(0x24382f))
                                            .text_color(palette.rgb(GREEN))
                                            .child("+ val count = 2"),
                                    ),
                            ),
                    )
                    .child(
                        div().flex().justify_end().child(
                            Button::new("settings-close")
                                .label("닫기")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                    ),
            )
        });
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
            if let Some(root) = &self.current {
                let collapsed = &mut self.sessions.get_mut(root).unwrap().collapsed;
                if !collapsed.remove(&row.path) {
                    collapsed.insert(row.path);
                }
            }
            cx.notify();
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
                    if let Some(hit) = self.results.get(self.result_cursor).cloned() {
                        self.open_location(hit, window, cx);
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
        self.results.clear();
        self.error = None;
        self.status = "프로젝트를 열어 시작하세요".into();
        self.filter.clear();
        self.text_search = false;
    }

    fn close_all_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.persist(cx);
        self.reset_active(cx);
        self.projects.clear();
        self.sessions.clear();
        self.persist(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn new(initial: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut prefs = Preferences::load();
        if !prefs.code_font_size.is_finite() || !(10. ..=24.).contains(&prefs.code_font_size) {
            prefs.code_font_size = 13.;
        }
        let projects = prefs.open_projects.clone();
        let mut pending_projects: VecDeque<_> = initial.into_iter().collect();
        let initial = pending_projects
            .pop_front()
            .or_else(|| prefs.last_root.clone());
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("파일 이름 검색   ⌘P"));
        let base_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("main")
                .placeholder("기준 브랜치")
        });
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
        subscriptions.push(cx.subscribe_in(
            &base_input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.compare(true, window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        ));
        subscriptions.push(cx.observe_window_appearance(window, |this, window, cx| {
            if this.prefs.theme == ThemeChoice::System {
                theme::apply(this.prefs.theme, this.prefs.code_font_size, window, cx);
                cx.notify();
            }
        }));
        let mut this = Self {
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
            base_input,
            filter: String::new(),
            text_search: false,
            loading: false,
            file_loading: false,
            request: 0,
            file_request: 0,
            navigation_request: 0,
            git_ui: git_actions::GitUi::default(),
            commit_ui: git_commit::CommitUi::default(),
            error: None,
            analysis_feedback: AnalysisFeedback::default(),
            status: "저장소를 열어 시작하세요".into(),
            results: vec![],
            result_revisions: HashMap::new(),
            pending_location_revision: None,
            result_title: String::new(),
            result_semantic: false,
            results_open: false,
            result_scroll: UniformListScrollHandle::new(),
            file_scroll: UniformListScrollHandle::new(),
            file_cursor: 0,
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
            _subscriptions: subscriptions,
        };
        this.reset_git_history();
        this.install_git_lifecycle(window, cx);
        cx.on_release(|this, cx| {
            this.cancel_git_job();
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
            std::future::ready(())
        })
        .detach();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |view, cx| {
            loop {
                executor.timer(Duration::from_millis(800)).await;
                if view
                    .update(cx, |this, cx| {
                        if !this.loading
                            && !this.checking_changes
                            && !this.count_pending
                            && this.changed.swap(false, Ordering::Relaxed)
                        {
                            this.check_changes(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        theme::apply(this.prefs.theme, this.prefs.code_font_size, window, cx);
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
            .unwrap_or_default();
        let scroll = session
            .documents
            .get(&path)
            .map(|doc| doc.read(cx).scroll_offset())
            .unwrap_or_default();
        Some(ReadingPoint {
            scroll_x: scroll.x.into(),
            scroll_y: scroll.y.into(),
            path,
            code: session.saved.code,
            row: scroll_row(&session.scroll),
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
                saved.diff_row = scroll_row(&session.scroll);
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
                            this.open_git_history(window, cx);
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

    fn watch(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.watcher = None;
        self.changed = Arc::new(AtomicBool::new(false));
        let scope = self.changed.clone();
        let changed = scope.clone();
        let task = cx.background_spawn(async move {
            let objects = root.join(".git/objects");
            let mut watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event
                        && !event.kind.is_access()
                        && (event.need_rescan()
                            || event.paths.iter().any(|path| !path.starts_with(&objects)))
                    {
                        changed.store(true, Ordering::Relaxed);
                    }
                })
                .ok()?;
            watcher.watch(&root, RecursiveMode::Recursive).ok()?;
            Some(watcher)
        });
        cx.spawn(async move |view, cx| {
            let watcher = task.await;
            let _ = view.update(cx, |app, _| {
                if Arc::ptr_eq(&app.changed, &scope) {
                    app.watcher = watcher;
                }
            });
        })
        .detach();
    }

    fn check_graph_changes(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        if self.count_pending {
            return;
        }
        let generation = self.request;
        self.count_pending = true;
        let target = root.clone();
        let task =
            cx.background_spawn(async move { pokarak_core::worktree_change_summary(&target) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |app, cx| {
                if app.request == generation && app.current.as_ref() == Some(&root) {
                    app.count_pending = false;
                    if let Ok((count, fingerprint)) = result {
                        if let Some(previous) = &app.graph_fingerprint {
                            app.external_change |= previous != &fingerprint;
                        } else {
                            app.graph_fingerprint = Some(fingerprint);
                            app.change_count = Some(count);
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn check_changes(&mut self, cx: &mut Context<Self>) {
        if self.checking_changes || self.external_change {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        if self.git_history.active {
            if self.graph_page_loaded() {
                self.check_graph_changes(cx);
            }
            return;
        }
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        let mode = snapshot.compare.clone();
        let old_fingerprint = snapshot.fingerprint.clone();
        let generation = self.request;
        let check_root = root.clone();
        self.checking_changes = true;
        let task = cx.background_spawn(async move { pokarak_core::snapshot(&check_root, &mode) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.request != generation || this.current.as_ref() != Some(&root) {
                    return;
                }
                this.checking_changes = false;
                if let Ok(snapshot) = result
                    && snapshot.fingerprint != old_fingerprint
                {
                    this.external_change = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening_path.is_some() {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        self.checking_changes = false;
        self.count_pending = false;
        if self.git_history.active {
            self.refresh_graph_inventory(root, window, cx);
            return;
        }
        let mode = self
            .session()
            .map(|s| s.saved.comparison.clone())
            .unwrap_or_default();
        self.persist(cx);
        self.request += 1;
        let token = self.request;
        self.loading = true;
        self.error = None;
        self.status = "변경사항을 읽는 중…".into();
        let task = cx.background_spawn(async move { pokarak_core::snapshot(&root, &mode) });
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
                    }
                    Err(error) => {
                        this.error = Some(format!("{error:#}"));
                        this.status = "비교 기준을 확인하세요".into();
                        if let Some(s) = this
                            .current
                            .as_ref()
                            .and_then(|root| this.sessions.get_mut(root))
                            && let Some(snapshot) = &s.snapshot
                        {
                            s.saved.comparison = snapshot.compare.clone();
                        }
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
                        app.change_count = None;
                        app.graph_fingerprint = None;
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
                    }
                    Err(error) => {
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
        self.inventory = snapshot.worktrees.clone();
        self.change_count = Some(snapshot.worktree.changed_files);
        self.graph_fingerprint = None;
        self.count_pending = false;
        let root = self.current.clone().unwrap();
        let session = self.sessions.get_mut(&root).unwrap();
        let selection = session
            .saved
            .selected
            .clone()
            .filter(|p| {
                p.is_absolute()
                    || snapshot.files.contains(p)
                    || snapshot.changes.iter().any(|f| &f.path == p)
            })
            .or_else(|| snapshot.changes.first().map(|f| f.path.clone()));
        session.saved.comparison = snapshot.compare.clone();
        if let CompareMode::Branch(base) = &snapshot.compare {
            self.base_input
                .update(cx, |input, cx| input.set_value(base.clone(), window, cx));
        }
        session.saved.selected = selection.clone();
        session.saved.tabs.retain(|path| {
            if path.is_absolute() {
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
        self.status = "준비됨".into();
        if let Some(path) = selection.filter(|_| !self.git_history.active) {
            self.open_file(path, false, None, window, cx);
        }
        self.sync_git_history(window, cx);
        self.restore_git_completion(cx);
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
        let task = cx.background_spawn(async move {
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
            let text = if let Some(diff) = &diff {
                if diff.binary {
                    String::new()
                } else if diff.new_text.is_empty()
                    && matches!(file.as_ref().map(|f| &f.status), Some(FileStatus::Deleted))
                {
                    diff.old_text.clone()
                } else {
                    diff.new_text.clone()
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
            anyhow::Ok((text, diff))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.current.as_ref() != Some(&root) || this.file_request != token {
                    return;
                }
                this.file_loading = false;
                match result {
                    Ok((text, diff)) => {
                        let session = this.sessions.get_mut(&root).unwrap();
                        if let Some(diff) = diff {
                            session.diffs.insert(path.clone(), diff.into());
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
                            editor.set_readonly(true, cx);
                            if editor.value().as_ref() != text {
                                editor.set_value(text, window, cx);
                            }
                            if let Some(point) = &history_restore {
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
                        if !session.saved.code {
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
                                    }
                                });
                            });
                        });
                    }
                    Err(error) => {
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

    fn compare(&mut self, branch: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_git_graph();
        let Some(root) = &self.current else {
            return;
        };
        let mode = if branch {
            let base = self.base_input.read(cx).value().to_string();
            if base.trim().is_empty() {
                self.error = Some("기준 브랜치를 입력하세요. 예: main 또는 origin/main".into());
                cx.notify();
                return;
            }
            CompareMode::Branch(base.trim().into())
        } else {
            CompareMode::Uncommitted
        };
        self.sessions.get_mut(root).unwrap().saved.comparison = mode;
        self.refresh(window, cx);
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
        if self.navigating || self.git_history.active || window.has_active_dialog(cx) {
            return;
        }
        let (Some(root), Some(path)) = (self.current.clone(), self.selected()) else {
            return;
        };
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
        self.results = result.locations;
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

    fn file_rows(&self) -> Vec<FileRow> {
        let Some(session) = self.session() else {
            return vec![];
        };
        let Some(snapshot) = &session.snapshot else {
            return vec![];
        };
        let query = if self.text_search {
            String::new()
        } else {
            self.filter.to_lowercase()
        };
        let paths = if session.saved.all_files {
            snapshot.files.clone()
        } else {
            snapshot.changes.iter().map(|f| f.path.clone()).collect()
        };
        let mut rows = vec![];
        let mut folders = HashSet::new();
        for path in paths {
            if !query.is_empty() && !path.to_string_lossy().to_lowercase().contains(&query) {
                continue;
            }
            if session.saved.all_files && query.is_empty() {
                let components: Vec<_> = path.components().collect();
                let mut dir = PathBuf::new();
                let mut hidden = false;
                for (depth, part) in components
                    .iter()
                    .take(components.len().saturating_sub(1))
                    .enumerate()
                {
                    dir.push(part.as_os_str());
                    if folders.insert(dir.clone()) && !hidden {
                        rows.push(FileRow {
                            path: dir.clone(),
                            label: part.as_os_str().to_string_lossy().into(),
                            depth,
                            directory: true,
                            change: None,
                        });
                    }
                    if session.collapsed.contains(&dir) {
                        hidden = true;
                    }
                }
                if hidden {
                    continue;
                }
            }
            let change = snapshot.changes.iter().find(|f| f.path == path).cloned();
            rows.push(FileRow {
                label: if session.saved.all_files && query.is_empty() {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into()
                } else {
                    path.to_string_lossy().into()
                },
                depth: if session.saved.all_files && query.is_empty() {
                    path.components().count().saturating_sub(1)
                } else {
                    0
                },
                path,
                directory: false,
                change,
            });
        }
        rows
    }

    fn render_sidebar_header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(44.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("sidebar-worktrees")
                    .label("워크트리")
                    .ghost()
                    .small()
                    .text_size(px(12.))
                    .selected(!self.prefs.show_branches)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.prefs.show_branches = false;
                        this.prefs.save();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("sidebar-branches")
                    .label("브랜치")
                    .ghost()
                    .small()
                    .text_size(px(12.))
                    .selected(self.prefs.show_branches)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.prefs.show_branches = true;
                        this.prefs.save();
                        cx.notify();
                    })),
            )
            .when(!self.prefs.show_branches, |row| {
                row.child(div().flex_1()).child(
                    Button::new("manage-worktrees")
                        .icon(IconName::Settings)
                        .small()
                        .ghost()
                        .tooltip("워크트리 생성·정리")
                        .accessibility_label("워크트리 관리")
                        .disabled(self.git_is_busy() || self.loading)
                        .on_click(cx.listener(|app, _, w, cx| app.open_worktree_manager(w, cx))),
                )
            })
            .into_any_element()
    }

    fn render_branches(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let branches = Arc::new(
            self.snapshot()
                .map(|s| s.branches.clone())
                .unwrap_or_default(),
        );
        let current = self
            .snapshot()
            .map(|s| s.worktree.branch.clone())
            .unwrap_or_default();
        let base = match self.snapshot().map(|s| &s.compare) {
            Some(CompareMode::Branch(base)) => Some(base.clone()),
            _ => None,
        };
        let root = self.current.clone();
        let weak = cx.entity().downgrade();
        div()
            .size_full()
            .v_flex()
            .bg(palette.rgb(SIDE))
            .overflow_hidden()
            .child(self.render_sidebar_header(cx))
            .child(if branches.is_empty() {
                div()
                    .p_3()
                    .text_color(palette.rgb(MUTED))
                    .child("아직 커밋된 브랜치가 없습니다.")
                    .into_any_element()
            } else {
                uniform_list("branches", branches.len(), move |range, _, _| {
                    range
                        .map(|index| {
                            let name = branches[index].clone();
                            let target = weak.clone();
                            let scope = root.clone();
                            let selected = base.as_ref() == Some(&name);
                            let tooltip = format!("{name}을 기준으로 현재 워크트리와 비교");
                            let branch = name.clone();
                            let activate = move |window: &mut Window, cx: &mut App| {
                                let _ = target.update(cx, |this, cx| {
                                    if this.current != scope {
                                        return;
                                    }
                                    this.base_input.update(cx, |input, cx| {
                                        input.set_value(branch.clone(), window, cx)
                                    });
                                    this.compare(true, window, cx);
                                });
                            };
                            let menu_target = weak.clone();
                            let menu_scope = root.clone();
                            let menu_branch = name.clone();
                            let can_integrate = name != current;
                            let a11y_activate = activate.clone();
                            let key_activate = activate.clone();
                            div()
                                .h(px(32.))
                                .w_full()
                                .px_2()
                                .child(
                                    div()
                                        .id(("branch", index))
                                        .size_full()
                                        .role(accesskit::Role::Button)
                                        .aria_label(tooltip.clone())
                                        .aria_selected(selected)
                                        .tab_index(0)
                                        .on_a11y_action(
                                            accesskit::Action::Click,
                                            move |_, w, cx| a11y_activate(w, cx),
                                        )
                                        .on_key_down(move |event, window, cx| {
                                            if !event.keystroke.modifiers.modified()
                                                && matches!(
                                                    event.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                )
                                            {
                                                window.prevent_default();
                                                cx.stop_propagation();
                                                if !event.is_held {
                                                    key_activate(window, cx);
                                                }
                                            }
                                        })
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .rounded(px(5.))
                                        .cursor_pointer()
                                        .bg(if selected {
                                            palette.rgb(0x2b3947)
                                        } else {
                                            palette.rgb(SIDE)
                                        })
                                        .hover(|s| {
                                            s.bg(if selected {
                                                palette.rgb(0x2b3947)
                                            } else {
                                                palette.rgb(PANEL)
                                            })
                                        })
                                        .focus_visible(|s| {
                                            s.border_1().border_color(palette.rgb(BLUE))
                                        })
                                        .tooltip(move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                tooltip.clone(),
                                            )
                                            .build(window, cx)
                                        })
                                        .on_click(move |_, window, cx| activate(window, cx))
                                        .child(
                                            Icon::new(IconName::GitBranch)
                                                .size(px(15.))
                                                .flex_none()
                                                .text_color(if selected {
                                                    palette.rgb(BLUE)
                                                } else {
                                                    palette.rgb(MUTED)
                                                }),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .child(name.clone()),
                                        )
                                        .when(name == current, |row| {
                                            row.child(
                                                div()
                                                    .flex_none()
                                                    .text_size(px(10.))
                                                    .text_color(palette.rgb(MUTED))
                                                    .child("현재"),
                                            )
                                        })
                                        .context_menu(move |mut menu, _, _| {
                                            if can_integrate {
                                                for (kind, label) in [
                                                    (
                                                        IntegrationKind::Merge,
                                                        "현재 브랜치에 Merge…",
                                                    ),
                                                    (
                                                        IntegrationKind::Rebase,
                                                        "이 브랜치 위로 Rebase…",
                                                    ),
                                                ] {
                                                    let target = menu_target.clone();
                                                    let scope = menu_scope.clone();
                                                    let branch = menu_branch.clone();
                                                    menu = menu.item(
                                                        PopupMenuItem::new(label).on_click(
                                                            move |_, w, cx| {
                                                                let _ =
                                                                    target.update(cx, |app, cx| {
                                                                        if app.current == scope {
                                                                            app.open_integration(
                                                                                kind,
                                                                                Some(
                                                                                    branch.clone(),
                                                                                ),
                                                                                w,
                                                                                cx,
                                                                            );
                                                                        }
                                                                    });
                                                            },
                                                        ),
                                                    );
                                                }
                                            }
                                            menu
                                        }),
                                )
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                })
                .flex_1()
                .h_full()
                .into_any_element()
            })
            .into_any_element()
    }

    fn render_worktrees(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.git_history.active {
            return self.render_graph_sidebar(cx);
        }
        if self.prefs.show_branches {
            return self.render_branches(cx);
        }
        let palette = Palette::new(cx);
        let worktrees = self
            .snapshot()
            .map(|s| s.worktrees.clone())
            .unwrap_or_default();
        let mut panel = div()
            .size_full()
            .v_flex()
            .bg(palette.rgb(SIDE))
            .overflow_hidden()
            .child(self.render_sidebar_header(cx));
        let mut list = div()
            .id("worktree-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2();
        for (index, tree) in worktrees.into_iter().enumerate() {
            let active = self.current.as_ref() == Some(&tree.path);
            let path = tree.path.clone();
            let a11y_path = path.clone();
            let a11y_target = cx.entity().downgrade();
            let missing = tree.is_prunable || !path.exists();
            let branch = if tree.is_detached {
                format!("{} (detached)", &tree.head[..tree.head.len().min(8)])
            } else {
                tree.branch.clone()
            };
            list = list.child(
                div()
                    .id(("worktree", index))
                    .w_full()
                    .role(accesskit::Role::Button)
                    .aria_label(format!("워크트리 {}", branch))
                    .on_a11y_action(accesskit::Action::Click, move |_, window, cx| {
                        let _ = a11y_target.update(cx, |this, cx| {
                            this.open_repository(a11y_path.clone(), window, cx)
                        });
                    })
                    .px_2()
                    .py_2()
                    .mb_1()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .bg(if active {
                        palette.rgb(0x2b3947)
                    } else {
                        palette.rgb(SIDE)
                    })
                    .hover(|s| s.bg(palette.rgb(PANEL)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !missing {
                            this.open_repository(path.clone(), window, cx);
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::GitBranch).size(px(15.)).text_color(
                                if active {
                                    palette.rgb(BLUE)
                                } else {
                                    palette.rgb(MUTED)
                                },
                            ))
                            .child(div().flex_1().text_ellipsis().child(branch))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.rgb(MUTED))
                                    .child(tree.changed_files.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .pl(px(23.))
                            .text_size(px(11.))
                            .text_color(palette.rgb(MUTED))
                            .text_ellipsis()
                            .child(
                                tree.path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            ),
                    )
                    .when(tree.is_locked || missing, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(palette.rgb(0xe4bb79))
                                .child(if missing {
                                    "경로가 없습니다"
                                } else {
                                    "잠긴 워크트리"
                                }),
                        )
                    }),
            );
        }
        panel = panel.child(list);
        panel.into_any_element()
    }

    fn render_files(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let all = self.session().map(|s| s.saved.all_files).unwrap_or(false);
        let selected = self.selected();
        let keyboard_index = if self.search_input.focus_handle(cx).is_focused(window) {
            Some(self.file_cursor)
        } else {
            None
        };
        let rows = Arc::new(self.file_rows());
        let count = rows.iter().filter(|r| !r.directory).count();
        let weak = cx.entity().downgrade();
        div()
            .size_full()
            .v_flex()
            .bg(palette.rgb(BG))
            .overflow_hidden()
            .child(
                div()
                    .h(px(44.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new("changes")
                            .label("변경 파일")
                            .ghost()
                            .small()
                            .selected(!all)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(root) = &this.current {
                                    this.sessions.get_mut(root).unwrap().saved.all_files = false;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("all-files")
                            .label("전체 파일")
                            .ghost()
                            .small()
                            .selected(all)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(root) = &this.current {
                                    this.sessions.get_mut(root).unwrap().saved.all_files = true;
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette.rgb(MUTED))
                            .child(count.to_string()),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Input::new(&self.search_input)
                            .small()
                            .prefix(IconName::Search),
                    )
                    .child(
                        Button::new("text-search")
                            .icon(IconName::TextSearch)
                            .ghost()
                            .small()
                            .tooltip("본문 검색  ⌘⇧F")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.focus_search(true, window, cx)
                            })),
                    ),
            )
            .child(if count == 0 {
                div()
                    .p_4()
                    .text_color(palette.rgb(MUTED))
                    .child(if self.filter.is_empty() {
                        "변경된 파일이 없습니다. 전체 파일에서 코드를 읽을 수 있습니다."
                    } else {
                        "일치하는 파일이 없습니다."
                    })
                    .into_any_element()
            } else {
                uniform_list("files", rows.len(), move |range, _, cx| {
                    range
                        .map(|ix| {
                            let row = rows[ix].clone();
                            let target = weak.clone();
                            let path = row.path.clone();
                            let a11y_target = weak.clone();
                            let a11y_path = path.clone();
                            let directory = row.directory;
                            let selected = if let Some(index) = keyboard_index {
                                ix == index
                            } else {
                                selected.as_ref() == Some(&path)
                            };
                            let status = row.change.as_ref().map(|c| file_status(&c.status));
                            div()
                                .id(("file", ix))
                                .w_full()
                                .role(accesskit::Role::Button)
                                .aria_label(row.path.to_string_lossy().to_string())
                                .on_a11y_action(accesskit::Action::Click, move |_, window, cx| {
                                    let _ = a11y_target.update(cx, |this, cx| {
                                        if directory {
                                            if let Some(root) = &this.current {
                                                let collapsed = &mut this
                                                    .sessions
                                                    .get_mut(root)
                                                    .unwrap()
                                                    .collapsed;
                                                if !collapsed.remove(&a11y_path) {
                                                    collapsed.insert(a11y_path.clone());
                                                }
                                            }
                                            cx.notify();
                                        } else {
                                            this.open_file(
                                                a11y_path.clone(),
                                                true,
                                                None,
                                                window,
                                                cx,
                                            );
                                        }
                                    });
                                })
                                .h(px(29.))
                                .px_2()
                                .pl(px(10. + row.depth as f32 * 13.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .bg(if selected {
                                    palette.rgb(0x2c3a48)
                                } else {
                                    palette.rgb(BG)
                                })
                                .hover(|s| s.bg(palette.rgb(PANEL)))
                                .child(if row.directory {
                                    Icon::new(IconName::Folder)
                                        .size(px(14.))
                                        .text_color(palette.rgb(MUTED))
                                        .into_any_element()
                                } else {
                                    file_icons::icon_for_path(&row.path, cx).into_any_element()
                                })
                                .child(div().flex_1().min_w_0().text_ellipsis().child(row.label))
                                .when_some(status, |d, (label, color)| {
                                    d.child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(palette.rgb(color))
                                            .child(label),
                                    )
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = target.update(cx, |this, cx| {
                                        if row.directory {
                                            if let Some(root) = &this.current {
                                                let dirs = &mut this
                                                    .sessions
                                                    .get_mut(root)
                                                    .unwrap()
                                                    .collapsed;
                                                if !dirs.remove(&path) {
                                                    dirs.insert(path.clone());
                                                }
                                            }
                                            cx.notify();
                                        } else {
                                            this.open_file(path.clone(), true, None, window, cx);
                                        }
                                    });
                                })
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(&self.file_scroll)
                .h_full()
                .flex_1()
                .into_any_element()
            })
            .into_any_element()
    }

    fn render_comparison(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let mode = self.snapshot().map(|s| &s.compare);
        let draft = self.base_input.read(cx).value();
        let pending = matches!(mode,Some(CompareMode::Branch(base)) if base.as_str()!=draft.trim());
        div()
            .h(px(42.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(palette.rgb(LINE))
            .child(
                Button::new("uncommitted")
                    .label("미커밋")
                    .ghost()
                    .small()
                    .selected(matches!(mode, Some(CompareMode::Uncommitted)))
                    .tooltip("HEAD와 현재 파일 비교: staged + unstaged")
                    .on_click(cx.listener(|this, _, w, cx| this.compare(false, w, cx))),
            )
            .child(
                Button::new("branch-compare")
                    .label("브랜치 변경")
                    .ghost()
                    .small()
                    .selected(matches!(mode, Some(CompareMode::Branch(_))))
                    .on_click(cx.listener(|this, _, w, cx| this.compare(true, w, cx))),
            )
            .child(
                div()
                    .w(px(145.))
                    .child(Input::new(&self.base_input).small()),
            )
            .child(
                Button::new("apply-base")
                    .label(if pending { "적용 *" } else { "적용" })
                    .ghost()
                    .small()
                    .disabled(self.loading || draft.trim().is_empty())
                    .tooltip("입력한 기준 브랜치를 적용합니다. Enter로도 적용할 수 있습니다.")
                    .on_click(cx.listener(|this, _, window, cx| this.compare(true, window, cx))),
            )
            .child(
                div()
                    .max_w(px(260.))
                    .text_ellipsis()
                    .text_size(px(11.))
                    .text_color(palette.rgb(MUTED))
                    .child(match mode {
                        Some(CompareMode::Branch(base)) => {
                            format!("적용됨: {base} 공통 조상 → 현재 파일")
                        }
                        _ => "적용됨: HEAD → 현재 파일".into(),
                    }),
            )
            .child(div().flex_1())
            .child(
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
                    })),
            )
            .into_any_element()
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
        let mut tabs = div()
            .id("file-tabs")
            .flex()
            .items_center()
            .h(px(34.))
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
                            tab.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
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
            .h(px(40.))
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
                    .text_size(px(11.))
                    .child(path.to_string_lossy().to_string()),
            )
            .child(
                Button::new("diff")
                    .label("변경")
                    .ghost()
                    .small()
                    .selected(!code && !self.prefs.split_diff)
                    .disabled(diff.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_split_diff(false);
                        this.toggle_code(false, cx)
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
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.set_split_diff(true);
                        app.toggle_code(false, cx)
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
                        .disabled(!code || deleted || self.navigating)
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
                        .disabled(!code || deleted || self.navigating)
                        .on_click(cx.listener(|this, _, w, cx| {
                            this.request_navigation(NavigationKind::Implementation, None, w, cx)
                        })),
                )
            });
        let content = if diff.map(|d| d.binary).unwrap_or(false) {
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
                            if event.modifiers.platform && !deleted {
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
                            .readonly(true)
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
            diff.split(cx)
        } else if let Some(diff) = diff {
            let font_size = px(self.prefs.code_font_size);
            let line_number_width = (font_size * 3.5).max(px(45.));
            let widest = diff
                .lines
                .iter()
                .enumerate()
                .max_by_key(|(_, line)| line.text.len())
                .map(|(index, _)| index);
            let lines = Arc::new(diff.lines.clone());
            let weak = cx.entity().downgrade();
            uniform_list("diff-lines", lines.len(), move |range, _, _| {
                range
                    .map(|ix| {
                        let line = lines[ix].clone();
                        let (background, color, sign) = match line.kind {
                            DiffKind::Addition => (0x24382f, GREEN, "+"),
                            DiffKind::Deletion => (0x3c2a30, RED, "−"),
                            DiffKind::Header => (0x283744, BLUE, ""),
                            DiffKind::Context => (BG, TEXT, ""),
                        };
                        let target = weak.clone();
                        div()
                            .id(("diff-row", ix))
                            .min_w_full()
                            .h(theme::code_row_height(font_size))
                            .flex()
                            .items_center()
                            .bg(palette.rgb(background))
                            .font_family("JetBrains Mono")
                            .text_size(font_size)
                            .text_color(palette.rgb(color))
                            .child(
                                div()
                                    .w(line_number_width)
                                    .flex_none()
                                    .text_right()
                                    .pr_2()
                                    .text_color(palette.rgb(MUTED))
                                    .child(
                                        line.old_line.map(|n| n.to_string()).unwrap_or_default(),
                                    ),
                            )
                            .child(
                                div()
                                    .w(line_number_width)
                                    .flex_none()
                                    .text_right()
                                    .pr_2()
                                    .text_color(palette.rgb(MUTED))
                                    .child(
                                        line.new_line.map(|n| n.to_string()).unwrap_or_default(),
                                    ),
                            )
                            .child(div().w(px(22.)).flex_none().child(sign))
                            .child(div().whitespace_nowrap().child(if line.text.is_empty() {
                                " ".into()
                            } else {
                                line.text.clone()
                            }))
                            .on_click(move |event, window, cx| {
                                if event.click_count() == 2
                                    && let Some(line_number) = line.new_line
                                {
                                    let _ = target.update(cx, |this, cx| {
                                        if let Some(path) = this.selected() {
                                            this.open_file(
                                                path,
                                                true,
                                                Some(Position::new(
                                                    line_number.saturating_sub(1) as u32,
                                                    0,
                                                )),
                                                window,
                                                cx,
                                            );
                                        }
                                    });
                                }
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&session.scroll)
            .with_width_from_item(widest)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .size_full()
            .into_any_element()
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
            .when(deleted && code, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.))
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
                        .text_size(px(11.))
                        .text_color(palette.rgb(0xe4bb79))
                        .child("큰 파일의 일부만 표시합니다."),
                )
            })
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
            .into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let results = Arc::new(self.results.clone());
        let weak = cx.entity().downgrade();
        let root = self.current.clone().unwrap_or_default();
        let cursor = self.result_cursor;
        div()
            .id("results-panel")
            .max_h(relative(0.3))
            .min_h_0()
            .track_focus(&self.results_focus)
            .h(px(210.))
            .flex_none()
            .v_flex()
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .bg(palette.rgb(SIDE))
            .child(
                div()
                    .h(px(34.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(format!("{}  {}", self.result_title, self.results.len()))
                    .child(
                        div()
                            .text_size(px(11.))
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
                uniform_list("results", results.len(), move |range, _, _| {
                    range
                        .map(|ix| {
                            let hit = results[ix].clone();
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
                                .h(px(40.))
                                .px_3()
                                .cursor_pointer()
                                .hover(|s| s.bg(palette.rgb(PANEL)))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(palette.rgb(BLUE))
                                        .text_ellipsis()
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(11.))
                                        .text_color(palette.rgb(MUTED))
                                        .text_ellipsis()
                                        .child(hit.preview.clone()),
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
                    .size(px(52.))
                    .text_color(palette.rgb(BLUE)),
            )
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("변경된 코드를, 가볍게."),
            )
            .child(
                div()
                    .text_color(palette.rgb(MUTED))
                    .child("워크트리를 오가며 diff와 코드를 읽으세요."),
            )
            .child(
                Button::new("welcome-open")
                    .label("프로젝트 열기")
                    .icon(IconName::FolderOpen)
                    .on_click(cx.listener(|this, _, w, cx| this.choose_repository(w, cx))),
            );
        let mut recent = div()
            .id("recent-projects")
            .w(px(620.))
            .max_w(relative(0.9))
            .max_h(px(200.))
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
                    .h(px(40.))
                    .flex_none()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .text_size(px(11.))
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
                        .text_size(px(12.))
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
        } else if self.current.is_some() && self.git_history.active {
            h_resizable("graph-main-panes")
                .child(
                    resizable_panel()
                        .size(px(210.))
                        .size_range(px(160.)..px(420.))
                        .child(self.render_worktrees(cx)),
                )
                .child(resizable_panel().child(self.render_git_graph(window, cx)))
                .into_any_element()
        } else if self.current.is_some() {
            h_resizable("main-panes")
                .child(
                    resizable_panel()
                        .size(px(210.))
                        .size_range(px(160.)..px(420.))
                        .child(self.render_worktrees(cx)),
                )
                .child(
                    resizable_panel()
                        .size(px(285.))
                        .size_range(px(200.)..px(600.))
                        .child(self.render_files(window, cx)),
                )
                .child(resizable_panel().child(self.render_reader(window, cx)))
                .into_any_element()
        } else {
            self.render_welcome(cx)
        };
        let context = self
            .opening_path
            .as_ref()
            .or(self.current.as_ref())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "로컬 Git 프로젝트".into());
        let root = div().id("pokarak");
        #[cfg(target_os = "macos")]
        let root = root.on_action(
            cx.listener(|app, _: &OpenWorktreeTerminal, _, cx| app.open_worktree_terminal(cx)),
        );
        root.key_context(if self.git_history.active { "Pokarak GitGraph" } else { "Pokarak" }).track_focus(&self.focus).size_full().overflow_hidden().v_flex().bg(palette.rgb(BG)).text_color(palette.rgb(TEXT)).text_size(px(13.)).font_family("IBM Plex Sans")
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
            .on_action(cx.listener(|this,_:&Back,w,cx|{if !this.git_history.active {this.navigate_history(true,w,cx)}}))
            .on_action(cx.listener(|this,_:&Forward,w,cx|{if !this.git_history.active {this.navigate_history(false,w,cx)}}))
            .on_action(cx.listener(|this,_:&FindFile,w,cx|this.focus_search(false,w,cx)))
            .on_action(cx.listener(|this,_:&FindText,w,cx|this.focus_search(true,w,cx)))
            .on_action(cx.listener(|this,_:&FindReferences,w,cx|{if !w.has_active_dialog(cx) {this.request_navigation(NavigationKind::References,None,w,cx)}}))
            .on_action(cx.listener(|this,_:&FindDefinition,w,cx|{if !w.has_active_dialog(cx) {this.request_navigation(NavigationKind::Definition,None,w,cx)}}))
            .on_action(cx.listener(|this,_:&ToggleCode,w,cx|{if this.git_history.active {this.toggle_history_code(w,cx)}else{let code=!this.session().map(|s|s.saved.code).unwrap_or(false);this.toggle_code(code,cx)}}))
            .child(self.render_toolbar(window, cx))
            .when(self.opening_path.is_none() && self.current.is_some() && !self.git_history.active,|d|d.child(self.render_comparison(cx)))
            .when(self.opening_path.is_none() && self.external_change,|d|d.child(div().h(px(32.)).flex_none().px_3().flex().items_center().gap_3().bg(palette.rgb(0x3b3326)).text_color(palette.rgb(0xe4c28c)).text_size(px(12.))
                .child("새 변경사항이 있습니다. 읽던 위치를 유지한 채 갱신할 수 있습니다.")
                .child(Button::new("apply-refresh").label("갱신").ghost().small().on_click(cx.listener(|this,_,w,cx|this.refresh(w,cx))))))
            .when(self.trust_prompt,|d|d.child(div().p_3().v_flex().gap_2().bg(palette.rgb(0x293848)).border_b_1().border_color(palette.rgb(LINE))
                .child(div().font_weight(FontWeight::MEDIUM).child("이 워크트리에서 코드 분석을 시작할까요?"))
                .child(div().text_size(px(12.)).text_color(palette.rgb(MUTED)).child("언어 서버가 Gradle/Maven 설정을 읽거나 실행할 수 있습니다. 신뢰하는 프로젝트에서만 시작하세요. 파일과 diff는 분석 없이도 읽을 수 있습니다."))
                .child(div().flex().gap_2()
                    .child(Button::new("trust-start").label("신뢰하고 분석 허용").small().on_click(cx.listener(|this,_,_,cx|this.enable_analysis(cx))))
                    .child(Button::new("trust-cancel").label("파일만 보기").ghost().small().on_click(cx.listener(|this,_,_,cx|{this.trust_prompt=false;cx.notify();}))))))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .when(self.error.is_some(), |d| d.child(self.render_error_summary(cx)))
            .when(self.analysis_feedback.output_open, |d| d.child(self.render_output(cx)))
            .when(self.results_open && !self.analysis_feedback.output_open,|d|d.child(self.render_results(cx)))
            .child(div().h(px(27.)).flex_none().px_3().flex().items_center().gap_4().border_t_1().border_color(palette.rgb(LINE)).bg(palette.rgb(SIDE)).text_size(px(11.)).text_color(palette.rgb(MUTED))
                .child(div().flex_1().min_w_0().text_ellipsis().child(context))
                .child(self.output_button("status-output", cx))
                .child(div().max_w(px(400.)).min_w_0().text_ellipsis().child(self.status.clone()))
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
    match path.extension().and_then(|x| x.to_str()).unwrap_or("") {
        "rs" => "rust",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "ts" => "typescript",
        "tsx" => "tsx",
        "html" | "htm" => "html",
        "vue" => "vue",
        "svelte" => "svelte",
        "css" => "css",
        "scss" => "scss",
        "json" => "json",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "md" => "markdown",
        "sh" => "bash",
        "xml" => "xml",
        "sql" => "sql",
        "py" => "python",
        _ => "text",
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

fn scroll_row(handle: &UniformListScrollHandle) -> usize {
    let y: f32 = handle.0.borrow().base_handle.offset().y.into();
    (-y / 22.).max(0.).floor() as usize
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
