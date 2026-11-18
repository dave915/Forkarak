//! Explicit, trust-gated native language tools over the exact retained source.
//! Observing/viewing code never starts a process. All server calls run off-thread.
mod model;
pub(super) mod on_type;
mod popup;
#[cfg(test)]
mod real;
#[cfg(test)]
mod tests;
mod views;
mod workspace;
use super::*;
use code_edit::FileKey;
use gpui_kit::component::input::{GutterDecoration, GutterDecorationCollection};
use model::*;
pub(super) use model::{Identity as RequestIdentity, configs as formatter_configs};
use pokarak_navigation::{
    CodeAction, CompletionItem, Diagnostic, DocumentSnapshot, DocumentSymbol, FormattingOptions,
    HoverResult, Position as LspPosition, SignatureHelp, TextEdit, TextRange, WorkspaceEdit,
};

actions!(
    forkarak_language,
    [Complete, Diagnostics, Symbols, Signature, Hover, Dismiss]
);

#[derive(Clone, Debug)]
pub(super) enum Tool {
    PrepareRename,
    Rename(String),
    QuickFix,
    Resolve(Arc<CodeAction>),
    Complete,
    Diagnostics,
    Symbols,
    Signature,
    Hover,
    Format { save_after: bool, commit: bool },
    Navigate(NavigationKind),
    Symbol,
}
impl Tool {
    pub(super) fn title(&self) -> &'static str {
        match self {
            Self::PrepareRename | Self::Rename(_) => "이름 바꾸기",
            Self::QuickFix | Self::Resolve(_) => "빠른 수정",
            Self::Complete => "자동완성",
            Self::Diagnostics => "오류 확인",
            Self::Symbols => "파일 구조",
            Self::Signature => "함수 도움말",
            Self::Hover => "타입 · 설명",
            Self::Format { .. } => "파일 포맷",
            Self::Navigate(NavigationKind::Definition) => "정의",
            Self::Navigate(NavigationKind::References) => "사용처",
            Self::Navigate(NavigationKind::Implementation) => "구현체",
            Self::Symbol => "정의 · 사용처",
        }
    }
}
enum Reply {
    RenameInput(TextRange, String),
    Actions(workspace::GroupedActions, Vec<Diagnostic>),
    Workspace(WorkspaceEdit),
    Complete(pokarak_navigation::CompletionResult),
    Diagnostics(Vec<Diagnostic>),
    Symbols(Vec<DocumentSymbol>),
    Signature(Option<SignatureHelp>),
    Hover(Option<HoverResult>),
    Format(Vec<TextEdit>),
    Navigation(NavigationResult, reader_navigation::ResultSource),
    Symbol(
        reader_navigation::SymbolNavigation,
        reader_navigation::ResultSource,
    ),
}
struct Panel {
    identity: Identity,
    tool: Tool,
    value: Reply,
    cursor: usize,
    collapsed: HashSet<usize>,
}

#[derive(Default)]
pub(super) struct LanguageTools {
    snapshots: HashMap<FileKey, DocumentSnapshot>,
    observers: HashMap<FileKey, (EntityId, Subscription)>,
    next_version: i64,
    on_type_task: Option<Task<()>>,
    input_languages: HashSet<String>,
    view: u64,
    request: u64,
    documentation_request: u64,
    active: Option<(FileKey, EntityId)>,
    configuration: Option<serde_json::Value>,
    server_configuration: Option<serde_json::Value>,
    pub pending: Option<Tool>,
    panel: Option<Panel>,
    input: Option<Entity<InputState>>,
    input_subscription: Option<Subscription>,
    focus: Option<FocusHandle>,
    scroll: UniformListScrollHandle,
    markers: Option<(Entity<EditorState>, GutterDecorationCollection)>,
    retry: Option<Tool>,
    result_sources: HashMap<PathBuf, DocumentSnapshot>,
    trust_request: Option<PendingAnalysis>,
    rename_input: Option<Entity<InputState>>,
    diagnostics: Option<(Identity, Vec<Diagnostic>)>,
}

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-space", Complete, Some("ForkarakCode")),
        KeyBinding::new("alt-f1", Hover, Some("ForkarakCode")),
        KeyBinding::new("escape", Dismiss, Some("ForkarakLanguage")),
    ]);
}
pub(super) fn actions(
    app: &Pokarak,
    root: Stateful<Div>,
    cx: &mut Context<Pokarak>,
) -> Stateful<Div> {
    let root = popup::actions(app, root, cx);
    root.when(app.language_target(cx).is_some(), |root| {
        root.on_action(
            cx.listener(|a, _: &Complete, w, cx| a.request_language(Tool::Complete, None, w, cx)),
        )
        .on_action(cx.listener(|a, action: &workspace::EditAt, w, cx| a.edit_at(action, w, cx)))
        .on_action(cx.listener(|a, _: &editor_tools::RenameSymbol, w, cx| {
            a.request_language(Tool::PrepareRename, None, w, cx)
        }))
        .on_action(cx.listener(|a, _: &editor_tools::QuickFix, w, cx| {
            a.request_language(Tool::QuickFix, None, w, cx)
        }))
        .on_action(cx.listener(|a, _: &FindImplementation, w, cx| {
            a.request_navigation(NavigationKind::Implementation, None, w, cx)
        }))
        .on_action(cx.listener(|a, _: &Diagnostics, w, cx| {
            a.request_language(Tool::Diagnostics, None, w, cx)
        }))
        .on_action(
            cx.listener(|a, _: &Symbols, w, cx| a.request_language(Tool::Symbols, None, w, cx)),
        )
        .on_action(
            cx.listener(|a, _: &Signature, w, cx| a.request_language(Tool::Signature, None, w, cx)),
        )
        .on_action(cx.listener(|a, _: &Hover, w, cx| a.request_language(Tool::Hover, None, w, cx)))
    })
    .on_action(cx.listener(|a, _: &Dismiss, w, cx| a.dismiss_language(w, cx)))
}

