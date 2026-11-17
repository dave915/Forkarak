//! Snapshot-bound signatures, hover and candidate documentation.
use crate::{BufferResult, CompletionItem, DocumentSnapshot, Navigator, TextRange};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkupContent {
    pub kind: String,
    pub value: String,
}
#[derive(Clone, Debug)]
pub struct HoverResult {
    pub contents: Vec<MarkupContent>,
    pub range: Option<TextRange>,
}
#[derive(Clone, Debug)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: Option<u32>,
    pub active_parameter: Option<u32>,
}
#[derive(Clone, Debug)]
pub struct SignatureInformation {
    pub label: String,
    pub documentation: Option<MarkupContent>,
    pub parameters: Vec<ParameterInformation>,
    pub active_parameter: Option<u32>,
}
#[derive(Clone, Debug)]
pub struct ParameterInformation {
    pub label: ParameterLabel,
    pub documentation: Option<MarkupContent>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterLabel {
    Text(String),
    Offsets { start: u32, end: u32 },
}

pub(crate) fn documentation(value: &Value) -> Result<Option<MarkupContent>> {
    if value.is_null() {
        return Ok(None);
    }
    if let Some(value) = value.as_str() {
        return Ok(Some(MarkupContent {
            kind: "plaintext".into(),
            value: value.into(),
        }));
    }
    let kind = value["kind"]
        .as_str()
        .context("Invalid documentation markup kind")?;
    if !matches!(kind, "plaintext" | "markdown") {
        bail!("Unsupported documentation markup kind {kind}");
    }
    Ok(Some(MarkupContent {
        kind: kind.into(),
        value: value["value"]
            .as_str()
            .context("Invalid documentation value")?
            .into(),
    }))
}
fn number(value: &Value) -> Result<Option<u32>> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(u32::try_from(
        value
            .as_u64()
            .context("Invalid active signature/parameter index")?,
    )?))
}
fn signature_help(value: Value) -> Result<Option<SignatureHelp>> {
    if value.is_null() {
        return Ok(None);
    }
    let mut signatures = Vec::new();
    for signature in value["signatures"]
        .as_array()
        .context("Invalid signature list")?
    {
        let label = signature["label"]
            .as_str()
            .context("Missing signature label")?;
        let mut parameters = Vec::new();
        if let Some(values) = signature.get("parameters") {
            for parameter in values.as_array().context("Invalid signature parameters")? {
                let parameter_label = if let Some(label) = parameter["label"].as_str() {
                    ParameterLabel::Text(label.into())
                } else {
                    let offsets = parameter["label"]
                        .as_array()
                        .filter(|v| v.len() == 2)
                        .context("Invalid signature parameter offsets")?;
                    let start = number(&offsets[0])?.context("Missing parameter start")?;
                    let end = number(&offsets[1])?.context("Missing parameter end")?;
                    let mut boundaries = vec![0];
                    let mut length = 0;
                    for character in label.chars() {
                        length += character.len_utf16() as u32;
                        boundaries.push(length);
                    }
                    if start > end || !boundaries.contains(&start) || !boundaries.contains(&end) {
                        bail!("Signature parameter offsets split UTF-16 or exceed its label");
                    }
                    ParameterLabel::Offsets { start, end }
                };
                parameters.push(ParameterInformation {
                    label: parameter_label,
                    documentation: documentation(&parameter["documentation"])?,
                });
            }
        }
        let active_parameter = number(&signature["activeParameter"])?;
        if active_parameter.is_some_and(|n| n as usize >= parameters.len())
            && !parameters.is_empty()
        {
            bail!("Active parameter exceeds signature parameters");
        }
        signatures.push(SignatureInformation {
            label: label.into(),
            documentation: documentation(&signature["documentation"])?,
            parameters,
            active_parameter,
        });
    }
    let active_signature = number(&value["activeSignature"])?;
    if active_signature.is_some_and(|n| n as usize >= signatures.len()) && !signatures.is_empty() {
        bail!("Active signature exceeds signature list");
    }
    Ok(Some(SignatureHelp {
        signatures,
        active_signature,
        active_parameter: number(&value["activeParameter"])?,
    }))
}
fn hover(value: Value, snapshot: &DocumentSnapshot) -> Result<Option<HoverResult>> {
    if value.is_null() {
        return Ok(None);
    }
    let contents = value.get("contents").context("Hover missing contents")?;
    let values = if let Some(values) = contents.as_array() {
        values.clone()
    } else {
        vec![contents.clone()]
    };
    let mut contents = Vec::new();
    for value in values {
        if let Some(value) = value.as_str() {
            contents.push(MarkupContent {
                kind: "markdown".into(),
                value: value.into(),
            });
        } else if let Some(language) = value["language"].as_str() {
            let source = value["value"]
                .as_str()
                .context("Invalid marked source string")?;
            let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(3.max(longest + 1));
            let language = if language
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'))
            {
                language
            } else {
                ""
            };
            contents.push(MarkupContent {
                kind: "markdown".into(),
                value: format!("{fence}{language}\n{source}\n{fence}"),
            });
        } else {
            contents.push(documentation(&value)?.context("Invalid hover documentation")?);
        }
    }
    let range = if value["range"].is_null() {
        None
    } else {
        Some(serde_json::from_value::<TextRange>(value["range"].clone())?)
    };
    if let Some(range) = range {
        range.validate(snapshot.text())?;
    }
    Ok(Some(HoverResult { contents, range }))
}
impl Navigator {
    pub fn signature_help_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        line: u32,
        column: u32,
    ) -> Result<BufferResult<Option<SignatureHelp>>> {
        let result = (|| {
            crate::paths::validate_position(snapshot.text(), line, column)?;
            let value=self.request_buffer(snapshot,"textDocument/signatureHelp","signatureHelpProvider",json!({"textDocument":{"uri":crate::path_to_uri(snapshot.path())?},"position":{"line":line,"character":column},"context":{"triggerKind":1,"isRetrigger":false}}))?;
            signature_help(value)
        })();
        self.finish_buffer(snapshot, result)
    }
    pub fn hover_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        line: u32,
        column: u32,
    ) -> Result<BufferResult<Option<HoverResult>>> {
        let result = (|| {
            crate::paths::validate_position(snapshot.text(), line, column)?;
            let value=self.request_buffer(snapshot,"textDocument/hover","hoverProvider",json!({"textDocument":{"uri":crate::path_to_uri(snapshot.path())?},"position":{"line":line,"character":column}}))?;
            hover(value, snapshot)
        })();
        self.finish_buffer(snapshot, result)
    }
    pub fn resolve_completion_buffer(
        &mut self,
        snapshot: &DocumentSnapshot,
        item: &CompletionItem,
    ) -> Result<BufferResult<CompletionItem>> {
        let result = (|| {
            if !item.source.is_current()
                || item.source.root() != snapshot.root()
                || item.source.path() != snapshot.path()
                || item.source.version() != snapshot.version()
                || item.source.text() != snapshot.text()
            {
                bail!("Stale completion candidate or mismatched source identity");
            }
            let value = self.request_buffer(
                snapshot,
                "completionItem/resolve",
                "completionProvider",
                item.wire.clone(),
            )?;
            crate::buffer::decode_completions(json!([value]), snapshot)?
                .items
                .into_iter()
                .next()
                .context("Completion resolution returned no item")
        })();
        self.finish_buffer(snapshot, result)
    }
}
