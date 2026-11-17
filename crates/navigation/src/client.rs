use crate::{
    CancellationHandle, DocumentSnapshot, NavigationKind, NavigatorOptions, ServerConfig,
    config::{discover_program, is_jetbrains},
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
    buffer_version: Option<i64>,
}

pub(crate) struct Client {
    pub transport: Transport,
    root: PathBuf,
    import_root: PathBuf,
    options: NavigatorOptions,
    capabilities: Value,
    settings: Value,
    next_id: u64,
    documents: HashMap<PathBuf, Document>,
    diagnostics: HashMap<PathBuf, (i64, Value)>,
    unversioned_diagnostics: std::collections::HashSet<PathBuf>,
    sync_kind: u64,
    open_close: bool,
    server_messages: String,
    jetbrains: bool,
    import_in_progress: bool,
    import_error: Option<String>,
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

    pub(crate) fn check_import_permission(
        config: &ServerConfig,
        options: &NavigatorOptions,
    ) -> Result<()> {
        if (matches!(
            config.language.as_str(),
            "kotlin" | "rust" | "go" | "groovy" | "svelte" | "sql"
        ) || is_jetbrains(config))
            && !options.allow_project_imports
        {
            bail!(
                "{} analysis is disabled: explicitly enable allowProjectImports after trusting project/tool import actions.",
                config.language
            );
        }
        Ok(())
    }

