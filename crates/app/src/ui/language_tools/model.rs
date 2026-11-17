use super::*;

/// One request is tied to source, view, editor and the exact originating selection.
#[derive(Clone)]
pub(in crate::ui) struct Identity {
    pub snapshot: DocumentSnapshot,
    pub key: FileKey,
    pub editor: EntityId,
    pub selection: std::ops::Range<usize>,
    pub cursor: usize,
    pub settings: serde_json::Value,
    pub view: u64,
}
impl Identity {
    pub fn matches(&self, other: &Self, selection: bool) -> bool {
        self.snapshot.is_current()
            && other.snapshot.is_current()
            && self.key == other.key
            && self.editor == other.editor
            && self.view == other.view
            && self.settings == other.settings
            && self.snapshot.version() == other.snapshot.version()
            && self.snapshot.text() == other.snapshot.text()
            && (!selection || (self.selection == other.selection && self.cursor == other.cursor))
    }
}

pub(in crate::ui) fn configs(
    value: Option<&serde_json::Value>,
) -> Result<Vec<pokarak_navigation::ServerConfig>, String> {
    let Some(value) = value else {
        return Ok(vec![]);
    };
    let owned;
    let value = if let Some(text) = value.as_str() {
        owned = serde_json::from_str(text).map_err(|e| format!("언어 서버 설정 JSON: {e}"))?;
        &owned
    } else {
        value
    };
    let values = value
        .as_object()
        .ok_or("언어 서버 설정은 언어별 경로·인수 객체여야 합니다.")?;
    values
        .iter()
        .map(|(language, item)| {
            let path = item
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("{language}: 서버 실행 경로가 필요합니다."))?;
            let args = match item.get("args") {
                None => vec![],
                Some(value) => value
                    .as_array()
                    .ok_or("서버 인수는 문자열 배열이어야 합니다.")?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "서버 인수는 문자열이어야 합니다.".to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            };
            Ok(pokarak_navigation::ServerConfig {
                language: normalized_language(language).into(),
                program: path.into(),
                args,
            })
        })
        .collect()
}

pub(super) fn completion_edit(
    identity: &Identity,
    item: &CompletionItem,
) -> Result<(String, usize), String> {
    let rope = gpui_kit::component::input::Rope::from(identity.snapshot.text());
    let primary = if let Some(edit) = item
        .plain_text_text_edit()
        .map_err(|e| format!("완성 snippet: {e:#}"))?
    {
        edit
    } else {
        let span = if identity.selection.is_empty() {
            rope.word_range(identity.cursor)
                .map(|r| r.start..identity.cursor)
                .unwrap_or(identity.cursor..identity.cursor)
        } else {
            identity.selection.clone()
        };
        TextEdit {
            range: TextRange {
                start: utf16(&rope, span.start),
                end: utf16(&rope, span.end),
            },
            new_text: item
                .plain_text_insertion()
                .map_err(|e| format!("완성 snippet: {e:#}"))?,
        }
    };
    let mut edits = item.additional_text_edits.clone();
    edits.push(primary.clone());
    let replacement = TextEdit::apply_all(identity.snapshot.text(), &edits)
        .map_err(|e| format!("완성 편집을 적용할 수 없습니다: {e:#}"))?;
    let start = offset(&rope, primary.range.start).ok_or("완성 위치가 현재 소스와 다릅니다.")?;
    let mut caret = start as isize + primary.new_text.len() as isize;
    for edit in &item.additional_text_edits {
        let a = offset(&rope, edit.range.start).ok_or("추가 편집 위치 오류")?;
        let b = offset(&rope, edit.range.end).ok_or("추가 편집 위치 오류")?;
        if b <= start {
            caret += edit.new_text.len() as isize - (b - a) as isize;
        }
    }
    let caret = usize::try_from(caret).map_err(|_| "완성 커서 위치 오류")?;
    if caret > replacement.len() || !replacement.is_char_boundary(caret) {
        return Err("완성 커서 위치 오류".into());
    }
    Ok((replacement, caret))
}

