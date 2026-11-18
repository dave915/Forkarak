//! Keep native editor coordinates separate from the UTF-16 navigation protocol.
use super::*;
use gpui_kit::component::input::{Rope, RopeExt};
use pokarak_navigation::DocumentSnapshot;

/// The position actually submitted to the provider, including an explicit retry.
#[derive(Clone)]
pub(super) struct Query {
    pub source: DocumentSnapshot,
    pub position: Position,
    pub tool: language_tools::Tool,
}

impl Query {
    pub fn symbol(&self) -> Option<&str> {
        let line = self
            .source
            .text()
            .lines()
            .nth(self.position.line as usize)?;
        let mut chars = line.char_indices();
        let mut units = 0;
        let mut start = 0;
        while units < self.position.character {
            let (i, ch) = chars.next()?;
            units += ch.len_utf16() as u32;
            if !identifier_char(ch) {
                start = i + ch.len_utf8();
            }
        }
        let end = chars
            .find(|(_, ch)| !identifier_char(*ch))
            .map_or(line.len(), |(i, _)| i);
        (units == self.position.character && start < end).then(|| &line[start..end])
    }

    pub fn matches_origin(&self, root: &Path, path: &Path, text: &str) -> bool {
        self.source.root() == root
            && self.source.path() == root.join(path)
            && self.source.text() == text
    }
}

pub(super) struct ResultSource {
    pub query: Query,
    pub revisions: HashMap<PathBuf, FileRevision>,
}

impl ResultSource {
    pub fn new(
        source: &DocumentSnapshot,
        position: pokarak_navigation::Position,
        tool: language_tools::Tool,
        revisions: HashMap<PathBuf, FileRevision>,
    ) -> Self {
        Self {
            query: Query {
                source: source.clone(),
                position: Position::new(position.line, position.character),
                tool,
            },
            revisions,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ResultGroup {
    Definition,
    Usages,
}
impl ResultGroup {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Definition => "정의 · 의미 분석",
            Self::Usages => "사용처 · 의미 분석",
        }
    }
}

/// One clicked-source operation, published once after both semantic requests.
pub(super) struct SymbolNavigation {
    pub definitions: Vec<SourceLocation>,
    pub usages: Vec<SourceLocation>,
    pub title: String,
    pub status: String,
}

fn same_location(a: &SourceLocation, b: &SourceLocation) -> bool {
    a.path == b.path && a.line == b.line && a.column == b.column
}

impl SymbolNavigation {
    pub(super) fn from_results(
        definitions: anyhow::Result<NavigationResult>,
        usages: anyhow::Result<NavigationResult>,
    ) -> Self {
        fn semantic(result: anyhow::Result<NavigationResult>) -> anyhow::Result<NavigationResult> {
            let result = result?;
            anyhow::ensure!(
                result.semantic,
                "텍스트 후보는 의미 분석 결과가 아닙니다: {}",
                result.status
            );
            Ok(result)
        }
        fn unique(result: &anyhow::Result<NavigationResult>) -> Vec<SourceLocation> {
            let mut hits = vec![];
            if let Ok(result) = result {
                for hit in &result.locations {
                    if !hits.iter().any(|previous| same_location(previous, hit)) {
                        hits.push(hit.clone());
                    }
                }
            }
            hits
        }
        let definitions = semantic(definitions);
        let usages = semantic(usages);
        let definition_hits = unique(&definitions);
        let mut usage_hits = unique(&usages);
        // LSP references include declarations. Keep the two roles distinct.
        usage_hits.retain(|hit| !definition_hits.iter().any(|d| same_location(d, hit)));
        let label = |name: &str, count: usize, result: &anyhow::Result<NavigationResult>| {
            if result.is_ok() {
                format!("{name} {count}개")
            } else {
                format!("{name} 확인 실패")
            }
        };
        let title = format!(
            "{} · {}",
            label("정의", definition_hits.len(), &definitions),
            label("사용처", usage_hits.len(), &usages)
        );
        let detail = |name: &str, result: &anyhow::Result<NavigationResult>| match result {
            Ok(result) => format!("{name}: {}", result.status),
            Err(error) => format!("{name}: {error:#}"),
        };
        let status = format!(
            "{title} · {} · {}",
            detail("정의", &definitions),
            detail("사용처", &usages)
        );
        Self {
            definitions: definition_hits,
            usages: usage_hits,
            title,
            status,
        }
    }

    pub(super) fn unique_definition(&self) -> Option<SourceLocation> {
        (self.definitions.len() == 1).then(|| self.definitions[0].clone())
    }

