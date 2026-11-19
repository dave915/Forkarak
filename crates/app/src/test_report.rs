//! JUnit output for the modules selected by one completed test run.
use crate::task_runtime::{StackLocation, resolve_stack_location_from};
use anyhow::Result;
use roxmltree::{Document, Node};
use rustix::{
    fs::{AtFlags, Dir, FileType, Mode, OFlags, open, openat, statat},
    io::Errno,
};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::File,
    io::Read,
    os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct TestSummary {
    pub total: usize,
    pub skipped: usize,
    pub failures: Vec<TestFailure>,
}

#[derive(Clone, Debug)]
pub(crate) struct TestFailure {
    pub name: String,
    pub message: String,
    pub location: Option<StackLocation>,
}

pub(crate) fn read_junit_reports(
    root: &Path,
    dirs: &[PathBuf],
    started: SystemTime,
) -> Result<Option<TestSummary>> {
    if dirs.is_empty() {
        return Ok(None);
    }
    let root = root.canonicalize()?;
    let root_fd = open(&root, dir_flags(), Mode::empty())?;
    let mut summary = None;
    for dir in dirs.iter().collect::<BTreeSet<_>>() {
        let Ok(relative) = dir.strip_prefix(&root) else {
            continue;
        };
        let (module, name_prefix) = if relative.ends_with("build/test-results/test") {
            (dir.ancestors().nth(3).unwrap(), "")
        } else if relative.ends_with("target/surefire-reports") {
            (dir.ancestors().nth(2).unwrap(), "TEST-")
        } else {
            continue;
        };
        let Some(directory) = open_report_directory(&root_fd, relative) else {
            continue;
        };
        let mut names = BTreeSet::new();
        for entry in Dir::read_from(&directory)?.flatten() {
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if matches!(entry.file_type(), FileType::RegularFile | FileType::Unknown)
                && name.as_bytes().starts_with(name_prefix.as_bytes())
                && Path::new(name).extension().is_some_and(|ext| ext == "xml")
            {
                names.insert(name.to_owned());
            }
        }
        for name in names {
            if let Some(xml) = read_fresh_xml(&directory, &name, started)
                && let Ok(doc) = Document::parse(&xml)
                && matches!(
                    doc.root_element().tag_name().name(),
                    "testsuite" | "testsuites"
                )
            {
                collect_cases(
                    doc.root_element(),
                    module,
                    summary.get_or_insert_with(TestSummary::default),
                );
            }
        }
    }
    Ok(summary)
}

fn dir_flags() -> OFlags {
    #[cfg(target_os = "macos")]
    let nofollow = OFlags::NOFOLLOW_ANY;
    #[cfg(not(target_os = "macos"))]
    let nofollow = OFlags::NOFOLLOW;
    OFlags::RDONLY | OFlags::DIRECTORY | nofollow | OFlags::CLOEXEC
}

fn open_report_directory(root: &OwnedFd, relative: &Path) -> Option<OwnedFd> {
    let mut dir = root.try_clone().ok()?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        if name.eq_ignore_ascii_case(".git") {
            return None;
        }
        dir = openat(&dir, name, dir_flags(), Mode::empty()).ok()?;
        // Linked worktrees have a .git file; submodules and independent repos
        // can also be nested below the selected root.
        if !matches!(
            statat(&dir, ".git", AtFlags::SYMLINK_NOFOLLOW),
            Err(Errno::NOENT)
        ) {
            return None;
        }
    }
    Some(dir)
}

fn read_fresh_xml(directory: &OwnedFd, name: &OsStr, started: SystemTime) -> Option<String> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let mut file = File::from(openat(directory, name, flags, Mode::empty()).ok()?);
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.modified().ok()? < started {
        return None;
    }
    let mut xml = String::new();
    file.read_to_string(&mut xml).ok()?;
    Some(xml)
}

fn collect_cases(suite: Node<'_, '_>, module: &Path, summary: &mut TestSummary) {
    let mut nodes = vec![suite];
    while let Some(node) = nodes.pop() {
        match node.tag_name().name() {
            "testsuites" | "testsuite" => {
                nodes.extend(node.children().rev().filter(Node::is_element));
            }
            "testcase" => {
                summary.total += 1;
                let mut outcomes = node.children().filter(Node::is_element);
                if let Some(failure) = outcomes
                    .clone()
                    .find(|n| matches!(n.tag_name().name(), "failure" | "error"))
                {
                    summary.failures.push(test_failure(node, failure, module));
                } else if outcomes.any(|n| n.tag_name().name() == "skipped") {
                    summary.skipped += 1;
                }
            }
            _ => {}
        }
    }
}

fn test_failure(case: Node<'_, '_>, failure: Node<'_, '_>, module: &Path) -> TestFailure {
    let method = case.attribute("name").unwrap_or_default();
    let name = match case.attribute("classname") {
        Some(class) if !class.is_empty() && !method.is_empty() => format!("{class}.{method}"),
        Some(class) if method.is_empty() => class.to_owned(),
        _ => method.to_owned(),
    };
    let attribute = failure.attribute("message").unwrap_or_default();
    let body = failure
        .children()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .flat_map(str::lines);
    let message = attribute
        .lines()
        .chain(body.clone())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .or_else(|| failure.attribute("type"))
        .unwrap_or_default()
        .to_owned();
    let location = body
        .chain(attribute.lines())
        .find_map(|line| frame_location(module, line));
    TestFailure {
        name,
        message,
        location,
    }
}

fn frame_location(module: &Path, line: &str) -> Option<StackLocation> {
    // Messages and trailing annotations can also mention file:line. Only the
    // source field of a stack frame identifies where the exception occurred.
    let (frame, _) = line.trim().split_once(')')?;
    let (symbol, source) = frame.strip_prefix("at ")?.split_once('(')?;
    if symbol.contains(char::is_whitespace) || !symbol.contains('.') || !source.contains(':') {
        return None;
    }
    let target = resolve_stack_location_from(module, module, frame)?;
    let relative = target.path.strip_prefix(module).ok()?;
    if relative
        .components()
        .any(|p| p.as_os_str().eq_ignore_ascii_case(".git"))
    {
        return None;
    }
    // The canonical destination of a source link may be another checkout or
    // build nested under this module, even when this module is the root.
    let parents = target
        .path
        .parent()?
        .ancestors()
        .take_while(|p| *p != module);
    for dir in parents {
        if [".git", "build.gradle", "build.gradle.kts", "pom.xml"]
            .iter()
            .any(|marker| dir.join(marker).symlink_metadata().is_ok())
        {
            return None;
        }
    }
    Some(target)
}

#[cfg(test)]
mod tests;