pub(super) fn utf16(text: &gpui_kit::component::input::Rope, offset: usize) -> LspPosition {
    let point = text.offset_to_point(offset);
    let start = text.line_start_offset(point.row);
    LspPosition {
        line: point.row as u32,
        character: text.slice(start..offset).len_utf16() as u32,
    }
}
pub(super) fn offset(
    text: &gpui_kit::component::input::Rope,
    position: LspPosition,
) -> Option<usize> {
    let point =
        reader_navigation::editor_position(text, Position::new(position.line, position.character))?;
    Some(text.position_to_offset(&point))
}

#[derive(Clone)]
pub(super) struct SymbolRow {
    pub name: String,
    pub detail: String,
    pub position: LspPosition,
    pub depth: usize,
    pub branch: bool,
    pub id: usize,
}
pub(super) fn symbol_rows(
    symbols: &[DocumentSymbol],
    query: &str,
    collapsed: &HashSet<usize>,
) -> Vec<SymbolRow> {
    fn walk(
        symbols: &[DocumentSymbol],
        query: &str,
        collapsed: &HashSet<usize>,
        depth: usize,
        next: &mut usize,
        out: &mut Vec<SymbolRow>,
    ) {
        for symbol in symbols {
            let id = *next;
            *next += 1;
            let mut children = vec![];
            walk(
                &symbol.children,
                query,
                collapsed,
                depth + 1,
                next,
                &mut children,
            );
            let matches = query.is_empty()
                || format!("{} {}", symbol.name, symbol.detail.as_deref().unwrap_or(""))
                    .to_lowercase()
                    .contains(query);
            if matches || !children.is_empty() {
                out.push(SymbolRow {
                    name: symbol.name.clone(),
                    detail: symbol.detail.clone().unwrap_or_default(),
                    position: symbol.selection_range.start,
                    depth,
                    branch: !symbol.children.is_empty(),
                    id,
                });
                if !query.is_empty() || !collapsed.contains(&id) {
                    out.extend(children);
                }
            }
        }
    }
    let mut out = vec![];
    walk(
        symbols,
        &query.trim().to_lowercase(),
        collapsed,
        0,
        &mut 0,
        &mut out,
    );
    out
}

pub(super) fn parameter_text(
    label: &str,
    parameter: &pokarak_navigation::ParameterLabel,
) -> Option<String> {
    match parameter {
        pokarak_navigation::ParameterLabel::Text(text) => Some(text.clone()),
        pokarak_navigation::ParameterLabel::Offsets { start, end } => {
            let text = gpui_kit::component::input::Rope::from(label);
            let a = offset(
                &text,
                LspPosition {
                    line: 0,
                    character: *start,
                },
            )?;
            let b = offset(
                &text,
                LspPosition {
                    line: 0,
                    character: *end,
                },
            )?;
            Some(label.get(a..b)?.into())
        }
    }
}

/// Keep backend language-family routing without letting lower-priority exact
/// defaults override a user's family-level configuration. Same-layer exact
/// overrides remain available alongside a family entry.
pub(in crate::ui) fn normalized_language(language: &str) -> &str {
    match language {
        "ts" | "js/ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "kt" | "kts" => "kotlin",
        "rs" => "rust",
        "py" => "python",
        "sh" | "bash" | "shell" => "shellscript",
        "yml" => "yaml",
        "md" => "markdown",
        "make" => "makefile",
        "gradle" => "groovy",
        other => other,
    }
}
pub(super) fn overlay_configs(
    all: &mut Vec<pokarak_navigation::ServerConfig>,
    mut overrides: Vec<pokarak_navigation::ServerConfig>,
) {
    for config in &mut overrides {
        config.language = normalized_language(&config.language).into();
    }
    all.retain(|old| {
        let language = normalized_language(&old.language);
        !overrides.iter().any(|new| {
            new.language == language
                || (new.language == pokarak_navigation::language_family(&new.language)
                    && new.language == pokarak_navigation::language_family(language))
        })
    });
    all.extend(overrides);
}
