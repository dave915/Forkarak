use super::{
    TestTarget, closest_manifest,
    syntax::{contains, name, parse, symbol_method, text},
};
use crate::extension_host::jvm;
use anyhow::{Context, Result, ensure};
use pokarak_navigation::DocumentSymbol;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use tree_sitter::Node;

struct Target {
    flag: &'static str,
    name: String,
    entry: PathBuf,
}

fn targets(root: &Path, dir: &Path, manifest: &toml::Value, package: &str) -> Result<Vec<Target>> {
    let mut targets = Vec::new();
    for (key, flag, folder, auto) in [
        ("lib", "--lib", "src", "autolib"),
        ("bin", "--bin", "src/bin", "autobins"),
        ("test", "--test", "tests", "autotests"),
    ] {
        let folder = dir.join(folder);
        let mut candidates = Vec::new();
        if key == "lib" || key == "bin" {
            candidates.push((
                package.to_owned(),
                dir.join(if key == "lib" {
                    "src/lib.rs"
                } else {
                    "src/main.rs"
                }),
            ));
        }
        if key != "lib" && jvm::confined(root, &folder)?.is_dir() {
            for entry in fs::read_dir(&folder)? {
                let path = entry?.path();
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if path.extension().is_some_and(|ext| ext == "rs") {
                    candidates.push((stem.to_owned(), path));
                } else if path.is_dir() {
                    candidates.push((
                        path.file_name().unwrap().to_string_lossy().into_owned(),
                        path.join("main.rs"),
                    ));
                }
            }
        }
        let entries: Vec<_> = match manifest.get(key) {
            Some(value) if key == "lib" => vec![value],
            Some(value) => value.as_array().into_iter().flatten().collect(),
            None => Vec::new(),
        };
        let mut explicit_names = HashSet::new();
        let mut explicit_paths = HashSet::new();
        for entry in entries {
            let target_name = entry
                .get("name")
                .and_then(|s| s.as_str())
                .unwrap_or(package);
            explicit_names.insert(target_name);
            let path = entry
                .get("path")
                .and_then(|s| s.as_str())
                .map(|s| dir.join(s))
                .or_else(|| {
                    candidates
                        .iter()
                        .find(|(name, path)| {
                            (key == "lib" || name == target_name) && path.is_file()
                        })
                        .map(|(_, p)| p.clone())
                });
            if let Some(path) = path {
                explicit_paths.insert(path.clone());
                if entry.get("harness").and_then(|v| v.as_bool()) != Some(false)
                    && let Some(entry) = jvm::file(root, &path)?
                {
                    targets.push(Target {
                        flag,
                        name: target_name.to_owned(),
                        entry,
                    });
                }
            }
        }
        if manifest["package"].get(auto).and_then(|v| v.as_bool()) != Some(false) {
            candidates.sort();
            for (name, path) in candidates {
                if !(explicit_names.contains(name.as_str())
                    || explicit_paths.contains(&path)
                    || (key == "lib" && manifest.get("lib").is_some()))
                    && let Some(entry) = jvm::file(root, &path)?
                {
                    targets.push(Target { flag, name, entry });
                }
            }
        }
    }
    Ok(targets)
}

fn attrs(item: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(item.prev_named_sibling(), |n| n.prev_named_sibling())
        .take_while(|n| {
            matches!(
                n.kind(),
                "attribute_item" | "line_comment" | "block_comment"
            )
        })
        .filter_map(|n| {
            (n.kind() == "attribute_item")
                .then(|| n.named_child(0))
                .flatten()
        })
}

fn attr_name<'s>(attr: Node<'_>, source: &'s str) -> Option<&'s str> {
    Some(text(attr.named_child(0)?, source))
}

fn mod_name<'s>(item: Node<'_>, source: &'s str) -> Option<&'s str> {
    Some(name(item, source)?.trim_start_matches("r#"))
}

fn joined(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        suffix.to_owned()
    } else {
        format!("{prefix}::{suffix}")
    }
}

// Resolve the source's actual module path, also distinguishing src/main.rs from
// src/lib.rs when a package has both. Inspect only module branches on its path.
struct ModuleSearch<'a> {
    root: &'a Path,
    selected: &'a Path,
    visited: HashSet<PathBuf>,
}