impl Pokarak {
    pub(super) fn language_settings(&self) -> serde_json::Value {
        let preferences = serde_json::Value::Array(
            [
                "language.servers",
                "language.enabled",
                "extensions.enabled",
                "extensions.installed",
                "language.completion",
                "language.diagnostics",
                "language.unsaved",
                "formatters.config",
                "editor.format_on_save",
                "editor.tab_size",
                "editor.use_spaces",
            ]
            .into_iter()
            .map(|k| {
                self.prefs
                    .option(self.current_project.as_deref(), k)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            })
            .collect(),
        );
        serde_json::json!([
            preferences,
            self.registered_extension_language_servers(),
            self.registered_extension_formatters()
        ])
    }
    fn language_trusted(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|p| self.prefs.trusted.contains(p))
            || self
                .current_project
                .as_ref()
                .is_some_and(|p| self.prefs.trusted.contains(p))
    }
    fn language_target(&self, cx: &App) -> Option<(FileKey, Entity<EditorState>)> {
        if self.git_history.active
            || self.loading
            || self.file_loading
            || self.opening_path.is_some()
            || !self.session().is_some_and(|s| s.saved.code)
        {
            return None;
        }
        let root = self.current.as_ref()?;
        let path = self.selected()?;
        let key = FileKey::new(root, &path)?; // external dependency sources stay read-only
        if self.snapshot().is_some_and(|s| {
            s.changes
                .iter()
                .any(|f| f.path == path && f.status == FileStatus::Deleted)
        }) {
            return None;
        }
        if !self.code_edits.can_edit(root, &path) {
            return None;
        } // historical, binary, deleted, invalid sources
        let editor = self.active_editor()?;
        let _ = editor.read(cx);
        Some((key, editor))
    }
    pub(super) fn invalidate_language_view(&mut self, cx: &mut Context<Self>) {
        self.language_tools.on_type_task = None;
        self.language_tools.view = self.language_tools.view.wrapping_add(1);
        self.language_tools.request = self.language_tools.request.wrapping_add(1);
        self.language_tools.documentation_request =
            self.language_tools.documentation_request.wrapping_add(1);
        for snapshot in self.language_tools.snapshots.values() {
            snapshot.invalidate();
        }
        self.language_tools.pending = None;
        if self.language_tools.trust_request.take().is_some() {
            self.trust_prompt = false;
        }
        self.language_tools.panel = None;
        self.language_tools.rename_input = None;
        self.navigating = false;
        self.clear_language_markers(cx);
    }
    fn clear_language_markers(&mut self, cx: &mut Context<Self>) {
        self.language_tools.diagnostics = None;
        if let Some((editor, collection)) = self.language_tools.markers.take() {
            collection.dispose(cx);
            editor.update(cx, |e, cx| {
                if let Some(d) = e.diagnostics_mut() {
                    d.clear();
                }
                cx.notify();
            });
        }
    }
    pub(super) fn sync_language_tools(&mut self, cx: &mut Context<Self>) {
        let active = self.language_target(cx).map(|(k, e)| (k, e.entity_id()));
        let settings = self.language_settings();
        if self.language_tools.active != active {
            self.invalidate_language_view(cx);
            self.language_tools.active = active;
        }
        if self.language_tools.configuration.as_ref() != Some(&settings) {
            if self.language_tools.configuration.is_some() && self.navigation.is_some() {
                self.stop_analysis(cx);
                self.status =
                    "분석 설정이 변경되었습니다. 다음 명시적 요청에서 새 설정을 사용합니다.".into();
            }
            self.language_tools.configuration = Some(settings);
        }
        if self.navigation.is_some() && !self.language_trusted() {
            self.stop_analysis(cx);
        }
        if let Some((key, editor)) = self.language_target(cx) {
            let id = editor.entity_id();
            if self
                .language_tools
                .observers
                .get(&key)
                .is_none_or(|(old, _)| *old != id)
            {
                let observed = key.clone();
                let subscription = cx.observe(&editor, move |a, editor, cx| {
                    if a.language_tools
                        .observers
                        .get(&observed)
                        .is_none_or(|(old, _)| *old != editor.entity_id())
                    {
                        return;
                    }
                    let changed = a.language_tools.snapshots.get(&observed).is_some_and(|s| {
                        s.is_current() && s.text() != editor.read(cx).value().as_ref()
                    }) || a
                        .language_tools
                        .result_sources
                        .get(&observed.root.join(&observed.path))
                        .is_some_and(|s| {
                            s.is_current() && s.text() != editor.read(cx).value().as_ref()
                        });
                    if changed {
                        if let Some(snapshot) = a.language_tools.snapshots.get(&observed) {
                            snapshot.invalidate();
                        }
                        if a.current.as_ref() != Some(&observed.root) {
                            return;
                        }
                        // A sibling dirty buffer is context for this request as well.
                        a.language_tools.request = a.language_tools.request.wrapping_add(1);
                        a.language_tools.pending = None;
                        if a.language_tools.trust_request.take().is_some() {
                            a.trust_prompt = false;
                        }
                        a.language_tools.panel = None;
                        if a.result_semantic && a.results_root.as_ref() == Some(&observed.root) {
                            a.results_open = false;
                            for source in a.language_tools.result_sources.values() {
                                source.invalidate();
                            }
                            a.language_tools.result_sources.clear();
                        }
                        a.clear_language_markers(cx);
                        a.navigating = false;
                        cx.notify();
                    }
                });
                self.language_tools
                    .observers
                    .insert(key, (id, subscription));
            }
        }
    }
    pub(super) fn snapshot_for(
        &mut self,
        key: &FileKey,
        text: String,
    ) -> Result<DocumentSnapshot, String> {
        if let Some(s) = self
            .language_tools
            .snapshots
            .get(key)
            .filter(|s| s.is_current() && s.text() == text)
        {
            return Ok(s.clone());
        }
        if let Some(old) = self.language_tools.snapshots.remove(key) {
            old.invalidate();
        }
        self.language_tools.next_version = self
            .language_tools
            .next_version
            .checked_add(1)
            .filter(|v| *v <= i32::MAX as i64)
            .ok_or("분석 버전 한도에 도달했습니다. 앱을 다시 여세요.")?;
        let snapshot = DocumentSnapshot::new(
            key.root.clone(),
            key.path.clone(),
            text,
            self.language_tools.next_version,
        )
        .map_err(|e| format!("{e:#}"))?;
        self.language_tools
            .snapshots
            .insert(key.clone(), snapshot.clone());
        Ok(snapshot)
    }
    fn language_identity(&mut self, cx: &App) -> Result<Identity, String> {
        let (key, editor) = self
            .language_target(cx)
            .ok_or("현재 워크트리의 코드 파일을 선택하세요. 이력·삭제 파일은 분석하지 않습니다.")?;
        let snapshot = self.snapshot_for(&key, editor.read(cx).value().to_string())?;
        Ok(Identity {
            snapshot,
            key,
            editor: editor.entity_id(),
            selection: editor.read(cx).selected_range(),
            cursor: editor.read(cx).cursor(),
            settings: self.language_settings(),
            view: self.language_tools.view,
        })
    }
    fn accepts_language(&mut self, expected: &Identity, selection: bool, cx: &App) -> bool {
        self.language_trusted()
            && self
                .language_identity(cx)
                .is_ok_and(|current| expected.matches(&current, selection))
    }
    pub(super) fn language_request_identity(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<RequestIdentity, String> {
        self.sync_language_tools(cx);
        self.language_identity(cx)
    }
    pub(super) fn resume_language_request(
        &mut self,
        identity: RequestIdentity,
        tool: Tool,
        requested: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Preparation installs tools, never grants project trust. A still-matching
        // untrusted request returns through the ordinary explicit trust prompt.
        if self
            .language_identity(cx)
            .is_ok_and(|current| identity.matches(&current, true))
        {
            // Explicit Continue may follow a new installation. The old
            // Navigator's immutable defaults must not survive that change.
            let edit_approved = self.navigation.is_some()
                && self.language_trusted()
                && matches!(tool, Tool::PrepareRename | Tool::QuickFix);
            self.stop_analysis(cx);
            if edit_approved {
                self.enable_analysis(cx);
            }
            self.request_language(tool, requested, window, cx);
        } else {
            self.status =
                "설치 중 선택·소스·설정이 변경되었습니다. 원하는 파일에서 분석을 다시 요청하세요."
                    .into();
            cx.notify();
        }
    }
    pub(super) fn external_format_identity(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Identity, String> {
        self.language_request_identity(cx)
    }
    pub(super) fn accepts_external_format(&mut self, identity: &Identity, cx: &App) -> bool {
        self.accepts_language(identity, true, cx)
    }
    pub(super) fn semantic_result_sources(&self) -> HashMap<PathBuf, DocumentSnapshot> {
        self.language_tools.result_sources.clone()
    }
    pub(super) fn semantic_result_source(&self, path: &Path) -> Option<DocumentSnapshot> {
        self.language_tools.result_sources.get(path).cloned()
    }

    pub(super) fn restore_result_sources(&mut self, sources: HashMap<PathBuf, DocumentSnapshot>) {
        self.language_tools.result_sources = sources;
    }

    pub(super) fn semantic_result_sources_current(&self, cx: &App) -> bool {
        let Some(root) = self
            .current
            .as_ref()
            .filter(|root| self.results_root.as_ref() == Some(root))
        else {
            return false;
        };
        if !self.language_trusted()
            || self.navigation.is_none()
            || self.language_tools.result_sources.is_empty()
        {
            return false;
        }
        let drafts = self.code_edits.analysis_drafts(root);
        if drafts.iter().any(|(path, text)| {
            self.language_tools
                .result_sources
                .get(&root.join(path))
                .is_none_or(|s| s.text() != text)
        }) {
            return false;
        }
        self.language_tools.result_sources.values().all(|source| {
            if !source.is_current() || source.root() != root {
                return false;
            }
            let Ok(path) = source.path().strip_prefix(root) else {
                return false;
            };
            if self
                .result_revisions
                .get(source.path())
                .is_some_and(|revision| file_revision(source.path()).as_ref() != Some(revision))
            {
                return false;
            }
            if let Some(editor) = self.sessions.get(root).and_then(|s| s.documents.get(path)) {
                return editor.read(cx).value().as_ref() == source.text();
            }
            drafts
                .iter()
                .any(|(draft_path, text)| draft_path == path && text == source.text())
        })
    }
    fn retain_navigation_results(
        &mut self,
        root: &Path,
        response: reader_navigation::ResultSource,
    ) -> anyhow::Result<()> {
        // Request snapshots expire when the view changes. Result snapshots have
        // their own lifetime, checked against every retained editor on activation.
        // This keeps usages usable after the automatic definition jump without
        // relaxing the request identity or displayed-source guards.
        let sources = self
            .language_tools
            .snapshots
            .values()
            .filter(|s| s.is_current() && s.root() == root)
            .map(|s| {
                DocumentSnapshot::new(
                    root.to_owned(),
                    s.path().to_owned(),
                    s.text().to_owned(),
                    s.version(),
                )
                .map(|result| (result.path().to_owned(), result))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;
        let mut query = response.query;
        query.source = sources
            .get(query.source.path())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("탐색을 요청한 소스가 변경되었습니다."))?;
        // Following a result can replace a preview tab. Retain the originating
        // reader so its exact displayed source remains available for Back/rerun.
        self.pin_preview(root, query.source.path().strip_prefix(root)?);
        self.result_query = Some(query);
        self.result_semantic = true;
        self.result_revisions = response.revisions;
        self.result_revisions.extend(
            sources
                .keys()
                .filter_map(|p| file_revision(p).map(|r| (p.clone(), r))),
        );
        self.language_tools.result_sources = sources;
        self.results_root = Some(root.to_owned());
        Ok(())
    }
    pub(super) fn configure_language_servers(
        &self,
    ) -> Result<Vec<pokarak_navigation::ServerConfig>, String> {
        let custom = configs(
            self.prefs
                .option(self.current_project.as_deref(), "language.servers"),
        )?;
        let mut all = pokarak_navigation::default_configs();
        overlay_configs(&mut all, self.effective_managed_server_configs()?);
        // Discovery is read-only. Only the explicitly requested Navigator starts
        // one enabled extension tool; ordinary file viewing does not start it.
        overlay_configs(
            &mut all,
            self.registered_extension_language_servers()
                .into_iter()
                .map(|r| pokarak_navigation::ServerConfig {
                    language: r.language,
                    program: r.executable.to_string_lossy().into(),
                    args: r.args,
                })
                .collect(),
        );
        // Preserve the previous user configuration, then apply project/app UI
        // overrides last. Navigator receives None to avoid reapplying the file.
        if let Some(path) = pokarak_navigation::NavigatorOptions::default().config_file {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text)
                        .map_err(|e| format!("언어 서버 설정 {}: {e}", path.display()))?;
                    if let Some(servers) = value.get("servers") {
                        overlay_configs(
                            &mut all,
                            serde_json::from_value::<Vec<pokarak_navigation::ServerConfig>>(
                                servers.clone(),
                            )
                            .map_err(|e| format!("언어 서버 설정 {}: {e}", path.display()))?,
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("언어 서버 설정 {}: {error}", path.display())),
            }
        }
        overlay_configs(&mut all, custom);
        all.retain(|config| self.managed_language_enabled(&config.language));
        Ok(all)
    }
    pub(super) fn remember_language_configuration(
        &mut self,
        signature: serde_json::Value,
        configs: &[pokarak_navigation::ServerConfig],
    ) {
        // Probe once when analysis is explicitly activated. Missing programs
        // must not be searched for or repeatedly launched by editor keystrokes.
        self.language_tools.input_languages = configs
            .iter()
            .filter(|config| pokarak_navigation::discover_program(&config.program).is_some())
            .map(|config| config.language.clone())
            .collect();
        self.language_tools.server_configuration = Some(signature);
    }
    pub(super) fn configure_language_formatters(
        &self,
    ) -> Result<Vec<pokarak_navigation::FormatterConfig>, String> {
        let mut all: Vec<_> = self
            .effective_managed_formatter_configs()?
            .into_iter()
            .map(|c| pokarak_navigation::ServerConfig {
                language: c.language,
                program: c.program,
                args: c.args,
            })
            .collect();
        overlay_configs(
            &mut all,
            self.registered_extension_formatters()
                .into_iter()
                .map(|r| pokarak_navigation::ServerConfig {
                    language: r.language,
                    program: r.executable.to_string_lossy().into(),
                    args: r.args,
                })
                .collect(),
        );
        overlay_configs(
            &mut all,
            configs(
                self.prefs
                    .option(self.current_project.as_deref(), "formatters.config"),
            )?,
        );
        all.retain(|c| self.managed_language_enabled(&c.language));
        Ok(all
            .into_iter()
            .map(|c| pokarak_navigation::FormatterConfig {
                language: c.language,
                program: c.program,
                args: c.args,
            })
            .collect())
    }

    fn language_configuration_signature(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!([
            self.configure_language_servers()?,
            self.configure_language_formatters()?
        ]))
    }
    fn extension_tool_for(&self, tool: &Tool, path: &Path) -> bool {
        let language =
            pokarak_navigation::language_for_path(path).unwrap_or_else(|| language(path));
        let family = pokarak_navigation::language_family(language);
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let tools = if matches!(tool, Tool::Format { .. }) {
            self.registered_extension_formatters()
        } else {
            self.registered_extension_language_servers()
        };
        tools.iter().any(|r| {
            r.language == language
                || r.language == family
                || r.file_extensions
                    .iter()
                    .any(|e| e.trim_start_matches('.') == extension)
        })
    }
    pub(super) fn approve_analysis_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The button is the approval boundary. Construction before this point
        // never grants trust, launches a server or imports a Java project.
        let pending = self.language_tools.trust_request.take();
        let resume = if let Some(pending) = pending {
            let current = self.language_identity(cx).ok();
            let Some(request) = pending.resume(current.as_ref()) else {
                self.cancel_analysis_prompt(cx);
                self.status =
                    "선택 또는 소스가 변경되었습니다. 원하는 위치에서 다시 요청하세요.".into();
                return;
            };
            Some(request)
        } else {
            None
        };
        self.enable_analysis(cx);
        if self.navigation.is_some()
            && let Some((tool, position)) = resume
        {
            self.request_language(tool, position, window, cx);
        }
    }
    pub(super) fn cancel_analysis_prompt(&mut self, cx: &mut Context<Self>) {
        self.language_tools.trust_request = None;
        self.trust_prompt = false;
        cx.notify();
    }
    pub(super) fn show_explicit_analysis_prompt(&mut self) {
        self.language_tools.trust_request = None;
        self.trust_prompt = true;
    }
    pub(super) fn request_language(
        &mut self,
        tool: Tool,
        requested: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.language_tools.on_type_task = None;
        // Another explicit action replaces a waiting gesture. Approval takes
        // the pending request before returning through this ordinary dispatcher.
        self.language_tools.trust_request = None;
        if window.has_active_dialog(cx) || self.git_is_busy() || self.active_worktree_has_git_job()
        {
            return;
        }
        self.sync_language_tools(cx);
        if self.language_target(cx).is_none() {
            self.status = "현재 워크트리의 코드 파일을 선택하세요. 이력은 읽기 전용입니다.".into();
            cx.notify();
            return;
        }
        let project_root = self.current_project.clone();
        let project = project_root.as_deref();
        let setting = match tool {
            Tool::Complete => Some("language.completion"),
            Tool::Diagnostics => Some("language.diagnostics"),
            _ => None,
        };
        if setting.is_some_and(|key| !self.prefs.option_bool(project, key, true)) {
            self.status = format!(
                "{} 기능이 설정에서 꺼져 있습니다. 언어 분석 설정에서 켜세요.",
                tool.title()
            );
            cx.notify();
            return;
        }
        let (key, _) = self.language_target(cx).unwrap();
        let language =
            pokarak_navigation::language_for_path(&key.path).unwrap_or_else(|| language(&key.path));
        let family = pokarak_navigation::language_family(language);
        if self
            .prefs
            .option(project, "language.enabled")
            .and_then(|v| v.get(language).or_else(|| v.get(family)))
            .and_then(|v| v.as_bool())
            == Some(false)
        {
            self.status =
                format!("{language} 언어 지원이 꺼져 있습니다. 언어 지원 관리에서 켜세요.");
            cx.notify();
            return;
        }
        if !matches!(tool, Tool::Format { .. })
            && !self.prefs.option_bool(project, "language.unsaved", true)
            && !self.code_edits.analysis_drafts(&key.root).is_empty()
        {
            self.status="미저장 코드 분석이 꺼져 있습니다. 저장하거나 언어 분석 설정에서 허용하세요. 디스크 코드로 대신 분석하지 않습니다.".into();
            cx.notify();
            return;
        }
        if matches!(tool, Tool::PrepareRename | Tool::QuickFix)
            && (self.navigation.is_none() || !self.language_trusted())
        {
            if let Ok(identity) = self.language_identity(cx) {
                self.language_tools.trust_request = Some(PendingAnalysis::Edit(identity, tool));
                self.trust_prompt = true;
                self.status = "이 워크트리의 분석을 허용하면 요청을 계속합니다.".into();
                cx.notify();
            }
            return;
        }
        if !self.extension_tool_for(&tool, &key.path)
            && self.prepare_managed_language_request(tool.clone(), requested, window, cx)
        {
            return;
        }
        if !self.language_trusted() {
            self.language_tools.trust_request = self
                .language_identity(cx)
                .ok()
                .and_then(|identity| PendingSymbolRequest::remember(&tool, identity, requested))
                .map(PendingAnalysis::Symbol);
            self.trust_prompt = true;
            self.status = if self.language_tools.trust_request.is_some() {
                "이 워크트리의 분석을 허용하면 클릭한 심볼의 정의와 사용처를 찾습니다."
            } else {
                "이 워크트리의 분석을 허용한 뒤 명령을 다시 실행하세요."
            }
            .into();
            cx.notify();
            return;
        }
        if matches!(tool, Tool::Complete | Tool::Format { .. })
            && !self.code_edits.editing(&key.root, &key.path)
        {
            self.start_code_edit(window, cx);
        }
        if matches!(tool, Tool::Complete | Tool::Format { .. })
            && !self.code_edits.editing(&key.root, &key.path)
        {
            return;
        }
        // Refresh discovery only on an explicit request. This also detects a
        // manual install/update without making file rendering wait for discovery.
        let configuration = match self.language_configuration_signature() {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        if self.navigation.is_some()
            && self.language_tools.server_configuration.as_ref() != Some(&configuration)
        {
            self.stop_analysis(cx);
        }
        if self.navigation.is_none() {
            self.enable_analysis(cx);
        }
        self.run_language_request(tool, requested, false, window, cx);
    }

    fn run_language_request(
        &mut self,
        tool: Tool,
        requested: Option<Position>,
        automatic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((key, editor)) = self.language_target(cx) else {
            return;
        };
        let Some(navigator) = self.navigation.clone() else {
            return;
        };
        // Capture every retained dirty source before taking the background mutex.
        self.capture_code_buffers(cx);
        let drafts = self.code_edits.analysis_drafts(&key.root);
        let retained = match drafts
            .into_iter()
            .map(|(path, text)| {
                self.snapshot_for(
                    &FileKey {
                        root: key.root.clone(),
                        path,
                    },
                    text,
                )
            })
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(v) => v,
            Err(e) => {
                if !automatic {
                    self.error = Some(e);
                    cx.notify();
                }
                return;
            }
        };
        let identity = match self.language_identity(cx) {
            Ok(v) => v,
            Err(e) => {
                if !automatic {
                    self.error = Some(e);
                    cx.notify();
                }
                return;
            }
        };
        let Some(target) = reader_navigation::navigation_target(
            editor.read(cx).text(),
            identity.cursor,
            requested,
        ) else {
            return;
        };
        let _symbol_start = target.symbol_start;
        let position = LspPosition {
            line: target.position.line,
            character: target.position.character,
        };
        let anchor = editor
            .read(cx)
            .range_to_bounds(&(identity.cursor..identity.cursor));
        let project = self.current_project.as_deref();
        let options = FormattingOptions {
            tab_size: self
                .prefs
                .option_usize(project, "editor.tab_size", 4)
                .clamp(1, 32) as u32,
            insert_spaces: self.prefs.option_bool(project, "editor.use_spaces", true),
        };
        if matches!(tool, Tool::Navigate(_) | Tool::Symbol) {
            self.navigation_request += 1;
        }
        // A fresh explicit request supersedes any open edit list or preview.
        if !automatic {
            self.language_tools.panel = None;
        }
        let release: Vec<_> = self
            .language_tools
            .snapshots
            .values()
            .filter(|s| !s.is_current() && s.root() == identity.snapshot.root())
            .map(|s| s.path().to_owned())
            .collect();
        self.language_tools.request = self.language_tools.request.wrapping_add(1);
        let token = self.language_tools.request;
        let navigation_token = self.navigation_request;
        self.language_tools.pending = Some(tool.clone());
        self.navigating = matches!(tool, Tool::Navigate(_) | Tool::Symbol);
        if !automatic {
            self.language_tools.retry = Some(match &tool {
                Tool::Rename(_) => Tool::PrepareRename,
                Tool::Resolve(_) => Tool::QuickFix,
                other => other.clone(),
            });
            self.status = format!("{} 요청 중…", tool.title());
        }
        let expected = identity.clone();
        let operation = tool.clone();
        let task = cx.background_spawn(async move {
            let result = (|| {
                let mut nav = navigator
                    .lock()
                    .map_err(|_| anyhow::anyhow!("분석 서버 상태를 읽을 수 없습니다"))?;
                let mut result = (|| {
                    for path in release {
                        nav.release_buffer(&path)?;
                    }
                    execute(
                        &mut nav,
                        &expected.snapshot,
                        &retained,
                        operation,
                        position,
                        &options,
                    )
                })();
                let output = if automatic {
                    if let Ok(Reply::Complete(items)) = &mut result {
                        on_type::rank_input_candidates(
                            items,
                            expected.snapshot.text(),
                            expected.cursor,
                        );
                    }
                    String::new()
                } else {
                    format!("{}\n{}", nav.server_messages(), nav.stderr())
                };
                anyhow::Ok((result, output))
            })();
            match result {
                Ok(v) => v,
                Err(e) => (Err(e), String::new()),
            }
        });
        cx.spawn_in(window, async move |app, cx| {
            let (result, output) = task.await;
            let _ = app.update_in(cx, |a, window, cx| {
                if a.language_tools.request != token {
                    return;
                }
                a.language_tools.pending = None;
                a.navigating = false;
                if a.navigation_request != navigation_token
                    || !a.accepts_language(&identity, true, cx)
                {
                    if !automatic {
                        a.status =
                            "선택 또는 소스가 변경되어 오래된 분석 결과를 적용하지 않았습니다."
                                .into();
                    }
                    cx.notify();
                    return;
                }
                if matches!(
                    tool,
                    Tool::PrepareRename | Tool::QuickFix | Tool::Rename(_) | Tool::Resolve(_)
                ) && (window.has_active_dialog(cx)
                    || a.git_is_busy()
                    || a.active_worktree_has_git_job())
                {
                    cx.notify();
                    return;
                }
                if automatic {
                    if !a.can_show_input_assistance(&identity.key, &editor, &tool, window, cx)
                        || matches!(&result, Err(_) | Ok(Reply::Signature(None)))
                        || matches!(&result, Ok(Reply::Complete(items)) if items.items.is_empty())
                    {
                        cx.notify();
                        return;
                    }
                } else {
                    a.observe_feedback();
                    a.analysis_feedback.append(&output);
                }
                match result {
                    Ok(Reply::Workspace(edit)) => {
                        a.error = None;
                        a.present_workspace_edit(identity, tool, edit, window, cx);
                    }
                    Ok(Reply::Format(edits)) => {
                        let result = TextEdit::apply_all(identity.snapshot.text(), &edits)
                            .map_err(|e| format!("{e:#}"))
                            .and_then(|text| {
                                a.apply_editor_draft(
                                    &identity.key.root,
                                    &identity.key.path,
                                    identity.editor,
                                    identity.snapshot.text(),
                                    text,
                                    window,
                                    cx,
                                )
                            });
                        match result {
                            Ok(()) => {
                                a.status =
                                    "포맷 결과를 초안에 반영했습니다. ⌘S로 저장하세요.".into();
                                if let Tool::Format {
                                    save_after: true,
                                    commit,
                                } = tool
                                {
                                    a.save_code_file_after_format(commit, window, cx);
                                }
                            }
                            Err(e) => a.error = Some(e),
                        }
                    }
                    Ok(Reply::Navigation(result, revisions)) => {
                        a.error = None;
                        if let Err(error) =
                            a.retain_navigation_results(&identity.key.root, revisions)
                        {
                            a.error = Some(format!("{error:#}"));
                            cx.notify();
                            return;
                        }
                        a.status = result.status.clone();
                        if matches!(
                            tool,
                            Tool::Navigate(
                                NavigationKind::Definition | NavigationKind::Implementation
                            )
                        ) && result.semantic
                            && result.locations.len() == 1
                        {
                            a.results_open = false;
                            a.open_location(result.locations[0].clone(), window, cx);
                        } else {
                            a.show_results(tool.title().into(), result, window, cx);
                            a.result_anchor = anchor;
                        }
                    }
                    Ok(Reply::Symbol(result, revisions)) => {
                        a.error = None;
                        if let Err(error) =
                            a.retain_navigation_results(&identity.key.root, revisions)
                        {
                            a.error = Some(format!("{error:#}"));
                            cx.notify();
                            return;
                        }
                        a.show_symbol_navigation(result, anchor, window, cx);
                    }
                    Ok(value) => {
                        if let Reply::Diagnostics(items) | Reply::Actions(_, items) = &value {
                            a.install_language_diagnostics(items, cx);
                            a.language_tools.diagnostics = Some((identity.clone(), items.clone()));
                        }
                        if !automatic {
                            a.error = None;
                            a.status = format!("{} 완료 · 현재 표시 소스 기준", tool.title());
                        }
                        a.language_tools.panel = Some(Panel {
                            identity,
                            tool: tool.clone(),
                            value,
                            cursor: 0,
                            collapsed: HashSet::new(),
                        });
                        a.prepare_language_panel(window, cx);
                        if matches!(tool, Tool::Complete) {
                            a.resolve_candidate(window, cx);
                        }
                    }
                    Err(e) => {
                        if let Tool::Navigate(kind) = &tool {
                            a.record_analysis_failure(
                                identity.key.root.clone(),
                                identity.key.path.clone(),
                                kind.clone(),
                                Position::new(position.line, position.character),
                                format!("{}: {e:#}", tool.title()),
                            );
                        }
                        a.error = Some(format!("{}: {e:#}", tool.title()));
                        a.status = "분석 실패 · Details 또는 다시 시도에서 확인하세요.".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn install_language_diagnostics(&mut self, items: &[Diagnostic], cx: &mut Context<Self>) {
        self.clear_language_markers(cx);
        let Some(editor) = self.active_editor() else {
            return;
        };
        let marks = items
            .iter()
            .filter_map(|d| {
                let e = editor.read(cx);
                let a = offset(e.text(), d.range.start)?;
                let b = offset(e.text(), d.range.end)?;
                Some(GutterDecoration::new(a..b).with_color(rgb(severity(d.severity).1).into()))
            })
            .collect();
        let collection = editor.update(cx, |e, cx| {
            let text = e.text().clone();
            if let Some(set) = e.diagnostics_mut() {
                set.reset(&text);
                for d in items {
                    let (Some(start), Some(end)) = (
                        reader_navigation::editor_position(
                            &text,
                            Position::new(d.range.start.line, d.range.start.character),
                        ),
                        reader_navigation::editor_position(
                            &text,
                            Position::new(d.range.end.line, d.range.end.character),
                        ),
                    ) else {
                        continue;
                    };
                    use gpui_kit::base::input::{
                        Diagnostic as NativeDiagnostic, DiagnosticSeverity as S,
                    };
                    let sev = match d.severity {
                        Some(1) => S::Error,
                        Some(2) => S::Warning,
                        Some(4) => S::Hint,
                        _ => S::Info,
                    };
                    let mut item =
                        NativeDiagnostic::new(start..end, d.message.clone()).with_severity(sev);
                    if let Some(s) = &d.source {
                        item = item.with_source(s.clone());
                    }
                    if let Some(code) = &d.code {
                        item = item.with_code(
                            code.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| code.to_string()),
                        );
                    }
                    set.push(item);
                }
            }
            e.create_gutter_decorations_collection(marks, cx)
        });
        self.language_tools.markers = Some((editor, collection));
    }
    fn dismiss_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.language_tools.dismiss_panel();
        if let Some(e) = self.active_editor() {
            e.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    fn resolve_candidate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.language_tools.panel else {
            return;
        };
        let Reply::Complete(items) = &panel.value else {
            return;
        };
        let Some(item) = items.items.get(panel.cursor).cloned() else {
            return;
        };
        let identity = panel.identity.clone();
        let index = panel.cursor;
        if !self.accepts_language(&identity, true, cx) {
            return;
        }
        let Some(nav) = self.navigation.clone() else {
            return;
        };
        self.language_tools.documentation_request =
            self.language_tools.documentation_request.wrapping_add(1);
        let token = self.language_tools.documentation_request;
        let snapshot = identity.snapshot.clone();
        let task = cx.background_spawn(async move {
            let mut nav = nav.lock().ok()?;
            popup::resolved_candidate(&mut nav, &snapshot, item)
        });
        cx.spawn_in(window, async move |app, cx| {
            let Some(resolved) = task.await else {
                return;
            };
            let _ = app.update_in(cx, |a, _, cx| {
                if a.language_tools.documentation_request != token
                    || !a.accepts_language(&identity, true, cx)
                {
                    return;
                }
                let Some(panel) = a
                    .language_tools
                    .panel
                    .as_mut()
                    .filter(|p| p.cursor == index && p.identity.matches(&identity, true))
                else {
                    return;
                };
                let Reply::Complete(items) = &mut panel.value else {
                    return;
                };
                let Some(original) = items.items.get_mut(index) else {
                    return;
                };
                if resolved.label != original.label {
                    return;
                }
                *original = resolved;
                cx.notify();
            });
        })
        .detach();
    }
    fn apply_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.language_tools.panel else {
            return;
        };
        let Reply::Complete(items) = &panel.value else {
            return;
        };
        let Some(item) = items.items.get(panel.cursor).cloned() else {
            return;
        };
        let identity = panel.identity.clone();
        if !self.accepts_language(&identity, true, cx) {
            self.status =
                "선택·초안이 변경되어 완성을 적용하지 않았습니다. 다시 요청하세요.".into();
            self.dismiss_language(window, cx);
            return;
        }
        let result = completion_edit(&identity, &item).and_then(|(text, caret)| {
            self.apply_editor_draft(
                &identity.key.root,
                &identity.key.path,
                identity.editor,
                identity.snapshot.text(),
                text,
                window,
                cx,
            )?;
            if let Some(editor) = self.active_editor() {
                editor.update(cx, |e, cx| {
                    let position = e.text().offset_to_position(caret);
                    e.set_cursor_position(position, window, cx);
                });
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                self.status = "완성을 초안에 반영했습니다.".into();
                self.dismiss_language(window, cx);
            }
            Err(e) => {
                self.status = e;
                cx.notify();
            }
        }
    }
    fn jump_language(
        &mut self,
        position: LspPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(identity) = self
            .language_tools
            .panel
            .as_ref()
            .map(|p| p.identity.clone())
        else {
            return;
        };
        if !self.accepts_language(&identity, false, cx) {
            return;
        }
        if let Some(editor) = self.active_editor() {
            let point = reader_navigation::editor_position(
                editor.read(cx).text(),
                Position::new(position.line, position.character),
            );
            if let Some(point) = point {
                if let Some(previous) = self.reading_point(cx)
                    && let Some(session) = self.sessions.get_mut(&identity.key.root)
                {
                    session.history.push(previous);
                    session.forward.clear();
                }
                editor.update(cx, |e, cx| e.set_cursor_position(point, window, cx));
                editor.read(cx).focus_handle(cx).focus(window, cx);
            }
        }
        self.persist(cx);
        cx.notify();
    }
}
fn severity(value: Option<u32>) -> (&'static str, u32) {
    match value {
        Some(1) => ("오류", RED),
        Some(2) => ("경고", 0xe4bb79),
        Some(4) => ("힌트", MUTED),
        _ => ("정보", BLUE),
    }
}

/// Shared dispatch used by the native commands and authored protocol fixtures.
fn execute(
    nav: &mut Navigator,
    snapshot: &DocumentSnapshot,
    retained: &[DocumentSnapshot],
    tool: Tool,
    position: LspPosition,
    options: &FormattingOptions,
) -> anyhow::Result<Reply> {
    for source in retained {
        nav.retain_buffer(source)?;
    }
    anyhow::ensure!(snapshot.is_current(), "요청한 소스가 변경되었습니다");
    Ok(match tool {
        Tool::PrepareRename => {
            let (range, name) = nav
                .prepare_rename_buffer(snapshot, position.line, position.character)?
                .value
                .ok_or_else(|| {
                    anyhow::anyhow!("이 위치에서 이름을 바꿀 수 있는 심볼이 없습니다.")
                })?;
            Reply::RenameInput(range, name)
        }
        Tool::Rename(name) => Reply::Workspace(
            nav.rename_buffer(snapshot, position.line, position.character, &name)?
                .value,
        ),
        Tool::QuickFix => {
            let capabilities = nav.capabilities_buffer(snapshot)?.value;
            let provider = &capabilities["codeActionProvider"];
            anyhow::ensure!(
                provider == true || provider.is_object(),
                "이 언어 서버는 빠른 수정을 지원하지 않습니다"
            );
            let diagnostics = nav
                .diagnostics(snapshot)
                .map(|r| r.value)
                .unwrap_or_default();
            anyhow::ensure!(snapshot.is_current(), "요청한 소스가 변경되었습니다");
            let (range, context) = workspace::action_context(position, &diagnostics);
            let actions = nav.code_actions_buffer(snapshot, range, &context)?.value;
            Reply::Actions(workspace::GroupedActions::new(actions), diagnostics)
        }
        Tool::Resolve(action) => {
            let action = nav.resolve_code_action_buffer(snapshot, &action)?.value;
            anyhow::ensure!(
                action.disabled.is_none(),
                "{}",
                action.disabled.as_deref().unwrap_or_default()
            );
            Reply::Workspace(
                action
                    .edit
                    .ok_or_else(|| anyhow::anyhow!("이 항목에 적용할 수정이 없습니다."))?,
            )
        }
        Tool::Complete => Reply::Complete(
            nav.complete_buffer(snapshot, position.line, position.character)?
                .value,
        ),
        Tool::Diagnostics => Reply::Diagnostics(nav.diagnostics(snapshot)?.value),
        Tool::Symbols => Reply::Symbols(nav.document_symbols(snapshot)?.value),
        Tool::Signature => Reply::Signature(
            nav.signature_help_buffer(snapshot, position.line, position.character)?
                .value,
        ),
        Tool::Hover => Reply::Hover(
            nav.hover_buffer(snapshot, position.line, position.character)?
                .value,
        ),
        Tool::Format { .. } => Reply::Format(nav.format_buffer(snapshot, options)?.value),
        Tool::Navigate(kind) => {
            let result = nav
                .navigate_buffer(snapshot, position.line, position.character, kind.clone())?
                .value;
            let revisions = result_revisions(&result.locations);
            Reply::Navigation(
                result,
                reader_navigation::ResultSource::new(
                    snapshot,
                    position,
                    Tool::Navigate(kind),
                    revisions,
                ),
            )
        }
        Tool::Symbol => {
            let definitions = nav
                .navigate_buffer(
                    snapshot,
                    position.line,
                    position.character,
                    NavigationKind::Definition,
                )
                .map(|r| r.value);
            // Both calls use this exact snapshot, position, retained context and
            // Navigator lock. A stale source must never start the second call.
            anyhow::ensure!(snapshot.is_current(), "요청한 소스가 변경되었습니다");
            let usages = nav
                .navigate_buffer(
                    snapshot,
                    position.line,
                    position.character,
                    NavigationKind::References,
                )
                .map(|r| r.value);
            anyhow::ensure!(snapshot.is_current(), "요청한 소스가 변경되었습니다");
            let result = reader_navigation::SymbolNavigation::from_results(definitions, usages);
            let hits: Vec<_> = result
                .definitions
                .iter()
                .chain(&result.usages)
                .cloned()
                .collect();
            let revisions = result_revisions(&hits);
            Reply::Symbol(
                result,
                reader_navigation::ResultSource::new(snapshot, position, Tool::Symbol, revisions),
            )
        }
    })
}
