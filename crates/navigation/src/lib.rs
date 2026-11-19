//! Read-only, root-scoped semantic navigation over stdio LSP.
//! Construct only after trust; call on an application background thread.
mod buffer;
mod build_root;
mod cancellation;
mod client;
mod config;
mod formatter;
mod intelligence;
mod languages;
pub mod managed;
mod paths;
mod snippet;
mod transport;
mod workspace_edit;

use anyhow::{Context, Result, bail};
use client::{Client, read_document};
use config::{family, is_jdt, is_jetbrains, language, merge_user_config};
use paths::{canonical_root, other_worktree, resolved_path, validate_position};
use pokarak_core::SourceLocation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub use buffer::{
    BufferResult, CodeAction, CompletionItem, CompletionResult, Diagnostic, DocumentSnapshot,
    DocumentSymbol, FileEdits, FormatterConfig, FormattingOptions, Position, TextEdit, TextRange,
    WorkspaceEdit,
};
pub use cancellation::CancellationHandle;
pub use config::{NavigatorOptions, default_configs, discover_program, installed_configs};
pub use intelligence::{
    HoverResult, MarkupContent, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureInformation,
};
pub use languages::{LanguageSupport, language_catalog, language_family, language_for_path};
pub use managed::{
    InstallControl, InstallPhase, InstallProgress, InstalledTool, ManagedTools, ToolArtifact,
    ToolDistribution, ToolPackage, ToolStatus,
};
pub use paths::{path_to_uri, uri_to_path};

/// This request failed with the stream and acknowledged document state intact.
#[derive(Debug)]
struct RequestFailure(String);

impl fmt::Display for RequestFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RequestFailure {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub language: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum NavigationKind {
    Definition,
    References,
    Implementation,
}

#[derive(Clone, Debug)]
pub struct NavigationResult {
    pub locations: Vec<SourceLocation>,
    pub semantic: bool,
    pub status: String,
}

#[derive(Clone)]
struct ClientOrigin {
    key: String,
    config: ServerConfig,
    import_root: PathBuf,
}

pub struct Navigator {
    root: PathBuf,
    buffer_versions: BTreeMap<PathBuf, DocumentSnapshot>,
    formatters: Vec<FormatterConfig>,
    configs: Vec<ServerConfig>,
    options: NavigatorOptions,
    clients: BTreeMap<String, Client>,
    external_sources: BTreeMap<PathBuf, ClientOrigin>,
    setup_error: Option<String>,
    last_status: String,
    last_stderr: String,
    last_messages: String,
    stopped: bool,
    cancellation: CancellationHandle,
}

impl Navigator {
    pub fn new(root: PathBuf, configs: Vec<ServerConfig>) -> Self {
        Self::with_options(root, configs, NavigatorOptions::default())
    }

    pub fn with_options(
        root: PathBuf,
        mut configs: Vec<ServerConfig>,
        mut options: NavigatorOptions,
    ) -> Self {
        let mut setup_error = None;
        let root = canonical_root(&root).unwrap_or_else(|error| {
            setup_error = Some(format!("{error:#}"));
            root
        });
        if let Err(error) = merge_user_config(&mut configs, &mut options) {
            setup_error = Some(format!("{error:#}"));
        }
        Self {
            root,
            buffer_versions: BTreeMap::new(),
            formatters: Vec::new(),
            configs,
            options,
            clients: BTreeMap::new(),
            external_sources: BTreeMap::new(),
            last_status: setup_error
                .clone()
                .unwrap_or_else(|| "Analysis idle; language servers start on navigation".into()),
            setup_error,
            last_stderr: String::new(),
            last_messages: String::new(),
            stopped: false,
            cancellation: CancellationHandle::default(),
        }
    }

