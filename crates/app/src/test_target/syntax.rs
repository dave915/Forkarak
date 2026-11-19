use anyhow::{Context, Result};
use pokarak_navigation::DocumentSymbol;
use tree_sitter::{Language, Node, Parser, Tree};

pub(super) fn parse(source: &str, language: Language) -> Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language)?;
    parser
        .parse(source, None)
        .context("테스트 선언을 읽을 수 없습니다")
}

pub(super) fn text<'s>(node: Node<'_>, source: &'s str) -> &'s str {
    &source[node.byte_range()]
}

pub(super) fn name<'s>(node: Node<'_>, source: &'s str) -> Option<&'s str> {
    Some(text(node.child_by_field_name("name")?, source).trim_matches('`'))
}

pub(super) fn contains(node: Node<'_>, line: u32) -> bool {
    let line = line as usize;
    node.start_position().row <= line && line <= node.end_position().row
}

/// JDT names include the parameter signature; the test runner takes just the method name.
pub(super) fn symbol_method(symbols: &[DocumentSymbol], line: u32) -> Option<&str> {
    symbols
        .iter()
        .filter(|s| {
            s.range.start.line <= line
                && (line < s.range.end.line
                    || (line == s.range.end.line
                        && (s.range.end.character > 0 || s.range.start.line == line)))
        })
        .find_map(|s| {
            symbol_method(&s.children, line).or_else(|| {
                matches!(s.kind, 6 | 12)
                    .then(|| {
                        s.name
                            .split('(')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .trim_matches('`')
                    })
                    .filter(|name| !name.is_empty())
            })
        })
}