    fn excerpts(self) -> Vec<ResultExcerpt> {
        self.definitions
            .into_iter()
            .map(|location| ResultExcerpt {
                location,
                group: Some(ResultGroup::Definition),
            })
            .chain(self.usages.into_iter().map(|location| ResultExcerpt {
                location,
                group: Some(ResultGroup::Usages),
            }))
            .collect()
    }
}

pub(super) fn is_symbol_click(event: &MouseDownEvent) -> bool {
    event.button == MouseButton::Left
        && event.modifiers.platform
        && !event.modifiers.shift
        && !event.modifiers.alt
        && !event.modifiers.control
        && event.click_count == 1
}

pub(super) struct NavigationTarget {
    pub position: Position,
    pub symbol_start: Position,
}

fn utf16_position(text: &Rope, offset: usize) -> Position {
    let point = text.offset_to_point(offset);
    let start = text.line_start_offset(point.row);
    Position::new(
        point.row as u32,
        text.slice(start..start + point.column).len_utf16() as u32,
    )
}

fn byte_offset(text: &Rope, position: Position) -> Option<usize> {
    let row = position.line as usize;
    if row >= text.lines_len() {
        return None;
    }
    let line = text.slice_line(row);
    let column = position.character as usize;
    if column > line.len_utf16() {
        return None;
    }
    let offset = line.utf16_to_byte_idx(column);
    // A server position must never split a surrogate pair.
    if line.byte_to_utf16_idx(offset) != column {
        return None;
    }
    Some(text.line_start_offset(row) + offset)
}

pub(super) fn editor_position(text: &Rope, position: Position) -> Option<Position> {
    byte_offset(text, position).map(|offset| text.offset_to_position(offset))
}

pub(super) fn navigation_target(
    text: &Rope,
    cursor: usize,
    requested: Option<Position>,
) -> Option<NavigationTarget> {
    let offset = match requested {
        Some(position) => byte_offset(text, position)?,
        None => cursor,
    };
    let position = utf16_position(text, offset);
    let symbol_start = text
        .word_range(offset)
        .map(|range| utf16_position(text, range.start))
        .unwrap_or(position);
    Some(NavigationTarget {
        position,
        symbol_start,
    })
}

fn text_hit(text: &Rope, utf16_index: usize) -> Option<Position> {
    if utf16_index >= text.len_utf16() {
        return None;
    }
    let offset = text.offset_utf16_to_offset(utf16_index);
    let ch = text.char_at(offset)?;
    if text.offset_to_offset_utf16(offset) != utf16_index || !identifier_char(ch) {
        return None;
    }
    Some(utf16_position(text, offset))
}

fn identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

fn inside_text_viewport(viewport: Bounds<Pixels>, gutter: Pixels, point: Point<Pixels>) -> bool {
    viewport.contains(&point) && point.x >= viewport.left() + gutter
}

fn clicked_position(
    editor: &mut EditorState,
    point: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) -> Option<Position> {
    // The gutter stays fixed even when the text scrolls horizontally.
    if !inside_text_viewport(editor.input_bounds(), editor.text_origin_offset()?, point) {
        return None;
    }
    if let Some(index) = editor.character_index_for_point(point, window, cx) {
        return text_hit(editor.text(), index);
    }
    // gpui-base 0.10.0 clips this native hit test against a viewport-sized
    // rectangle whose origin has already scrolled. Outside that rectangle,
    // inspect its exact laid-out glyph bounds instead; never guess a column.
    if editor.text_bounds()?.contains(&point) {
        return None;
    }
    glyph_hit(editor, point)
}

fn glyph_hit(editor: &EditorState, point: Point<Pixels>) -> Option<Position> {
    let line_height = editor.line_height()?;
    let text = editor.text();
    for row in editor.visible_row_range()? {
        let start = text.line_start_offset(row);
        let end = text.line_end_offset(row);
        let Some(first) = editor.range_to_bounds(&(start..start)) else {
            continue;
        };
        let Some(last) = editor.range_to_bounds(&(end..end)) else {
            continue;
        };
        if point.y < first.top() || point.y >= last.bottom() {
            continue;
        }
        let mut offset = start;
        for ch in text.slice(start..end).chars() {
            let next = offset + ch.len_utf8();
            if identifier_char(ch)
                && editor
                    .range_to_bounds(&(offset..next))
                    .is_some_and(|bounds| {
                        // A range crossing a wrap is not a glyph rectangle. Fail
                        // closed when the public API cannot express that glyph.
                        bounds.size.height == line_height
                            && bounds.size.width > px(0.)
                            && bounds.contains(&point)
                    })
            {
                return Some(utf16_position(text, offset));
            }
            offset = next;
        }
    }
    None
}

impl Pokarak {
    pub(super) fn request_clicked_definition(
        &mut self,
        editor: &Entity<EditorState>,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // A rendered handler must not navigate a different file/worktree.
        if self.active_editor().as_ref() != Some(editor) {
            return false;
        }
        let position = editor.update(cx, |editor, cx| {
            let position = clicked_position(editor, point, window, cx)?;
            let offset = byte_offset(editor.text(), position)?;
            // Native modifier-click can leave the primary cursor elsewhere.
            // Align keyboard actions, references, implementation and history
            // with the symbol just clicked, removing secondary selections.
            editor.focus(window, cx);
            editor.set_selected_range(offset..offset, cx);
            Some(position)
        });
        if let Some(position) = position {
            self.request_language(language_tools::Tool::Symbol, Some(position), window, cx);
            return true;
        }
        false
    }