impl ModuleSearch<'_> {
    fn find(&mut self, file: &Path, directory: &Path, prefix: &str) -> Result<Option<String>> {
        if file == self.selected {
            return Ok(Some(prefix.to_owned()));
        }
        if !self.visited.insert(file.to_owned()) {
            return Ok(None);
        }
        let source = fs::read_to_string(file)?;
        let tree = parse(&source, tree_sitter_rust::LANGUAGE.into())?;
        self.declarations(tree.root_node(), &source, file, directory, prefix)
    }

    fn declarations(
        &mut self,
        node: Node<'_>,
        source: &str,
        file: &Path,
        directory: &Path,
        prefix: &str,
    ) -> Result<Option<String>> {
        for item in node
            .named_children(&mut node.walk())
            .filter(|n| n.kind() == "mod_item")
        {
            let Some(name) = mod_name(item, source) else {
                continue;
            };
            let prefix = joined(prefix, name);
            let child_dir = directory.join(name);
            let found = if let Some(body) = item.child_by_field_name("body") {
                self.declarations(body, source, file, &child_dir, &prefix)?
            } else {
                let override_path = attrs(item)
                    .find(|a| attr_name(*a, source) == Some("path"))
                    .and_then(|a| a.child_by_field_name("value"))
                    .and_then(|n| serde_json::from_str::<String>(text(n, source)).ok());
                let mut files = vec![child_dir.with_extension("rs"), child_dir.join("mod.rs")];
                if let Some(override_path) = override_path {
                    let base = if node.kind() == "source_file" {
                        file.parent().unwrap()
                    } else {
                        directory
                    };
                    files = vec![base.join(override_path)];
                }
                let mut result = None;
                for candidate in files {
                    let Some(entry) = candidate
                        .canonicalize()
                        .ok()
                        .filter(|p| p.starts_with(self.root) && p.is_file())
                    else {
                        continue;
                    };
                    let base = if entry.file_name().is_some_and(|n| n == "mod.rs") {
                        entry.parent().unwrap().to_owned()
                    } else {
                        entry.with_extension("")
                    };
                    if (entry == self.selected || self.selected.starts_with(&base))
                        && let Some(found) = self.find(&entry, &base, &prefix)?
                    {
                        result = Some(found);
                        break;
                    }
                }
                result
            };
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }
}

fn functions(
    node: Node<'_>,
    source: &str,
    prefix: &str,
    cursor: Option<u32>,
    symbols: &[DocumentSymbol],
    names: &mut Vec<(String, bool)>,
) {
    for item in node.named_children(&mut node.walk()) {
        let Some(name) = mod_name(item, source) else {
            continue;
        };
        let full = joined(prefix, name);
        match item.kind() {
            "mod_item" => {
                if let Some(body) = item.child_by_field_name("body") {
                    functions(body, source, &full, cursor, symbols, names);
                }
            }
            "function_item" => {
                let mut test_line = None;
                for attr in attrs(item) {
                    if attr_name(attr, source)
                        .is_some_and(|path| path.rsplit("::").next() == Some("test"))
                    {
                        test_line = Some(attr.start_position().row as u32);
                    }
                }
                if let Some(start) = test_line {
                    let current = cursor.is_some_and(|line| {
                        // rust-analyzer's range can exclude the attribute. In either case the
                        // selected symbol must refer to an annotated Rust test in this file.
                        let method = symbol_method(symbols, line);
                        (contains(item, line)
                            && method.is_none_or(|symbol| symbol.trim_start_matches("r#") == name))
                            || (start..=item.start_position().row as u32).contains(&line)
                    });
                    names.push((full, current));
                }
            }
            _ => {}
        }
    }
}

pub(super) fn detect(
    root: &Path,
    path: &Path,
    source: &str,
    cursor: Option<u32>,
    symbols: &[DocumentSymbol],
) -> Result<Option<TestTarget>> {
    let Some((directory, manifest)) = closest_manifest(root, path, "Cargo.toml")? else {
        return Ok(None);
    };
    let manifest: toml::Value =
        toml::from_str(&fs::read_to_string(manifest)?).context("Cargo.toml을 읽을 수 없습니다")?;
    let Some(package) = manifest
        .get("package")
        .and_then(|t| t.get("name"))
        .and_then(|v| v.as_str())
    else {
        return Ok(None);
    };
    if let Some(workspace) = manifest["package"]
        .get("workspace")
        .and_then(|v| v.as_str())
    {
        let workspace = directory.join(workspace).canonicalize()?;
        ensure!(
            workspace.starts_with(root),
            "Cargo 작업 공간이 현재 워크트리 밖에 있습니다"
        );
        // The workspace manifest can itself be a symlink.
        jvm::confined(root, &workspace.join("Cargo.toml"))?;
    }
    let mut candidates = targets(root, &directory, &manifest, package)?;
    candidates.sort_by_key(|t| t.entry != path);
    for target in candidates {
        let parent = target.entry.parent().unwrap();
        if !path.starts_with(parent) {
            continue;
        }
        let mut search = ModuleSearch {
            root,
            selected: path,
            visited: HashSet::new(),
        };
        let Some(prefix) = search.find(&target.entry, parent, "")? else {
            continue;
        };
        let tree = parse(source, tree_sitter_rust::LANGUAGE.into())?;
        let mut names = Vec::new();
        functions(
            tree.root_node(),
            source,
            &prefix,
            cursor,
            symbols,
            &mut names,
        );
        let current = names.iter().position(|(_, current)| *current);
        let whole_integration = target.flag == "--test" && prefix.is_empty() && current.is_none();
        if names.is_empty() && !whole_integration {
            return Ok(None);
        }
        let mut command = vec![
            "cargo".into(),
            "test".into(),
            "-p".into(),
            package.to_owned(),
            target.flag.into(),
        ];
        if target.flag != "--lib" {
            command.push(target.name)
        }
        let label = current
            .map(|i| names[i].0.clone())
            .unwrap_or_else(|| path.file_name().unwrap().to_string_lossy().into_owned());
        if !whole_integration {
            let mut names = if let Some(i) = current {
                vec![names.swap_remove(i).0]
            } else {
                names.into_iter().map(|(name, _)| name).collect()
            }
            .into_iter();
            if let Some(first) = names.next() {
                command.extend([first, "--".into(), "--exact".into()]);
                command.extend(names);
            }
        }
        return Ok(Some(TestTarget::new(root, &directory, label, command)?));
    }
    Ok(None)
}
