//! Client-requested edits are decoded into previews. The server never writes files.
use crate::{
    BufferResult, CodeAction, Diagnostic, DocumentSnapshot, FileEdits, Navigator, Position,
    TextEdit, TextRange, WorkspaceEdit,
    buffer::byte_offset,
    client::Client,
    paths::{resolved_path, validate_position},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

const STALE: &str = "편집 중 내용이 바뀌어 적용하지 않았습니다. 다시 시도하세요";
const RESOURCE_OPERATION: &str = "파일 생성·이동이 포함된 수정은 아직 지원하지 않습니다";

#[derive(Debug)]
pub(crate) struct EditRequest {
    source: DocumentSnapshot,
    retained: BTreeMap<PathBuf, DocumentSnapshot>,
    session: PathBuf,
    started: SystemTime,
}

impl EditRequest {
    fn check(&self, snapshot: &DocumentSnapshot, client: &Client) -> Result<()> {
        self.source.check()?;
        ensure!(
            snapshot.same_revision(&self.source) && client.session_path() == self.session,
            "{STALE}"
        );
        for source in self.retained.values() {
            source.check()?;
        }
        Ok(())
    }

    fn target_path(&self, uri: &str) -> Result<PathBuf> {
        let root = self.source.root();
        let spelling = crate::uri_to_path(uri)?;
        let path = resolved_path(&spelling)?;
        let relative = path
            .strip_prefix(root)
            .context("현재 워크트리 밖의 파일은 수정할 수 없습니다")?;
        ensure!(
            !relative
                .components()
                .any(|p| p.as_os_str().eq_ignore_ascii_case(".git"))
                && !path
                    .ancestors()
                    .skip(1)
                    .take_while(|p| *p != root)
                    .any(|p| p.join(".git").exists()),
            "현재 워크트리 밖의 파일은 수정할 수 없습니다"
        );
        ensure!(path.is_file(), "수정할 파일이 없습니다");
        if path != spelling {
            ensure!(spelling.starts_with(root), "{STALE}");
            check_change_times(root, &spelling, self.started)?;
        }
        Ok(path)
    }

    fn original(
        &self,
        path: &Path,
        version: Option<i64>,
        client: &Client,
        disk: &mut DiskSources,
    ) -> Result<String> {
        let open = client.edit_document(path);
        if let Some(version) = version {
            ensure!(open.is_some_and(|(_, sent, _)| sent == version), "{STALE}");
        }
        if let Some(source) = self.retained.get(path) {
            source.check()?;
            ensure!(
                open.is_some_and(|(text, _, retained)| text == source.text()
                    && retained == Some(source.version())),
                "{STALE}"
            );
            return Ok(source.text().to_owned());
        }
        // Closed targets have no LSP document version. Establish that this path and
        // its bytes predate the request, even when the server identifies it only on reply.
        let root = self.source.root();
        check_change_times(root, path, self.started)?;
        let original = crate::client::read_document(path)?;
        check_change_times(root, path, self.started)?;
        if let Some((sent, _, retained)) = open {
            ensure!(retained.is_none() && original == sent, "{STALE}");
        }
        disk.verify(path, &original)?;
        Ok(original)
    }
}

// The first response discovers unopened targets. Only accept a subsequent read-only
// response after pinning those exact bytes. This avoids scanning an entire checkout.
#[derive(Default)]
struct DiskSources {
    files: BTreeMap<PathBuf, String>,
    pinned: bool,
}

impl DiskSources {
    fn verify(&mut self, path: &Path, original: &str) -> Result<()> {
        if let Some(expected) = self.files.get(path) {
            ensure!(expected == original, "{STALE}");
        } else {
            ensure!(!self.pinned, "{STALE}");
            self.files.insert(path.to_owned(), original.to_owned());
        }
        Ok(())
    }
}

// ctime also advances on replacement/rename and on backdating mtime. Check ancestors
// to catch directory swaps that leave an old inode's own timestamps untouched.
fn check_change_times(root: &Path, path: &Path, started: SystemTime) -> Result<()> {
    #[cfg(unix)]
    let stamp = {
        let since = started.duration_since(SystemTime::UNIX_EPOCH)?;
        (
            i64::try_from(since.as_secs())?,
            i64::from(since.subsec_nanos()),
        )
    };
    for part in path.ancestors().take_while(|part| *part != root) {
        let meta = fs::symlink_metadata(part)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!((meta.ctime(), meta.ctime_nsec()) < stamp, "{STALE}");
        }
        #[cfg(not(unix))]
        ensure!(
            meta.modified()? < started && meta.created()? < started,
            "{STALE}"
        );
    }
    Ok(())
}