    /// Explicit analysis activation only. Returns the server's actual negotiated
    /// capabilities, tied to the same current worktree/file/version as buffer results.
    pub fn capabilities_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
    ) -> Result<BufferResult<Value>> {
        let result = (|| {
            self.retain_buffer(snapshot)?;
            let language =
                language(snapshot.path()).context("No configured language for this file")?;
            let origin = self.origin_for(snapshot.path(), language)?;
            self.with_client_context(&origin, Some(snapshot), |client| Ok(client.capabilities()))
        })();
        self.finish_buffer(snapshot, result)
    }

    /// Canonical worktree namespace. The UI must discard responses for a root it has left.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Retain a clone outside the application's Navigator mutex for close/quit.
    /// Cancellation is permanent for this instance; create a new Navigator to resume.
    pub fn cancellation_handle(&self) -> CancellationHandle {
        self.cancellation.clone()
    }

    /// Lines and columns are zero-based; columns count UTF-16 code units.
    pub fn navigate(
        &mut self,
        path: &Path,
        line: u32,
        column: u32,
        kind: NavigationKind,
    ) -> Result<NavigationResult> {
        let result = self.navigate_inner(path, line, column, kind);
        let result = self.cancellation.check().and(result);
        match &result {
            Ok(result) => self.last_status = result.status.clone(),
            Err(error) => self.last_status = format!("{error:#}"),
        }
        result
    }

    fn navigate_inner(
        &mut self,
        path: &Path,
        line: u32,
        column: u32,
        kind: NavigationKind,
    ) -> Result<NavigationResult> {
        if self.stopped {
            bail!("Navigator has been shut down for {}", self.root.display());
        }
        self.cancellation.check()?;
        if let Some(error) = &self.setup_error {
            bail!("{error}");
        }
        let path = resolved_path(&self.root.join(path))?;
        if other_worktree(&self.root, &path) {
            bail!("Refusing source from another worktree: {}", path.display());
        }
        if !path.starts_with(&self.root) && !self.external_sources.contains_key(&path) {
            bail!(
                "Source is outside this worktree and was not returned as a dependency: {}",
                path.display()
            );
        }
        let language =
            language(&path).context("No language-server configuration for this file type")?;
        let text = read_document(&path)?;
        validate_position(&text, line, column)?;
        // A dependency source keeps the originating import session even if its
        // language or filesystem root differs from the selected worktree source.
        let origin = self.origin_for(&path, language)?;
        let response = self.with_client_context(&origin, None, |client| {
            client.navigate(&path, text, language, line, column, &kind)
        })?;
        let (locations, skipped) = decode_locations(&self.root, response)?;
        self.external_sources.extend(
            locations
                .iter()
                .filter(|hit| !hit.path.starts_with(&self.root))
                .map(|hit| (hit.path.clone(), origin.clone())),
        );
        let mut status = if locations.is_empty() {
            "Semantic navigation completed: no locations returned".into()
        } else {
            format!("Semantic navigation: {} location(s)", locations.len())
        };
        if !skipped.is_empty() {
            status.push_str(&format!("; skipped {}", skipped.join("; ")));
        }
        if matches!(kind, NavigationKind::References) && is_jdt(&origin.config) {
            status.push_str("; JDT LS Java scope: Kotlin callers may be missing");
        }
        Ok(NavigationResult {
            locations,
            semantic: true,
            status,
        })
    }

    fn check_ready(&self) -> Result<()> {
        self.cancellation.check()?;
        if self.stopped {
            bail!("Navigator has been shut down");
        }
        if let Some(error) = &self.setup_error {
            bail!("{error}");
        }
        Ok(())
    }

