use crate::{
    NavigationKind, NavigatorOptions, ServerConfig,
    config::discover_program,
    paths::{end_position, other_worktree, path_to_uri},
    transport::Transport,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_DOCUMENT: u64 = 8 * 1024 * 1024;

pub(crate) fn read_document(path: &Path) -> Result<String> {
    let file = fs::File::open(path)
        .with_context(|| format!("Cannot open source file {}", path.display()))?;
    if !file.metadata()?.is_file() {
        bail!("Source is not a regular file: {}", path.display());
    }
    let mut text = String::new();
    file.take(MAX_DOCUMENT + 1)
        .read_to_string(&mut text)
        .with_context(|| format!("Cannot read UTF-8 source {}", path.display()))?;
    if text.len() as u64 > MAX_DOCUMENT {
        bail!(
            "Source exceeds the 8 MiB navigation limit: {}",
            path.display()
        );
    }
    Ok(text)
}

struct Document {
    text: String,
    version: i64,
}

pub(crate) struct Client {
    pub transport: Transport,
    root: PathBuf,
    options: NavigatorOptions,
    capabilities: Value,
    settings: Value,
    next_id: u64,
    documents: HashMap<PathBuf, Document>,
    sync_kind: u64,
    open_close: bool,
    // Keep server state outside the inspected repository, unique per root + instance.
    _workspace: tempfile::TempDir,
}

impl Client {
    fn read_current_document(&self, path: &Path) -> Result<String> {
        let resolved = path
            .canonicalize()
            .with_context(|| format!("Cannot resolve open source {}", path.display()))?;
        if resolved != path || other_worktree(&self.root, &resolved) {
            bail!(
                "Open source changed its filesystem target; restart analysis: {}",
                path.display()
            );
        }
        read_document(path)
    }

    pub fn start(root: &Path, config: &ServerConfig, options: &NavigatorOptions) -> Result<Self> {
        if config.language == "kotlin" && !options.allow_project_imports {
            bail!(
                "Kotlin analysis is disabled: Kotlin servers may execute Gradle/Maven or classpath scripts during project import. Explicitly enable allowProjectImports after trusting those actions."
            );
        }
        let program = discover_program(&config.program).with_context(|| format!("Language server unavailable for {}: executable {:?} was not found on the absolute PATH entries. Configure an installed executable in ~/.config/pokarak/language-servers.json; nothing was installed.", config.language, config.program))?;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        root.hash(&mut hash);
        let workspace = tempfile::Builder::new()
            .prefix(&format!("pokarak-lsp-{:016x}-", hash.finish()))
            .tempdir()?;
        let workspace_path = workspace.path().canonicalize()?;
        let mut args: Vec<_> = config
            .args
            .iter()
            .map(|arg| {
                arg.replace("{root}", &root.to_string_lossy())
                    .replace("{workspace}", &workspace_path.to_string_lossy())
            })
            .collect();
        if config.language == "java" {
            // JDT's data directory must never be shared between worktrees or instances.
            for index in 0..args.len() {
                if args[index] == "-data" {
                    if index + 1 == args.len() {
                        bail!("Java server -data requires an argument");
                    }
                    args[index + 1] = workspace_path.join("data").to_string_lossy().into_owned();
                }
            }
            if !args.iter().any(|arg| arg == "-data") {
                args.extend([
                    "-data".into(),
                    workspace_path.join("data").to_string_lossy().into_owned(),
                ]);
            }
        }
        let transport = Transport::spawn(&program, &args, root)?;
        let settings = json!({
            "java": {
                "autobuild": {"enabled": false},
                "configuration": {"updateBuildConfiguration": "disabled"},
                "import": {"gradle": {"enabled": options.allow_project_imports}, "maven": {"enabled": options.allow_project_imports}},
                "references": {"includeDecompiledSources": false}
            }
        });
        let mut client = Self {
            transport,
            root: root.to_path_buf(),
            options: options.clone(),
            capabilities: Value::Null,
            settings,
            next_id: 1,
            documents: HashMap::new(),
            sync_kind: 0,
            open_close: false,
            _workspace: workspace,
        };
        let deadline = Instant::now() + options.initialize_timeout;
        let initialized = (|| -> Result<()> {
            let mut init_options = json!({
                "disableAutomaticTypingAcquisition": true,
                // The adapter's automatic syntax server can answer definitions with
                // import aliases while the semantic project is still loading.
                "tsserver": {"logDirectory": workspace_path.join("tsserver-logs"), "logVerbosity": "off", "useSyntaxServer": "never"},
                "settings": client.settings
            });
            // No build, installation, executable-command, edit, or dynamic-registration capability.
            if config.language == "java" {
                init_options = json!({"settings": client.settings});
            }
            let response = client.request("initialize", json!({
                "processId": std::process::id(),
                "clientInfo": {"name": "pokarak", "version": env!("CARGO_PKG_VERSION")},
                "rootPath": root, "rootUri": path_to_uri(root)?,
                "workspaceFolders": client.workspace_folders()?,
                "capabilities": {
                    "general": {"positionEncodings": ["utf-16"]},
                    "workspace": {"configuration": true, "workspaceFolders": true, "applyEdit": false},
                    "textDocument": {
                        "synchronization": {"dynamicRegistration": false, "didSave": false, "willSave": false},
                        "definition": {"dynamicRegistration": false, "linkSupport": true},
                        "references": {"dynamicRegistration": false},
                        "implementation": {"dynamicRegistration": false, "linkSupport": true}
                    },
                    "window": {"workDoneProgress": true}
                },
                "initializationOptions": init_options
            }), deadline)?;
            client.capabilities = response
                .get("capabilities")
                .filter(|v| v.is_object())
                .context("Initialize response has no capabilities object")?
                .clone();
            if let Some(encoding) = client
                .capabilities
                .get("positionEncoding")
                .and_then(Value::as_str)
                && encoding != "utf-16"
            {
                bail!(
                    "Server selected unsupported position encoding {encoding}; Pokarak requires UTF-16"
                );
            }
            let sync = &client.capabilities["textDocumentSync"];
            client.sync_kind = sync
                .as_u64()
                .or_else(|| sync["change"].as_u64())
                .unwrap_or(0);
            client.open_close = sync["openClose"]
                .as_bool()
                .unwrap_or(sync.as_u64().is_some_and(|n| n > 0));
            if client.sync_kind > 2 {
                bail!("Invalid textDocumentSync capability");
            }
            client.notify("initialized", json!({}), deadline)?;
            client.notify(
                "workspace/didChangeConfiguration",
                json!({"settings": client.settings}),
                deadline,
            )?;
            Ok(())
        })();
        if let Err(error) = initialized {
            client.transport.stop(Duration::ZERO);
            bail!(
                "{}",
                client.error_message(
                    &format!("{} initialization failed: {error:#}", config.language),
                    &config.language
                )
            );
        }
        Ok(client)
    }

    fn workspace_folders(&self) -> Result<Value> {
        Ok(
            json!([{"uri": path_to_uri(&self.root)?, "name": self.root.file_name().unwrap_or_default().to_string_lossy()}]),
        )
    }

    pub fn error_message(&self, error: &str, language: &str) -> String {
        let mut message = error.to_owned();
        if let Some(exit) = self.transport.exit_status() {
            message.push_str(&format!("; {exit}"));
        }
        let logs = self.transport.stderr();
        if !logs.is_empty() {
            message.push_str(&format!("\nServer stderr:\n{logs}"));
        }
        if matches!(language, "java" | "kotlin") {
            message.push_str("\nCheck the server's required JDK runtime (the project's Java 17 may be too old). Configure an explicit launcher/JDK argv; Pokarak never installs a JDK or runs a build to repair this.");
        }
        message
    }

    fn notify(&self, method: &str, params: Value, deadline: Instant) -> Result<()> {
        self.transport.send(
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
            deadline,
        )
    }

    fn request(&mut self, method: &str, params: Value, deadline: Instant) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.transport
            .send(
                &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
                deadline,
            )
            .with_context(|| format!("Sending {method}"))?;
        loop {
            let message = self
                .transport
                .receive(deadline)
                .with_context(|| format!("Waiting for {method}"))?;
            if message.get("method").is_some() {
                if message.get("id").is_some() {
                    self.server_request(&message, deadline)?;
                }
                // Diagnostics/progress/telemetry never count as a navigation response.
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("LSP {method} error: {error}");
            }
            return message
                .get("result")
                .cloned()
                .with_context(|| format!("LSP {method} response has neither result nor error"));
        }
    }

    fn server_request(&self, message: &Value, deadline: Instant) -> Result<()> {
        let method = message["method"].as_str().unwrap_or("");
        let result = match method {
            "workspace/configuration" => {
                let items = message["params"]["items"].as_array().context("Invalid workspace/configuration items")?;
                let results: Vec<_> = items.iter().map(|item| {
                    let mut value = &self.settings;
                    if let Some(section) = item["section"].as_str() {
                        for key in section.split('.') { value = &value[key]; }
                    }
                    value.clone()
                }).collect();
                json!(results)
            }
            "workspace/workspaceFolders" => self.workspace_folders()?,
            "window/workDoneProgress/create" | "window/showMessageRequest" => Value::Null,
            "workspace/applyEdit" => json!({"applied": false, "failureReason": "Pokarak is a read-only viewer"}),
            _ => return self.transport.send(&json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": format!("Unsupported client method: {method}")}}), deadline),
        };
        self.transport.send(
            &json!({"jsonrpc": "2.0", "id": message["id"], "result": result}),
            deadline,
        )
    }

    /// Refresh every opened document, so edits to a previously visited dependency
    /// cannot remain shadowed by an old in-memory LSP buffer.
    pub fn refresh(&mut self, deadline: Instant) -> Result<()> {
        let paths: Vec<_> = self.documents.keys().cloned().collect();
        for path in paths {
            if !path.exists() {
                if self.open_close {
                    self.notify(
                        "textDocument/didClose",
                        json!({"textDocument": {"uri": path_to_uri(&path)?}}),
                        deadline,
                    )?;
                }
                self.documents.remove(&path);
                continue;
            }
            let text = self.read_current_document(&path)?;
            let old = &self.documents[&path];
            if old.text == text {
                continue;
            }
            if self.sync_kind == 0 {
                bail!(
                    "Server cannot synchronize disk changes; restart analysis for {}",
                    path.display()
                );
            }
            let version = old.version + 1;
            let change = if self.sync_kind == 2 {
                let (line, column) = end_position(&old.text);
                json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": line, "character": column}}, "text": text})
            } else {
                json!({"text": text})
            };
            self.notify("textDocument/didChange", json!({"textDocument": {"uri": path_to_uri(&path)?, "version": version}, "contentChanges": [change]}), deadline)?;
            self.documents.insert(path, Document { text, version });
        }
        Ok(())
    }

    pub fn navigate(
        &mut self,
        path: &Path,
        text: String,
        language: &str,
        line: u32,
        column: u32,
        kind: &NavigationKind,
    ) -> Result<Value> {
        let deadline = Instant::now() + self.options.request_timeout;
        self.refresh(deadline)?;
        if !self.documents.contains_key(path) {
            if self.open_close {
                self.notify("textDocument/didOpen", json!({"textDocument": {"uri": path_to_uri(path)?, "languageId": language, "version": 1, "text": text}}), deadline)?;
            }
            self.documents
                .insert(path.to_path_buf(), Document { text, version: 1 });
        }
        let (method, capability) = match kind {
            NavigationKind::Definition => ("textDocument/definition", "definitionProvider"),
            NavigationKind::References => ("textDocument/references", "referencesProvider"),
            NavigationKind::Implementation => {
                ("textDocument/implementation", "implementationProvider")
            }
        };
        if self.capabilities[capability].is_null() || self.capabilities[capability] == false {
            bail!("Language server does not support {method}");
        }
        let mut params = json!({"textDocument": {"uri": path_to_uri(path)?}, "position": {"line": line, "character": column}});
        if matches!(kind, NavigationKind::References) {
            params["context"] = json!({"includeDeclaration": true});
        }
        let result = self.request(method, params, deadline)?;
        for (path, document) in &self.documents {
            if self.read_current_document(path)? != document.text {
                bail!(
                    "Source changed on disk during navigation; retry against the current file: {}",
                    path.display()
                );
            }
        }
        Ok(result)
    }

    pub fn shutdown(&mut self) {
        let deadline = Instant::now() + self.options.shutdown_timeout;
        let _ = self.request("shutdown", Value::Null, deadline);
        let _ = self.notify("exit", Value::Null, deadline);
        self.transport
            .stop(deadline.saturating_duration_since(Instant::now()));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.transport.stop(Duration::ZERO);
    }
}
