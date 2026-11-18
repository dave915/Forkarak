//! App-owned language preparation. Reading a file or constructing this state never
//! installs tools, grants project trust, or starts a language server.
mod policy;
#[cfg(test)]
mod tests;
mod views;
use super::*;
use pokarak_navigation::{
    FormatterConfig, InstallControl, InstallPhase, InstallProgress, ManagedTools, ServerConfig,
    ToolDistribution, ToolPackage, ToolStatus, language_catalog, language_family,
    language_for_path,
};
use policy::*;
use std::collections::BTreeMap;

actions!(forkarak_languages, [ManageLanguages]);

struct PreparedRequest {
    package: String,
    identity: language_tools::RequestIdentity,
    tool: language_tools::Tool,
    position: Option<Position>,
}

pub(super) struct LanguageManager {
    tools: Result<ManagedTools, String>,
    packages: Vec<ToolPackage>,
    statuses: BTreeMap<String, ToolStatus>,
    attempts: BTreeMap<String, InstallProgress>,
    running: BTreeMap<String, InstallControl>,
    expanded: HashSet<String>,
    error: Option<String>,
    status_failed: bool,
    pending: Option<PreparedRequest>,
    timer: Option<Task<()>>,
}
impl Default for LanguageManager {
    fn default() -> Self {
        let tools = managed_directory().and_then(|root| {
            ManagedTools::new(root, bundled_resources()).map_err(|e| format!("{e:#}"))
        });
        let packages = tools.as_ref().map(|t| t.catalog()).unwrap_or_default();
        Self {
            tools,
            packages,
            statuses: BTreeMap::new(),
            attempts: BTreeMap::new(),
            running: BTreeMap::new(),
            expanded: HashSet::new(),
            error: None,
            status_failed: false,
            pending: None,
            timer: None,
        }
    }
}
impl LanguageManager {
    fn refresh(&mut self) {
        let result = (|| {
            let tools = self.tools.as_ref().map_err(Clone::clone)?;
            let statuses = tools.statuses().map_err(|e| format!("{e:#}"))?;
            self.packages = tools.catalog();
            self.statuses = statuses
                .into_iter()
                .map(|s| (s.package.clone(), s))
                .collect();
            for package in &self.packages {
                if !self.running.contains_key(&package.id)
                    && !self.attempts.contains_key(&package.id)
                {
                    match tools
                        .last_attempt(&package.id)
                        .map_err(|e| format!("{e:#}"))?
                    {
                        Some(p) => {
                            self.attempts.insert(package.id.clone(), p);
                        }
                        None => {
                            self.attempts.remove(&package.id);
                        }
                    }
                }
            }
            Ok::<_, String>(())
        })();
        self.status_failed = result.is_err();
        self.error = result.err();
        if self.status_failed {
            // A corrupt/unreadable manifest cannot leave a cached version marked
            // ready or enable Continue. Failed updates with a valid old manifest
            // still refresh normally and retain that active version.
            self.statuses.clear();
        }
    }
    fn progress(&self, package: &str) -> Option<InstallProgress> {
        self.running
            .get(package)
            .map(InstallControl::snapshot)
            .or_else(|| self.attempts.get(package).cloned())
    }
    fn cancel_request(&mut self) {
        self.pending = None;
    }
    fn ready(&self, package: &str) -> bool {
        self.statuses
            .get(package)
            .is_some_and(|s| s.installed.is_some() && s.problem.is_none())
    }
}

fn managed_directory() -> Result<PathBuf, String> {
    if let Some(session) = crate::prefs::explicit_session_dir() {
        return Ok(canonical_data_parent(session)?.join("language-tools"));
    }
    let home = std::env::var_os("HOME").ok_or("앱 데이터 폴더를 찾을 수 없습니다.")?;
    absolute_directory(
        PathBuf::from(home).join("Library/Application Support/Forkarak/language-tools"),
    )
}
// Resolve only the explicitly selected data parent (e.g. macOS /tmp),
// before appending our private tools directory. Never canonicalize a tools
// symlink into another directory; the backend must keep rejecting that.
fn canonical_data_parent(path: &Path) -> Result<PathBuf, String> {
    absolute_directory(path.to_path_buf())?;
    let mut existing = path;
    let mut missing = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or("앱 데이터 폴더를 확인할 수 없습니다.")?;
        missing.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or("앱 데이터 폴더를 확인할 수 없습니다.")?;
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|e| format!("앱 데이터 폴더: {e}"))?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}
fn absolute_directory(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err("언어 지원 저장 폴더는 절대 경로여야 합니다.".into())
    }
}
fn bundled_resources() -> Option<PathBuf> {
    crate::updater::installed_bundle().map(|p| p.join("Contents/Resources"))
}

