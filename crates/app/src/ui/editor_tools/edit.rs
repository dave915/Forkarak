//! Pure UTF-8 transformations. Each command is installed as one undoable edit.
use std::{ops::Range, path::Path};

#[derive(Clone, Copy)]
pub(super) enum LineCommand {
    Comment,
    Up,
    Down,
    DuplicateUp,
    DuplicateDown,
}

fn line_span(text: &str, selection: Range<usize>) -> Range<usize> {
    let start = text[..selection.start].rfind('\n').map_or(0, |i| i + 1);
    let last = if selection.end > selection.start && text[..selection.end].ends_with('\n') {
        selection.end - 1
    } else {
        selection.end
    };
    let end = text[last..].find('\n').map_or(text.len(), |i| last + i + 1);
    start..end
}

fn comment_token(path: &Path) -> Option<(&'static str, &'static str)> {
    Some(
        match path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "rs" | "java" | "kt" | "kts" | "js" | "jsx" | "ts" | "tsx" | "c" | "h" | "cpp"
            | "cc" | "cs" | "swift" | "go" | "scala" | "dart" => ("//", ""),
            "py" | "rb" | "sh" | "zsh" | "bash" | "yaml" | "yml" | "toml" | "properties"
            | "conf" | "r" | "pl" => ("#", ""),
            "sql" | "lua" | "hs" => ("--", ""),
            "css" | "scss" | "less" => ("/*", " */"),
            "html" | "htm" | "xml" | "svg" | "vue" | "svelte" => ("<!--", " -->"),
            _ if path
                .file_name()
                .is_some_and(|n| n == "Dockerfile" || n == "Makefile" || n == ".gitignore") =>
            {
                ("#", "")
            }
            _ => return None,
        },
    )
}

pub(super) fn transform(
    text: &str,
    selection: Range<usize>,
    path: &Path,
    command: LineCommand,
) -> Result<Option<(Range<usize>, String, Range<usize>)>, String> {
    let span = line_span(text, selection);
    let block = &text[span.clone()];
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let result = match command {
        LineCommand::Comment => {
            let (prefix, suffix) =
                comment_token(path).ok_or("이 파일 형식에는 주석 문법이 없습니다.")?;
            let uncomment = block.lines().filter(|s| !s.trim().is_empty()).all(|s| {
                s.trim_start().starts_with(prefix)
                    && (suffix.is_empty() || s.trim_end().ends_with(suffix.trim()))
            });
            let mut output = String::new();
            for line in block.split_inclusive('\n') {
                let content = line.trim_end_matches(['\r', '\n']);
                let ending = &line[content.len()..];
                let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
                let body = &content[indent..];
                output.push_str(&content[..indent]);
                if body.is_empty() {
                    output.push_str(ending);
                    continue;
                }
                if uncomment {
                    let body = body.strip_prefix(prefix).unwrap_or(body);
                    let body = body.strip_prefix(' ').unwrap_or(body);
                    let body = if suffix.is_empty() {
                        body
                    } else {
                        body.trim_end()
                            .strip_suffix(suffix.trim())
                            .unwrap_or(body)
                            .strip_suffix(' ')
                            .unwrap_or_else(|| {
                                body.trim_end().strip_suffix(suffix.trim()).unwrap_or(body)
                            })
                    };
                    output.push_str(body);
                } else {
                    output.push_str(prefix);
                    output.push(' ');
                    output.push_str(body);
                    output.push_str(suffix);
                }
                output.push_str(ending);
            }
            let selected = span.start..span.start + output.len();
            (span, output, selected)
        }
        LineCommand::DuplicateUp | LineCommand::DuplicateDown => {
            let unit = if block.ends_with('\n') {
                block.to_string()
            } else {
                format!("{block}{newline}")
            };
            let output = format!("{unit}{block}");
            let offset = if matches!(command, LineCommand::DuplicateDown) {
                unit.len()
            } else {
                0
            };
            let selected = span.start + offset..span.start + offset + block.len();
            (span, output, selected)
        }
        LineCommand::Up => {
            if span.start == 0 {
                return Ok(None);
            }
            let previous = text[..span.start - 1].rfind('\n').map_or(0, |i| i + 1);
            let before = &text[previous..span.start];
            let (block, before) = exchange_endings(block, before, newline);
            let selected = previous..previous + block.len();
            (previous..span.end, format!("{block}{before}"), selected)
        }
        LineCommand::Down => {
            if span.end == text.len() {
                return Ok(None);
            }
            let next = text[span.end..]
                .find('\n')
                .map_or(text.len(), |i| span.end + i + 1);
            let after = &text[span.end..next];
            let (after, block) = exchange_endings(after, block, newline);
            let selected = span.start + after.len()..span.start + after.len() + block.len();
            (span.start..next, format!("{after}{block}"), selected)
        }
    };
    Ok(Some(result))
}

