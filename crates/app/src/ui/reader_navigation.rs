//! Keep native editor coordinates separate from the UTF-16 navigation protocol.
use super::*;
use gpui_kit::component::input::{Rope, RopeExt};

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
    if text.offset_to_offset_utf16(offset) != utf16_index || text.char_at(offset)?.is_whitespace() {
        return None;
    }
    Some(utf16_position(text, offset))
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
            if !ch.is_whitespace()
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
    ) {
        // A rendered handler must not navigate a different file/worktree.
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        let position = editor.update(cx, |editor, cx| {
            let position = clicked_position(editor, point, window, cx)?;
            let offset = byte_offset(editor.text(), position)?;
            // Native modifier-click can leave the primary cursor elsewhere.
            // Align keyboard actions, references, implementation and history
            // with the symbol just clicked, removing secondary selections.
            editor.set_selected_range(offset..offset, cx);
            Some(position)
        });
        if let Some(position) = position {
            self.request_navigation(NavigationKind::Definition, Some(position), window, cx);
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