impl Pokarak {
    /// Read-only discovery. Frontend merges its custom paths after these defaults.
    pub(super) fn effective_managed_server_configs(&self) -> Result<Vec<ServerConfig>, String> {
        let tools = self.language_manager.tools.as_ref().map_err(Clone::clone)?;
        let mut configs = tools.server_configs().map_err(|e| format!("{e:#}"))?;
        configs.retain(|c| self.managed_language_enabled(&c.language));
        Ok(configs)
    }
    pub(super) fn effective_managed_formatter_configs(
        &self,
    ) -> Result<Vec<FormatterConfig>, String> {
        let tools = self.language_manager.tools.as_ref().map_err(Clone::clone)?;
        let mut configs = tools.formatter_configs().map_err(|e| format!("{e:#}"))?;
        configs.retain(|c| self.managed_language_enabled(&c.language));
        Ok(configs)
    }
    pub(super) fn managed_language_enabled(&self, language: &str) -> bool {
        enabled(
            self.prefs
                .option(self.current_project.as_deref(), "language.enabled"),
            language,
        )
    }
    /// Quit/release owns cancellation explicitly; InstallControl clones alone do not.
    pub(super) fn cancel_language_installers(&mut self, cx: &mut App) -> Task<()> {
        self.language_manager.cancel_request();
        self.language_manager.timer = None;
        let controls = std::mem::take(&mut self.language_manager.running)
            .into_values()
            .collect::<Vec<_>>();
        cx.background_spawn(async move {
            for control in &controls {
                let _ = control.cancel();
            }
            for control in controls {
                let _ = control.wait();
            }
        })
    }
    pub(super) fn cancel_managed_language_request(&mut self) {
        self.language_manager.cancel_request();
    }
    pub(super) fn prepare_managed_language_request(
        &mut self,
        tool: language_tools::Tool,
        position: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = self.selected() else {
            return false;
        };
        let Some(language) = language_for_path(&path) else {
            return false;
        };
        if !self.managed_language_enabled(language) {
            self.status = "이 언어의 분석이 꺼져 있습니다. 언어 지원 관리에서 변경하세요.".into();
            cx.notify();
            return true;
        }
        // Explicit custom paths are authoritative even if absent/broken: the user
        // receives their real startup failure instead of an unsolicited install.
        if custom_override(
            self.prefs
                .option(self.current_project.as_deref(), "language.servers"),
            language,
        ) || (matches!(tool, language_tools::Tool::Format { .. })
            && custom_override(
                self.prefs
                    .option(self.current_project.as_deref(), "formatters.config"),
                language,
            ))
            || legacy_override(language)
        {
            return false;
        }
        let support = language_catalog()
            .into_iter()
            .find(|l| l.language == language);
        let Some(support) = support else {
            return false;
        };
        let Some(package) = support.package else {
            self.status = format!(
                "{}: 파일 보기·텍스트 검색만 지원합니다. {}",
                support.name,
                support.limitations.join(" ")
            );
            // Text-only types still use existing navigation's clearly labelled
            // text-candidate fallback; never imply semantic support.
            cx.notify();
            return false;
        };
        self.language_manager.refresh();
        if let Some(error) = self.language_manager.error.clone() {
            self.error = Some(format!("언어 지원 상태를 읽지 못했습니다: {error}"));
            cx.notify();
            return true;
        }
        if self.language_manager.ready(&package) {
            return false;
        }
        let identity = match self.language_request_identity(cx) {
            Ok(identity) => identity,
            Err(error) => {
                self.status = error;
                cx.notify();
                return true;
            }
        };
        self.language_manager.pending = Some(PreparedRequest {
            package: package.clone(),
            identity,
            tool,
            position,
        });
        self.language_manager.expanded.insert(package);
        self.open_language_manager(window, cx);
        true
    }
    fn begin_language_install(
        &mut self,
        package: String,
        update: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.language_manager.running.contains_key(&package) {
            return;
        }
        let control = (|| {
            let tools = self.language_manager.tools.as_ref().map_err(Clone::clone)?;
            if update {
                tools.begin_update(&package)
            } else {
                tools.begin_install(&package)
            }
            .map_err(|e| format!("{e:#}"))
        })();
        let control = match control {
            Ok(control) => control,
            Err(error) => {
                self.language_manager.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.language_manager.error = None;
        self.language_manager
            .running
            .insert(package.clone(), control.clone());
        // Waiting and installer subprocesses never hold the UI or Navigator mutex.
        let wait = cx.background_spawn(async move { control.wait() });
        cx.spawn_in(window, async move |view, cx| {
            let outcome = wait.await;
            let _ = view.update_in(cx, |app, _, cx| {
                let manager = &mut app.language_manager;
                if let Some(control) = manager.running.remove(&package) {
                    // Keep the terminal result on its package row, also if persisting it failed.
                    manager.attempts.insert(package.clone(), control.snapshot());
                }
                manager.refresh();
                if manager.progress(&package).is_none()
                    && let Err(error) = outcome
                {
                    manager.error = Some(format!("{error:#}"));
                }
                // Only package state changes. A late installer completion cannot
                // close another dialog, grant trust, restart analysis or act on a worktree.
                cx.notify();
            });
        })
        .detach();
        self.watch_language_installs(cx);
        cx.notify();
    }
    fn watch_language_installs(&mut self, cx: &mut Context<Self>) {
        if self.language_manager.timer.is_some() {
            return;
        }
        let executor = cx.background_executor().clone();
        self.language_manager.timer = Some(cx.spawn(async move |view, cx| {
            loop {
                executor.timer(Duration::from_millis(250)).await;
                let running = view
                    .update(cx, |app, cx| {
                        cx.notify();
                        if app.language_manager.running.is_empty() {
                            app.language_manager.timer = None;
                            false
                        } else {
                            true
                        }
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        }));
    }
    fn cancel_language_install(&mut self, package: &str, cx: &mut Context<Self>) {
        if let Some(control) = self.language_manager.running.get(package).cloned() {
            // Process cancellation/reaping never blocks GPUI. The original wait
            // task remains authoritative for its final progress and error.
            cx.background_spawn(async move {
                let _ = control.cancel();
            })
            .detach();
        }
        if self
            .language_manager
            .pending
            .as_ref()
            .is_some_and(|p| p.package == package)
        {
            self.language_manager.pending = None;
        }
        cx.notify();
    }
    fn continue_prepared_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.language_manager.pending.take() else {
            return;
        };
        if !self.language_manager.ready(&pending.package) {
            self.language_manager.pending = Some(pending);
            return;
        }
        window.close_dialog(cx);
        self.resume_language_request(pending.identity, pending.tool, pending.position, window, cx);
    }
    fn set_managed_language_enabled(
        &mut self,
        language: &str,
        value: bool,
        cx: &mut Context<Self>,
    ) {
        let root = self.current_project.clone();
        let old = self
            .prefs
            .option(root.as_deref(), "language.enabled")
            .cloned();
        let mut values = old.clone().unwrap_or_else(|| serde_json::json!({}));
        if !values.is_object() {
            values = serde_json::json!({});
        }
        values[language] = serde_json::Value::Bool(value);
        self.prefs
            .set_option(root.as_deref(), "language.enabled", values);
        if let Err(error) = self.prefs.try_save() {
            if let Some(old) = old {
                self.prefs
                    .set_option(root.as_deref(), "language.enabled", old);
            } else {
                self.prefs
                    .remove_option(root.as_deref(), "language.enabled");
            }
            self.language_manager.error = Some(error.to_string());
        } else {
            self.stop_analysis(cx);
        }
        cx.notify();
    }
}