// A final unterminated line stays final after swapping: never join two source lines.
fn exchange_endings(first: &str, second: &str, newline: &str) -> (String, String) {
    if !first.ends_with('\n') && second.ends_with('\n') {
        (
            format!("{first}{newline}"),
            second.strip_suffix(newline).unwrap_or(second).to_string(),
        )
    } else {
        (first.to_string(), second.to_string())
    }
}

pub(super) fn word_or_next(text: &str, selection: Range<usize>) -> Option<Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if selection.is_empty() {
        let start = text[..selection.start]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(selection.start, |(i, _)| i);
        let end = text[selection.end..]
            .char_indices()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(selection.end, |(i, c)| selection.end + i + c.len_utf8());
        return (start != end).then_some(start..end);
    }
    let needle = &text[selection.clone()];
    text[selection.end..]
        .find(needle)
        .map(|i| selection.end + i..selection.end + i + needle.len())
        .or_else(|| {
            text[..selection.start]
                .find(needle)
                .map(|i| i..i + needle.len())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn apply(text: &str, range: Range<usize>, path: &str, cmd: LineCommand) -> String {
        let (range, replacement, _) = transform(text, range, Path::new(path), cmd)
            .unwrap()
            .unwrap();
        format!(
            "{}{}{}",
            &text[..range.start],
            replacement,
            &text[range.end..]
        )
    }
    #[test]
    fn comment_roundtrip_crlf_unicode_and_line_boundary() {
        let source = "  안녕();\r\n  next();\r\nlast();";
        let end = source.find("last").unwrap();
        let commented = apply(source, 0..end, "x.rs", LineCommand::Comment);
        assert_eq!(commented, "  // 안녕();\r\n  // next();\r\nlast();");
        assert_eq!(
            apply(
                &commented,
                0..commented.find("last").unwrap(),
                "x.rs",
                LineCommand::Comment
            ),
            source
        );
        assert_eq!(
            apply("<div/>\n", 0..0, "x.html", LineCommand::Comment),
            "<!-- <div/> -->\n"
        );
        assert_eq!(
            apply("<!-- <div/> -->\n", 0..0, "x.html", LineCommand::Comment),
            "<div/>\n"
        );
    }
    #[test]
    fn move_and_duplicate_keep_unterminated_lines() {
        assert_eq!(apply("a\nb", 2..2, "x.rs", LineCommand::Up), "b\na");
        assert_eq!(apply("a\nb", 0..0, "x.rs", LineCommand::Down), "b\na");
        assert_eq!(
            apply("a\r\nb", 3..3, "x.rs", LineCommand::DuplicateDown),
            "a\r\nb\r\nb"
        );
        assert_eq!(
            apply("one\ntwo\nthree\nfour\n", 4..14, "x.rs", LineCommand::Up),
            "two\nthree\none\nfour\n"
        );
    }
    #[test]
    fn repeated_word_selection_uses_utf8_byte_ranges() {
        let text = "한글 한글 한글";
        assert_eq!(word_or_next(text, 3..3), Some(0..6));
        assert_eq!(word_or_next(text, 0..6), Some(7..13));
        assert_eq!(word_or_next(text, 14..20), Some(0..6));
    }
}