impl Navigator {
    fn edit_client(&mut self, source: &DocumentSnapshot) -> Result<&Client> {
        let language =
            crate::config::language(source.path()).context("지원되지 않는 언어입니다")?;
        let origin = self.origin_for(source.path(), language)?;
        let client = self.clients.get(&origin.key).context(STALE)?;
        let health = client.transport.check_health();
        self.client_result(&origin.key, health)?;
        Ok(&self.clients[&origin.key])
    }

    fn edit_request(&mut self, source: &DocumentSnapshot) -> Result<(Arc<EditRequest>, Value)> {
        let started = SystemTime::now();
        let caps = self.capabilities_buffer(source)?.value;
        let session = self.edit_client(source)?.session_path().to_owned();
        let request = EditRequest {
            source: source.clone(),
            session,
            started,
            retained: self.buffer_versions.clone(),
        };
        Ok((Arc::new(request), caps))
    }

    fn request_edits<T>(
        &mut self,
        request: &EditRequest,
        method: &str,
        provider: &str,
        params: Value,
        decode: impl Fn(&Value, &Client, &mut DiskSources) -> Result<T>,
    ) -> Result<T> {
        let source = &request.source;
        let mut disk = DiskSources::default();
        loop {
            let value = self.request_buffer(source, method, provider, params.clone())?;
            let client = self.edit_client(source)?;
            request.check(source, client)?;
            let result = decode(&value, client, &mut disk)?;
            request.check(source, client)?;
            if disk.files.is_empty() || disk.pinned {
                return Ok(result);
            }
            disk.pinned = true;
        }
    }