    fn origin_for(&self, path: &Path, language: &str) -> Result<ClientOrigin> {
        let origin = if let Some(origin) = self.external_sources.get(path) {
            origin.clone()
        } else {
            let mut config = self.configs.iter().rev().find(|c| c.language == language)
                .or_else(|| self.configs.iter().rev().find(|c| c.language == family(language)))
                .cloned().with_context(|| format!("No language server configured for {language}; semantic navigation is unavailable"))?;
            Client::check_import_permission(&config, &self.options)?;
            // Check installation before inspecting Maven reactors or creating any
            // server workspace. Discovery checks files only and preserves argv.
            if config.language == "kotlin" || is_jetbrains(&config) {
                config.program = config::kotlin_program(&config)?
                    .to_string_lossy()
                    .into_owned();
            } else if config.language == "java" {
                config.program = config::java_program(&config)?
                    .to_string_lossy()
                    .into_owned();
            }
            let import_root = if language == "kotlin" || is_jetbrains(&config) || is_jdt(&config) {
                build_root::discover(&self.root, path, is_jdt(&config))?
            } else {
                self.root.clone()
            };
            let server_family = if is_jetbrains(&config) {
                "jetbrains-jvm"
            } else {
                family(language)
            };
            let key = serde_json::to_string(&(
                server_family,
                &config.program,
                &config.args,
                &import_root,
            ))?;
            ClientOrigin {
                key,
                config,
                import_root,
            }
        };
        Ok(origin)
    }

    fn with_client_context<T>(
        &mut self,
        origin: &ClientOrigin,
        snapshot: Option<&DocumentSnapshot>,
        operation: impl FnOnce(&mut Client) -> Result<T>,
    ) -> Result<T> {
        let key = &origin.key;
        if let Some(client) = self.clients.get(key) {
            let health = client.transport.check_health();
            self.client_result(key, health)?;
        } else {
            let mut launch_config = origin.config.clone();
            launch_config.language = family(&launch_config.language).into();
            let client = Client::start(
                &self.root,
                &origin.import_root,
                &launch_config,
                &self.options,
                &self.cancellation,
                snapshot,
            )?;
            self.clients.insert(key.clone(), client);
        }

        // A reused session and a replacement must both see every retained draft
        // for their import root, including when navigating a disk-backed source.
        let mut retained = Vec::new();
        for source in self.buffer_versions.values() {
            if let Some(language) = language(source.path())
                && self.origin_for(source.path(), language)?.key == *key
            {
                retained.push((source, language));
            }
        }
        let client = self.clients.get_mut(key).unwrap();
        client.transport.set_context(
            snapshot,
            retained.iter().map(|(doc, _)| (*doc).clone()).collect(),
        );
        let result = (|| {
            client.transport.check_snapshots()?;
            for (source, language) in retained {
                client.restore_buffer(source, language)?;
            }
            let result = operation(client)?;
            client.transport.check_snapshots()?;
            Ok(result)
        })();
        client.transport.set_context(None, Vec::new());
        self.client_result(key, result)
    }

    fn client_result<T>(&mut self, key: &str, result: Result<T>) -> Result<T> {
        result.map_err(|error| {
            let client = self.clients.get_mut(key).unwrap();
            if error.is::<RequestFailure>() && client.transport.check_health().is_ok() {
                return anyhow::Error::msg(client.error_message(&format!("{error:#}"), ""));
            }
            client.transport.stop(Duration::ZERO);
            self.last_stderr = client.transport.stderr();
            self.last_messages = client.server_messages().to_owned();
            let message = client.error_message(&format!("{error:#}"), "");
            self.clients.remove(key);
            anyhow::Error::msg(message)
        })
    }

