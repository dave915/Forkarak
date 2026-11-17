//! Token ranges in the original UTF-8 source, shared by code excerpts and patch views.
use super::language;
use gpui_kit::{App, HighlightStyle, StyledText, component::ActiveTheme};
use std::{
    collections::HashMap,
    ops::Range,
    path::Path,
    sync::{Arc, LazyLock},
};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

// These are the same TextMate grammars used by GPUI Kit's syntect editor.
static GRAMMARS: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

#[derive(Debug)]
struct Token {
    bytes: Range<usize>,
    name: &'static str,
}

#[derive(Clone, Default, Debug)]
pub(super) struct CodeSyntax {
    starts: Arc<[usize]>,
    tokens: Arc<[Token]>,
}

fn grammar(path: &Path) -> Option<&'static SyntaxReference> {
    let label = language(path);
    if matches!(label, "text" | "plaintext") {
        return None;
    }
    let alias = match label {
        "csharp" => "cs",
        "make" => "Makefile",
        "bash" => "sh",
        _ => label,
    };
    GRAMMARS
        .find_syntax_by_token(label)
        .or_else(|| GRAMMARS.find_syntax_by_token(alias))
        .or_else(|| {
            path.extension()
                .and_then(|s| GRAMMARS.find_syntax_by_extension(s.to_str()?))
        })
}

// Resolve TextMate categories with the same theme keys as the file editor. A
// delimiter can inherit the enclosing comment/string instead of masking it.
fn theme_name(scope: Scope) -> Option<&'static str> {
    let scope = scope.build_string();
    let mut parts = scope.split('.');
    let top = parts.next()?;
    let group = parts.next().unwrap_or("");
    let detail = parts.next().unwrap_or("");
    Some(match (top, group, detail) {
        ("punctuation", "definition", "string" | "comment") => return None,
        ("comment", "line" | "block", "documentation") => "comment.doc",
        ("comment", _, _) => "comment",
        ("constant", "character", "escape") => "string.escape",
        ("string", "regexp", _) => "string.regex",
        ("string", _, _) => "string",
        ("constant", "numeric", _) => "number",
        ("constant", "language", "boolean") => "boolean",
        ("constant", _, _) | ("support", "constant", _) | ("entity", "name", "constant") => {
            "constant"
        }
        ("keyword", "operator", _) => "operator",
        ("keyword", "control", "import") => "preproc",
        ("keyword", _, _) | ("storage", "modifier" | "type", _) => "keyword",
        ("entity", "name", "function") | ("support" | "variable", "function", _) => "function",
        ("entity", "name", "type" | "class" | "enum" | "struct" | "trait" | "interface")
        | ("support", "type" | "class", _)
        | ("entity", "other", "inherited-class") => "type",
        ("entity", "name", "tag") => "tag",
        ("entity", "other", "attribute-name") => "attribute",
        ("entity", "name", "label") => "label",
        ("entity", "name", "section") | ("markup", "heading", _) => "title",
        ("variable", "language", _) => "variable.special",
        ("variable", "other", "member" | "property") | ("support", "variable", "property") => {
            "property"
        }
        ("variable", _, _) => "variable",
        ("markup", "bold", _) => "emphasis.strong",
        ("markup", "italic", _) => "emphasis",
        ("markup", "underline", "link") => "link_uri",
        ("markup", "raw", _) => "text.literal",
        ("markup", "list", _) => "punctuation.list_marker",
        ("punctuation", "section", _) | ("punctuation", "definition", "block") => {
            "punctuation.bracket"
        }
        ("punctuation", "separator" | "terminator", _) => "punctuation.delimiter",
        ("punctuation", _, _) => "punctuation",
        _ => return None,
    })
}

impl CodeSyntax {
    pub(super) fn new(path: &Path, text: &str) -> Self {
        let Some(grammar) = grammar(path) else {
            return Self::default();
        };
        let mut parser = ParseState::new(grammar);
        let mut stack = ScopeStack::new();
        let mut names = HashMap::new();
        let mut starts = Vec::new();
        let mut tokens = Vec::new();
        for line in text.split_inclusive('\n') {
            starts.push(tokens.len());
            let width = line.trim_end_matches(['\n', '\r']).len();
            if let Ok(ops) = parser.parse_line(line, &GRAMMARS) {
                let mut begin = 0;
                for (offset, op) in ops {
                    let end = offset.min(width);
                    Self::record(&stack, &mut names, begin..end, &mut tokens);
                    let _ = stack.apply(&op);
                    begin = end;
                }
                Self::record(&stack, &mut names, begin..width, &mut tokens);
            }
        }
        starts.push(tokens.len());
        Self {
            starts: starts.into(),
            tokens: tokens.into(),
        }
    }

    fn record(
        stack: &ScopeStack,
        names: &mut HashMap<Scope, Option<&'static str>>,
        bytes: Range<usize>,
        tokens: &mut Vec<Token>,
    ) {
        if bytes.is_empty() {
            return;
        }
        if let Some(name) = stack
            .as_slice()
            .iter()
            .rev()
            .find_map(|scope| *names.entry(*scope).or_insert_with(|| theme_name(*scope)))
        {
            tokens.push(Token { bytes, name });
        }
    }

    fn line(&self, index: usize) -> &[Token] {
        self.starts
            .get(index..)
            .and_then(|parts| parts.get(..2))
            .map_or(&[], |pair| &self.tokens[pair[0]..pair[1]])
    }

    pub(super) fn spans(&self, line: usize, cx: &App) -> Vec<(Range<usize>, HighlightStyle)> {
        let palette = &cx.theme().highlight_theme;
        self.line(line)
            .iter()
            .filter_map(|t| palette.style(t.name).map(|style| (t.bytes.clone(), style)))
            .collect()
    }

    pub(super) fn styled_line(
        &self,
        line: usize,
        text: &str,
        expand_tabs: bool,
        cx: &App,
    ) -> StyledText {
        let mut spans = self.spans(line, cx);
        spans.retain(|(r, _)| text.get(r.clone()).is_some());
        if expand_tabs && text.contains('\t') {
            let tabs: Vec<_> = text.match_indices('\t').map(|(at, _)| at).collect();
            let expanded = |pos| pos + 3 * tabs.partition_point(|at| *at < pos);
            for (r, _) in &mut spans {
                *r = expanded(r.start)..expanded(r.end);
            }
            StyledText::new(text.replace('\t', "    ")).with_highlights(spans)
        } else {
            StyledText::new(text.to_owned()).with_highlights(spans)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_source_preserves_multiline_scopes_and_utf8_offsets() {
        let source = "/* first\n\tcomment */ const n = 42;\r\nconst s = \"こんにちは\";\n";
        let syntax = CodeSyntax::new(Path::new("shipping.ts"), source);
        let lines: Vec<_> = source.lines().collect();
        let fragments = |line: usize, kind| {
            syntax
                .line(line)
                .iter()
                .filter(|token| token.name == kind)
                .map(|token| &lines[line][token.bytes.clone()])
                .collect::<String>()
        };
        assert!(fragments(1, "comment").contains("comment"));
        assert_eq!(fragments(1, "number"), "42");
        assert_eq!(fragments(2, "string"), "\"こんにちは\"");
        assert!(syntax.line(3).is_empty());
    }

    #[test]
    fn unknown_files_and_empty_sources_have_no_spans() {
        for (path, text) in [("notes.txt", "const value = 7;"), ("empty.ts", "")] {
            assert!(CodeSyntax::new(path.as_ref(), text).line(0).is_empty());
        }
    }
}
