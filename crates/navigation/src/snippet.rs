//! Plain-text fallback for server snippets. No variables are executed; transforms
//! are rejected rather than inserting snippet syntax into the user's source.
use anyhow::{Result, bail};
use std::collections::HashMap;
struct Parser {
    chars: Vec<char>,
    at: usize,
    values: HashMap<String, String>,
}
impl Parser {
    fn text(&mut self, nested: bool, depth: usize) -> Result<String> {
        if depth > 64 {
            bail!("Snippet nesting exceeds limit");
        }
        let mut out = String::new();
        while self.at < self.chars.len() {
            let ch = self.chars[self.at];
            self.at += 1;
            match ch {
                '}' if nested => return Ok(out),
                '\\' => {
                    let escaped = *self
                        .chars
                        .get(self.at)
                        .ok_or_else(|| anyhow::anyhow!("Trailing snippet escape"))?;
                    self.at += 1;
                    out.push(escaped);
                }
                '$' => out.push_str(&self.placeholder(depth + 1)?),
                other => out.push(other),
            }
        }
        if nested {
            bail!("Unclosed snippet placeholder");
        }
        Ok(out)
    }
    fn placeholder(&mut self, depth: usize) -> Result<String> {
        let braced = self.chars.get(self.at) == Some(&'{');
        if braced {
            self.at += 1;
        }
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            self.at += 1;
        }
        if start == self.at {
            if braced {
                bail!("Empty snippet placeholder");
            }
            return Ok("$".into());
        }
        let name: String = self.chars[start..self.at].iter().collect();
        let mut value = self.values.get(&name).cloned().unwrap_or_default();
        if braced {
            match self.chars.get(self.at).copied() {
                Some('}') => self.at += 1,
                Some(':') => {
                    self.at += 1;
                    let default = self.text(true, depth)?;
                    if value.is_empty() {
                        value = default;
                    }
                }
                Some('|') => {
                    self.at += 1;
                    let mut first = String::new();
                    let mut choose = true;
                    let mut closed = false;
                    while self.at < self.chars.len() {
                        let ch = self.chars[self.at];
                        self.at += 1;
                        match ch {
                            '\\' => {
                                let escaped = *self
                                    .chars
                                    .get(self.at)
                                    .ok_or_else(|| anyhow::anyhow!("Truncated choice escape"))?;
                                self.at += 1;
                                if choose {
                                    first.push(escaped);
                                }
                            }
                            ',' => choose = false,
                            '|' if self.chars.get(self.at) == Some(&'}') => {
                                self.at += 1;
                                closed = true;
                                break;
                            }
                            ch => {
                                if choose {
                                    first.push(ch);
                                }
                            }
                        }
                    }
                    if !closed {
                        bail!("Unclosed snippet choice");
                    }
                    if value.is_empty() {
                        value = first;
                    }
                }
                Some('/') => bail!(
                    "Snippet transforms require a snippet editor and cannot be inserted as plain text"
                ),
                _ => bail!("Malformed snippet placeholder"),
            }
        }
        self.values.insert(name, value.clone());
        Ok(value)
    }
}
pub(crate) fn plain(source: &str) -> Result<String> {
    if source.len() > 1024 * 1024 {
        bail!("Snippet exceeds size limit");
    }
    Parser {
        chars: source.chars().collect(),
        at: 0,
        values: HashMap::new(),
    }
    .text(false, 0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expands_real_html_defaults_choices_mirrors_and_unicode() {
        assert_eq!(
            plain("<${1:div} class=\"${2|한,other|}\">$0</$1>").unwrap(),
            "<div class=\"한\"></div>"
        );
        assert_eq!(
            plain("${1:call(${2:value})} / $2 / \\$HOME").unwrap(),
            "call(value) / value / $HOME"
        );
        assert_eq!(plain("${TM_FILENAME:untitled}").unwrap(), "untitled");
    }
    #[test]
    fn refuses_transforms_and_malformed_placeholders() {
        for text in ["${1/a/b/}", "${1:unclosed", "${1|a,b}", "${}"] {
            assert!(plain(text).is_err(), "{text}");
        }
    }
}