    pub fn start(
        root: &Path,
        import_root: &Path,
        config: &ServerConfig,
        options: &NavigatorOptions,
        cancellation: &CancellationHandle,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<Self> {
        cancellation.check()?;
        Self::check_import_permission(config, options)?;
        let jetbrains = is_jetbrains(config);
        let program = discover_program(&config.program).with_context(|| format!("Language server unavailable for {}: executable {:?} was not found on the absolute PATH entries. Configure an installed executable in ~/.config/pokarak/language-servers.json; nothing was installed.", config.language, config.program))?;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        root.hash(&mut hash);
        let workspace = tempfile::Builder::new()
            .prefix(&format!("pokarak-lsp-{:016x}-", hash.finish()))
            .tempdir()?;
        let workspace_path = workspace.path().canonicalize()?;
        fs::create_dir(workspace_path.join("home"))?;
        let vue_tsdk = if config.language == "vue" {
            config
                .args
                .iter()
                .position(|a| a == "--tsdk")
                .and_then(|i| config.args.get(i + 1))
                .cloned()
        } else {
            None
        };
        let mut args: Vec<_> = config
            .args
            .iter()
            .map(|arg| {
                arg.replace("{root}", &root.to_string_lossy())
                    .replace("{workspace}", &workspace_path.to_string_lossy())
            })
            .collect();
        let managed_tsserver = if crate::config::family(&config.language) == "typescript"
            && config.args.iter().any(|arg| arg == "FORKARAK_MANAGED=1")
        {
            if let Some(index) = args.iter().position(|arg| arg == "--tsserver-path") {
                let path = args
                    .get(index + 1)
                    .context("Managed TypeScript SDK path missing")?
                    .clone();
                if !Path::new(&path).is_absolute() || !Path::new(&path).is_file() {
                    bail!("Managed TypeScript SDK is unavailable");
                }
                args.drain(index..=index + 1);
                Some(path)
            } else {
                None
            }
        } else {
            None
        };
        if config.language == "vue"
            && let Some(index) = args.iter().position(|a| a == "--tsdk")
        {
            if index + 1 >= args.len() {
                bail!("Vue TypeScript SDK argument is missing");
            }
            args.drain(index..=index + 1);
        }
        if config.args.iter().any(|a| a == "FORKARAK_MANAGED=1")
            && crate::config::is_jdt(config)
            && let Some(i) = args.iter().position(|a| a == "-configuration")
        {
            let source = PathBuf::from(args.get(i + 1).context("JDT configuration path missing")?);
            let destination = workspace_path.join("configuration");
            fs::create_dir(&destination)?;
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    fs::copy(entry.path(), destination.join(entry.file_name()))?;
                }
            }
            args[i + 1] = destination.to_string_lossy().into_owned();
        }
        if config.language == "java" && !jetbrains {
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
        let transport = Transport::spawn(&program, &args, import_root, cancellation)?;
        let settings = json!({
            "java": {
                "autobuild": {"enabled": false},
                "signatureHelp":{"enabled":true},
                "configuration": {"updateBuildConfiguration": "disabled"},
                "import": {"gradle": {"enabled": options.allow_project_imports}, "maven": {"enabled": options.allow_project_imports}},
                "references": {"includeDecompiledSources": false}
            },
            "rust-analyzer": {"cargo":{"buildScripts":{"enable":false},"autoreload":false},"procMacro":{"enable":false},"checkOnSave":false,"check":{"enable":false}},
            "json":{"validate":{"enable":true},"format":{"enable":true},"schemas":[]},
            "html":{"validate":{"scripts":true,"styles":true},"format":{"enable":true}},
            "css":{"validate":true,"format":{"enable":true}},
            "scss":{"validate":true,"format":{"enable":true}},
            "less":{"validate":true,"format":{"enable":true}},
            "yaml": {"validate":true,"completion":true,"hover":true,"schemaStore":{"enable":false},"schemaDownload":{"enable":false}},
            "xml": {"downloadExternalResources":{"enabled":false},"validation":{"enabled":true,"resolveExternalEntities":false}},
            "bashIde":{"shellcheckPath":"","shfmt":{"path":""}}
        });
        let mut client = Self {
            transport,
            root: root.to_path_buf(),
            import_root: import_root.to_path_buf(),
            options: options.clone(),
            capabilities: Value::Null,
            settings,
            next_id: 1,
            documents: HashMap::new(),
            diagnostics: HashMap::new(),
            unversioned_diagnostics: std::collections::HashSet::new(),
            sync_kind: 0,
            open_close: false,
            server_messages: String::new(),
            jetbrains,
            import_in_progress: false,
            import_error: None,
            _workspace: workspace,
        };
        client.transport.set_snapshot(snapshot.cloned());
        let deadline = Instant::now() + options.initialize_timeout;
        let initialized = (|| -> Result<()> {
            let mut init_options = json!({
                "disableAutomaticTypingAcquisition": true,
                // The adapter's automatic syntax server can answer definitions with
                // import aliases while the semantic project is still loading.
                "tsserver": {"logDirectory": workspace_path.join("tsserver-logs"), "logVerbosity": "off", "useSyntaxServer": "never"},
                "settings": client.settings,
                "provideFormatter": true
            });
            if let Some(path) = &managed_tsserver {
                init_options["tsserver"]["path"] = json!(path);
            }
            // No build, installation, executable-command, edit, or dynamic-registration capability.
            if jetbrains {
                init_options = json!({"indexDir": workspace_path.join("index").to_string_lossy()});
            } else if config.language == "java" {
                init_options = json!({"settings": client.settings});
            } else if config.language == "vue" {
                let sdk=vue_tsdk.as_deref().context("Vue standalone server requires an explicit managed TypeScript SDK (--tsdk absolute-path)")?;
                if !Path::new(sdk).is_absolute() || !Path::new(sdk).join("typescript.js").is_file()
                {
                    bail!("Vue TypeScript SDK is unavailable");
                }
                init_options = json!({"typescript":{"tsdk":sdk},"vue":{"hybridMode":false}});
            } else if config.language == "rust" {
                init_options = client.settings["rust-analyzer"].clone();
            }
            let response = client.request("initialize", json!({
                "processId": std::process::id(),
                "clientInfo": {"name": "Forkarak", "version": env!("CARGO_PKG_VERSION")},
                "rootPath": import_root, "rootUri": path_to_uri(import_root)?,
                "workspaceFolders": client.workspace_folders()?,
                "capabilities": {
                    "general": {"positionEncodings": ["utf-16"]},
                    "workspace": {"configuration": true, "workspaceFolders": true, "applyEdit": false},
                    "textDocument": {
                        "synchronization": {"dynamicRegistration": false, "didSave": false, "willSave": false},
                        "definition": {"dynamicRegistration": false, "linkSupport": true},
                        "references": {"dynamicRegistration": false},
                        "implementation": {"dynamicRegistration": false, "linkSupport": true},
                        "completion": {"dynamicRegistration": false, "completionItem": {"snippetSupport": true, "insertReplaceSupport": true, "documentationFormat":["markdown","plaintext"], "resolveSupport":{"properties":["documentation","detail","additionalTextEdits"]}}},
                        "hover":{"dynamicRegistration":false,"contentFormat":["markdown","plaintext"]},
                        "signatureHelp":{"dynamicRegistration":false,"signatureInformation":{"documentationFormat":["markdown","plaintext"],"parameterInformation":{"labelOffsetSupport":true},"activeParameterSupport":true}},
                        "publishDiagnostics": {"versionSupport": true},
                        "diagnostic": {"dynamicRegistration": false, "relatedDocumentSupport": false},
                        "formatting": {"dynamicRegistration": false},
                        "documentSymbol": {"dynamicRegistration": false, "hierarchicalDocumentSymbolSupport": true}
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
                    "Server selected unsupported position encoding {encoding}; Forkarak requires UTF-16"
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
        client.transport.set_snapshot(None);
        Ok(client)
    }

    fn workspace_folders(&self) -> Result<Value> {
        Ok(
            json!([{"uri": path_to_uri(&self.import_root)?, "name": self.import_root.file_name().unwrap_or_default().to_string_lossy()}]),
        )
    }

    pub fn error_message(&self, error: &str, _language: &str) -> String {
        let mut message = error.to_owned();
        if let Some(exit) = self.transport.exit_status() {
            message.push_str(&format!("; {exit}"));
        }
        let logs = self.transport.stderr();
        if !logs.is_empty() {
            message.push_str(&format!("\nServer stderr:\n{logs}"));
        }
        if !self.server_messages.is_empty() {
            message.push_str(&format!("\nServer messages:\n{}", self.server_messages));
        }
        message
    }

    pub(crate) fn capabilities(&self) -> Value {
        self.capabilities.clone()
    }

    pub fn server_messages(&self) -> &str {
        &self.server_messages
    }

    fn record_notification(&mut self, message: &Value) {
        let params = &message["params"];
        match message["method"].as_str() {
            Some("textDocument/publishDiagnostics") => {
                if let Some(path) = params["uri"]
                    .as_str()
                    .and_then(|uri| crate::uri_to_path(uri).ok())
                    && let Some(document) = self.documents.get(&path)
                {
                    if let Some(version) = params["version"].as_i64() {
                        if version == document.version && params["diagnostics"].is_array() {
                            self.diagnostics
                                .insert(path, (version, params["diagnostics"].clone()));
                        }
                    } else {
                        self.unversioned_diagnostics.insert(path);
                    }
                }
            }
            Some("intellij/importLog") => {
                if params["started"] == true {
                    self.import_in_progress = true;
                    self.import_error = None;
                }
                if params["failed"] == true {
                    self.import_error =
                        Some(format!("Project import failed: {}", params["message"]));
                }
            }
            Some("intellij/workspaceImportState") => self.record_import_state(params),
            Some("intellij/workspaceImportStatus")
                if params["blockedFolders"]
                    .as_array()
                    .is_some_and(|folders| !folders.is_empty()) =>
            {
                self.import_error = Some(format!("Project import blocked: {params}"));
            }
            _ => {}
        }
        if matches!(
            message["method"].as_str(),
            Some(
                "window/logMessage"
                    | "window/showMessage"
                    | "intellij/workspaceImportStatus"
                    | "intellij/workspaceImportState"
                    | "intellij/importLog"
                    | "language/status"
            )
        ) {
            self.server_messages
                .push_str(&format!("{}: {}\n", message["method"], message["params"]));
            if self.server_messages.len() > 16 * 1024 {
                let mut start = self.server_messages.len() - 16 * 1024;
                while !self.server_messages.is_char_boundary(start) {
                    start += 1;
                }
                self.server_messages.drain(..start);
            }
        }
    }

    fn record_import_state(&mut self, state: &Value) {
        self.import_in_progress = state["phase"] == "IN_PROGRESS";
        if matches!(state["phase"].as_str(), Some("FAILED" | "CANCELLED")) {
            self.import_error = Some(format!("Project import did not complete: {state}"));
        }
        if let Some(folders) = state["folders"].as_array() {
            for folder in folders {
                if matches!(folder["status"].as_str(), Some("FAILED" | "CANCELLED")) {
                    self.import_error = Some(format!("Project import failed: {folder}"));
                }
            }
        }
    }

    fn wait_for_import(&mut self, deadline: Instant) -> Result<()> {
        while self.import_in_progress {
            let message = self
                .transport
                .receive(deadline)
                .context("Waiting for JetBrains project import")?;
            self.record_notification(&message);
            if message.get("method").is_some() && message.get("id").is_some() {
                self.server_request(&message, deadline)?;
            }
        }
        if let Some(error) = &self.import_error {
            bail!("{error}");
        }
        Ok(())
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
                self.record_notification(&message);
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
            "client/registerCapability" if message["params"]["registrations"].as_array().is_some_and(|items|items.iter().all(|i|i["method"]=="workspace/didChangeConfiguration")) => Value::Null,
            "client/unregisterCapability" if message["params"]["unregisterations"].as_array().is_some_and(|items|items.iter().all(|i|i["method"]=="workspace/didChangeConfiguration")) => Value::Null,
            "workspace/workspaceFolders" => self.workspace_folders()?,
            "window/workDoneProgress/create" | "window/showMessageRequest" => Value::Null,
            "workspace/applyEdit" => json!({"applied": false, "failureReason": "Forkarak only reads source files during language analysis"}),
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
            if self.documents[&path].buffer_version.is_some() {
                continue;
            }
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
            self.documents.insert(
                path,
                Document {
                    text,
                    version,
                    buffer_version: None,
                },
            );
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
        // JetBrains imports the project after initialize; its first semantic
        // response needs the same cold-start budget, including dependency import.
        let timeout = if self.jetbrains {
            self.options
                .initialize_timeout
                .max(self.options.request_timeout)
        } else {
            self.options.request_timeout
        };
        let deadline = Instant::now() + timeout;
        self.refresh(deadline)?;
        if self
            .documents
            .get(path)
            .is_some_and(|doc| doc.buffer_version.is_some())
        {
            bail!(
                "Source has an editor snapshot; use navigate_buffer or explicitly release_buffer first"
            );
        }
        if !self.documents.contains_key(path) {
            if self.open_close {
                self.notify("textDocument/didOpen", json!({"textDocument": {"uri": path_to_uri(path)?, "languageId": language, "version": 1, "text": text}}), deadline)?;
            }
            self.documents.insert(
                path.to_path_buf(),
                Document {
                    text,
                    version: 1,
                    buffer_version: None,
                },
            );
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
        if self.jetbrains {
            let state = self.request("intellij/workspaceImportState", json!({}), deadline)?;
            self.record_import_state(&state);
            self.wait_for_import(deadline)?;
        }
        let mut result = self.request(method, params.clone(), deadline)?;
        if self.jetbrains {
            // A fresh server can initially report FINISHED before the first import
            // begins. Never return that early response while import is still active.
            let was_importing = self.import_in_progress;
            self.wait_for_import(deadline)?;
            if was_importing {
                result = self.request(method, params, deadline)?;
            }
        }
        for (path, document) in &self.documents {
            if document.buffer_version.is_none()
                && self.read_current_document(path)? != document.text
            {
                bail!(
                    "Source changed on disk during navigation; retry against the current file: {}",
                    path.display()
                );
            }
        }
        Ok(result)
    }

    pub(crate) fn restore_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        language: &str,
    ) -> Result<()> {
        self.sync_buffer(
            snapshot,
            language,
            Instant::now() + self.options.request_timeout,
        )
    }

    pub fn release_buffer(&mut self, path: &Path) {
        if let Some(doc) = self.documents.get_mut(path) {
            doc.buffer_version = None;
        }
        self.diagnostics.remove(path);
        self.unversioned_diagnostics.remove(path);
    }

    pub fn buffer_request(
        &mut self,
        snapshot: &DocumentSnapshot,
        language: &str,
        method: &str,
        capability: &str,
        params: Value,
    ) -> Result<Value> {
        self.transport.set_snapshot(Some(snapshot.clone()));
        let result = self.buffer_request_inner(snapshot, language, method, capability, params);
        self.transport.set_snapshot(None);
        result
    }

    fn buffer_request_inner(
        &mut self,
        snapshot: &DocumentSnapshot,
        language: &str,
        method: &str,
        capability: &str,
        params: Value,
    ) -> Result<Value> {
        snapshot.check()?;
        let timeout = if self.jetbrains {
            self.options
                .initialize_timeout
                .max(self.options.request_timeout)
        } else {
            self.options.request_timeout
        };
        let deadline = Instant::now() + timeout;
        self.refresh(deadline)?;
        self.sync_buffer(snapshot, language, deadline)?;
        let pull =
            self.capabilities[capability].is_object() || self.capabilities[capability] == true;
        if method == "completionItem/resolve"
            && self.capabilities["completionProvider"]["resolveProvider"] != true
        {
            bail!("Language server does not support completionItem/resolve");
        }
        if method != "textDocument/diagnostic" && !pull {
            bail!("Language server does not support {method}");
        }
        if self.jetbrains {
            let state = self.request("intellij/workspaceImportState", json!({}), deadline)?;
            self.record_import_state(&state);
            self.wait_for_import(deadline)?;
        }
        let mut result = if method == "textDocument/diagnostic" && !pull {
            self.snapshot_diagnostics(snapshot, language, deadline)?
        } else {
            self.request(method, params.clone(), deadline)?
        };
        if self.jetbrains {
            let was_importing = self.import_in_progress;
            self.wait_for_import(deadline)?;
            if was_importing {
                result = if method == "textDocument/diagnostic" && !pull {
                    self.snapshot_diagnostics(snapshot, language, deadline)?
                } else {
                    self.request(method, params, deadline)?
                };
            }
        }
        if method == "textDocument/diagnostic" && pull {
            if result["kind"] != "full" {
                bail!("Diagnostic server returned unchanged without a previous report");
            }
            result = result
                .get("items")
                .filter(|items| items.is_array())
                .context("Invalid diagnostic report")?
                .clone();
        }
        for (path, doc) in &self.documents {
            if doc.buffer_version.is_none() && self.read_current_document(path)? != doc.text {
                bail!("Source changed on disk during buffer analysis");
            }
        }
        snapshot.check()?;
        Ok(result)
    }

    fn snapshot_diagnostics(
        &mut self,
        snapshot: &DocumentSnapshot,
        language: &str,
        deadline: Instant,
    ) -> Result<Value> {
        if crate::config::family(language) == "typescript"
            && self.capabilities["executeCommandProvider"]["commands"]
                .as_array()
                .is_some_and(|commands| {
                    commands
                        .iter()
                        .any(|command| command == "typescript.tsserverRequest")
                })
        {
            return self.typescript_diagnostics(snapshot, deadline);
        }
        self.push_diagnostics(snapshot.path(), deadline)
    }

    /// This is deliberately not a generic execute-command API. These two fixed
    /// synchronous read-only requests see the didOpen/didChange source revision.
    fn typescript_diagnostics(
        &mut self,
        snapshot: &DocumentSnapshot,
        deadline: Instant,
    ) -> Result<Value> {
        let mut diagnostics = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for command in ["syntacticDiagnosticsSync", "semanticDiagnosticsSync"] {
            snapshot.check()?;
            let response = self.request("workspace/executeCommand",json!({
                "command":"typescript.tsserverRequest",
                "arguments":[command,{"file":crate::path_to_uri(snapshot.path())?,"includeLinePosition":true},{"executionTarget":0,"expectsResult":true,"isAsync":false}]
            }),deadline)?;
            if response["success"] == false {
                bail!("TypeScript diagnostic request failed: {response}");
            }
            let items = response["body"]
                .as_array()
                .context("Invalid synchronous TypeScript diagnostic response")?;
            for diagnostic in items {
                let location = |value: &Value| -> Result<Value> {
                    let line = value["line"]
                        .as_u64()
                        .and_then(|n| n.checked_sub(1))
                        .and_then(|n| u32::try_from(n).ok())
                        .context("Invalid one-based TypeScript diagnostic line")?;
                    let character = value["offset"]
                        .as_u64()
                        .and_then(|n| n.checked_sub(1))
                        .and_then(|n| u32::try_from(n).ok())
                        .context("Invalid one-based TypeScript diagnostic UTF-16 offset")?;
                    Ok(json!({"line":line,"character":character}))
                };
                let (start, end, message) = if diagnostic["startLocation"].is_object() {
                    (
                        &diagnostic["startLocation"],
                        &diagnostic["endLocation"],
                        &diagnostic["message"],
                    )
                } else {
                    (
                        &diagnostic["start"],
                        &diagnostic["end"],
                        &diagnostic["text"],
                    )
                };
                let message = message
                    .as_str()
                    .context("TypeScript diagnostic missing message")?;
                let severity = match diagnostic["category"].as_str() {
                    Some("error") => 1,
                    Some("warning") => 2,
                    Some("suggestion") => 4,
                    _ => 3,
                };
                let converted = json!({"range":{"start":location(start)?,"end":location(end)?},"message":message,"severity":severity,"code":diagnostic["code"],"source":diagnostic["source"].as_str().unwrap_or("typescript")});
                if seen.insert(serde_json::to_string(&converted)?) {
                    diagnostics.push(converted);
                }
            }
        }
        Ok(json!(diagnostics))
    }

    fn push_diagnostics(&mut self, path: &Path, deadline: Instant) -> Result<Value> {
        loop {
            let doc = &self.documents[path];
            if let Some((version, items)) = self.diagnostics.get(path)
                && *version == doc.version
            {
                return Ok(items.clone());
            }
            if self.unversioned_diagnostics.contains(path) {
                bail!(
                    "Server publishes unversioned diagnostics without a synchronous diagnostic API; cannot attach them safely to this source snapshot"
                );
            }
            let message = self
                .transport
                .receive(deadline)
                .context("Waiting for versioned diagnostics")?;
            self.record_notification(&message);
            if message.get("method").is_some() && message.get("id").is_some() {
                self.server_request(&message, deadline)?;
            }
        }
    }

    fn sync_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        language: &str,
        deadline: Instant,
    ) -> Result<()> {
        let path = snapshot.path();
        let text = snapshot.text();
        if !self.open_close || self.sync_kind == 0 {
            bail!("Server cannot synchronize editor snapshots");
        }
        let version = if let Some(old) = self.documents.get(path) {
            if old.text == text && old.buffer_version == Some(snapshot.version()) {
                return Ok(());
            }
            let version = old
                .version
                .checked_add(1)
                .filter(|v| *v <= i32::MAX as i64)
                .context("LSP document version exhausted; restart analysis")?
                .max(snapshot.version());
            let change = if self.sync_kind == 2 {
                let (line, character) = end_position(&old.text);
                json!({"range":{"start":{"line":0,"character":0},"end":{"line":line,"character":character}},"text":text})
            } else {
                json!({"text":text})
            };
            self.notify("textDocument/didChange",json!({"textDocument":{"uri":path_to_uri(path)?,"version":version},"contentChanges":[change]}),deadline)?;
            version
        } else {
            self.notify("textDocument/didOpen",json!({"textDocument":{"uri":path_to_uri(path)?,"languageId":language,"version":snapshot.version(),"text":text}}),deadline)?;
            snapshot.version()
        };
        self.documents.insert(
            path.to_path_buf(),
            Document {
                text: text.to_owned(),
                version,
                buffer_version: Some(snapshot.version()),
            },
        );
        self.diagnostics.remove(path);
        self.unversioned_diagnostics.remove(path);
        Ok(())
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
