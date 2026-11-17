//! Bounded .editorconfig reads, then existing-file detection, then preferences.
use gpui_kit::component::input::TabSize;
use std::{collections::HashMap, path::Path};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Indent {
    pub size: usize,
    pub tabs: bool,
}
impl Indent {
    pub(super) fn tab_size(self) -> TabSize {
        TabSize {
            tab_size: self.size,
            hard_tabs: self.tabs,
        }
    }
}

pub(super) fn resolve(
    root: &Path,
    path: &Path,
    text: &str,
    editorconfig: bool,
    detect: bool,
    fallback: Indent,
) -> Indent {
    let detected = if detect {
        detect_indent(text)
            .map(|rule| {
                if rule.tabs {
                    Indent {
                        size: fallback.size,
                        tabs: true,
                    }
                } else {
                    rule
                }
            })
            .unwrap_or(fallback)
    } else {
        fallback
    };
    let mut properties = HashMap::new();
    if editorconfig && let Some(relative) = relative_path(root, path) {
        let mut dirs = vec![root.to_path_buf()];
        if let Some(parent) = relative.parent() {
            let mut directory = root.to_path_buf();
            for component in parent.components().take(64) {
                directory.push(component);
                dirs.push(directory.clone());
            }
        }
        // Never follow .editorconfig or directory symlinks outside this worktree.
        for directory in dirs {
            let file = directory.join(".editorconfig");
            if !file.canonicalize().is_ok_and(|p| p.starts_with(root)) {
                continue;
            }
            if !std::fs::metadata(&file).is_ok_and(|m| m.len() <= 128 * 1024) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(file) else {
                continue;
            };
            let target = root
                .join(relative)
                .strip_prefix(&directory)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if source
                .lines()
                .take_while(|line| !line.trim_start().starts_with('['))
                .any(|line| {
                    line.split_once('=').is_some_and(|(key, value)| {
                        key.trim().eq_ignore_ascii_case("root")
                            && value.trim().eq_ignore_ascii_case("true")
                    })
                })
            {
                properties.clear();
            }
            parse(&source, &target, &mut properties);
        }
    }
    let tabs = match properties.get("indent_style").map(String::as_str) {
        Some("tab") => true,
        Some("space") => false,
        _ => detected.tabs,
    };
    let size = properties
        .get("indent_size")
        .filter(|s| s.as_str() != "tab")
        .or_else(|| properties.get("tab_width"))
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| (1..=16).contains(n))
        .unwrap_or(detected.size);
    Indent { size, tabs }
}

fn relative_path<'a>(root: &Path, path: &'a Path) -> Option<&'a Path> {
    let path = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    path.components()
        .all(|p| matches!(p, std::path::Component::Normal(_)))
        .then_some(path)
}

fn parse(source: &str, target: &str, properties: &mut HashMap<String, String>) {
    let mut matches = false;
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let pattern = &line[1..line.len() - 1];
            let name = if pattern.contains('/') {
                target
            } else {
                target.rsplit('/').next().unwrap_or(target)
            };
            matches = glob(pattern.trim_start_matches('/'), name);
        } else if matches
            && let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':'))
        {
            let (key, value) = (
                key.trim().to_ascii_lowercase(),
                value.trim().to_ascii_lowercase(),
            );
            if value == "unset" {
                properties.remove(&key);
            } else {
                properties.insert(key, value);
            }
        }
    }
}