    pub(super) fn show_symbol_navigation(
        &mut self,
        result: SymbolNavigation,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let definition = result.unique_definition();
        self.status = result.status.clone();
        self.show_result_excerpts(
            result.title.clone(),
            true,
            result.excerpts(),
            definition.is_none(),
            window,
            cx,
        );
        self.result_anchor = anchor;
        if let Some(definition) = definition {
            self.open_location(definition, window, cx);
            if self.results_open {
                self.prepare_result_rows(cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        editor_position, inside_text_viewport, navigation_target, text_hit, utf16_position,
    };
    use gpui_kit::component::input::{Position, Rope, RopeExt};
    use gpui_kit::{Bounds, point, px, size};

    fn semantic(
        locations: Vec<pokarak_core::SourceLocation>,
    ) -> anyhow::Result<pokarak_navigation::NavigationResult> {
        Ok(pokarak_navigation::NavigationResult {
            locations,
            semantic: true,
            status: "fixture semantic response".into(),
        })
    }
    fn hit(line: u32) -> pokarak_core::SourceLocation {
        pokarak_core::SourceLocation {
            path: "/fixture/Java.java".into(),
            line,
            column: 4,
            preview: "greet()".into(),
        }
    }

    #[test]
    fn clicked_results_deduplicate_definitions_and_separate_usages_from_declarations() {
        use super::{ResultGroup, SymbolNavigation};
        let result = SymbolNavigation::from_results(
            semantic(vec![hit(1), hit(1)]),
            semantic(vec![hit(1), hit(3), hit(3), hit(5)]),
        );
        assert_eq!(result.unique_definition().unwrap().line, 1);
        assert_eq!(
            result.usages.iter().map(|h| h.line).collect::<Vec<_>>(),
            [3, 5]
        );
        assert!(result.title.contains("정의 1개 · 사용처 2개"));
        let rows = result.excerpts();
        assert_eq!(rows[0].group, Some(ResultGroup::Definition));
        assert_eq!(rows[1].group, Some(ResultGroup::Usages));
        assert_eq!(rows[2].location.line, 5);
        let result =
            SymbolNavigation::from_results(semantic(vec![hit(1), hit(7)]), semantic(vec![hit(3)]));
        assert!(result.unique_definition().is_none());
        assert_eq!(result.definitions.len(), 2);
    }

    #[test]
    fn a_failed_half_keeps_available_semantics_and_text_candidates_never_become_references() {
        use super::SymbolNavigation;
        let result = SymbolNavigation::from_results(
            semantic(vec![hit(1)]),
            Err(anyhow::anyhow!("references unavailable")),
        );
        assert!(result.unique_definition().is_some());
        assert!(result.usages.is_empty());
        assert!(result.title.contains("사용처 확인 실패"));
        assert!(result.status.contains("references unavailable"));
        let result = SymbolNavigation::from_results(
            Err(anyhow::anyhow!("definition unavailable")),
            semantic(vec![hit(3)]),
        );
        assert!(result.unique_definition().is_none());
        assert_eq!(result.usages.len(), 1);
        let result = SymbolNavigation::from_results(
            semantic(vec![]),
            Ok(pokarak_navigation::NavigationResult {
                locations: vec![hit(3)],
                semantic: false,
                status: "Text candidate".into(),
            }),
        );
        assert!(result.usages.is_empty());
        assert!(
            result
                .status
                .contains("텍스트 후보는 의미 분석 결과가 아닙니다")
        );
    }

    #[test]
    fn only_plain_native_command_left_press_invokes_symbol_navigation() {
        use gpui_kit::{MouseButton, MouseDownEvent};
        let mut event = MouseDownEvent {
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        };
        assert!(!super::is_symbol_click(&event));
        event.modifiers.platform = true;
        assert!(super::is_symbol_click(&event));
        event.modifiers.alt = true;
        assert!(!super::is_symbol_click(&event));
        event.modifiers.alt = false;
        event.modifiers.shift = true;
        assert!(!super::is_symbol_click(&event));
        event.modifiers.shift = false;
        event.click_count = 2;
        assert!(!super::is_symbol_click(&event));
        event.click_count = 1;
        event.button = MouseButton::Right;
        assert!(!super::is_symbol_click(&event));
    }

    #[test]
    fn punctuation_is_not_a_clicked_identifier() {
        let text = Rope::from("total.call($java_value); 한글");
        for (offset, ch) in text.to_string().char_indices() {
            let target = text_hit(&text, text.offset_to_offset_utf16(offset));
            assert_eq!(
                target.is_some(),
                ch.is_alphanumeric() || ch == '_' || ch == '$',
                "{ch}"
            );
        }
    }

    #[test]
    fn clicked_target_overrides_old_primary_cursor() {
        let source = "class OrderService {\n    fun order() = pricing.total()\n}";
        let text = Rope::from(source);
        let old_cursor = source.find("OrderService").unwrap();
        let clicked = source.find("total").unwrap() + 2;
        let hit = text_hit(&text, text.offset_to_offset_utf16(clicked)).unwrap();
        let target = navigation_target(&text, old_cursor, Some(hit)).unwrap();
        assert_eq!(target.position, Position::new(1, 28));
        assert_eq!(target.symbol_start, Position::new(1, 26));
        assert_ne!(target.position, utf16_position(&text, old_cursor));
    }

    #[test]
    fn utf16_navigation_and_editor_round_trip_after_astral_text() {
        let source = "// 한😀\r\nconst note = '😀'; pricing.total()\r\n";
        let text = Rope::from(source);
        let offset = source.find("total").unwrap() + 2;
        let native = text.offset_to_position(offset);
        let keyboard = navigation_target(&text, offset, None).unwrap();
        let clicked = text_hit(&text, text.offset_to_offset_utf16(offset)).unwrap();
        assert_eq!(keyboard.position, Position::new(1, 29));
        assert_eq!(clicked, keyboard.position);
        assert_eq!(keyboard.symbol_start, Position::new(1, 27));
        assert_eq!(native, Position::new(1, 28));
        assert_eq!(editor_position(&text, clicked), Some(native));
        // Back/forward persists the native position without UTF-16 conversion.
        assert_eq!(text.position_to_offset(&native), offset);
    }

    #[test]
    fn semantic_target_converts_utf16_before_placing_caret() {
        let source = "// header\nconst label = '😀'; export function total() {}";
        let text = Rope::from(source);
        let offset = source.find("total").unwrap();
        let lsp = Position::new(1, 36);
        let native = editor_position(&text, lsp).unwrap();
        assert_eq!(native, Position::new(1, 35));
        assert_eq!(text.position_to_offset(&native), offset);
        assert_eq!(
            navigation_target(&text, offset, None).unwrap().position,
            lsp
        );
    }

    #[test]
    fn invalid_utf16_and_nontext_hits_do_not_produce_a_target() {
        let text = Rope::from("😀 total\r\n");
        for position in [
            Position::new(0, 1),
            Position::new(0, 30),
            Position::new(9, 0),
        ] {
            assert!(editor_position(&text, position).is_none());
            assert!(navigation_target(&text, 3, Some(position)).is_none());
        }
        for index in [1, 2, 8, 9, 10, 100] {
            assert!(text_hit(&text, index).is_none(), "index {index}");
        }
        assert_eq!(
            editor_position(&text, Position::new(1, 0)),
            Some(Position::new(1, 0))
        );
    }

    #[test]
    fn viewport_excludes_fixed_gutter_and_outside_points() {
        let viewport = Bounds::new(point(px(100.), px(50.)), size(px(500.), px(300.)));
        let gutter = px(40.);
        for point in [
            point(px(99.), px(80.)),
            point(px(130.), px(80.)),
            point(px(150.), px(49.)),
            point(px(601.), px(80.)),
            point(px(150.), px(351.)),
        ] {
            assert!(!inside_text_viewport(viewport, gutter, point));
        }
        assert!(inside_text_viewport(
            viewport,
            gutter,
            point(px(150.), px(80.))
        ));
    }
}
