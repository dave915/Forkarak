//! Describe a test command for the current worktree. Detection only inspects files.
mod cargo;
mod java;
mod syntax;
#[cfg(test)]
mod tests;

use crate::{extension_host::jvm, task_runtime::TaskProfile};
use anyhow::{Context, Result, ensure};
use pokarak_navigation::DocumentSymbol;
use serde_json::Value;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug)]
pub(crate) struct TestTarget {
    pub profile: TaskProfile,
    pub label: String,
    pub report_dirs: Vec<PathBuf>,
}

impl TestTarget {
    fn new(root: &Path, cwd: &Path, label: String, command: Vec<String>) -> Result<Self> {
        let profile = TaskProfile {
            name: format!("테스트 · {label}"),
            command,
            cwd: cwd.strip_prefix(root)?.to_owned(),
            env: Default::default(),
        };
        Ok(Self {
            profile,
            label,
            report_dirs: Vec::new(),
        })
    }
}

pub(crate) fn detect(
    root: &Path,
    path: &Path,
    source: &str,
    cursor_line: u32,
    file_scope: bool,
    symbols: &[DocumentSymbol],
    settings: &Value,
) -> Result<Option<TestTarget>> {
    ensure!(
        path.components().all(|c| matches!(c, Component::Normal(_))),
        "워크트리 상대 경로가 필요합니다"
    );
    let root = root.canonicalize()?;
    let Some(path) = jvm::file(&root, &root.join(path))? else {
        return Ok(None);
    };
    // Trust for the enclosing worktree does not cover a nested Git checkout.
    if path
        .ancestors()
        .skip(1)
        .take_while(|p| *p != root)
        .any(|dir| dir.join(".git").symlink_metadata().is_ok())
    {
        return Ok(None);
    }
    let cursor = (!file_scope).then_some(cursor_line);
    match path.extension().and_then(|e| e.to_str()) {
        Some("java" | "kt") => java::detect(&root, &path, source, cursor, symbols, settings),
        Some("rs") => cargo::detect(&root, &path, source, cursor, symbols),
        Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx") => npm(&root, &path),
        _ => Ok(None),
    }
}

fn closest_manifest(root: &Path, path: &Path, name: &str) -> Result<Option<(PathBuf, PathBuf)>> {
    for directory in path.ancestors().skip(1).take_while(|p| p.starts_with(root)) {
        if let Some(manifest) = jvm::file(root, &directory.join(name))? {
            return Ok(Some((directory.to_owned(), manifest)));
        }
    }
    Ok(None)
}

fn npm(root: &Path, path: &Path) -> Result<Option<TestTarget>> {
    let Some((package, manifest)) = closest_manifest(root, path, "package.json")? else {
        return Ok(None);
    };
    let data: Value =
        serde_json::from_slice(&fs::read(manifest)?).context("package.json을 읽을 수 없습니다")?;
    if data["scripts"]["test"]
        .as_str()
        .is_none_or(|script| script.trim().is_empty())
    {
        return Ok(None);
    }
    let relative = path
        .strip_prefix(&package)?
        .to_str()
        .context("테스트 경로가 UTF-8이 아닙니다")?;
    let argument = if relative.starts_with('-') {
        format!("./{relative}")
    } else {
        relative.to_owned()
    };
    let label = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    Ok(Some(TestTarget::new(
        root,
        &package,
        label,
        vec!["npm".into(), "test".into(), "--".into(), argument],
    )?))
}
