//! Read-only, root-scoped semantic navigation over stdio LSP.
//! Construct only after trust; call on an application background thread.
mod cancellation;
mod client;
mod config;
mod paths;
mod transport;

use anyhow::{Context, Result, bail};
use client::{Client, read_document};
use config::{family, is_jdt, is_jetbrains, language, merge_user_config};
use paths::{canonical_root, other_worktree, resolved_path, validate_position};
use pokarak_core::SourceLocation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub use cancellation::CancellationHandle;
pub use config::{NavigatorOptions, default_configs, discover_program, installed_configs};
pub use paths::{path_to_uri, uri_to_path};

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

pub struct Navigator {
    root: PathBuf,
    configs: Vec<ServerConfig>,
    options: NavigatorOptions,
    clients: BTreeMap<String, Client>,
    external_sources: HashSet<PathBuf>,
    setup_error: Option<String>,
    last_status: String,
    last_stderr: String,
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
            configs,
            options,
            clients: BTreeMap::new(),
            external_sources: HashSet::new(),
            last_status: setup_error
                .clone()
                .unwrap_or_else(|| "Analysis idle; language servers start on navigation".into()),
            setup_error,
            last_stderr: String::new(),
            stopped: false,
            cancellation: CancellationHandle::default(),
        }
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
        if !path.starts_with(&self.root) && !self.external_sources.contains(&path) {
            bail!(
                "Source is outside this worktree and was not returned as a dependency: {}",
                path.display()
            );
        }
        let language =
            language(&path).context("No language-server configuration for this file type")?;
        let text = read_document(&path)?;
        validate_position(&text, line, column)?;
        let config = self.configs.iter().rev().find(|c| c.language == language)
            .or_else(|| self.configs.iter().rev().find(|c| c.language == family(language)))
            .cloned().with_context(|| format!("No language server configured for {language}; semantic navigation is unavailable"))?;
        let server_family = if is_jetbrains(&config) {
            "jetbrains-jvm"
        } else {
            family(language)
        };
        let key = serde_json::to_string(&(server_family, &config.program, &config.args))?;
        if !self.clients.contains_key(&key) {
            let mut launch_config = config.clone();
            launch_config.language = family(language).into();
            let client = Client::start(
                &self.root,
                &launch_config,
                &self.options,
                &self.cancellation,
            )?;
            self.clients.insert(key.clone(), client);
        }
        let client = self.clients.get_mut(&key).unwrap();
        let response = client.navigate(&path, text, language, line, column, &kind);
        let response = match response {
            Ok(value) => value,
            Err(error) => {
                // A timeout or protocol error invalidates this process and all pending IDs.
                // A later explicit navigation may start a fresh session.
                client.transport.stop(Duration::ZERO);
                self.last_stderr = client.transport.stderr();
                let error = client.error_message(&format!("{error:#}"), language);
                self.clients.remove(&key);
                bail!("{error}");
            }
        };
        let (locations, skipped) = decode_locations(&self.root, response)?;
        self.external_sources.extend(
            locations
                .iter()
                .filter(|hit| !hit.path.starts_with(&self.root))
                .map(|hit| hit.path.clone()),
        );
        let mut status = if locations.is_empty() {
            "Semantic navigation completed: no locations returned".into()
        } else {
            format!("Semantic navigation: {} location(s)", locations.len())
        };
        if !skipped.is_empty() {
            status.push_str(&format!("; skipped {}", skipped.join("; ")));
        }
        if matches!(kind, NavigationKind::References) && is_jdt(&config) {
            status.push_str("; JDT LS Java scope: Kotlin callers may be missing");
        }
        Ok(NavigationResult {
            locations,
            semantic: true,
            status,
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
            if let Err(error) = client.refresh(Instant::now() + self.options.request_timeout) {
                client.transport.stop(Duration::ZERO);
                self.last_stderr = client.transport.stderr();
                self.last_status =
                    client.error_message(&format!("Disk synchronization failed: {error:#}"), "");
                self.clients.remove(&key);
                bail!("{}", self.last_status);
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

    /// Bounded protocol log/import messages from currently active language servers.
    pub fn server_messages(&self) -> String {
        self.clients
            .values()
            .map(|client| client.server_messages())
            .filter(|s| !s.is_empty())
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
