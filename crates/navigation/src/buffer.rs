//! Immutable displayed-source identities and explicit editor analysis APIs.
use crate::{
    NavigationKind, NavigationResult, Navigator,
    config::{family, language, normalize_language},
    decode_locations,
    paths::{canonical_root, other_worktree, resolved_path, validate_position},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    root: PathBuf,
    path: PathBuf,
    text: Arc<str>,
    version: i64,
    valid: Arc<AtomicBool>,
}
impl DocumentSnapshot {
    /// Only displayed working sources belong here; historical content stays read-only.
    pub fn new(root: PathBuf, path: PathBuf, text: String, version: i64) -> Result<Self> {
        let root = canonical_root(&root)?;
        let path = resolved_path(&root.join(path))?;
        if version < 0 || version > i32::MAX as i64 {
            bail!("Document version must be a nonnegative LSP integer");
        }
        if text.len() > 8 * 1024 * 1024 {
            bail!("Source exceeds the 8 MiB navigation limit");
        }
        if !path.starts_with(&root) || other_worktree(&root, &path) {
            bail!("Buffer source is outside its worktree");
        }
        Ok(Self {
            root,
            path,
            text: text.into(),
            version,
            valid: Arc::new(AtomicBool::new(true)),
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn version(&self) -> i64 {
        self.version
    }
    /// Call outside the Navigator mutex on edits, source changes or view switches.
    pub fn invalidate(&self) {
        self.valid.store(false, Ordering::Release);
    }
    pub fn is_current(&self) -> bool {
        self.valid.load(Ordering::Acquire)
    }
    pub(crate) fn same_revision(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.valid, &other.valid)
    }
    pub(crate) fn check_revision(&self) -> Result<()> {
        if !self.is_current() {
            bail!(crate::RequestFailure(
                "Stale document snapshot; displayed source changed".into()
            ));
        }
        Ok(())
    }
    pub(crate) fn check(&self) -> Result<()> {
        self.check_revision()?;
        if resolved_path(&self.path)? != self.path || other_worktree(&self.root, &self.path) {
            bail!("Source changed its filesystem target");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct BufferResult<T> {
    pub snapshot: DocumentSnapshot,
    pub value: T,
}
impl<T> BufferResult<T> {
    pub fn is_current(&self) -> bool {
        self.snapshot.is_current()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: Position,
    pub end: Position,
}
impl TextRange {
    pub(crate) fn validate(&self, text: &str) -> Result<()> {
        if self.start > self.end {
            bail!("Reversed source range");
        }
        validate_position(text, self.start.line, self.start.character)?;
        validate_position(text, self.end.line, self.end.character)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub range: TextRange,
    #[serde(rename = "newText")]
    pub new_text: String,
}
impl TextEdit {
    /// Validate all ranges against the original source and apply from right to left.
    pub fn apply_all(text: &str, edits: &[TextEdit]) -> Result<String> {
        let mut edits: Vec<_> = edits.iter().collect();
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut previous: Option<TextRange> = None;
        for edit in &edits {
            edit.range.validate(text)?;
            if let Some(range) = previous
                && (range.end > edit.range.start || range.start == edit.range.start)
            {
                bail!("Overlapping or ambiguous source edits");
            }
            previous = Some(edit.range);
        }
        let mut output = text.to_owned();
        for edit in edits.into_iter().rev() {
            output.replace_range(
                byte_offset(text, edit.range.start)?..byte_offset(text, edit.range.end)?,
                &edit.new_text,
            );
        }
        Ok(output)
    }
}
pub(crate) fn byte_offset(text: &str, pos: Position) -> Result<usize> {
    validate_position(text, pos.line, pos.character)?;
    let row = text
        .split_inclusive('\n')
        .take(pos.line as usize)
        .map(str::len)
        .sum::<usize>();
    let mut units = 0;
    for (offset, ch) in text[row..].char_indices() {
        if units == pos.character {
            return Ok(row + offset);
        }
        units += ch.len_utf16() as u32;
    }
    Ok(text.len())
}
#[derive(Clone, Debug)]
pub struct WorkspaceEdit {
    pub files: Vec<FileEdits>,
}

#[derive(Clone, Debug)]
pub struct FileEdits {
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
    /// Exact source used to interpret every UTF-16 range, including line endings.
    pub original: String,
}

#[derive(Clone, Debug)]
pub struct CodeAction {
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
    pub disabled: Option<String>,
    pub edit: Option<WorkspaceEdit>,
    /// Opaque resolver data. A deferred action without a data property uses JSON null.
    pub resolve_data: Option<Value>,
    pub(crate) wire: Value,
    pub(crate) request: Arc<crate::workspace_edit::EditRequest>,
}

#[derive(Clone, Debug)]
pub struct CompletionResult {
    pub items: Vec<CompletionItem>,
    pub is_incomplete: bool,
}
#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub documentation: Option<crate::MarkupContent>,
    pub(crate) wire: Value,
    pub(crate) source: DocumentSnapshot,
    pub detail: Option<String>,
    pub kind: Option<u32>,
    pub insert_text: String,
    pub insert_text_format: u32,
    pub text_edit: Option<TextEdit>,
    pub additional_text_edits: Vec<TextEdit>,
}
impl CompletionItem {
    /// For UIs without a snippet editor: use defaults/first choices, remove tabstops,
    /// and reject unsupported transforms. Never execute snippet variables.
    pub fn plain_text_insertion(&self) -> Result<String> {
        self.source.check()?;
        if self.insert_text_format == 2 {
            crate::snippet::plain(&self.insert_text)
        } else {
            Ok(self.insert_text.clone())
        }
    }
    pub fn plain_text_text_edit(&self) -> Result<Option<TextEdit>> {
        self.source.check()?;
        self.text_edit
            .as_ref()
            .map(|edit| {
                Ok(TextEdit {
                    range: edit.range,
                    new_text: if self.insert_text_format == 2 {
                        crate::snippet::plain(&edit.new_text)?
                    } else {
                        edit.new_text.clone()
                    },
                })
            })
            .transpose()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: TextRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
}
#[derive(Clone, Debug)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: u32,
    pub range: TextRange,
    pub selection_range: TextRange,
    pub children: Vec<DocumentSymbol>,
}
#[derive(Clone, Debug)]
pub struct FormattingOptions {
    pub tab_size: u32,
    pub insert_spaces: bool,
}
impl Default for FormattingOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatterConfig {
    pub language: String,
    pub program: String,
    pub args: Vec<String>,
}

impl Navigator {
    /// Retain another displayed working draft without starting a server. The next
    /// explicit analysis call synchronizes all retained drafts for that session.
    pub fn retain_buffer(&mut self, snapshot: &DocumentSnapshot) -> Result<()> {
        self.check_snapshot(snapshot)
    }

    /// Explicitly relinquish a retained editor buffer before returning to disk-backed navigation.
    pub fn release_buffer(&mut self, path: &Path) -> Result<()> {
        self.check_ready()?;
        let path = resolved_path(&self.root.join(path))?;
        if !path.starts_with(&self.root) || other_worktree(&self.root, &path) {
            bail!("Source is outside this worktree");
        }
        if let Some(snapshot) = self.buffer_versions.remove(&path) {
            snapshot.invalidate();
        }
        for client in self.clients.values_mut() {
            client.release_buffer(&path);
        }
        Ok(())
    }
    pub fn set_formatter_configs(&mut self, mut configs: Vec<FormatterConfig>) -> Result<()> {
        for config in &mut configs {
            config.language = normalize_language(&config.language).into();
            if config.program.trim().is_empty() {
                bail!("Empty formatter program");
            }
            if !crate::language_catalog()
                .iter()
                .any(|l| l.language == config.language && l.package.is_some())
            {
                bail!("Unsupported formatter language {}", config.language);
            }
        }
        self.formatters = configs;
        Ok(())
    }
    fn check_snapshot(&mut self, snapshot: &DocumentSnapshot) -> Result<()> {
        self.check_ready()?;
        snapshot.check()?;
        if snapshot.root != self.root {
            bail!("Snapshot belongs to another worktree");
        }
        if let Some(previous) = self.buffer_versions.get(snapshot.path()) {
            if previous.version > snapshot.version
                || (previous.version == snapshot.version && previous.text != snapshot.text)
            {
                bail!("Stale document version or reused version with different source");
            }
            if !Arc::ptr_eq(&previous.valid, &snapshot.valid) {
                previous.invalidate();
            }
        }
        self.buffer_versions
            .insert(snapshot.path.clone(), snapshot.clone());
        Ok(())
    }
    pub(crate) fn finish_buffer<T>(
        &mut self,
        snapshot: &DocumentSnapshot,
        result: Result<T>,
    ) -> Result<BufferResult<T>> {
        let result = self
            .cancellation
            .check()
            .and_then(|_| snapshot.check())
            .and(result);
        self.last_status = match &result {
            Ok(_) => "Explicit buffer analysis completed".into(),
            Err(error) => format!("{error:#}"),
        };
        result.map(|value| BufferResult {
            snapshot: snapshot.clone(),
            value,
        })
    }
    pub(crate) fn request_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        method: &str,
        capability: &str,
        params: Value,
    ) -> Result<Value> {
        self.check_snapshot(snapshot)?;
        let language = language(snapshot.path())
            .context("No language-server configuration for this file type")?;
        let origin = self.origin_for(snapshot.path(), language)?;
        self.with_client_context(&origin, Some(snapshot), |client| {
            client.buffer_request(snapshot, language, method, capability, params)
        })
    }
    pub fn navigate_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        line: u32,
        column: u32,
        kind: NavigationKind,
    ) -> Result<BufferResult<NavigationResult>> {
        let result = (|| {
            validate_position(snapshot.text(), line, column)?;
            let (method, capability) = match kind {
                NavigationKind::Definition => ("textDocument/definition", "definitionProvider"),
                NavigationKind::References => ("textDocument/references", "referencesProvider"),
                NavigationKind::Implementation => {
                    ("textDocument/implementation", "implementationProvider")
                }
            };
            let mut params = json!({"textDocument":{"uri":crate::path_to_uri(snapshot.path())?}, "position":{"line":line,"character":column}});
            if matches!(kind, NavigationKind::References) {
                params["context"] = json!({"includeDeclaration":true});
            }
            let response = self.request_buffer(snapshot, method, capability, params)?;
            let (mut locations, skipped) = decode_locations(&self.root, response)?;
            for hit in &mut locations {
                if let Some(source) = self.buffer_versions.get(&hit.path) {
                    hit.preview = source
                        .text()
                        .lines()
                        .nth(hit.line as usize)
                        .unwrap_or("")
                        .chars()
                        .take(300)
                        .collect();
                }
            }
            let language = language(snapshot.path()).unwrap();
            let origin = self.origin_for(snapshot.path(), language)?;
            self.external_sources.extend(
                locations
                    .iter()
                    .filter(|hit| !hit.path.starts_with(&self.root))
                    .map(|hit| (hit.path.clone(), origin.clone())),
            );
            let mut status = format!(
                "Semantic buffer navigation: {} location(s)",
                locations.len()
            );
            if !skipped.is_empty() {
                status.push_str(&format!("; skipped {}", skipped.join("; ")));
            }
            if matches!(kind, NavigationKind::References) && crate::config::is_jdt(&origin.config) {
                status.push_str("; JDT LS Java scope: Kotlin callers may be missing");
            }
            Ok(NavigationResult {
                locations,
                semantic: true,
                status,
            })
        })();
        self.finish_buffer(snapshot, result)
    }
    pub fn complete_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        line: u32,
        column: u32,
    ) -> Result<BufferResult<CompletionResult>> {
        let result = (|| {
            validate_position(snapshot.text(), line, column)?;
            let response = self.request_buffer(snapshot,"textDocument/completion","completionProvider",json!({"textDocument":{"uri":crate::path_to_uri(snapshot.path())?},"position":{"line":line,"character":column},"context":{"triggerKind":1}}))?;
            decode_completions(response, snapshot)
        })();
        self.finish_buffer(snapshot, result)
    }
    pub fn diagnostics(
        &mut self,
        snapshot: &DocumentSnapshot,
    ) -> Result<BufferResult<Vec<Diagnostic>>> {
        let result = (|| {
            let response = self.request_buffer(
                snapshot,
                "textDocument/diagnostic",
                "diagnosticProvider",
                json!({"textDocument":{"uri":crate::path_to_uri(snapshot.path())?}}),
            )?;
            let diagnostics: Vec<Diagnostic> = serde_json::from_value(response)?;
            for diagnostic in &diagnostics {
                diagnostic.range.validate(snapshot.text())?;
            }
            Ok(diagnostics)
        })();
        self.finish_buffer(snapshot, result)
    }
    pub fn format_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        options: &FormattingOptions,
    ) -> Result<BufferResult<Vec<TextEdit>>> {
        let result = (|| {
            self.check_snapshot(snapshot)?;
            if options.tab_size == 0 || options.tab_size > 32 {
                bail!("Invalid formatting tab size");
            }
            let language = language(snapshot.path()).context("Unsupported formatter file type")?;
            if let Some(config) = self
                .formatters
                .iter()
                .rev()
                .find(|c| c.language == language)
                .or_else(|| {
                    self.formatters
                        .iter()
                        .rev()
                        .find(|c| c.language == family(language))
                })
            {
                return crate::formatter::format(
                    snapshot,
                    config,
                    &self.cancellation,
                    self.options.request_timeout,
                );
            }
            let response = self.request_buffer(snapshot,"textDocument/formatting","documentFormattingProvider",json!({"textDocument":{"uri":crate::path_to_uri(snapshot.path())?},"options":{"tabSize":options.tab_size,"insertSpaces":options.insert_spaces}}))?;
            let edits: Vec<TextEdit> = if response.is_null() {
                vec![]
            } else {
                serde_json::from_value(response)?
            };
            TextEdit::apply_all(snapshot.text(), &edits)?;
            Ok(edits)
        })();
        self.finish_buffer(snapshot, result)
    }
    pub fn document_symbols(
        &mut self,
        snapshot: &DocumentSnapshot,
    ) -> Result<BufferResult<Vec<DocumentSymbol>>> {
        let result = (|| {
            let response = self.request_buffer(
                snapshot,
                "textDocument/documentSymbol",
                "documentSymbolProvider",
                json!({"textDocument":{"uri":crate::path_to_uri(snapshot.path())?}}),
            )?;
            if response.is_null() {
                return Ok(vec![]);
            }
            response
                .as_array()
                .context("Invalid document symbols")?
                .iter()
                .map(|value| decode_symbol(value, snapshot, 0))
                .collect()
        })();
        self.finish_buffer(snapshot, result)
    }
}

pub(crate) fn decode_completions(
    response: Value,
    snapshot: &DocumentSnapshot,
) -> Result<CompletionResult> {
    if response.is_null() {
        return Ok(CompletionResult {
            items: vec![],
            is_incomplete: false,
        });
    }
    let values = response
        .as_array()
        .or_else(|| response["items"].as_array())
        .context("Invalid completion list")?;
    // itemDefaults are not advertised; opaque candidate data is retained for resolution.
    let mut items = Vec::new();
    for value in values {
        let label = value["label"]
            .as_str()
            .context("Completion missing label")?
            .to_owned();
        let edit = if value["textEdit"].is_null() {
            None
        } else if value["textEdit"]["range"].is_object() {
            Some(serde_json::from_value::<TextEdit>(
                value["textEdit"].clone(),
            )?)
        } else {
            Some(TextEdit {
                range: serde_json::from_value(value["textEdit"]["replace"].clone())?,
                new_text: value["textEdit"]["newText"]
                    .as_str()
                    .context("Completion edit missing text")?
                    .into(),
            })
        };
        let additional: Vec<TextEdit> = if value["additionalTextEdits"].is_null() {
            vec![]
        } else {
            serde_json::from_value(value["additionalTextEdits"].clone())?
        };
        let mut all = additional.clone();
        if let Some(edit) = &edit {
            all.push(edit.clone());
        }
        TextEdit::apply_all(snapshot.text(), &all)?;
        let format = value["insertTextFormat"].as_u64().unwrap_or(1);
        if !matches!(format, 1 | 2) {
            bail!("Unsupported completion insertion format");
        }
        items.push(CompletionItem {
            label: label.clone(),
            documentation: crate::intelligence::documentation(&value["documentation"])?,
            wire: value.clone(),
            source: snapshot.clone(),
            detail: value["detail"].as_str().map(str::to_owned),
            kind: value["kind"].as_u64().map(u32::try_from).transpose()?,
            insert_text: value["insertText"].as_str().unwrap_or(&label).into(),
            insert_text_format: format as u32,
            text_edit: edit,
            additional_text_edits: additional,
        });
    }
    Ok(CompletionResult {
        items,
        is_incomplete: response["isIncomplete"].as_bool().unwrap_or(false),
    })
}
fn decode_symbol(
    value: &Value,
    snapshot: &DocumentSnapshot,
    depth: usize,
) -> Result<DocumentSymbol> {
    if depth > 64 {
        bail!("Document symbol nesting exceeds limit");
    }
    let (range, selection_range) = if value.get("location").is_some() {
        if resolved_path(&crate::uri_to_path(
            value["location"]["uri"]
                .as_str()
                .context("Symbol URI missing")?,
        )?)? != snapshot.path
        {
            bail!("Document symbol belongs to another file");
        }
        let range = serde_json::from_value::<TextRange>(value["location"]["range"].clone())?;
        (range, range)
    } else {
        (
            serde_json::from_value(value["range"].clone())?,
            serde_json::from_value(value["selectionRange"].clone())?,
        )
    };
    range.validate(snapshot.text())?;
    selection_range.validate(snapshot.text())?;
    if selection_range.start < range.start || selection_range.end > range.end {
        bail!("Symbol selection is outside its source range");
    }
    let children = value["children"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|v| decode_symbol(v, snapshot, depth + 1))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(DocumentSymbol {
        name: value["name"]
            .as_str()
            .context("Symbol missing name")?
            .into(),
        detail: value["detail"].as_str().map(str::to_owned),
        kind: u32::try_from(value["kind"].as_u64().context("Symbol kind missing")?)?,
        range,
        selection_range,
        children,
    })
}
