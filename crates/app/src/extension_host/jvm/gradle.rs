//! Literal Gradle projectDir assignments in either settings DSL. Never evaluate a script.
use super::{confined, file};
use anyhow::{Context, Result};
use std::{fs, path::Path};

/// Keep quoted strings as single tokens and skip comments, so a pasted example
/// inside a settings file cannot rename a project.
fn tokens(mut source: &str) -> Vec<&str> {
    let mut result = Vec::new();
    while !source.is_empty() {
        source = source.trim_start();
        if source.is_empty() {
            break;
        }
        if source.starts_with("//") {
            source = source.split_once('\n').map_or("", |(_, tail)| tail);
            continue;
        }
        if let Some(rest) = source.strip_prefix("/*") {
            source = rest.split_once("*/").map_or("", |(_, tail)| tail);
            continue;
        }
        let first = source.as_bytes()[0];
        let width = if matches!(first, b'\'' | b'"') {
            let delimiter = if source.as_bytes().starts_with(&[first; 3]) {
                3
            } else {
                1
            };
            let mut index = delimiter;
            while index < source.len() {
                if source.as_bytes()[index..].starts_with(&source.as_bytes()[..delimiter]) {
                    index += delimiter;
                    break;
                }
                if source.as_bytes()[index] == b'\\' {
                    index += 1;
                }
                index = (index + 1).min(source.len());
            }
            index
        } else if first.is_ascii_alphanumeric() || first == b'_' {
            source
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count()
        } else {
            source.chars().next().unwrap().len_utf8()
        };
        let (token, rest) = source.split_at(width);
        result.push(token);
        source = rest;
    }
    result
}

fn literal(token: &str) -> Option<&str> {
    let quote = token.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let value = token.strip_prefix(quote)?.strip_suffix(quote)?;
    (!value.is_empty() && !value.contains(['\\', quote]) && (quote == '\'' || !value.contains('$')))
        .then_some(value)
}

fn mapping<'a>(tokens: &[&'a str]) -> Option<(&'a str, &'a str)> {
    let ["project", "(", id, ")", ".", "projectDir", "=", rhs @ ..] = tokens else {
        return None;
    };
    let path = match rhs.strip_prefix(&["new"]).unwrap_or(rhs) {
        ["file", "(", path, ")", ..]
        | ["rootProject", ".", "file", "(", path, ")", ..]
        | ["File", "(", "rootDir" | "settingsDir", ",", path, ")", ..] => literal(path)?,
        _ => return None,
    };
    Some((literal(id)?.trim_start_matches(':'), path))
}

pub(super) fn task(root: &Path, build: &Path, module: &Path) -> Result<String> {
    let mut directory = build.to_owned();
    let mut prefix = String::new();
    if build != module {
        for name in ["settings.gradle", "settings.gradle.kts"] {
            let Some(settings) = file(root, &build.join(name))? else {
                continue;
            };
            let source = fs::read_to_string(settings)?;
            if source.contains("projectDir") {
                let parts = tokens(&source);
                let mut depth: usize = 0;
                for (i, token) in parts.iter().enumerate() {
                    match *token {
                        "{" => depth += 1,
                        "}" => depth = depth.saturating_sub(1),
                        "project" if depth == 0 => {
                            if let Some((id, path)) = mapping(&parts[i..]) {
                                let mapped = build.join(path);
                                if mapped.starts_with(build)
                                    && !mapped
                                        .components()
                                        .any(|c| c == std::path::Component::ParentDir)
                                {
                                    let mapped = confined(root, &mapped)?;
                                    if module.starts_with(&mapped) && mapped.starts_with(&directory)
                                    {
                                        directory = mapped;
                                        prefix = format!(":{id}");
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            break;
        }
    }
    for part in module.strip_prefix(directory)? {
        prefix.push(':');
        prefix.push_str(
            part.to_str()
                .context("Gradle 프로젝트 경로가 UTF-8이 아닙니다")?,
        );
    }
    prefix.push_str(":test");
    Ok(prefix)
}