    pub fn prepare_rename_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        line: u32,
        column: u32,
    ) -> Result<BufferResult<Option<(TextRange, String)>>> {
        let result = (|| {
            validate_position(snapshot.text(), line, column)?;
            let caps = self.capabilities_buffer(snapshot)?.value;
            let provider = &caps["renameProvider"];
            require_provider(provider, "이 언어 서버는 이름 바꾸기를 지원하지 않습니다")?;
            if provider["prepareProvider"] != true {
                return word_at(snapshot.text(), line, column);
            }
            let response = self.request_buffer(
                snapshot,
                "textDocument/prepareRename",
                "renameProvider",
                position_params(snapshot, line, column)?,
            )?;
            if response.is_null() {
                return Ok(None);
            }
            if response["defaultBehavior"] == true {
                return word_at(snapshot.text(), line, column);
            }
            let range: TextRange =
                serde_json::from_value(response.get("range").unwrap_or(&response).clone())?;
            range.validate(snapshot.text())?;
            let text = &snapshot.text()[byte_offset(snapshot.text(), range.start)?
                ..byte_offset(snapshot.text(), range.end)?];
            Ok(Some((
                range,
                response["placeholder"].as_str().unwrap_or(text).to_owned(),
            )))
        })();
        self.finish_buffer(snapshot, result)
    }

    pub fn rename_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        line: u32,
        column: u32,
        new_name: &str,
    ) -> Result<BufferResult<WorkspaceEdit>> {
        let result = (|| {
            validate_position(snapshot.text(), line, column)?;
            ensure!(!new_name.trim().is_empty(), "새 이름을 입력하세요");
            let (request, caps) = self.edit_request(snapshot)?;
            require_provider(
                &caps["renameProvider"],
                "이 언어 서버는 이름 바꾸기를 지원하지 않습니다",
            )?;
            let mut params = position_params(snapshot, line, column)?;
            params["newName"] = json!(new_name);
            self.request_edits(
                &request,
                "textDocument/rename",
                "renameProvider",
                params,
                |value, client, disk| {
                    if value.is_null() {
                        return Ok(WorkspaceEdit { files: vec![] });
                    }
                    decode_edit(value, &request, client, disk)
                },
            )
        })();
        self.finish_buffer(snapshot, result)
    }

    pub fn code_actions_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        range: TextRange,
        diagnostics: &[Diagnostic],
    ) -> Result<BufferResult<Vec<CodeAction>>> {
        let result = (|| {
            range.validate(snapshot.text())?;
            let diagnostics: Vec<_> = diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.range.start.line <= range.end.line
                        && diagnostic.range.end.line >= range.start.line
                })
                .collect();
            for diagnostic in &diagnostics {
                diagnostic.range.validate(snapshot.text())?;
            }
            let (request, caps) = self.edit_request(snapshot)?;
            let provider = &caps["codeActionProvider"];
            require_provider(provider, "이 언어 서버는 빠른 수정을 지원하지 않습니다")?;
            let params = json!({
                "textDocument":{"uri":crate::path_to_uri(snapshot.path())?},"range":range,
                "context":{"diagnostics":diagnostics,"only":["quickfix","refactor","source"],"triggerKind":1}
            });
            self.request_edits(
                &request,
                "textDocument/codeAction",
                "codeActionProvider",
                params,
                |value, client, disk| {
                    if value.is_null() {
                        return Ok(vec![]);
                    }
                    let mut actions = Vec::new();
                    for wire in value.as_array().context("잘못된 빠른 수정 목록입니다")?
                    {
                        if let Some(action) = decode_action(
                            wire,
                            &request,
                            client,
                            disk,
                            provider["resolveProvider"] == true,
                        )? {
                            actions.push(action);
                        }
                    }
                    Ok(actions)
                },
            )
        })();
        self.finish_buffer(snapshot, result)
    }

    pub fn resolve_code_action_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        action: &CodeAction,
    ) -> Result<BufferResult<CodeAction>> {
        let result = (|| {
            self.check_ready()?;
            snapshot.check()?;
            let client = self.edit_client(snapshot)?;
            action.request.check(snapshot, client)?;
            if action.disabled.is_some() {
                return Ok(action.clone());
            }
            if let Some(edit) = &action.edit {
                for file in &edit.files {
                    ensure!(
                        action.request.original(
                            &file.path,
                            None,
                            client,
                            &mut DiskSources::default()
                        )? == file.original,
                        "{STALE}"
                    );
                }
                return Ok(action.clone());
            }
            ensure!(
                client.capabilities()["codeActionProvider"]["resolveProvider"] == true,
                "이 언어 서버는 빠른 수정 확인을 지원하지 않습니다"
            );
            self.request_edits(
                &action.request,
                "codeAction/resolve",
                "codeActionProvider",
                action.wire.clone(),
                |wire, client, disk| {
                    decode_action(wire, &action.request, client, disk, false)?
                        .context("이 빠른 수정에 적용할 텍스트 변경이 없습니다")
                },
            )
        })();
        self.finish_buffer(snapshot, result)
    }
}

fn require_provider(provider: &Value, message: &str) -> Result<()> {
    ensure!(provider == true || provider.is_object(), "{message}");
    Ok(())
}

fn position_params(source: &DocumentSnapshot, line: u32, character: u32) -> Result<Value> {
    Ok(
        json!({"textDocument":{"uri":crate::path_to_uri(source.path())?},"position":{"line":line,"character":character}}),
    )
}

