use crate::prefs::{Preferences, SavedView};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Icon, Selectable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Editor, EditorState, Input, InputEvent, InputState, Position},
    resizable::{h_resizable, resizable_panel},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use pokarak_core::{
    ChangedFile, CompareMode, DiffDocument, DiffKind, FileStatus, Snapshot, SourceLocation,
};
use pokarak_navigation::{NavigationKind, NavigationResult, Navigator};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
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

actions!(
    pokarak,
    [
        Quit,
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
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus([
        Menu::new("포카락").items([MenuItem::action("포카락 종료", Quit)]),
        Menu::new("파일").items([
            MenuItem::action("저장소 열기…", OpenRepository),
            MenuItem::action("새로고침", Refresh),
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
        KeyBinding::new("cmd-o", OpenRepository, None),
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
}

struct Session {
    snapshot: Option<Snapshot>,
    saved: SavedView,
    documents: HashMap<PathBuf, Entity<EditorState>>,
    diffs: HashMap<PathBuf, DiffDocument>,
    scroll: UniformListScrollHandle,
    history: Vec<ReadingPoint>,
    forward: Vec<ReadingPoint>,
    collapsed: HashSet<PathBuf>,
}
impl Session {
    fn new(saved: SavedView) -> Self {
        Self {
            snapshot: None,
            saved,
            documents: HashMap::new(),
            diffs: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            history: vec![],
            forward: vec![],
            collapsed: HashSet::new(),
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
    focus: FocusHandle,
    prefs: Preferences,
    current: Option<PathBuf>,
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
    error: Option<String>,
    status: String,
    results: Vec<SourceLocation>,
    result_title: String,
    result_semantic: bool,
    results_open: bool,
    result_scroll: UniformListScrollHandle,
    navigation: Option<Arc<Mutex<Navigator>>>,
    navigating: bool,
    trust_prompt: bool,
    watcher: Option<RecommendedWatcher>,
    changed: Arc<AtomicBool>,
    external_change: bool,
    _subscriptions: Vec<Subscription>,
}

impl Pokarak {
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = Preferences::load();
        let initial = initial.or_else(|| prefs.last_root.clone());
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("파일 이름 검색   ⌘P"));
        let base_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("main")
                .placeholder("기준 브랜치")
        });
        let subscriptions = vec![cx.subscribe_in(
            &search_input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.filter = input.read(cx).value().to_string();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } if this.text_search => {
                    this.search_contents(window, cx)
                }
                _ => {}
            },
        )];
        let mut this = Self {
            focus: cx.focus_handle(),
            prefs,
            current: None,
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
            error: None,
            status: "저장소를 열어 시작하세요".into(),
            results: vec![],
            result_title: String::new(),
            result_semantic: false,
            results_open: false,
            result_scroll: UniformListScrollHandle::new(),
            navigation: None,
            navigating: false,
            trust_prompt: false,
            watcher: None,
            changed: Arc::new(AtomicBool::new(false)),
            external_change: false,
            _subscriptions: subscriptions,
        };
        cx.on_release(|this, cx| this.persist(cx)).detach();
        cx.spawn(async move |view, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(800)).await;
                if view
                    .update(cx, |this, cx| {
                        if this.changed.swap(false, Ordering::Relaxed) && !this.loading {
                            this.external_change = true;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this.focus.focus(window, cx);
        if let Some(path) = initial {
            this.open_repository(path, window, cx);
        }
        this
    }

    fn session(&self) -> Option<&Session> {
        self.current
            .as_ref()
            .and_then(|root| self.sessions.get(root))
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
        Some(ReadingPoint {
            path,
            code: session.saved.code,
            row: scroll_row(&session.scroll),
            line: position.line,
            column: position.character,
        })
    }

    fn persist(&mut self, cx: &App) {
        for (root, session) in &mut self.sessions {
            session.saved.diff_row = scroll_row(&session.scroll);
            if let Some(doc) = session
                .saved
                .selected
                .as_ref()
                .and_then(|p| session.documents.get(p))
            {
                let editor = doc.read(cx);
                let p = editor.cursor_position();
                let offset = editor.scroll_offset();
                session.saved.line = p.line;
                session.saved.column = p.character;
                session.saved.scroll_x = offset.x.into();
                session.saved.scroll_y = offset.y.into();
            }
            self.prefs
                .worktrees
                .insert(root.clone(), session.saved.clone());
        }
        self.prefs.last_root = self.current.clone();
        self.prefs.save();
    }

    fn choose_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Git 저장소 열기".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                if let Some(path) = paths.first() {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.open_repository(path.clone(), window, cx)
                    });
                }
            }
        })
        .detach();
    }

    fn open_repository(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.persist(cx);
        self.stop_analysis(cx);
        self.request += 1;
        self.file_request += 1;
        let token = self.request;
        self.loading = true;
        self.error = None;
        self.status = "저장소를 읽는 중…".into();
        self.results_open = false;
        let saved = self.prefs.worktrees.get(&path).cloned().unwrap_or_default();
        let mode = saved.comparison;
        let task = cx.background_spawn(async move {
            let root = pokarak_core::repository_root(&path)?;
            let snapshot = pokarak_core::snapshot(&root, &mode)?;
            anyhow::Ok((root, snapshot))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.request != token {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((root, snapshot)) => {
                        this.current = Some(root.clone());
                        this.prefs.recent.retain(|p| p != &root);
                        this.prefs.recent.insert(0, root.clone());
                        this.prefs.recent.truncate(8);
                        let saved = this.prefs.worktrees.get(&root).cloned().unwrap_or_default();
                        this.sessions
                            .entry(root.clone())
                            .or_insert_with(|| Session::new(saved));
                        this.install_snapshot(snapshot, window, cx);
                        this.watch(root);
                        this.persist(cx);
                    }
                    Err(error) => {
                        this.error = Some(format!("{error:#}"));
                        this.status = "저장소를 열 수 없습니다".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn watch(&mut self, root: PathBuf) {
        self.watcher = None;
        let changed = self.changed.clone();
        if let Ok(mut watcher) =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    if !event.kind.is_access()
                        && event.paths.iter().any(|path| {
                            !path.components().any(|c| {
                                matches!(
                                    c.as_os_str().to_str(),
                                    Some("target" | "node_modules" | ".gradle" | "objects")
                                )
                            })
                        })
                    {
                        changed.store(true, Ordering::Relaxed);
                    }
                }
            })
        {
            if watcher.watch(&root, RecursiveMode::Recursive).is_ok() {
                self.watcher = Some(watcher);
            }
        }
        self.changed.store(false, Ordering::Relaxed);
        self.external_change = false;
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
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
                    Err(error) => this.error = Some(format!("{error:#}")),
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
        session.snapshot = Some(snapshot);
        session.diffs.clear();
        self.status = "준비됨".into();
        if let Some(path) = selection {
            self.open_file(path, false, None, window, cx);
        }
    }

    fn open_file(
        &mut self,
        path: PathBuf,
        history: bool,
        position: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.current.clone() else {
            return;
        };
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
            session.history.truncate(100);
            session.forward.clear();
        }
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
        let restore = session.saved.clone();
        self.file_request += 1;
        self.navigation_request += 1;
        self.navigating = false;
        let token = self.file_request;
        self.file_loading = true;
        self.error = None;
        let load_root = root.clone();
        let load_path = path.clone();
        let task = cx.background_spawn(async move {
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
                            session.diffs.insert(path.clone(), diff);
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
                            if editor.value().as_ref() != text {
                                editor.set_value(text, window, cx);
                            }
                            if let Some(position) = position {
                                editor.set_cursor_position(position, window, cx);
                            } else if let Some((position, offset)) = previous_position {
                                editor.set_cursor_position(position, window, cx);
                                editor.set_scroll_offset(offset, cx);
                            } else if restore.selected.as_ref() == Some(&path) {
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
                        session
                            .scroll
                            .scroll_to_item(session.saved.diff_row, ScrollStrategy::Top);
                        this.status = if this.navigation.is_some() {
                            "코드 탐색 준비됨".into()
                        } else {
                            "파일 열람 중".into()
                        };
                        this.persist(cx);
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
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
            self.open_file(target.path, false, None, window, cx);
        }
    }

    fn toggle_code(&mut self, code: bool, cx: &mut Context<Self>) {
        if let Some(root) = &self.current {
            if let Some(s) = self.sessions.get_mut(root) {
                s.saved.code = code;
            }
        }
        self.persist(cx);
        cx.notify();
    }

    fn compare(&mut self, branch: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        self.navigation = Some(Arc::new(Mutex::new(Navigator::with_options(
            root,
            pokarak_navigation::default_configs(),
            pokarak_navigation::NavigatorOptions {
                allow_project_imports: true,
                ..Default::default()
            },
        ))));
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
        let position = position.unwrap_or_else(|| editor.read(cx).cursor_position());
        let displayed_text = editor.read(cx).value().to_string();
        let symbol_start = {
            use gpui_kit::component::input::RopeExt;
            let text = editor.read(cx).text();
            let offset = text.position_to_offset(&position);
            text.word_range(offset)
                .map(|range| text.offset_to_position(range.start))
                .unwrap_or(position)
        };
        let absolute = root.join(&path);
        let navigator = self.navigation.as_ref().unwrap().clone();
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
        self.status = format!("{title} 찾는 중…");
        let source = absolute.clone();
        let task = cx.background_spawn(async move {
            anyhow::ensure!(
                std::fs::read_to_string(&absolute)? == displayed_text,
                "파일이 외부에서 변경되었습니다. 새로고침 후 다시 탐색하세요."
            );
            let mut nav = navigator
                .lock()
                .map_err(|_| anyhow::anyhow!("분석 서버 상태를 읽을 수 없습니다"))?;
            let mut result = nav.navigate(&absolute, position.line, position.character, kind)?;
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
            anyhow::Ok((result, declaration))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.navigation_request != token
                    || this.current.as_ref() != Some(&root)
                    || this.selected().map(|p| root.join(p)) != Some(source)
                {
                    return;
                }
                this.navigating = false;
                match result {
                    Ok((result, declaration)) => {
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
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        this.error = Some(format!("{error:#}"));
                        this.status = "분석 서버 설정을 확인하세요".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn show_results(&mut self, title: String, result: NavigationResult, cx: &mut Context<Self>) {
        self.results = result.locations;
        self.result_semantic = result.semantic;
        self.result_title = title;
        self.results_open = true;
        self.result_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn open_location(
        &mut self,
        location: SourceLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let search_root = root.clone();
        let task =
            cx.background_spawn(async move { pokarak_core::search(&search_root, &query, 300) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.current.as_ref() != Some(&root) {
                    return;
                }
                match result {
                    Ok(locations) => this.show_results(
                        "본문 검색".into(),
                        NavigationResult {
                            locations,
                            semantic: false,
                            status: String::new(),
                        },
                        cx,
                    ),
                    Err(error) => this.error = Some(error.to_string()),
                }
                this.status = "본문 검색 완료".into();
                cx.notify();
            });
        })
        .detach();
    }

    fn focus_search(&mut self, text: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.text_search = text;
        if !text {
            if let Some(root) = &self.current {
                self.sessions.get_mut(root).unwrap().saved.all_files = true;
            }
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

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let repo_name = self
            .current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "저장소를 선택하세요".into());
        let content = div()
            .h(px(52.))
            .w_full()
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(SIDE))
            .child(
                Icon::new(IconName::Utensils)
                    .data(include_bytes!("../../../assets/icons/spork.svg").as_slice())
                    .size(px(24.))
                    .text_color(rgb(BLUE)),
            )
            .child(
                div()
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .mr_5()
                    .child("포카락"),
            )
            .child(
                Button::new("back")
                    .icon(IconName::ArrowLeft)
                    .ghost()
                    .small()
                    .tooltip("뒤로  ⌘[")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.navigate_history(true, window, cx)),
                    ),
            )
            .child(
                Button::new("forward")
                    .icon(IconName::ArrowRight)
                    .ghost()
                    .small()
                    .tooltip("앞으로  ⌘]")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.navigate_history(false, window, cx)),
                    ),
            )
            .child(
                div()
                    .ml_2()
                    .text_color(rgb(MUTED))
                    .text_ellipsis()
                    .child(repo_name),
            )
            .child(div().flex_1())
            .child(
                Button::new("refresh")
                    .icon(IconName::RefreshCw)
                    .label(if self.loading {
                        "읽는 중"
                    } else {
                        "새로고침"
                    })
                    .ghost()
                    .small()
                    .disabled(self.loading || self.current.is_none())
                    .tooltip("새로고침  ⌘R")
                    .on_click(cx.listener(|this, _, w, cx| this.refresh(w, cx))),
            )
            .child(
                Button::new("open")
                    .icon(IconName::FolderOpen)
                    .label("프로젝트 열기")
                    .small()
                    .on_click(cx.listener(|this, _, w, cx| this.choose_repository(w, cx))),
            );
        gpui_kit::component::TitleBar::new()
            .h(px(52.))
            .pl(px(90.))
            .bg(rgb(SIDE))
            .border_b_0()
            .child(content)
            .into_any_element()
    }

    fn render_worktrees(&self, cx: &mut Context<Self>) -> AnyElement {
        let worktrees = self
            .snapshot()
            .map(|s| s.worktrees.clone())
            .unwrap_or_default();
        let mut panel = div()
            .size_full()
            .v_flex()
            .bg(rgb(SIDE))
            .overflow_hidden()
            .child(
                div()
                    .h(px(44.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(FontWeight::MEDIUM).child("워크트리"))
                    .child(
                        div()
                            .text_color(rgb(MUTED))
                            .child(worktrees.len().to_string()),
                    ),
            );
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
                    .bg(if active { rgb(0x2b3947) } else { rgb(SIDE) })
                    .hover(|s| s.bg(rgb(PANEL)))
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
                            .child(
                                Icon::new(IconName::GitBranch)
                                    .size(px(15.))
                                    .text_color(if active { rgb(BLUE) } else { rgb(MUTED) }),
                            )
                            .child(div().flex_1().text_ellipsis().child(branch))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(MUTED))
                                    .child(tree.changed_files.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .pl(px(23.))
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
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
                        d.child(div().text_size(px(11.)).text_color(rgb(0xe4bb79)).child(
                            if missing {
                                "경로가 없습니다"
                            } else {
                                "잠긴 워크트리"
                            },
                        ))
                    }),
            );
        }
        panel = panel.child(list).child(
            div()
                .p_3()
                .border_t_1()
                .border_color(rgb(LINE))
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child("보기를 전환해도 브랜치는 바뀌지 않습니다."),
        );
        panel.into_any_element()
    }

    fn render_files(&self, cx: &mut Context<Self>) -> AnyElement {
        let all = self.session().map(|s| s.saved.all_files).unwrap_or(false);
        let selected = self.selected();
        let rows = Arc::new(self.file_rows());
        let count = rows.iter().filter(|r| !r.directory).count();
        let weak = cx.entity().downgrade();
        div()
            .size_full()
            .v_flex()
            .bg(rgb(BG))
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
                            .text_color(rgb(MUTED))
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
                    .text_color(rgb(MUTED))
                    .child(if self.filter.is_empty() {
                        "변경된 파일이 없습니다. 전체 파일에서 코드를 읽을 수 있습니다."
                    } else {
                        "일치하는 파일이 없습니다."
                    })
                    .into_any_element()
            } else {
                uniform_list("files", rows.len(), move |range, _, _cx| {
                    range
                        .map(|ix| {
                            let row = rows[ix].clone();
                            let target = weak.clone();
                            let path = row.path.clone();
                            let a11y_target = weak.clone();
                            let a11y_path = path.clone();
                            let directory = row.directory;
                            let selected = selected.as_ref() == Some(&path);
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
                                .bg(if selected { rgb(0x2c3a48) } else { rgb(BG) })
                                .hover(|s| s.bg(rgb(PANEL)))
                                .child(
                                    Icon::new(if row.directory {
                                        IconName::Folder
                                    } else {
                                        IconName::FileCode
                                    })
                                    .size(px(14.))
                                    .text_color(rgb(
                                        if row.directory {
                                            MUTED
                                        } else {
                                            status.map(|s| s.1).unwrap_or(MUTED)
                                        },
                                    )),
                                )
                                .child(div().flex_1().min_w_0().text_ellipsis().child(row.label))
                                .when_some(status, |d, (label, color)| {
                                    d.child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(rgb(color))
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
                .h_full()
                .flex_1()
                .into_any_element()
            })
            .into_any_element()
    }

    fn render_comparison(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.session().map(|s| &s.saved.comparison);
        div()
            .h(px(42.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(LINE))
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
            .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                if matches!(mode, Some(CompareMode::Branch(_))) {
                    "공통 조상 → 현재 파일"
                } else {
                    "HEAD → 현재 파일"
                },
            ))
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
        let Some(session) = self.session() else {
            return self.render_welcome(cx);
        };
        let Some(path) = &session.saved.selected else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
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
            .border_color(rgb(LINE));
        for (i, tab) in session.saved.tabs.iter().enumerate() {
            let target = tab.clone();
            tabs = tabs.child(
                Button::new(("tab", i))
                    .label(
                        tab.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                    )
                    .icon(IconName::FileCode)
                    .ghost()
                    .small()
                    .selected(tab == path)
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.open_file(target.clone(), true, None, w, cx)
                    })),
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
            .border_color(rgb(LINE))
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
                    .selected(!code)
                    .disabled(diff.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_code(false, cx))),
            )
            .child(
                Button::new("code")
                    .label("코드")
                    .ghost()
                    .small()
                    .selected(code)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_code(true, cx))),
            )
            .child(
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
            .child(
                Button::new("impl")
                    .label("구현체")
                    .ghost()
                    .small()
                    .disabled(!code || deleted || self.navigating)
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.request_navigation(NavigationKind::Implementation, None, w, cx)
                    })),
            );
        let content = if diff.map(|d| d.binary).unwrap_or(false) {
            div()
                .p_6()
                .text_color(rgb(MUTED))
                .child("바이너리 파일입니다. 텍스트 비교를 제공하지 않습니다.")
                .into_any_element()
        } else if code {
            if let Some(editor) = editor {
                div()
                    .size_full()
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            if event.modifiers.platform && !deleted {
                                this.request_navigation(
                                    NavigationKind::Definition,
                                    None,
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
                            .h(relative(1.))
                            .font_family("JetBrains Mono")
                            .text_size(px(13.)),
                    )
                    .into_any_element()
            } else {
                div()
                    .p_4()
                    .text_color(rgb(MUTED))
                    .child("파일을 읽는 중…")
                    .into_any_element()
            }
        } else if let Some(diff) = diff {
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
                            .w_full()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .bg(rgb(background))
                            .font_family("JetBrains Mono")
                            .text_size(px(12.))
                            .text_color(rgb(color))
                            .child(
                                div()
                                    .w(px(45.))
                                    .flex_none()
                                    .text_right()
                                    .pr_2()
                                    .text_color(rgb(MUTED))
                                    .child(
                                        line.old_line.map(|n| n.to_string()).unwrap_or_default(),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(45.))
                                    .flex_none()
                                    .text_right()
                                    .pr_2()
                                    .text_color(rgb(MUTED))
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
                                if event.click_count() == 2 {
                                    if let Some(line_number) = line.new_line {
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
                                }
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&session.scroll)
            .size_full()
            .into_any_element()
        } else {
            div()
                .p_4()
                .text_color(rgb(MUTED))
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
                        .text_color(rgb(RED))
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
                        .text_color(rgb(0xe4bb79))
                        .child("큰 파일의 일부만 표시합니다."),
                )
            })
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
            .into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let results = Arc::new(self.results.clone());
        let weak = cx.entity().downgrade();
        let root = self.current.clone().unwrap_or_default();
        div()
            .h(px(210.))
            .flex_none()
            .v_flex()
            .border_t_1()
            .border_color(rgb(LINE))
            .bg(rgb(SIDE))
            .child(
                div()
                    .h(px(34.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(format!("{}  {}", self.result_title, self.results.len()))
                    .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                        if self.result_semantic {
                            "언어 분석 결과"
                        } else {
                            "텍스트 검색 결과"
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new("close-results")
                            .icon(IconName::X)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.results_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(if results.is_empty() {
                div()
                    .p_3()
                    .text_color(rgb(MUTED))
                    .child("검색 결과가 없습니다.")
                    .into_any_element()
            } else {
                uniform_list("results", results.len(), move |range, _, _| {
                    range
                        .map(|ix| {
                            let hit = results[ix].clone();
                            let target = weak.clone();
                            let label = format!(
                                "{}:{}",
                                hit.path.strip_prefix(&root).unwrap_or(&hit.path).display(),
                                hit.line + 1
                            );
                            div()
                                .id(("hit", ix))
                                .h(px(40.))
                                .px_3()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(PANEL)))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(BLUE))
                                        .text_ellipsis()
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .font_family("JetBrains Mono")
                                        .text_size(px(11.))
                                        .text_color(rgb(MUTED))
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
                    .text_color(rgb(BLUE)),
            )
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("변경된 코드를, 가볍게."),
            )
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("워크트리를 오가며 diff와 코드를 읽으세요."),
            )
            .child(
                Button::new("welcome-open")
                    .label("Git 저장소 열기")
                    .icon(IconName::FolderOpen)
                    .on_click(cx.listener(|this, _, w, cx| this.choose_repository(w, cx))),
            );
        for (index, path) in self.prefs.recent.iter().enumerate() {
            let target = path.clone();
            content = content.child(
                Button::new(("recent", index))
                    .label(path.to_string_lossy().to_string())
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.open_repository(target.clone(), w, cx)
                    })),
            );
        }
        content.into_any_element()
    }
}

