use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};
use url::Url;

pub fn path_to_uri(path: &Path) -> Result<String> {
    Url::from_file_path(path)
        .map(String::from)
        .map_err(|_| anyhow::anyhow!("Cannot encode absolute source path {}", path.display()))
}

pub fn uri_to_path(uri: &str) -> Result<PathBuf> {
    let uri = Url::parse(uri).context("Invalid source URI")?;
    if uri.scheme() != "file" || uri.query().is_some() || uri.fragment().is_some() {
        bail!("Unsupported source URI {uri}: only local file URIs are readable");
    }
    if uri
        .host_str()
        .is_some_and(|host| host != "localhost" && !host.is_empty())
    {
        bail!("Unsupported remote source URI {uri}");
    }
    uri.to_file_path()
        .map_err(|_| anyhow::anyhow!("Cannot decode source URI {uri}"))
}

pub(crate) fn canonical_root(root: &Path) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("Cannot resolve worktree root {}", root.display()))?;
    if !root.is_dir() {
        bail!("Worktree root is not a directory: {}", root.display());
    }
    // A linked worktree's .git file is a boundary too. Never ascend to its main worktree.
    Ok(owner(&root).unwrap_or(root))
}

fn owner(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|p| p.join(".git").exists())
        .map(Path::to_path_buf)
}

fn common_dir(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    let git_dir = if marker.is_dir() {
        marker
    } else {
        let text = fs::read_to_string(marker).ok()?;
        let target = text.trim().strip_prefix("gitdir: ")?;
        root.join(target).canonicalize().ok()?
    };
    let common = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => git_dir.join(text.trim()),
        Err(_) => git_dir,
    };
    common.canonicalize().ok()
}

/// Dependency source trees are allowed; another checkout of this repository is not.
pub(crate) fn other_worktree(root: &Path, path: &Path) -> bool {
    let Some(target_root) = owner(path) else {
        return false;
    };
    target_root != root
        && common_dir(root).is_some_and(|common| Some(common) == common_dir(&target_root))
}

pub(crate) fn resolved_path(path: &Path) -> Result<PathBuf> {
    if let Ok(path) = path.canonicalize() {
        return Ok(path);
    }
    if !path.is_absolute() {
        bail!("Expected an absolute source path");
    }
    // Resolve existing ancestors as well, so symlinks cannot hide a sibling worktree.
    let mut tail = Vec::new();
    let mut ancestor = path;
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            break;
        };
        tail.push(name.to_owned());
        ancestor = ancestor.parent().context("Source path has no parent")?;
    }
    let mut normalized = ancestor
        .canonicalize()
        .unwrap_or_else(|_| ancestor.to_path_buf());
    for part in tail.into_iter().rev() {
        normalized.push(part);
    }
    let mut out = PathBuf::new();
    for part in normalized.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    Ok(out)
}

pub(crate) fn end_position(text: &str) -> (u32, u32) {
    let line = text.bytes().filter(|b| *b == b'\n').count() as u32;
    let tail = text
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');
    (line, tail.encode_utf16().count() as u32)
}

pub(crate) fn validate_position(text: &str, line: u32, column: u32) -> Result<()> {
    let row = text
        .split('\n')
        .nth(line as usize)
        .context("Source line is outside the file (lines are zero-based)")?
        .trim_end_matches('\r');
    let mut utf16 = 0;
    for c in row.chars() {
        if utf16 == column {
            return Ok(());
        }
        utf16 += c.len_utf16() as u32;
    }
    if utf16 != column {
        bail!("Column {column} is outside the line or splits a UTF-16 surrogate pair");
    }
    Ok(())
}
