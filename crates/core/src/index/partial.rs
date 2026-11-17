//! A line selection is bound to exact source bytes, index, HEAD and worktree.
//! Reading never runs external filters or creates Git objects.
use super::{IndexGuard, IndexSnapshot};
use crate::{DiffKind, DiffLine, FileStatus, MAX_TEXT_BYTES, filesystem, git};
use anyhow::{Context, Result, bail, ensure};
use similar::{ChangeTag, TextDiff};
use std::{
    collections::BTreeSet,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Side {
    Unstaged,
    Staged,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Version {
    text: String,
    mode: &'static str,
}

#[derive(Debug, Clone)]
pub struct PartialPreview {
    guard: IndexGuard,
    path: PathBuf,
    side: Side,
    old: Option<Version>,
    new: Option<Version>,
    rows: Vec<DiffLine>,
    // (display row, zero-based original line position) in diff order.
    changes: Vec<(usize, usize)>,
}

impl PartialPreview {
    /// The exact two snapshots used to calculate the selectable diff.
    pub fn source_texts(&self) -> (&str, &str) {
        (
            self.old.as_ref().map_or("", |v| v.text.as_str()),
            self.new.as_ref().map_or("", |v| v.text.as_str()),
        )
    }

    pub fn lines(&self) -> &[DiffLine] {
        &self.rows
    }

    pub fn hunk_lines(&self, header_row: usize) -> Vec<usize> {
        if !self
            .rows
            .get(header_row)
            .is_some_and(|r| r.kind == DiffKind::Header && r.text.starts_with("@@ "))
        {
            return Vec::new();
        }
        self.rows
            .iter()
            .enumerate()
            .skip(header_row + 1)
            .take_while(|(_, r)| r.kind != DiffKind::Header || !r.text.starts_with("@@ "))
            .filter(|(_, r)| matches!(r.kind, DiffKind::Addition | DiffKind::Deletion))
            .map(|(row, _)| row)
            .collect()
    }

    pub(crate) fn root(&self) -> &Path {
        &self.guard.root
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn validate(&self, root: &Path) -> Result<()> {
        ensure!(
            root == self.guard.root && super::guard(root)? == self.guard,
            "워크트리, 브랜치 또는 인덱스가 바뀌었습니다. 미리보기를 새로 여세요."
        );
        let line_endings = line_endings(root, &self.path)?;
        if self.side == Side::Unstaged {
            ensure!(
                working(root, &self.path, line_endings)? == self.new,
                "파일 내용이나 권한이 바뀌었습니다. 변경 내용을 다시 확인하세요."
            );
            if self.old.is_none() {
                let untracked = git::output(
                    root,
                    &[
                        "ls-files".as_ref(),
                        "--others".as_ref(),
                        "--exclude-standard".as_ref(),
                        "-z".as_ref(),
                        "--".as_ref(),
                        self.path.as_os_str(),
                    ],
                )?;
                ensure!(
                    untracked.strip_suffix(&[0]) == Some(self.path.as_os_str().as_encoded_bytes()),
                    "무시되거나 접근할 수 없는 파일입니다."
                );
            }
        }
        Ok(())
    }

    // Compose from the original full file, using display rows only as IDs.
    // This also preserves omitted context, CRLF, and missing final newlines.
    pub(crate) fn selected_content(
        &self,
        selected: &[usize],
    ) -> Result<Option<(&'static str, String)>> {
        let selected = selected.iter().copied().collect::<BTreeSet<_>>();
        ensure!(
            !selected.is_empty()
                && selected.iter().all(|i| self
                    .rows
                    .get(*i)
                    .is_some_and(|r| matches!(r.kind, DiffKind::Addition | DiffKind::Deletion))),
            "변경된 줄을 한 개 이상 선택하세요."
        );
        let (indexed, destination) = match self.side {
            Side::Unstaged => (&self.old, &self.new),
            Side::Staged => (&self.new, &self.old),
        };
        if selected.len() == self.changes.len() && destination.is_none() {
            return Ok(None);
        }
        let mode = indexed
            .as_ref()
            .or(destination.as_ref())
            .context("Missing file mode")?
            .mode;
        let mut result = String::new();
        let old = lines(&self.old);
        let new = lines(&self.new);
        let mut cursor = 0;
        for &(row, at) in &self.changes {
            if selected.contains(&row) != (self.side == Side::Unstaged) {
                continue;
            }
            result.extend(old[cursor..at].iter().copied());
            cursor = at;
            match self.rows[row].new_line {
                Some(line) => result.push_str(new[line - 1]),
                None => cursor += 1,
            }
        }
        result.extend(old[cursor..].iter().copied());
        Ok(Some((mode, result)))
    }
}

fn lines(version: &Option<Version>) -> Vec<&str> {
    version
        .as_ref()
        .map(|v| v.text.split_inclusive('\n').collect())
        .unwrap_or_default()
}

pub fn preview(snapshot: &IndexSnapshot, path: &Path, side: Side) -> Result<PartialPreview> {
    let (files, staged) = match side {
        Side::Unstaged => (&snapshot.unstaged, false),
        Side::Staged => (&snapshot.staged, true),
    };
    let file = files
        .iter()
        .find(|f| f.path == path)
        .context("파일 목록을 새로 고치세요.")?;
    ensure!(
        file.status != FileStatus::Conflicted && (!staged || file.old_path.is_none()),
        "이름 변경 또는 충돌은 파일 단위로 처리하세요."
    );
    let root = &snapshot.root;
    let line_endings = line_endings(root, path)?;
    let indexed = snapshot.entries.get(path).and_then(|stages| stages.get(&0));
    let (old, new) = if staged {
        (
            blob(root, snapshot.baseline.get(path))?,
            blob(root, indexed)?,
        )
    } else {
        (blob(root, indexed)?, working(root, path, line_endings)?)
    };
    let flags = git::output(
        root,
        &[
            "ls-files".as_ref(),
            "-v".as_ref(),
            "-z".as_ref(),
            "--".as_ref(),
            path.as_os_str(),
        ],
    )?;
    ensure!(
        flags.is_empty()
            || (flags.starts_with(b"H ")
                && flags[2..].strip_suffix(&[0]) == Some(path.as_os_str().as_encoded_bytes())),
        "특수 인덱스 항목은 파일 단위로 처리하세요."
    );
    let mut preview = PartialPreview {
        guard: snapshot.guard.clone(),
        path: path.into(),
        side,
        old,
        new,
        rows: vec![],
        changes: vec![],
    };
    let old_lines = lines(&preview.old);
    let new_lines = lines(&preview.new);
    ensure!(
        old_lines.len().max(new_lines.len()) <= 50_000,
        "50,000줄을 넘는 파일은 파일 단위로 처리하세요."
    );
    let diff = TextDiff::configure()
        .timeout(Duration::from_millis(200))
        .diff_slices(&old_lines, &new_lines);
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let mut at = first.old_range().start;
        let hunk_range = |start: usize, end: usize| {
            format!("{},{}", start + usize::from(end > start), end - start)
        };
        preview.rows.push(header(format!(
            "@@ -{} +{} @@",
            hunk_range(at, last.old_range().end),
            hunk_range(first.new_range().start, last.new_range().end)
        )));
        for change in group.iter().flat_map(|op| diff.iter_changes(op)) {
            let kind = match change.tag() {
                ChangeTag::Equal => DiffKind::Context,
                ChangeTag::Delete => DiffKind::Deletion,
                ChangeTag::Insert => DiffKind::Addition,
            };
            if kind != DiffKind::Context {
                preview.changes.push((preview.rows.len(), at));
            }
            if kind != DiffKind::Addition {
                at += 1;
            }
            let value = change.value();
            preview.rows.push(DiffLine {
                old_line: change.old_index().map(|i| i + 1),
                new_line: change.new_index().map(|i| i + 1),
                kind,
                text: value
                    .strip_suffix("\r\n")
                    .or_else(|| value.strip_suffix('\n'))
                    .unwrap_or(value)
                    .to_owned(),
            });
            if !value.ends_with('\n') {
                preview
                    .rows
                    .push(header("\\ No newline at end of file".into()));
            }
        }
    }
    ensure!(
        !preview.changes.is_empty(),
        "선택할 텍스트 변경이 없습니다. 파일 단위 작업을 이용하세요."
    );
    ensure!(
        super::guard(root)? == snapshot.guard,
        "인덱스가 바뀌었습니다. 미리보기를 새로 여세요."
    );
    Ok(preview)
}

fn header(text: String) -> DiffLine {
    DiffLine {
        old_line: None,
        new_line: None,
        kind: DiffKind::Header,
        text,
    }
}

fn regular_mode(mode: &str) -> Result<&'static str> {
    match mode {
        "100644" => Ok("100644"),
        "100755" => Ok("100755"),
        _ => bail!("일반 텍스트 파일에서만 줄을 선택할 수 있습니다."),
    }
}

fn version(bytes: Vec<u8>, truncated: bool, mode: &'static str) -> Result<Version> {
    ensure!(
        !truncated && bytes.len() <= MAX_TEXT_BYTES && !bytes.contains(&0),
        "2 MiB 이하의 UTF-8 텍스트에서만 줄을 선택할 수 있습니다."
    );
    Ok(Version {
        text: String::from_utf8(bytes).context("UTF-8 텍스트 파일만 지원합니다.")?,
        mode,
    })
}

fn blob(root: &Path, entry: Option<&git::Entry>) -> Result<Option<Version>> {
    entry
        .map(|entry| {
            let mode = regular_mode(&entry.mode)?;
            let (bytes, truncated) = git::blob(root, entry)?;
            version(bytes, truncated, mode)
        })
        .transpose()
}

#[derive(Copy, Clone)]
enum LineEndings {
    Raw,
    Convert,
    FromConfig,
}

fn line_endings(root: &Path, path: &Path) -> Result<LineEndings> {
    let attrs = git::output(
        root,
        &[
            "check-attr".as_ref(),
            "-z".as_ref(),
            "filter".as_ref(),
            "working-tree-encoding".as_ref(),
            "ident".as_ref(),
            "diff".as_ref(),
            "text".as_ref(),
            "eol".as_ref(),
            "crlf".as_ref(),
            "--".as_ref(),
            path.as_os_str(),
        ],
    )?;
    let mut conversions = false;
    let mut raw_text = false;
    let fields = attrs
        .strip_suffix(&[0])
        .context("Incomplete Git attributes")?
        .split(|b| *b == 0)
        .collect::<Vec<_>>();
    let (triples, remainder) = fields.as_chunks::<3>();
    ensure!(remainder.is_empty(), "Incomplete Git attributes");
    for &[_, attr, value] in triples {
        let configured = !matches!(value, b"unspecified" | b"unset");
        match attr {
            b"filter" | b"working-tree-encoding" | b"ident" => ensure!(
                !configured,
                "Git 내용 변환이 설정된 파일은 파일 단위로 처리하세요."
            ),
            b"diff" => ensure!(
                value != b"unset",
                "바이너리로 표시된 파일은 파일 단위로 처리하세요."
            ),
            b"text" => {
                raw_text = value == b"unset";
                conversions |= configured;
            }
            b"crlf" | b"eol" => conversions |= configured,
            _ => (),
        }
    }
    Ok(if raw_text {
        LineEndings::Raw
    } else if conversions {
        LineEndings::Convert
    } else {
        LineEndings::FromConfig
    })
}

fn working(root: &Path, path: &Path, line_endings: LineEndings) -> Result<Option<Version>> {
    let file = match filesystem::open(root, path)? {
        filesystem::Node::File(f) => f,
        filesystem::Node::Missing => return Ok(None),
        _ => bail!("일반 텍스트 파일에서만 줄을 선택할 수 있습니다."),
    };
    let mut bytes = Vec::new();
    let perms = file.metadata()?.permissions().mode();
    file.take(MAX_TEXT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.windows(2).any(|pair| pair == b"\r\n") {
        let converts = match line_endings {
            LineEndings::Raw => false,
            LineEndings::Convert => true,
            LineEndings::FromConfig => !git::line(&git::words(
                root,
                &["config", "--default", "false", "--get", "core.autocrlf"],
            )?)?
            .eq_ignore_ascii_case("false"),
        };
        ensure!(
            !converts,
            "Git 줄바꿈 변환이 설정된 CRLF 파일은 파일 단위로 처리하세요."
        );
    }
    let mode = if perms & 0o100 != 0
        && git::words(
            root,
            &[
                "config",
                "--type=bool",
                "--default",
                "true",
                "--get",
                "core.filemode",
            ],
        )? == b"true\n"
    {
        "100755"
    } else {
        "100644"
    };
    version(bytes, false, mode).map(Some)
}