impl Render for Pokarak {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.current.is_some() {
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
                        .child(self.render_files(cx)),
                )
                .child(resizable_panel().child(self.render_reader(window, cx)))
                .into_any_element()
        } else {
            self.render_welcome(cx)
        };
        let context = self
            .current
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "로컬 저장소 전용".into());
        div().id("pokarak").key_context("Pokarak").track_focus(&self.focus).size_full().overflow_hidden().v_flex().bg(rgb(BG)).text_color(rgb(TEXT)).text_size(px(13.)).font_family("IBM Plex Sans")
            .on_action(cx.listener(|this,_:&OpenRepository,w,cx|this.choose_repository(w,cx)))
            .on_action(cx.listener(|this,_:&Refresh,w,cx|this.refresh(w,cx)))
            .on_action(cx.listener(|this,_:&Back,w,cx|this.navigate_history(true,w,cx)))
            .on_action(cx.listener(|this,_:&Forward,w,cx|this.navigate_history(false,w,cx)))
            .on_action(cx.listener(|this,_:&FindFile,w,cx|this.focus_search(false,w,cx)))
            .on_action(cx.listener(|this,_:&FindText,w,cx|this.focus_search(true,w,cx)))
            .on_action(cx.listener(|this,_:&FindReferences,w,cx|this.request_navigation(NavigationKind::References,None,w,cx)))
            .on_action(cx.listener(|this,_:&FindDefinition,w,cx|this.request_navigation(NavigationKind::Definition,None,w,cx)))
            .on_action(cx.listener(|this,_:&ToggleCode,_,cx|{let code=!this.session().map(|s|s.saved.code).unwrap_or(false);this.toggle_code(code,cx)}))
            .child(self.render_toolbar(cx))
            .when(self.current.is_some(),|d|d.child(self.render_comparison(cx)))
            .when(self.external_change,|d|d.child(div().h(px(32.)).flex_none().px_3().flex().items_center().gap_3().bg(rgb(0x3b3326)).text_color(rgb(0xe4c28c)).text_size(px(12.))
                .child("새 변경사항이 있습니다. 읽던 위치를 유지한 채 갱신할 수 있습니다.")
                .child(Button::new("apply-refresh").label("갱신").ghost().small().on_click(cx.listener(|this,_,w,cx|this.refresh(w,cx))))))
            .when_some(self.error.clone(),|d,error|d.child(div().px_3().py_2().flex().items_start().gap_2().bg(rgb(0x3c2a30)).text_color(rgb(RED)).text_size(px(12.))
                .child(div().flex_1().child(error)).child(Button::new("dismiss-error").icon(IconName::X).ghost().small().on_click(cx.listener(|this,_,_,cx|{this.error=None;cx.notify();})))))
            .when(self.trust_prompt,|d|d.child(div().p_3().v_flex().gap_2().bg(rgb(0x293848)).border_b_1().border_color(rgb(LINE))
                .child(div().font_weight(FontWeight::MEDIUM).child("이 워크트리에서 코드 분석을 시작할까요?"))
                .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child("언어 서버가 Gradle/Maven 설정을 읽거나 실행할 수 있습니다. 신뢰하는 프로젝트에서만 시작하세요. 파일과 diff는 분석 없이도 읽을 수 있습니다."))
                .child(div().flex().gap_2()
                    .child(Button::new("trust-start").label("신뢰하고 분석 허용").small().on_click(cx.listener(|this,_,_,cx|this.enable_analysis(cx))))
                    .child(Button::new("trust-cancel").label("파일만 보기").ghost().small().on_click(cx.listener(|this,_,_,cx|{this.trust_prompt=false;cx.notify();}))))))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .when(self.results_open,|d|d.child(self.render_results(cx)))
            .child(div().h(px(27.)).flex_none().px_3().flex().items_center().gap_4().border_t_1().border_color(rgb(LINE)).bg(rgb(SIDE)).text_size(px(11.)).text_color(rgb(MUTED))
                .child(div().flex_1().min_w_0().text_ellipsis().child(context))
                .child(self.status.clone())
                .child("⌘클릭 정의  /  ⌥F7 사용처"))
    }
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