    /// Call on filesystem-change notifications; navigation also refreshes all open buffers.
    pub fn refresh(&mut self) -> Result<()> {
        self.cancellation.check()?;
        if self.stopped {
            bail!("Navigator has been shut down");
        }
        if let Some(error) = &self.setup_error {
            bail!("{error}");
        }
        let keys: Vec<_> = self.clients.keys().cloned().collect();
        for key in keys {
            let client = self.clients.get_mut(&key).unwrap();
            let result = client
                .transport
                .check_health()
                .and_then(|_| client.refresh(Instant::now() + self.options.request_timeout))
                .context("Disk synchronization failed");
            if let Err(error) = self.client_result(&key, result) {
                self.last_status = format!("{error:#}");
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn status(&self) -> String {
        if !self.stopped && self.cancellation.is_cancelled() {
            return format!("{}: Analysis cancelled", self.root.display());
        }
        let state = self
            .clients
            .values()
            .find_map(|client| {
                client
                    .transport
                    .exit_status()
                    .map(|exit| client.error_message(&exit, ""))
            })
            .unwrap_or_else(|| self.last_status.clone());
        format!("{}: {state}", self.root.display())
    }

    /// Bounded stderr tail, including the last session which failed during a request.
    pub fn stderr(&self) -> String {
        self.clients
            .values()
            .map(|client| client.transport.stderr())
            .filter(|s| !s.is_empty())
            .chain((!self.last_stderr.is_empty()).then(|| self.last_stderr.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Bounded protocol log/import messages, including the last failed session.
    pub fn server_messages(&self) -> String {
        self.clients
            .values()
            .map(|client| client.server_messages())
            .filter(|s| !s.is_empty())
            .chain((!self.last_messages.is_empty()).then_some(self.last_messages.as_str()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        for client in self.clients.values_mut() {
            client.shutdown();
        }
        self.clients.clear();
        let _ = self.cancellation.cancel();
        self.external_sources.clear();
        for snapshot in self.buffer_versions.values() {
            snapshot.invalidate();
        }
        self.buffer_versions.clear();
        self.stopped = true;
        self.last_status = "Analysis stopped".into();
    }
}

impl Drop for Navigator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn decode_locations(root: &Path, result: Value) -> Result<(Vec<SourceLocation>, Vec<String>)> {
    let entries = match result {
        Value::Null => vec![],
        Value::Array(values) => values,
        value if value.is_object() => vec![value],
        _ => bail!("Invalid navigation response: expected Location, LocationLink, array, or null"),
    };
    let mut locations = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let (uri, start) = if let Some(uri) = entry.get("targetUri") {
            (uri, &entry["targetSelectionRange"]["start"])
        } else {
            (&entry["uri"], &entry["range"]["start"])
        };
        let uri = uri
            .as_str()
            .context("Navigation location is missing its URI")?;
        let path = match uri_to_path(uri) {
            Ok(path) => resolved_path(&path)?,
            Err(error) => {
                skipped.push(error.to_string());
                continue;
            }
        };
        if other_worktree(root, &path) {
            skipped.push(format!("another worktree: {}", path.display()));
            continue;
        }
        let line = start["line"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .context("Invalid result line")?;
        let column = start["character"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .context("Invalid result UTF-16 column")?;
        if !seen.insert((path.clone(), line, column)) {
            continue;
        }
        let preview = read_document(&path)
            .ok()
            .and_then(|text| {
                text.lines()
                    .nth(line as usize)
                    .map(|s| s.chars().take(300).collect())
            })
            .unwrap_or_default();
        locations.push(SourceLocation {
            path,
            line,
            column,
            preview,
        });
    }
    Ok((locations, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uri_and_utf16_positions() {
        let path = Path::new("/tmp/한 space #%?😀.ts");
        let uri = path_to_uri(path).unwrap();
        assert!(uri.contains("%23%25%3F"));
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        assert!(uri_to_path("file://elsewhere/tmp/a.ts").is_err());
        assert!(uri_to_path("jdt://contents/a.class").is_err());
        assert!(uri_to_path("file:///tmp/a.ts#x").is_err());
        assert!(validate_position("😀x\r\n", 0, 2).is_ok());
        assert!(validate_position("😀x", 0, 1).is_err());
        assert!(validate_position("😀x", 0, 4).is_err());
        assert!(validate_position("x\n", 1, 0).is_ok());
        assert_eq!(paths::end_position("😀x\r\n한😀"), (1, 3));
    }
}