// EditorConfig wildcards: *, **, ?, character sets, alternatives and numeric braces.
fn glob(pattern: &str, target: &str) -> bool {
    if let Some(start) = pattern.find('{')
        && let Some(end) = pattern[start..].find('}').map(|n| start + n)
    {
        let options = &pattern[start + 1..end];
        if let Some((lo, hi)) = options.split_once("..") {
            if let (Ok(lo), Ok(hi)) = (lo.parse::<i64>(), hi.parse::<i64>())
                && hi >= lo && hi.saturating_sub(lo) < 1024
            {
                return (lo..=hi).any(|n| {
                    glob(
                        &format!("{}{}{}", &pattern[..start], n, &pattern[end + 1..]),
                        target,
                    )
                });
            }
        } else {
            return options.split(',').any(|part| {
                glob(
                    &format!("{}{}{}", &pattern[..start], part, &pattern[end + 1..]),
                    target,
                )
            });
        }
    }
    fn run(
        p: &[char],
        s: &[char],
        memo: &mut HashMap<(usize, usize), bool>,
        i: usize,
        j: usize,
    ) -> bool {
        if let Some(result) = memo.get(&(i, j)) {
            return *result;
        }
        let value = match p.get(i) {
            None => j == s.len(),
            Some('*') => {
                let double = p.get(i + 1) == Some(&'*');
                let next = i + if double { 2 } else { 1 };
                run(p, s, memo, next, j)
                    || (double && p.get(next) == Some(&'/') && run(p, s, memo, next + 1, j))
                    || (j < s.len() && (double || s[j] != '/') && run(p, s, memo, i, j + 1))
            }
            Some('?') => j < s.len() && s[j] != '/' && run(p, s, memo, i + 1, j + 1),
            Some('[') => {
                if let Some(end) = p[i + 1..].iter().position(|c| *c == ']').map(|n| i + 1 + n) {
                    let negative = p.get(i + 1) == Some(&'!');
                    let start = i + 1 + usize::from(negative);
                    let mut hit = false;
                    let mut k = start;
                    while k < end {
                        if k + 2 < end && p[k + 1] == '-' {
                            hit |= j < s.len() && p[k] <= s[j] && s[j] <= p[k + 2];
                            k += 3;
                        } else {
                            hit |= s.get(j) == Some(&p[k]);
                            k += 1;
                        }
                    }
                    j < s.len() && s[j] != '/' && hit != negative && run(p, s, memo, end + 1, j + 1)
                } else {
                    false
                }
            }
            Some('\\') if i + 1 < p.len() => {
                s.get(j) == p.get(i + 1) && run(p, s, memo, i + 2, j + 1)
            }
            Some(c) => s.get(j) == Some(c) && run(p, s, memo, i + 1, j + 1),
        };
        memo.insert((i, j), value);
        value
    }
    if pattern.len() > 1024 || target.len() > 4096 {
        return false;
    }
    run(
        &pattern.chars().collect::<Vec<_>>(),
        &target.chars().collect::<Vec<_>>(),
        &mut HashMap::new(),
        0,
        0,
    )
}

fn detect_indent(text: &str) -> Option<Indent> {
    let mut tabs = 0;
    let mut spaces = 0;
    let mut size = 0;
    for line in text.lines().take(2000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
        } else {
            let n = line.bytes().take_while(|b| *b == b' ').count();
            if n > 0 {
                spaces += 1;
                size = gcd(size, n);
            }
        }
    }
    if tabs > spaces {
        Some(Indent {
            size: 4,
            tabs: true,
        })
    } else if spaces > 0 && (1..=16).contains(&size) {
        Some(Indent { size, tabs: false })
    } else {
        None
    }
}
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_rules_override_detected_indent_without_touching_sources() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(
            root.join(".editorconfig"),
            "root=true\n[*]\nindent_style=space\nindent_size=2\n[*.{rs,kt}]\nindent_size=4\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/.editorconfig"),
            "[*.rs]\nindent_style=tab\nindent_size=tab\ntab_width=8\n",
        )
        .unwrap();
        let fallback = Indent {
            size: 3,
            tabs: false,
        };
        assert_eq!(
            resolve(
                &root,
                Path::new("src/main.rs"),
                "  hello\n",
                true,
                true,
                fallback
            ),
            Indent {
                size: 8,
                tabs: true
            }
        );
        assert_eq!(
            resolve(&root, Path::new("x.kt"), "  hello\n", true, true, fallback),
            Indent {
                size: 4,
                tabs: false
            }
        );
        assert_eq!(
            resolve(
                &root,
                Path::new("x.py"),
                "    hello\n",
                false,
                true,
                fallback
            ),
            Indent {
                size: 4,
                tabs: false
            }
        );
        assert_eq!(
            resolve(
                &root,
                Path::new("x.py"),
                "    hello\n",
                false,
                false,
                fallback
            ),
            fallback
        );
    }
    #[test]
    fn globs_cover_editorconfig_patterns() {
        for (pattern, path) in [
            ("**/*.rs", "main.rs"),
            ("**/*.rs", "a/b/main.rs"),
            ("*.{js,ts}", "x.ts"),
            ("file{1..3}.txt", "file2.txt"),
            ("test[0-9].rs", "test8.rs"),
        ] {
            assert!(glob(pattern, path), "{pattern} {path}");
        }
        assert!(!glob("src/*.rs", "src/nested/a.rs"));
    }
}
