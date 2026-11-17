//! Shared native task, log and extension surfaces. Every executable action is explicit.
use super::*;
use crate::{
    extension_host::{self, ExtensionHost, ExtensionRegistry, InstalledExtension, SPRING_ID},
    task_runtime::{self, LogBuffer, LogOptions, RunId, StreamKind, TaskProfile, TaskRuntime},
};
use anyhow::Context as _;
use gpui_kit::component::checkbox::Checkbox;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

actions!(forkarak_tasks, [ManageTasks]);
actions!(forkarak_extensions, [ManageExtensions]);
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Tasks,
    Extensions,
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Scope {
    Application,
    Project,
    #[default]
    Worktree,
}
impl Scope {
    fn label(self) -> &'static str {
        match self {
            Self::Application => "앱",
            Self::Project => "프로젝트",
            Self::Worktree => "워크트리",
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct TreeConfig {
    profiles: Option<Vec<TaskProfile>>,
    extension_enabled: HashMap<String, bool>,
    extension_settings: HashMap<String, Value>,
}
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct ExecutionConfig {
    worktrees: HashMap<PathBuf, TreeConfig>,
    trusted: HashSet<PathBuf>,
}
#[derive(Clone, PartialEq, Eq)]
enum LogSource {
    Task(String, RunId),
    Extension(String),
}
#[derive(Default)]
struct TreeView {
    query: Option<Entity<InputState>>,
    selected: Option<LogSource>,
    scroll: ScrollHandle,
    revision: u64,
}
pub(super) struct ExecutionUi {
    visible: bool,
    page: Page,
    scope: Scope,
    config: ExecutionConfig,
    pub tasks: TaskRuntime,
    pub host: Arc<ExtensionHost>,
    registry: ExtensionRegistry,
    views: HashMap<PathBuf, TreeView>,
    pending_restarts: HashMap<(PathBuf, String), (TaskProfile, LogOptions)>,
    busy: bool,
    request: u64,
    message: Option<String>,
}
impl Default for ExecutionUi {
    fn default() -> Self {
        let path = config_file();
        let config = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            visible: false,
            page: Page::Tasks,
            scope: Scope::Worktree,
            config,
            tasks: TaskRuntime::default(),
            host: Arc::new(ExtensionHost::default()),
            registry: ExtensionRegistry::new(extension_directory()),
            views: HashMap::new(),
            pending_restarts: HashMap::new(),
            busy: false,
            request: 0,
            message: None,
        }
    }
}
fn config_file() -> PathBuf {
    crate::prefs::config_path().with_file_name("execution.json")
}
pub(crate) fn extension_directory() -> PathBuf {
    crate::prefs::config_path().with_file_name("extensions")
}
impl ExecutionUi {
    fn save(&self) -> anyhow::Result<()> {
        let path = config_file();
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temp, &self.config)?;
        temp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
    pub(super) fn shutdown_root(&mut self, root: &Path) {
        self.pending_restarts.retain(|(p, _), _| p != root);
        self.tasks.shutdown_root(root);
        self.host.shutdown_root(root);
    }
    pub(super) fn shutdown(&mut self) {
        self.pending_restarts.clear();
        self.tasks = TaskRuntime::default();
        self.host.shutdown();
        self.host = Arc::new(ExtensionHost::default());
    }
}
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-alt-r", ManageTasks, None),
        KeyBinding::new("cmd-alt-e", ManageExtensions, None),
    ]);
}
impl Pokarak {
    pub(super) fn open_tasks(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.open_execution(Page::Tasks, w, cx);
    }
    pub(super) fn open_extensions(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.open_execution(Page::Extensions, w, cx);
    }
    fn open_execution(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        w.close_all_dialogs(cx);
        self.execution.visible = true;
        self.execution.page = page;
        self.execution.message = None;
        self.ensure_execution_view(w, cx);
        cx.notify();
    }
    fn ensure_execution_view(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let root = self.current.clone().unwrap_or_default();
        if self
            .execution
            .views
            .get(&root)
            .is_some_and(|v| v.query.is_some())
        {
            return;
        }
        let query = cx.new(|cx| InputState::new(w, cx).placeholder("작업·로그·확장 명령 검색"));
        cx.subscribe_in(&query, w, |_, _, event, w, cx| {
            if matches!(event, InputEvent::Change) {
                w.refresh();
                cx.notify();
            }
        })
        .detach();
        self.execution.views.entry(root).or_default().query = Some(query);
    }
    fn execution_profiles(&self) -> Vec<TaskProfile> {
        if let Some(profiles) = self
            .current
            .as_ref()
            .and_then(|p| self.execution.config.worktrees.get(p))
            .and_then(|c| c.profiles.clone())
        {
            return profiles;
        }
        self.prefs
            .option(self.current_project.as_deref(), "tasks.profiles")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
    fn managed_profiles(&self) -> Vec<TaskProfile> {
        if self.execution.scope == Scope::Worktree {
            return self.execution_profiles();
        }
        let value = if self.execution.scope == Scope::Application {
            self.prefs.options.get("tasks.profiles")
        } else {
            self.prefs
                .option(self.current_project.as_deref(), "tasks.profiles")
        };
        value
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
    fn execution_options(&self) -> LogOptions {
        let root = self.current_project.as_deref();
        LogOptions {
            auto_scroll: self.prefs.option_bool(root, "logs.auto_scroll", true),
            word_wrap: self.prefs.option_bool(root, "logs.word_wrap", true),
            max_lines: self.prefs.option_usize(root, "logs.max_lines", 10_000),
        }
        .bounded()
    }
    fn execution_trusted(&self, root: &Path) -> bool {
        self.prefs.trusted.iter().any(|p| p == root) || self.execution.config.trusted.contains(root)
    }
    fn save_execution_profiles(&mut self, profiles: Vec<TaskProfile>) -> anyhow::Result<()> {
        match self.execution.scope {
            Scope::Worktree => {
                let root = self
                    .current
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("워크트리를 여세요"))?;
                self.execution
                    .config
                    .worktrees
                    .entry(root)
                    .or_default()
                    .profiles = Some(profiles);
                self.execution.save()?;
            }
            scope => {
                let project = if scope == Scope::Project {
                    Some(
                        self.current_project
                            .clone()
                            .ok_or_else(|| anyhow::anyhow!("프로젝트를 여세요"))?,
                    )
                } else {
                    None
                };
                self.prefs.set_option(
                    project.as_deref(),
                    "tasks.profiles",
                    serde_json::to_value(profiles)?,
                );
                self.prefs.try_save()?;
            }
        }
        Ok(())
    }
    fn effective_extension_enabled(&self, extension: &InstalledExtension, root: &Path) -> bool {
        let id = &extension.manifest.id;
        let app = self
            .prefs
            .options
            .get("extensions.enabled")
            .and_then(|v| v.get(id))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let project = self
            .current_project
            .as_ref()
            .and_then(|p| self.prefs.project_options.get(p))
            .and_then(|o| o.get("extensions.enabled"))
            .and_then(|v| v.get(id))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let tree = self
            .execution
            .config
            .worktrees
            .get(root)
            .and_then(|c| c.extension_enabled.get(id))
            .copied()
            .unwrap_or(true);
        extension.enabled && app && project && tree
    }
    /// Declarative only: opening language settings must never activate an extension.
    pub(super) fn registered_extension_language_servers(
        &self,
    ) -> Vec<extension_host::ToolRegistration> {
        let root = self.current.as_deref().unwrap_or(Path::new(""));
        let overrides = self
            .execution
            .registry
            .list()
            .into_iter()
            .map(|extension| {
                let enabled = self.effective_extension_enabled(&extension, root);
                (extension.manifest.id, enabled)
            })
            .collect();
        self.execution.registry.language_servers(&overrides)
    }
    pub(super) fn registered_extension_formatters(&self) -> Vec<extension_host::ToolRegistration> {
        let root = self.current.as_deref().unwrap_or(Path::new(""));
        let overrides = self
            .execution
            .registry
            .list()
            .into_iter()
            .map(|extension| {
                let enabled = self.effective_extension_enabled(&extension, root);
                (extension.manifest.id, enabled)
            })
            .collect();
        self.execution.registry.formatters(&overrides)
    }
    fn extension_settings(&self, extension: &InstalledExtension, root: &Path) -> Value {
        let mut result = serde_json::Map::new();
        for setting in &extension.manifest.capabilities.settings {
            result.insert(setting.key.clone(), setting.default.clone());
        }
        if let Some(settings) = self
            .prefs
            .option(self.current_project.as_deref(), "extensions.settings")
            .and_then(|v| v.get(&extension.manifest.id))
            .and_then(Value::as_object)
        {
            result.extend(settings.clone());
        }
        if extension.manifest.id == SPRING_ID {
            for key in [
                "spring.jdk",
                "spring.gradle",
                "spring.maven",
                "spring.args",
                "spring.profiles",
                "spring.env",
            ] {
                if let Some(value) = self.prefs.option(self.current_project.as_deref(), key) {
                    result.insert(key.into(), value.clone());
                }
            }
        }
        if let Some(settings) = self
            .execution
            .config
            .worktrees
            .get(root)
            .and_then(|c| c.extension_settings.get(&extension.manifest.id))
            .and_then(Value::as_object)
        {
            result.extend(settings.clone());
        }
        Value::Object(result)
    }
    fn run_profile(
        &mut self,
        root: PathBuf,
        profile: TaskProfile,
        restart: bool,
        cx: &mut Context<Self>,
    ) {
        if self.current.as_ref() != Some(&root) || self.opening_path.is_some() {
            return;
        }
        let options = self.execution_options();
        let trusted = self.execution_trusted(&root);
        let name = profile.name.clone();
        let result = if restart
            && self
                .execution
                .tasks
                .run(&root, &name)
                .is_some_and(|r| r.process.status().active())
        {
            if !trusted {
                Err(anyhow::anyhow!("이 워크트리의 실행을 먼저 허용하세요"))
            } else {
                if let Some(run) = self.execution.tasks.run(&root, &name) {
                    self.execution.tasks.stop(&root, &name, run.id);
                }
                self.execution
                    .pending_restarts
                    .insert((root.clone(), name.clone()), (profile, options));
                self.execution.message = Some("프로세스 종료 후 다시 실행합니다".into());
                cx.notify();
                return;
            }
        } else {
            self.execution.tasks.start(&root, profile, trusted, options)
        };
        match result {
            Ok(id) => {
                self.execution.views.entry(root).or_default().selected =
                    Some(LogSource::Task(name, id));
                self.execution.message = None;
            }
            Err(e) => self.execution.message = Some(e.to_string()),
        }
        cx.notify();
    }
    pub(super) fn execution_tick(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let ready: Vec<_> = self
            .execution
            .pending_restarts
            .keys()
            .filter(|(root, name)| {
                !self
                    .execution
                    .tasks
                    .run(root, name)
                    .is_some_and(|r| r.process.status().active())
            })
            .cloned()
            .collect();
        for key in ready {
            if let Some((profile, options)) = self.execution.pending_restarts.remove(&key) {
                let trusted = self.execution_trusted(&key.0);
                if let Err(e) = self
                    .execution
                    .tasks
                    .start(&key.0, profile, trusted, options)
                {
                    self.execution.message = Some(e.to_string());
                } else if let Some(run) = self.execution.tasks.run(&key.0, &key.1) {
                    self.execution
                        .views
                        .entry(key.0.clone())
                        .or_default()
                        .selected = Some(LogSource::Task(key.1, run.id));
                }
            }
        }
        if !self.execution.visible {
            return;
        }
        self.ensure_execution_view(w, cx);
        // Observe only the selected worktree. Reader threads retain their own run/log identity.
        let root = self.current.clone().unwrap_or_default();
        let revision = self
            .execution
            .tasks
            .runs(&root)
            .iter()
            .map(|r| r.logs.lock().unwrap().revision + u64::from(!r.process.status().active()))
            .sum::<u64>();
        let extra = self
            .selected_execution_logs(&root)
            .map(|b| b.lock().unwrap().revision)
            .unwrap_or(0);
        let view = self.execution.views.entry(root).or_default();
        let revision = revision.wrapping_add(extra);
        if view.revision != revision {
            view.revision = revision;
            cx.notify();
        }
    }
    fn selected_execution_logs(&self, root: &Path) -> Option<Arc<Mutex<LogBuffer>>> {
        match self.execution.views.get(root)?.selected.as_ref()? {
            LogSource::Task(name, id) => self
                .execution
                .tasks
                .run(root, name)
                .filter(|r| r.id == *id)
                .map(|r| r.logs.clone()),
            LogSource::Extension(id) => self.execution.host.logs(root, id),
        }
    }
    pub(super) fn with_execution_manager(
        &mut self,
        body: AnyElement,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.execution.visible {
            return body;
        }
        self.ensure_execution_view(w, cx);
        let palette = Palette::new(cx);
        let root = self.current.clone().unwrap_or_default();
        let query = self.execution.views[&root].query.as_ref().unwrap().clone();
        let text = query.read(cx).value().to_string();
        let header = div()
            .flex()
            .flex_none()
            .h(ui_px(36.))
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(palette.rgb(LINE))
            .child(
                Button::new("execution-tasks")
                    .label("작업·로그")
                    .selected(self.execution.page == Page::Tasks)
                    .small()
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.execution.page = Page::Tasks;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("execution-extensions")
                    .label("확장")
                    .selected(self.execution.page == Page::Extensions)
                    .small()
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.execution.page = Page::Extensions;
                        cx.notify();
                    })),
            )
            .child(div().flex_1().min_w_0().child(Input::new(&query).small()))
            .child(
                Button::new("execution-close")
                    .icon(IconName::X)
                    .accessibility_label("작업·로그 닫기")
                    .tooltip("닫기")
                    .small()
                    .on_click(cx.listener(|app, _, w, cx| {
                        app.execution.visible = false;
                        app.focus.focus(w, cx);
                        cx.notify();
                    })),
            );
        let mut content = div()
            .size_full()
            .min_h_0()
            .v_flex()
            .bg(palette.rgb(BG))
            .child(header)
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(ui_text(11.))
                    .text_color(palette.rgb(MUTED))
                    .child(if root.as_os_str().is_empty() {
                        "프로젝트를 열면 워크트리별 실행과 로그를 사용할 수 있습니다".into()
                    } else {
                        root.display().to_string()
                    }),
            );
        if let Some(message) = &self.execution.message {
            content = content.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(palette.rgb(BLUE))
                    .child(message.clone()),
            );
        }
        if !root.as_os_str().is_empty() && !self.execution_trusted(&root) {
            let expected = root.clone();
            content = content.child(
                div()
                    .p_2()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child("이 워크트리에서는 아직 실행을 허용하지 않았습니다.")
                    .child(
                        Button::new("execution-trust")
                            .label("이 워크트리 실행 허용")
                            .small()
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if app.current.as_ref() == Some(&expected) {
                                    app.execution.config.trusted.insert(expected.clone());
                                    app.execution.message =
                                        app.execution.save().err().map(|e| e.to_string());
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        let panel = if self.execution.page == Page::Tasks {
            self.render_tasks(&root, &text, w, cx)
        } else {
            self.render_extensions(&root, &text, w, cx)
        };
        content.child(panel).into_any_element()
    }
    fn scope_controls(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child("저장 범위");
        for (id, scope) in [
            ("execution-scope-app", Scope::Application),
            ("execution-scope-project", Scope::Project),
            ("execution-scope-tree", Scope::Worktree),
        ] {
            row = row.child(
                Button::new(id)
                    .label(scope.label())
                    .small()
                    .selected(self.execution.scope == scope)
                    .disabled(scope != Scope::Application && self.current.is_none())
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.execution.scope = scope;
                        cx.notify();
                    })),
            );
        }
        row
    }
    fn render_tasks(
        &mut self,
        root: &Path,
        query: &str,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = Palette::new(cx);
        let mut list = div()
            .id("task-profiles")
            .max_h(ui_px(205.))
            .overflow_y_scroll()
            .v_flex()
            .gap_1();
        let profiles = self.managed_profiles();
        for (index, profile) in profiles.iter().enumerate() {
            if !query.is_empty()
                && !profile.name.to_lowercase().contains(&query.to_lowercase())
                && !profile
                    .command
                    .join(" ")
                    .to_lowercase()
                    .contains(&query.to_lowercase())
            {
                continue;
            }
            let name = profile.name.clone();
            let run = self.execution.tasks.run(root, &name);
            let active = run.is_some_and(|r| r.process.status().active());
            let status = run
                .map(|r| {
                    let status = r.process.status();
                    if status.active() {
                        format!(
                            "{} · {:.0}초",
                            status.label(),
                            r.started.elapsed().as_secs_f32()
                        )
                    } else {
                        status.label()
                    }
                })
                .unwrap_or_else(|| "대기".into());
            let id = run.map(|r| r.id);
            let (p, r) = (profile.clone(), root.to_path_buf());
            let (p2, r2) = (p.clone(), r.clone());
            let (name2, r3) = (name.clone(), r.clone());
            let (name3, r4) = (name.clone(), r.clone());
            let edit = profile.clone();
            let removed = name.clone();
            let remove_root = root.to_path_buf();
            let remove_scope = self.execution.scope;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(palette.rgb(LINE))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .v_flex()
                            .child(div().min_w_0().text_ellipsis().child(profile.name.clone()))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .text_size(ui_text(11.))
                                    .text_color(palette.rgb(MUTED))
                                    .child(format!("{} · {status}", profile.command.join(" "))),
                            ),
                    )
                    .child(
                        Button::new(("task-run", index))
                            .icon(IconName::Play)
                            .accessibility_label("실행")
                            .tooltip("실행")
                            .small()
                            .disabled(
                                active
                                    || root.as_os_str().is_empty()
                                    || !self.execution_trusted(root),
                            )
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.run_profile(r.clone(), p.clone(), false, cx)
                            })),
                    )
                    .child(
                        Button::new(("task-stop", index))
                            .icon(IconName::Square)
                            .accessibility_label("중지")
                            .tooltip("중지")
                            .small()
                            .disabled(!active)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if app.current.as_ref() != Some(&r3) {
                                    return;
                                }
                                if let Some(id) = id {
                                    app.execution
                                        .pending_restarts
                                        .remove(&(r3.clone(), name2.clone()));
                                    app.execution.tasks.stop(&r3, &name2, id);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new(("task-restart", index))
                            .icon(IconName::RotateCw)
                            .accessibility_label("재시작")
                            .tooltip("재시작")
                            .small()
                            .disabled(!self.execution_trusted(root))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.run_profile(r2.clone(), p2.clone(), true, cx)
                            })),
                    )
                    .child(
                        Button::new(("task-logs", index))
                            .icon(IconName::ScrollText)
                            .accessibility_label("로그")
                            .tooltip("로그")
                            .small()
                            .disabled(id.is_none())
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if let Some(id) = id {
                                    app.execution.views.entry(r4.clone()).or_default().selected =
                                        Some(LogSource::Task(name3.clone(), id));
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new(("task-edit", index))
                            .icon(IconName::Pencil)
                            .accessibility_label("편집")
                            .tooltip("편집")
                            .small()
                            .on_click(cx.listener(move |app, _, w, cx| {
                                app.edit_task(Some(edit.clone()), w, cx)
                            })),
                    )
                    .child(
                        Button::new(("task-remove", index))
                            .icon(IconName::Trash)
                            .accessibility_label("삭제")
                            .tooltip("삭제")
                            .small()
                            .disabled(active)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if app.current.as_ref() != Some(&remove_root)
                                    || app.execution.scope != remove_scope
                                {
                                    return;
                                }
                                let mut profiles = app.managed_profiles();
                                profiles.retain(|p| p.name != removed);
                                app.execution.message = app
                                    .save_execution_profiles(profiles)
                                    .err()
                                    .map(|e| e.to_string());
                                cx.notify();
                            })),
                    ),
            );
        }
        if profiles.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .child("저장된 작업이 없습니다. 실행 파일과 인수를 프로필로 추가하세요."),
            );
        }
        let mut panel = div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .px_2()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(self.scope_controls(cx))
                    .child(
                        Button::new("task-add")
                            .label("작업 추가…")
                            .small()
                            .disabled(root.as_os_str().is_empty())
                            .on_click(cx.listener(|app, _, w, cx| app.edit_task(None, w, cx))),
                    )
                    .child(
                        Button::new("spring-profiles")
                            .label("Spring 프로필 만들기…")
                            .small()
                            .disabled(
                                root.as_os_str().is_empty()
                                    || !self.execution_trusted(root)
                                    || self.execution.busy,
                            )
                            .on_click(cx.listener(|app, _, w, cx| {
                                app.invoke_extension(SPRING_ID.into(), "profiles".into(), w, cx)
                            })),
                    ),
            )
            .child(list);
        if let Some(logs) = self.selected_execution_logs(root) {
            panel = panel.child(self.render_execution_logs(root, logs, query, cx));
        } else {
            panel =
                panel.child(div().p_3().text_color(palette.rgb(MUTED)).child(
                    "실행한 작업의 로그를 선택하세요. 화면을 열어도 명령을 실행하지 않습니다.",
                ));
        }
        panel.into_any_element()
    }
    fn render_execution_logs(
        &mut self,
        root: &Path,
        logs: Arc<Mutex<LogBuffer>>,
        query: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = Palette::new(cx);
        let buffer = logs.lock().unwrap();
        let lines = buffer.search(query);
        let options = buffer.options;
        let count = lines.len();
        let lines: Vec<_> = lines.into_iter().skip(count.saturating_sub(1000)).collect();
        drop(buffer);
        let handle = self
            .execution
            .views
            .entry(root.to_path_buf())
            .or_default()
            .scroll
            .clone();
        if options.auto_scroll {
            handle.set_offset(point(px(0.), px(-10_000_000.)));
        }
        let opts = logs.clone();
        let opts2 = logs.clone();
        let opts3 = logs.clone();
        let copy = logs.clone();
        let q = query.to_string();
        let controls = div()
            .flex()
            .flex_none()
            .h(ui_px(34.))
            .items_center()
            .gap_2()
            .px_2()
            .child("실행 로그")
            .child(
                Checkbox::new("logs-scroll")
                    .label("따라가기")
                    .checked(options.auto_scroll)
                    .on_click(move |value, _, cx| {
                        let mut b = opts.lock().unwrap();
                        let mut o = b.options;
                        o.auto_scroll = *value;
                        b.configure(o);
                        cx.refresh_windows();
                    }),
            )
            .child(
                Checkbox::new("logs-wrap")
                    .label("줄바꿈")
                    .checked(options.word_wrap)
                    .on_click(move |value, _, cx| {
                        let mut b = opts2.lock().unwrap();
                        let mut o = b.options;
                        o.word_wrap = *value;
                        b.configure(o);
                        cx.refresh_windows();
                    }),
            )
            .child(
                Button::new("logs-limit")
                    .label(format!("{}줄", options.max_lines))
                    .small()
                    .on_click(move |_, _, cx| {
                        let mut b = opts3.lock().unwrap();
                        let mut o = b.options;
                        o.max_lines = match o.max_lines {
                            0..=1000 => 10_000,
                            1001..=10_000 => 50_000,
                            _ => 1000,
                        };
                        b.configure(o);
                        cx.refresh_windows();
                    }),
            )
            .child(
                Button::new("logs-copy")
                    .icon(IconName::Copy)
                    .accessibility_label("검색 로그 복사")
                    .tooltip("검색 로그 복사")
                    .small()
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            copy.lock()
                                .unwrap()
                                .search(&q)
                                .iter()
                                .map(|l| l.text.as_str())
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ));
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_ellipsis()
                    .text_size(ui_text(11.))
                    .text_color(palette.rgb(MUTED))
                    .child(format!("검색 {count}줄 · 최근 {}줄 표시", lines.len())),
            );
        let mut rows = div()
            .id("execution-log-lines")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .overflow_x_scroll()
            .track_scroll(&handle)
            .v_flex()
            .font_family("JetBrains Mono")
            .text_size(ui_text(12.));
        for line in lines {
            let color = match line.stream {
                StreamKind::Stderr => RED,
                StreamKind::Host => MUTED,
                StreamKind::Stdout => TEXT,
            };
            let location = task_runtime::resolve_stack_location(root, &line.text);
            let expected = root.to_path_buf();
            let row = div()
                .id(("execution-log", line.sequence as usize))
                .px_2()
                .py_0p5()
                .text_color(palette.rgb(color))
                .when(options.word_wrap, |d| d.whitespace_normal())
                .when(!options.word_wrap, |d| d.whitespace_nowrap())
                .child(line.text)
                .when_some(location, |d, location| {
                    let source = self
                        .execution
                        .views
                        .get(&expected)
                        .and_then(|v| v.selected.clone());
                    let revision = file_revision(&location.path);
                    let accessible_root = expected.clone();
                    let accessible_location = location.clone();
                    let accessible_source = source.clone();
                    let accessible_view = cx.entity().downgrade();
                    d.cursor_pointer()
                        .underline()
                        .role(accesskit::Role::Button)
                        .aria_label(format!(
                            "{}:{}로 이동",
                            location
                                .path
                                .strip_prefix(&expected)
                                .unwrap_or(&location.path)
                                .display(),
                            location.line + 1
                        ))
                        .on_a11y_action(accesskit::Action::Click, move |_, w, cx| {
                            let _ = accessible_view.update(cx, |app, cx| {
                                app.open_execution_location(
                                    &accessible_root,
                                    &accessible_location,
                                    &accessible_source,
                                    revision,
                                    w,
                                    cx,
                                )
                            });
                        })
                        .on_click(cx.listener(move |app, _, w, cx| {
                            app.open_execution_location(
                                &expected, &location, &source, revision, w, cx,
                            );
                        }))
                });
            rows = rows.child(row);
        }
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap_2()
            .border_t_1()
            .border_color(palette.rgb(LINE))
            .child(controls)
            .child(rows)
            .into_any_element()
    }
    fn open_execution_location(
        &mut self,
        root: &Path,
        location: &task_runtime::StackLocation,
        source: &Option<LogSource>,
        revision: Option<FileRevision>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current.as_deref() != Some(root)
            || self.opening_path.is_some()
            || self
                .execution
                .views
                .get(root)
                .and_then(|v| v.selected.clone())
                != *source
        {
            return;
        }
        if file_revision(&location.path) != revision {
            self.execution.message =
                Some("로그 대상 파일이 변경되었습니다. 최신 실행 로그를 확인하세요.".into());
            cx.notify();
            return;
        }
        let text = format!(
            "{}:{}:{}",
            location.path.display(),
            location.line + 1,
            location.column + 1
        );
        if let Some(target) = task_runtime::resolve_stack_location(root, &text) {
            self.execution.visible = false;
            self.git_history.active = false;
            self.open_file(
                target.path.strip_prefix(root).unwrap().to_path_buf(),
                true,
                Some(Position::new(target.line, target.column)),
                w,
                cx,
            );
        }
    }
    fn edit_task(&mut self, existing: Option<TaskProfile>, w: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let project = self.current_project.clone();
        let scope = self.execution.scope;
        let initial = existing.clone().unwrap_or(TaskProfile {
            name: "".into(),
            command: vec!["".into()],
            cwd: "".into(),
            env: BTreeMap::new(),
        });
        let name = input(&initial.name, w, cx);
        let program = input(
            initial.command.first().map(String::as_str).unwrap_or(""),
            w,
            cx,
        );
        let args = input(
            &serde_json::to_string(&initial.command.iter().skip(1).collect::<Vec<_>>()).unwrap(),
            w,
            cx,
        );
        let cwd = input(&initial.cwd.display().to_string(), w, cx);
        let env = input(&serde_json::to_string(&initial.env).unwrap(), w, cx);
        for field in [&name, &program, &args, &cwd, &env] {
            w.subscribe(field, cx, |_, _: &InputEvent, w, _| w.refresh())
                .detach();
        }
        let weak = cx.entity().downgrade();
        w.close_all_dialogs(cx);
        w.open_dialog(cx, move |dialog, _, cx| {
            let result = parse_profile(&name, &program, &args, &cwd, &env, &root, cx);
            let error = result.as_ref().err().map(ToString::to_string);
            let stale = weak
                .read_with(cx, |app, _| {
                    app.current.as_ref() != Some(&root)
                        || app.current_project != project
                        || app.execution.scope != scope
                })
                .unwrap_or(true);
            let body = div()
                .id("task-editor-fields")
                .max_h(ui_px(360.))
                .overflow_y_scroll()
                .v_flex()
                .gap_2()
                .child(format!("{} · {}", scope.label(), root.display()))
                .child(field("작업 이름", &name))
                .child(field("실행 파일 (쉘 해석 없음)", &program))
                .child(field("인수 · JSON 문자열 배열", &args))
                .child(field("작업 폴더 · 워크트리 내부 상대 경로", &cwd))
                .child(field("환경변수 · JSON 객체", &env))
                .when_some(error, |d, e| d.child(e));
            let (weak, root, project, existing) = (
                weak.clone(),
                root.clone(),
                project.clone(),
                existing.clone(),
            );
            dialog
                .title("실행 프로필")
                .width(ui_px(690.))
                .child(body)
                .footer(
                    div().flex().justify_end().gap_2().child(close()).child(
                        Button::new("task-save")
                            .label("저장")
                            .primary()
                            .disabled(stale || result.is_err())
                            .on_click(move |_, w, cx| {
                                let Some(profile) = result.as_ref().ok().cloned() else {
                                    return;
                                };
                                let _ = weak.update(cx, |app, cx| {
                                    if app.current.as_ref() != Some(&root)
                                        || app.current_project != project
                                        || app.execution.scope != scope
                                    {
                                        return;
                                    }
                                    let mut profiles = app.managed_profiles();
                                    let previous = existing.as_ref().map(|p| p.name.as_str());
                                    if profiles.iter().any(|p| {
                                        p.name == profile.name && Some(p.name.as_str()) != previous
                                    }) {
                                        app.execution.message =
                                            Some("같은 이름의 작업이 이미 있습니다".into());
                                        cx.notify();
                                        return;
                                    }
                                    if let Some(name) = previous {
                                        profiles.retain(|p| p.name != name);
                                    }
                                    profiles.push(profile);
                                    match app.save_execution_profiles(profiles) {
                                        Ok(()) => w.close_all_dialogs(cx),
                                        Err(e) => app.execution.message = Some(e.to_string()),
                                    }
                                    cx.notify();
                                });
                            }),
                    ),
                )
        });
    }
    fn render_extensions(
        &mut self,
        root: &Path,
        query: &str,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = Palette::new(cx);
        let mut rows = div()
            .id("installed-extensions")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .v_flex()
            .gap_2();
        let query = query.to_lowercase();
        let language_servers = self.registered_extension_language_servers();
        let formatters = self.registered_extension_formatters();
        for extension in self.execution.registry.list() {
            let id = extension.manifest.id.clone();
            let matching = extension.manifest.name.to_lowercase().contains(&query)
                || id.to_lowercase().contains(&query);
            let commands: Vec<_> = extension
                .manifest
                .capabilities
                .commands
                .iter()
                .filter(|c| {
                    matching
                        || c.title.to_lowercase().contains(&query)
                        || c.id.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
            if !matching && commands.is_empty() {
                continue;
            }
            let active = self.execution.host.active(root, &id);
            let effective = self.effective_extension_enabled(&extension, root);
            let project_on = self
                .current_project
                .as_ref()
                .and_then(|p| self.prefs.project_options.get(p))
                .and_then(|o| o.get("extensions.enabled"))
                .and_then(|v| v.get(&id))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let tree_on = self
                .execution
                .config
                .worktrees
                .get(root)
                .and_then(|c| c.extension_enabled.get(&id))
                .copied()
                .unwrap_or(true);
            let app_enabled = extension.enabled
                && self
                    .prefs
                    .options
                    .get("extensions.enabled")
                    .and_then(|value| value.get(&id))
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
            let global_id = id.clone();
            let project_id = id.clone();
            let tree_id = id.clone();
            let logs_id = id.clone();
            let settings_id = id.clone();
            let update_id = id.clone();
            let remove_id = id.clone();
            let stop_id = id.clone();
            let stop_root = root.to_path_buf();
            let expected = root.to_path_buf();
            let expected2 = expected.clone();
            let mut row = div()
                .p_2()
                .v_flex()
                .gap_2()
                .border_b_1()
                .border_color(palette.rgb(LINE))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().child(format!(
                            "{} · {} · API {} · {}",
                            extension.manifest.name,
                            extension.manifest.version,
                            extension.manifest.api_version,
                            if active {
                                "활성 프로세스"
                            } else {
                                "대기"
                            }
                        )))
                        .child(
                            Checkbox::new(SharedString::from(format!("ext-global-{id}")))
                                .label("앱에서 사용")
                                .checked(app_enabled)
                                .disabled(self.execution.busy)
                                .on_click(cx.listener(move |app, value, w, cx| {
                                    app.change_extension(
                                        global_id.clone(),
                                        ExtensionChange::Enabled(*value),
                                        w,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("ext-settings-{id}")))
                                .label("설정…")
                                .small()
                                .disabled(self.execution.busy)
                                .on_click(cx.listener(move |app, _, w, cx| {
                                    app.edit_extension_settings(settings_id.clone(), w, cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("ext-log-{id}")))
                                .label("로그")
                                .small()
                                .disabled(self.execution.host.logs(root, &id).is_none())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if app.current.as_ref() == Some(&expected) {
                                        app.execution
                                            .views
                                            .entry(expected.clone())
                                            .or_default()
                                            .selected = Some(LogSource::Extension(logs_id.clone()));
                                        app.execution.page = Page::Tasks;
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("ext-stop-{id}")))
                                .label("프로세스 중지")
                                .small()
                                .disabled(!active || self.execution.busy)
                                .on_click(cx.listener(move |app, _, w, cx| {
                                    app.change_extension(
                                        stop_id.clone(),
                                        ExtensionChange::StopRoots(vec![stop_root.clone()]),
                                        w,
                                        cx,
                                    )
                                })),
                        ),
                )
                .child(
                    div()
                        .text_size(ui_text(11.))
                        .text_color(palette.rgb(MUTED))
                        .child(format!(
                            "{id} · 명령 {} · 포맷터 {} · 언어 서버 {}",
                            extension.manifest.capabilities.commands.len(),
                            extension.manifest.capabilities.formatters.len(),
                            extension.manifest.capabilities.language_servers.len()
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(
                            Checkbox::new(SharedString::from(format!("ext-project-{id}")))
                                .label("이 프로젝트에서 사용")
                                .checked(project_on)
                                .disabled(self.current_project.is_none() || self.execution.busy)
                                .on_click(cx.listener(move |app, value, w, cx| {
                                    app.set_extension_override(
                                        project_id.clone(),
                                        *value,
                                        false,
                                        w,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Checkbox::new(SharedString::from(format!("ext-tree-{id}")))
                                .label("이 워크트리에서 사용")
                                .checked(tree_on)
                                .disabled(root.as_os_str().is_empty() || self.execution.busy)
                                .on_click(cx.listener(move |app, value, w, cx| {
                                    if app.current.as_ref() == Some(&expected2) {
                                        app.set_extension_override(
                                            tree_id.clone(),
                                            *value,
                                            true,
                                            w,
                                            cx,
                                        );
                                    }
                                })),
                        ),
                );
            for (label, tools) in [("언어 서버", &language_servers), ("포맷터", &formatters)]
            {
                for tool in tools.iter().filter(|tool| tool.extension_id == id) {
                    row = row.child(
                        div()
                            .text_size(ui_text(11.))
                            .text_color(palette.rgb(MUTED))
                            .child(format!(
                                "{} · {} · {} {}",
                                label,
                                tool.language,
                                tool.executable.display(),
                                tool.args.join(" ")
                            )),
                    );
                }
            }
            for command in commands {
                let id = id.clone();
                let command_id = command.id.clone();
                row = row.child(
                    Button::new(SharedString::from(format!(
                        "ext-command-{id}-{}",
                        command.id
                    )))
                    .label(command.title)
                    .small()
                    .disabled(
                        root.as_os_str().is_empty()
                            || !effective
                            || !self.execution_trusted(root)
                            || self.execution.busy,
                    )
                    .on_click(cx.listener(move |app, _, w, cx| {
                        app.invoke_extension(id.clone(), command_id.clone(), w, cx)
                    })),
                );
            }
            if !extension.builtin {
                row = row.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new(SharedString::from(format!("ext-update-{id}")))
                                .label("로컬 번들로 업데이트…")
                                .small()
                                .disabled(self.execution.busy)
                                .on_click(cx.listener(move |app, _, w, cx| {
                                    app.choose_extension_bundle(Some(update_id.clone()), w, cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("ext-remove-{id}")))
                                .label("제거…")
                                .small()
                                .disabled(self.execution.busy)
                                .on_click(cx.listener(move |app, _, w, cx| {
                                    app.confirm_extension_remove(remove_id.clone(), w, cx)
                                })),
                        ),
                );
            }
            rows = rows.child(row);
        }
        div().flex_1().min_h_0().v_flex().gap_2().child(div().px_2().flex().flex_wrap().items_center().gap_2().child(self.scope_controls(cx))
            .child(Button::new("extension-install").label("로컬 확장 설치…").small().disabled(self.execution.busy).on_click(cx.listener(|app,_,w,cx|app.choose_extension_bundle(None,w,cx)))))
            .child(div().px_3().text_size(ui_text(11.)).text_color(palette.rgb(MUTED)).child("확장은 별도 프로세스에서 로컬 코드 권한으로 실행됩니다. 신뢰하는 번들만 설치하세요. 활성화는 명령을 직접 실행할 때만 이루어집니다."))
            .child(rows).into_any_element()
    }
    fn set_extension_override(
        &mut self,
        id: String,
        enabled: bool,
        tree: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.current.clone() else {
            return;
        };
        if tree {
            self.execution
                .config
                .worktrees
                .entry(root.clone())
                .or_default()
                .extension_enabled
                .insert(id.clone(), enabled);
            self.execution.message = self.execution.save().err().map(|e| e.to_string());
        } else if let Some(project) = self.current_project.clone() {
            let mut value = self
                .prefs
                .project_options
                .get(&project)
                .and_then(|o| o.get("extensions.enabled"))
                .cloned()
                .unwrap_or(json!({}));
            value[&id] = json!(enabled);
            self.prefs
                .set_option(Some(&project), "extensions.enabled", value);
            self.execution.message = self.prefs.try_save().err().map(|e| e.to_string());
        }
        // Stop only the affected worktree/project. App-wide disable is a separate action.
        if !enabled {
            let roots = if tree {
                vec![root]
            } else {
                self.projects
                    .iter()
                    .find(|p| Some(&p.root) == self.current_project.as_ref())
                    .map(|p| p.worktrees.clone())
                    .unwrap_or_else(|| vec![root])
            };
            self.change_extension(id, ExtensionChange::StopRoots(roots), w, cx);
        }
        cx.notify();
    }
    fn invoke_extension(
        &mut self,
        id: String,
        command: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.execution.busy || self.opening_path.is_some() {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        let extension = match self.execution.registry.get(&id) {
            Ok(e) => e,
            Err(e) => {
                self.execution.message = Some(e.to_string());
                cx.notify();
                return;
            }
        };
        let trusted = self.execution_trusted(&root);
        let enabled = self.effective_extension_enabled(&extension, &root);
        let settings = self.extension_settings(&extension, &root);
        let options = self.execution_options();
        self.execution.busy = true;
        self.execution.request += 1;
        let token = self.execution.request;
        let navigation = self.request;
        let scope = self.execution.scope;
        let host = self.execution.host.clone();
        let target = root.clone();
        let task = cx.background_spawn(async move {
            host.invoke(
                &extension, &target, &command, settings, trusted, enabled, options,
            )
        });
        cx.spawn_in(w, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |app, _, cx| {
                if app.execution.request != token {
                    return;
                }
                app.execution.busy = false;
                if app.current.as_ref() != Some(&root)
                    || app.request != navigation
                    || app.execution.scope != scope
                {
                    app.execution.message =
                        Some("워크트리가 바뀌어 이전 확장 결과를 적용하지 않았습니다".into());
                    cx.notify();
                    return;
                }
                match result {
                    Ok(response) => {
                        let mut profiles = app.managed_profiles();
                        let mut names = HashSet::new();
                        let mut valid = true;
                        for profile in &response.tasks {
                            if !names.insert(profile.name.clone()) {
                                valid = false;
                            }
                        }
                        if valid {
                            for profile in response.tasks {
                                profiles.retain(|p| p.name != profile.name);
                                profiles.push(profile);
                            }
                            app.execution.message = match app.save_execution_profiles(profiles) {
                                Ok(()) => Some(
                                    "실행 프로필을 저장했습니다. 명령을 확인한 뒤 실행하세요."
                                        .into(),
                                ),
                                Err(e) => Some(e.to_string()),
                            };
                            app.execution
                                .views
                                .entry(root.clone())
                                .or_default()
                                .selected = Some(LogSource::Extension(id));
                            app.execution.page = Page::Tasks;
                        } else {
                            app.execution.message =
                                Some("확장 응답에 중복된 작업 이름이 있습니다".into());
                        }
                    }
                    Err(e) => app.execution.message = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn choose_extension_bundle(
        &mut self,
        update: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.execution.busy {
            return;
        }
        let path = input("", w, cx);
        w.subscribe(&path, cx, |_, _: &InputEvent, w, _| w.refresh())
            .detach();
        let weak = cx.entity().downgrade();
        w.close_all_dialogs(cx);
        w.open_dialog(cx, move |dialog, _, cx| {
            let source = PathBuf::from(path.read(cx).value().as_ref());
            let valid = source.is_absolute() && source.exists();
            let browse_input = path.clone();
            let body = div()
                .v_flex()
                .gap_2()
                .child("manifest.json 파일 또는 확장 번들 폴더의 절대 경로를 입력하세요.")
                .child(field("로컬 확장 경로", &path))
                .child(
                    Button::new("extension-browse")
                        .label("찾아보기…")
                        .small()
                        .on_click(move |_, w, cx| {
                            let picker = cx.prompt_for_paths(PathPromptOptions {
                                files: true,
                                directories: true,
                                multiple: false,
                                prompt: Some("로컬 확장 번들 선택".into()),
                            });
                            let input = browse_input.clone();
                            w.spawn(cx, async move |cx| {
                                if let Ok(Ok(Some(paths))) = picker.await
                                    && let Some(path) = paths.first()
                                {
                                    let _ = cx.update(|w, cx| {
                                        input.update(cx, |input, cx| {
                                            input.set_value(path.display().to_string(), w, cx)
                                        })
                                    });
                                }
                            })
                            .detach();
                        }),
                )
                .child(
                    "설치 시 번들을 앱 데이터에 복사합니다. 새 확장은 비활성 상태로 설치됩니다.",
                );
            let weak = weak.clone();
            let update = update.clone();
            dialog
                .title(if update.is_some() {
                    "로컬 확장 업데이트"
                } else {
                    "로컬 확장 설치"
                })
                .width(ui_px(660.))
                .child(body)
                .footer(
                    div().flex().justify_end().gap_2().child(close()).child(
                        Button::new("extension-install-confirm")
                            .label(if update.is_some() {
                                "업데이트"
                            } else {
                                "설치"
                            })
                            .primary()
                            .disabled(!valid)
                            .on_click(move |_, w, cx| {
                                w.close_all_dialogs(cx);
                                let _ = weak.update(cx, |app, cx| {
                                    app.change_extension(
                                        update.clone().unwrap_or_default(),
                                        ExtensionChange::Install(source.clone(), update.is_some()),
                                        w,
                                        cx,
                                    )
                                });
                            }),
                    ),
                )
        });
    }
    fn confirm_extension_remove(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        w.close_all_dialogs(cx);
        w.open_dialog(cx,move |dialog,_,_|{let weak=weak.clone();let id=id.clone();dialog.title("확장 제거").child(format!("{id} 확장의 실행 프로세스를 중지하고 앱 데이터의 설치 번들을 제거합니다. 저장소 파일은 변경하지 않습니다.")).footer(div().flex().justify_end().gap_2().child(close()).child(Button::new("extension-confirm-remove").label("제거").on_click(move |_,w,cx|{w.close_all_dialogs(cx);let _=weak.update(cx,|app,cx|app.change_extension(id.clone(),ExtensionChange::Remove,w,cx));})))});
    }
    fn change_extension(
        &mut self,
        id: String,
        change: ExtensionChange,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.execution.busy {
            return;
        }
        self.execution.busy = true;
        self.execution.request += 1;
        let token = self.execution.request;
        let host = self.execution.host.clone();
        let directory = extension_directory();
        let task=cx.background_spawn(async move {let mut registry=ExtensionRegistry::new(directory);let message=match change {
            ExtensionChange::Enabled(enabled)=>{if !enabled{host.deactivate_extension(&id);}registry.set_enabled(&id,enabled)?;if enabled{"확장을 사용하도록 설정했습니다"}else{"확장을 비활성화했습니다"}.to_owned()},
            ExtensionChange::StopRoots(roots)=>{for root in roots {host.deactivate(&root,&id);} "확장 프로세스를 중지했습니다".into()},
            ExtensionChange::Remove=>{host.deactivate_extension(&id);registry.remove(&id)?;"확장을 제거했습니다".into()},
            ExtensionChange::Install(path,update)=>{
                if update {let manifest=if path.is_dir(){path.join("manifest.json")}else{path.clone()};let candidate:extension_host::ExtensionManifest=serde_json::from_slice(&std::fs::read(manifest)?)?;anyhow::ensure!(candidate.id==id,"선택한 번들이 업데이트 대상 확장과 다릅니다");host.deactivate_extension(&id);}
                let installed=registry.install(&path,update)?;format!("{installed} 확장을 {}했습니다. 앱에서 사용을 켠 뒤 명령을 직접 선택하세요.",if update{"업데이트"}else{"설치"})}
        };anyhow::Ok(message)});
        cx.spawn_in(w,async move |this,cx|{let result=task.await;let _=this.update_in(cx,|app,_,cx|{if app.execution.request!=token{return;}app.execution.busy=false;app.execution.registry=ExtensionRegistry::new(extension_directory());let installed:Vec<_>=app.execution.registry.list().iter().map(|e|json!({"id":e.manifest.id,"version":e.manifest.version,"enabled":e.enabled})).collect();app.prefs.set_option(None,"extensions.installed",json!(installed));let mut enabled=app.prefs.options.get("extensions.enabled").cloned().unwrap_or(json!({}));for extension in app.execution.registry.list(){enabled[&extension.manifest.id]=json!(extension.enabled);}app.prefs.set_option(None,"extensions.enabled",enabled);app.execution.message=match result{Ok(message)=>Some(message),Err(e)=>Some(e.to_string())};if let Err(e)=app.prefs.try_save(){app.execution.message=Some(e.to_string());}cx.notify();});}).detach();
        cx.notify();
    }
    fn edit_extension_settings(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        let extension = match self.execution.registry.get(&id) {
            Ok(e) => e,
            Err(_) => return,
        };
        let root = self.current.clone().unwrap_or_default();
        let project = self.current_project.clone();
        let scope = self.execution.scope;
        let settings = self.extension_settings(&extension, &root);
        let mut fields = Vec::new();
        for setting in &extension.manifest.capabilities.settings {
            let value = settings.get(&setting.key).unwrap_or(&setting.default);
            let text = if value.is_string() {
                value.as_str().unwrap().to_owned()
            } else {
                serde_json::to_string(value).unwrap()
            };
            let entry = input(&text, w, cx);
            w.subscribe(&entry, cx, |_, _: &InputEvent, w, _| w.refresh())
                .detach();
            fields.push((setting.clone(), entry));
        }
        let weak = cx.entity().downgrade();
        w.close_all_dialogs(cx);
        w.open_dialog(cx, move |dialog, _, cx| {
            let stale = weak
                .read_with(cx, |app, _| {
                    app.current.clone().unwrap_or_default() != root
                        || app.current_project != project
                        || app.execution.scope != scope
                })
                .unwrap_or(true);
            let mut body = div()
                .id("extension-settings-fields")
                .max_h(ui_px(360.))
                .overflow_y_scroll()
                .v_flex()
                .gap_2()
                .child(format!("{} · {}", scope.label(), extension.manifest.name));
            let mut parsed = serde_json::Map::new();
            let mut error = None;
            for (setting, entry) in &fields {
                body = body.child(field(&setting.title, entry));
                let text = entry.read(cx).value().to_string();
                let value = if setting.default.is_string() {
                    Ok(json!(text))
                } else {
                    serde_json::from_str::<Value>(&text)
                };
                match value {
                    Ok(value) => {
                        let same = matches!(
                            (&setting.default, &value),
                            (Value::Null, _)
                                | (Value::Bool(_), Value::Bool(_))
                                | (Value::Number(_), Value::Number(_))
                                | (Value::String(_), Value::String(_))
                                | (Value::Array(_), Value::Array(_))
                                | (Value::Object(_), Value::Object(_))
                        );
                        if same {
                            parsed.insert(setting.key.clone(), value);
                        } else {
                            error =
                                Some(format!("{} · 설정 값의 형식을 확인하세요", setting.title));
                        }
                    }
                    Err(e) => error = Some(format!("{} · {e}", setting.title)),
                }
            }
            if fields.is_empty() {
                body = body.child("이 확장은 별도 설정을 등록하지 않았습니다.");
            }
            if let Some(error) = &error {
                body = body.child(error.clone());
            }
            let (weak, root, project, id) =
                (weak.clone(), root.clone(), project.clone(), id.clone());
            dialog
                .title("확장 설정")
                .width(ui_px(650.))
                .child(body)
                .footer(
                    div().flex().justify_end().gap_2().child(close()).child(
                        Button::new("extension-settings-save")
                            .label("저장")
                            .primary()
                            .disabled(
                                stale
                                    || error.is_some()
                                    || (scope != Scope::Application && root.as_os_str().is_empty()),
                            )
                            .on_click(move |_, w, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    if app.current.clone().unwrap_or_default() != root
                                        || app.current_project != project
                                        || app.execution.scope != scope
                                    {
                                        return;
                                    }
                                    let result = (|| -> anyhow::Result<()> {
                                        if scope == Scope::Worktree {
                                            app.execution
                                                .config
                                                .worktrees
                                                .entry(root.clone())
                                                .or_default()
                                                .extension_settings
                                                .insert(id.clone(), Value::Object(parsed.clone()));
                                            app.execution.save()?;
                                        } else {
                                            let target = if scope == Scope::Project {
                                                project.as_deref()
                                            } else {
                                                None
                                            };
                                            let mut settings = app
                                                .prefs
                                                .option(target, "extensions.settings")
                                                .cloned()
                                                .unwrap_or(json!({}));
                                            settings[&id] = Value::Object(parsed.clone());
                                            app.prefs.set_option(
                                                target,
                                                "extensions.settings",
                                                settings,
                                            );
                                            if id == SPRING_ID {
                                                for (key, value) in &parsed {
                                                    app.prefs.set_option(
                                                        target,
                                                        key,
                                                        value.clone(),
                                                    );
                                                }
                                            }
                                            app.prefs.try_save()?;
                                        }
                                        Ok(())
                                    })();
                                    match result {
                                        Ok(()) => {
                                            app.execution.message = Some(
                                            "설정을 저장했습니다. 다음 명령 실행부터 적용됩니다."
                                                .into(),
                                        );
                                            w.close_all_dialogs(cx);
                                        }
                                        Err(e) => app.execution.message = Some(e.to_string()),
                                    }
                                    cx.notify();
                                });
                            }),
                    ),
                )
        });
    }
}
enum ExtensionChange {
    Enabled(bool),
    StopRoots(Vec<PathBuf>),
    Remove,
    Install(PathBuf, bool),
}
fn input(text: &str, w: &mut Window, cx: &mut Context<Pokarak>) -> Entity<InputState> {
    cx.new(|cx| InputState::new(w, cx).default_value(text.to_owned()))
}
fn field(label: &str, input: &Entity<InputState>) -> Div {
    div()
        .v_flex()
        .gap_1()
        .child(label.to_owned())
        .child(Input::new(input).aria_label(label.to_owned()).small())
}
fn close() -> Button {
    Button::new("execution-dialog-close")
        .label("닫기")
        .small()
        .on_click(|_, w, cx| w.close_all_dialogs(cx))
}
fn parse_profile(
    name: &Entity<InputState>,
    program: &Entity<InputState>,
    args: &Entity<InputState>,
    cwd: &Entity<InputState>,
    env: &Entity<InputState>,
    root: &Path,
    cx: &App,
) -> anyhow::Result<TaskProfile> {
    let mut command = vec![program.read(cx).value().to_string()];
    let argv: Vec<String> = serde_json::from_str(args.read(cx).value().as_ref())
        .context("인수는 JSON 문자열 배열이어야 합니다")?;
    command.extend(argv);
    let profile = TaskProfile {
        name: name.read(cx).value().trim().to_owned(),
        command,
        cwd: PathBuf::from(cwd.read(cx).value().as_ref()),
        env: serde_json::from_str(env.read(cx).value().as_ref())
            .context("환경변수는 JSON 문자열 객체여야 합니다")?,
    };
    profile.validate(root)?;
    Ok(profile)
}