fn word_at(text: &str, line: u32, character: u32) -> Result<Option<(TextRange, String)>> {
    let offset = byte_offset(text, Position { line, character })?;
    let identifier = |c: char| c.is_alphanumeric() || matches!(c, '_' | '$');
    let left = text[..offset].trim_end_matches(identifier).len();
    let right = text.len() - text[offset..].trim_start_matches(identifier).len();
    if left == right {
        return Ok(None);
    }
    let start = character - text[left..offset].encode_utf16().count() as u32;
    let end = character + text[offset..right].encode_utf16().count() as u32;
    Ok(Some((
        TextRange {
            start: Position {
                line,
                character: start,
            },
            end: Position {
                line,
                character: end,
            },
        },
        text[left..right].into(),
    )))
}

fn decode_action(
    wire: &Value,
    request: &Arc<EditRequest>,
    client: &Client,
    disk: &mut DiskSources,
    can_resolve: bool,
) -> Result<Option<CodeAction>> {
    // Bare Commands and CodeActions whose only effect is a command are never executable here.
    if wire["command"].is_string() || (wire["edit"].is_null() && !wire["command"].is_null()) {
        return Ok(None);
    }
    let mut disabled = wire["disabled"]["reason"].as_str().map(str::to_owned);
    let data = wire.get("data").cloned().or_else(|| {
        (can_resolve && wire["edit"].is_null() && disabled.is_none()).then_some(Value::Null)
    });
    let edit = if wire["edit"].is_null() {
        if disabled.is_none() && (!can_resolve || data.is_none()) {
            return Ok(None);
        }
        None
    } else {
        // Keep the reason visible on this item without hiding other valid quick fixes.
        match decode_edit(&wire["edit"], request, client, disk) {
            Ok(edit) => Some(edit),
            Err(error) => {
                disabled = Some(error.to_string());
                None
            }
        }
    };
    Ok(Some(CodeAction {
        title: wire["title"]
            .as_str()
            .context("빠른 수정 이름이 없습니다")?
            .to_owned(),
        kind: wire["kind"].as_str().map(str::to_owned),
        preferred: wire["isPreferred"] == true,
        disabled,
        edit,
        resolve_data: data,
        wire: wire.clone(),
        request: request.clone(),
    }))
}

fn decode_edit(
    value: &Value,
    request: &EditRequest,
    client: &Client,
    disk: &mut DiskSources,
) -> Result<WorkspaceEdit> {
    ensure!(value.is_object(), "잘못된 워크스페이스 수정입니다");
    let mut files = BTreeMap::new();
    let mut add = |uri: &str, version: Option<i64>, edits: &Value| -> Result<()> {
        let path = request.target_path(uri)?;
        let original = request.original(&path, version, client, disk)?;
        let file = files.entry(path.clone()).or_insert_with(|| FileEdits {
            path,
            original: original.clone(),
            edits: vec![],
        });
        ensure!(file.original == original, "{STALE}");
        file.edits
            .extend(serde_json::from_value::<Vec<TextEdit>>(edits.clone())?);
        Ok(())
    };
    if let Some(changes) = value.get("documentChanges") {
        let changes = changes.as_array().context("잘못된 documentChanges입니다")?;
        if changes.iter().any(|change| change.get("kind").is_some()) {
            bail!(RESOURCE_OPERATION);
        }
        for change in changes {
            let doc = &change["textDocument"];
            let version = if doc["version"].is_null() {
                None
            } else {
                Some(doc["version"].as_i64().context("잘못된 문서 버전입니다")?)
            };
            add(
                doc["uri"].as_str().context("수정할 파일 URI가 없습니다")?,
                version,
                &change["edits"],
            )?;
        }
    } else if let Some(changes) = value.get("changes") {
        for (uri, edits) in changes.as_object().context("잘못된 changes입니다")? {
            add(uri, None, edits)?;
        }
    }
    let files: Vec<_> = files
        .into_values()
        .filter(|file| !file.edits.is_empty())
        .collect();
    for file in &files {
        TextEdit::apply_all(&file.original, &file.edits)?;
    }
    Ok(WorkspaceEdit { files })
}
