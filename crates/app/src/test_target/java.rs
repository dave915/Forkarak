use super::{
    TestTarget,
    syntax::{contains, name, parse, symbol_method, text},
};
use crate::extension_host::jvm::{self, JvmBuild, Project};
use anyhow::{Context, Result};
use pokarak_navigation::DocumentSymbol;
use serde_json::Value;
use std::path::Path;
use tree_sitter::Node;

struct Class<'a> {
    node: Node<'a>,
    binary_name: String,
    methods: Vec<Node<'a>>,
}

// Visit class members, stopping before method bodies (local and anonymous classes
// there cannot be selected as top-level test classes).
fn classes<'a>(node: Node<'a>, source: &str, owner: Option<usize>, result: &mut Vec<Class<'a>>) {
    let owner = match node.kind() {
        "class_declaration"
        | "interface_declaration"
        | "record_declaration"
        | "object_declaration" => {
            let Some(name) = name(node, source) else {
                return;
            };
            let binary_name = owner.map_or_else(
                || name.to_owned(),
                |i| format!("{}${name}", result[i].binary_name),
            );
            result.push(Class {
                node,
                binary_name,
                methods: Vec::new(),
            });
            Some(result.len() - 1)
        }
        "function_declaration" | "method_declaration" => {
            if let Some(i) = owner {
                result[i].methods.push(node);
            }
            return;
        }
        "program"
        | "source_file"
        | "class_body"
        | "interface_body"
        | "declaration"
        | "class_member_declaration" => owner,
        _ => return,
    };
    for child in node.named_children(&mut node.walk()) {
        classes(child, source, owner, result);
    }
}

fn annotated(method: Node<'_>, source: &str) -> bool {
    method
        .named_children(&mut method.walk())
        .filter(|n| n.kind() == "modifiers")
        .any(|modifiers| {
            modifiers.named_children(&mut modifiers.walk()).any(|attr| {
                if !matches!(attr.kind(), "annotation" | "marker_annotation") {
                    return false;
                }
                let name = attr
                    .child_by_field_name("name")
                    .map(|n| text(n, source))
                    .unwrap_or_else(|| {
                        text(attr, source)
                            .trim_start_matches('@')
                            .split('(')
                            .next()
                            .unwrap_or("")
                    });
                matches!(
                    name.trim().rsplit('.').next(),
                    Some(
                        "Test"
                            | "ParameterizedTest"
                            | "RepeatedTest"
                            | "TestFactory"
                            | "TestTemplate"
                    )
                )
            })
        })
}

fn package_identifiers(node: Node<'_>, source: &str, parts: &mut Vec<String>) {
    if node.kind() == "identifier" {
        parts.push(text(node, source).trim_matches('`').to_owned());
    } else if matches!(
        node.kind(),
        "package_declaration" | "package_header" | "scoped_identifier" | "qualified_identifier"
    ) {
        for child in node.named_children(&mut node.walk()) {
            package_identifiers(child, source, parts);
        }
    }
}

fn class_name_is_test(name: &str) -> bool {
    name.starts_with("Test")
        || name.ends_with("Test")
        || name.ends_with("Tests")
        || name.ends_with("TestCase")
        || name.ends_with("IT")
}

pub(super) fn detect(
    root: &Path,
    path: &Path,
    source: &str,
    cursor: Option<u32>,
    symbols: &[DocumentSymbol],
    settings: &Value,
) -> Result<Option<TestTarget>> {
    let Some(project) = Project::closest(root, path)? else {
        return Ok(None);
    };
    let kotlin = path.extension().is_some_and(|s| s == "kt");
    let grammar = if kotlin {
        tree_sitter_kotlin_ng::LANGUAGE.into()
    } else {
        tree_sitter_java::LANGUAGE.into()
    };
    let tree = parse(source, grammar)?;
    let top = tree.root_node();
    let mut found = Vec::new();
    classes(top, source, None, &mut found);
    let stem = path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    let class = cursor
        .and_then(|line| found.iter().rev().find(|c| contains(c.node, line)))
        .or_else(|| found.iter().find(|c| c.binary_name == stem))
        .or_else(|| found.iter().find(|c| !c.binary_name.contains('$')));
    let Some(class) = class else { return Ok(None) };
    let method = cursor
        .filter(|line| contains(class.node, *line))
        .and_then(|line| {
            symbol_method(symbols, line).or_else(|| {
                class
                    .methods
                    .iter()
                    .find(|declaration| {
                        contains(**declaration, line) && annotated(**declaration, source)
                    })
                    .and_then(|declaration| name(*declaration, source))
            })
        });
    let has_tests = class.methods.iter().any(|m| annotated(*m, source));
    let in_test_sources = ["src/test", "src/integrationTest"]
        .iter()
        .any(|dir| path.starts_with(project.module.join(dir)));
    if !has_tests && !class_name_is_test(&class.binary_name) && !in_test_sources {
        return Ok(None);
    }

    let mut parts = Vec::new();
    if let Some(package) = top
        .named_children(&mut top.walk())
        .find(|n| matches!(n.kind(), "package_declaration" | "package_header"))
    {
        package_identifiers(package, source, &mut parts);
    }
    parts.push(class.binary_name.clone());
    let fqn = parts.join(".");
    let mut command = vec![project.program(root, settings)?];
    let module = project.module.strip_prefix(&project.build_root)?;
    let report = match project.kind {
        JvmBuild::Gradle => {
            let task = project.gradle_test_task(root)?;
            let selector = method.map_or_else(|| fqn.clone(), |m| format!("{fqn}.{m}"));
            command.extend([task, "--tests".to_owned(), selector]);
            "build/test-results/test"
        }
        JvmBuild::Maven => {
            if !module.as_os_str().is_empty() {
                command.extend([
                    "-pl".to_owned(),
                    module
                        .to_str()
                        .context("Maven 모듈 경로가 UTF-8이 아닙니다")?
                        .to_owned(),
                ]);
            }
            let selector = method.map_or_else(|| fqn.clone(), |m| format!("{fqn}#{m}"));
            command.extend([format!("-Dtest={selector}"), "test".to_owned()]);
            "target/surefire-reports"
        }
    };
    let label = method.map_or_else(
        || class.binary_name.clone(),
        |m| format!("{}.{m}", class.binary_name),
    );
    let mut target = TestTarget::new(root, &project.build_root, label, command)?;
    target.profile.env = jvm::environment(root, settings)?;
    target
        .report_dirs
        .push(jvm::confined(root, &project.module.join(report))?);
    Ok(Some(target))
}
