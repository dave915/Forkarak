//! Explicit review checkpoints, keyed by worktree, comparison and exact contents.
//! The checkpoint file lives beside session settings, never inside a repository.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io, path::Component};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum ReviewContext {
    Working(Option<String>),
    Commit { oid: String, parent: Option<String> },
}

impl ReviewContext {
    fn working(mode: &CompareMode) -> Self {
        Self::Working(match mode {
            CompareMode::Uncommitted => None,
            CompareMode::Branch(base) => Some(base.clone()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct ReviewKey {
    root: PathBuf,
    path: PathBuf,
    context: ReviewContext,
}

impl ReviewKey {
    fn new(root: &Path, path: &Path, context: ReviewContext) -> Option<Self> {
        let path = if path.is_absolute() {
            path.strip_prefix(root).ok()?
        } else {
            path
        };
        if !root.is_absolute() || path.as_os_str().is_empty()
            || path.components().any(|part| !matches!(part, Component::Normal(name) if !name.eq_ignore_ascii_case(".git")))
        { return None; }
        Some(Self {
            root: root.to_owned(),
            path: path.to_owned(),
            context,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    key: ReviewKey,
    stamp: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReviewState {
    Unchecked,
    Pending,
    Reviewed,
}

struct CachedDocumentStamp {
    document: std::sync::Weak<DiffDocument>,
    stamp: Option<String>,
}

pub(super) struct ReviewedFiles {
    records: Vec<Record>,
    stored: HashMap<ReviewKey, String>,
    verified: HashMap<ReviewKey, String>,
    document_stamps: std::cell::RefCell<HashMap<usize, CachedDocumentStamp>>,
    path: PathBuf,
    error: Option<String>,
    unreadable: bool,
    serial: u64,
    checking: bool,
    checking_context: Option<ReviewContext>,
}

fn fingerprint(doc: &DiffDocument) -> Option<String> {
    if doc.binary || doc.truncated {
        return None;
    }
    let mut hash = Sha256::new();
    let old_path = doc
        .old_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    for bytes in [
        old_path.as_bytes(),
        doc.old_text.as_bytes(),
        doc.new_text.as_bytes(),
    ] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    if let Some(mode) = doc.file_mode_header() {
        hash.update(mode.as_bytes());
    }
    Some(format!("{:x}", hash.finalize()))
}

impl ReviewedFiles {
    pub(super) fn load() -> Self {
        Self::load_from(crate::prefs::config_path().with_file_name("reviewed-files.json"))
    }

    fn load_from(path: PathBuf) -> Self {
        let mut state = Self {
            records: Vec::new(),
            stored: HashMap::new(),
            verified: HashMap::new(),
            document_stamps: std::cell::RefCell::new(HashMap::new()),
            path,
            error: None,
            unreadable: false,
            serial: 0,
            checking: false,
            checking_context: None,
        };
        let result = std::fs::read(&state.path).and_then(|bytes| {
            if bytes.len() > 8 * 1024 * 1024 {
                return Err(io::Error::other("검토 기록이 너무 큽니다"));
            }
            serde_json::from_slice::<Vec<Record>>(&bytes).map_err(io::Error::other)
        });
        match result {
            Ok(records)
                if records.len() <= 10_000
                    && records.iter().all(|record| {
                        ReviewKey::new(
                            &record.key.root,
                            &record.key.path,
                            record.key.context.clone(),
                        )
                        .as_ref()
                            == Some(&record.key)
                            && record.stamp.len() == 64
                            && record.stamp.bytes().all(|byte| byte.is_ascii_hexdigit())
                    }) =>
            {
                state.records = records
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            result => {
                state.error = Some(format!(
                    "검토 기록을 복원할 수 없습니다: {}",
                    result
                        .err()
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "잘못된 형식".into())
                ));
                state.unreadable = true;
            }
        }
        state.stored = state
            .records
            .iter()
            .map(|record| (record.key.clone(), record.stamp.clone()))
            .collect();
        state
    }

    fn replace(&mut self, records: Vec<Record>) -> io::Result<()> {
        let result = (|| {
            if self.unreadable {
                return Err(io::Error::other(
                    "기존 검토 기록을 읽을 수 없어 덮어쓰지 않습니다",
                ));
            }
            if records.len() > 10_000 {
                return Err(io::Error::other("검토 기록은 최대 10,000개까지 보관됩니다"));
            }
            let parent = self
                .path
                .parent()
                .ok_or_else(|| io::Error::other("검토 기록 경로가 없습니다"))?;
            std::fs::create_dir_all(parent)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer(temporary.as_file_mut(), &records)?;
            temporary.as_file().sync_all()?;
            temporary.persist(&self.path)?;
            Ok(())
        })();
        self.error = result
            .as_ref()
            .err()
            .map(|error| format!("검토 기록 보관 실패 — 기존 표시를 유지합니다: {error}"));
        if result.is_ok() {
            self.stored = records
                .iter()
                .map(|record| (record.key.clone(), record.stamp.clone()))
                .collect();
            self.records = records;
        }
        result
    }

    fn state(&self, key: &ReviewKey) -> ReviewState {
        match self.stored.get(key) {
            None => ReviewState::Unchecked,
            Some(stamp) if self.verified.get(key) == Some(stamp) => ReviewState::Reviewed,
            Some(_) => ReviewState::Pending,
        }
    }

    fn cached_stamp(&self, document: &Arc<DiffDocument>) -> Option<String> {
        let address = Arc::as_ptr(document) as usize;
        let mut stamps = self.document_stamps.borrow_mut();
        if let Some(cached) = stamps.get(&address)
            && cached
                .document
                .upgrade()
                .is_some_and(|cached| Arc::ptr_eq(&cached, document))
        {
            return cached.stamp.clone();
        }
        stamps.retain(|_, cached| cached.document.strong_count() > 0);
        let stamp = fingerprint(document);
        stamps.insert(
            address,
            CachedDocumentStamp {
                document: Arc::downgrade(document),
                stamp: stamp.clone(),
            },
        );
        stamp
    }

    fn set(&mut self, key: ReviewKey, stamp: Option<String>) -> io::Result<()> {
        let mut records = self
            .records
            .iter()
            .filter(|record| record.key != key)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(stamp) = &stamp {
            records.push(Record {
                key: key.clone(),
                stamp: stamp.clone(),
            });
        }
        self.replace(records)?;
        if let Some(stamp) = stamp {
            self.verified.insert(key, stamp);
        } else {
            self.verified.remove(&key);
        }
        Ok(())
    }

    fn save_reviewed_and_should_advance(
        &mut self,
        key: ReviewKey,
        stamp: String,
        advance: bool,
    ) -> bool {
        self.set(key, Some(stamp)).is_ok() && advance
    }

    fn begin(&mut self, context: ReviewContext) -> u64 {
        self.serial = self.serial.wrapping_add(1);
        self.checking = true;
        self.checking_context = Some(context);
        self.serial
    }

    pub(super) fn invalidate_working(&mut self, root: &Path) {
        if !matches!(self.checking_context, Some(ReviewContext::Commit { .. })) {
            self.serial = self.serial.wrapping_add(1);
            self.checking = false;
            self.checking_context = None;
        }
        self.verified
            .retain(|key, _| key.root != root || !matches!(key.context, ReviewContext::Working(_)));
    }

    fn apply_verification(&mut self, results: Vec<(ReviewKey, anyhow::Result<Option<String>>)>) {
        let mut changed = false;
        let mut records = self.records.clone();
        for (key, result) in results {
            self.verified.remove(&key);
            match result {
                Ok(Some(stamp)) if self.stored.get(&key) == Some(&stamp) => {
                    self.verified.insert(key, stamp);
                }
                Ok(_) => {
                    records.retain(|record| record.key != key);
                    changed = true;
                }
                Err(error) => {
                    self.error = Some(format!(
                        "검토 상태를 확인할 수 없습니다. 완료 표시는 숨겼습니다: {error:#}"
                    ));
                }
            }
        }
        if changed {
            let _ = self.replace(records);
        }
    }
}

#[derive(Clone)]
enum ReviewSource {
    Working {
        mode: CompareMode,
        files: Vec<ChangedFile>,
    },
    Commit(Arc<pokarak_core::history::CommitDetail>),
}

impl ReviewSource {
    fn context(&self) -> ReviewContext {
        match self {
            Self::Working { mode, .. } => ReviewContext::working(mode),
            Self::Commit(detail) => ReviewContext::Commit {
                oid: detail.commit.oid.clone(),
                parent: detail.parent.clone(),
            },
        }
    }
    fn files(&self) -> &[ChangedFile] {
        match self {
            Self::Working { files, .. } => files,
            Self::Commit(detail) => &detail.files,
        }
    }
    fn stamp(&self, root: &Path, path: &Path) -> anyhow::Result<Option<String>> {
        let Some(file) = self.files().iter().find(|file| file.path == path) else {
            return Ok(None);
        };
        let doc = match self {
            Self::Working { mode, .. } => pokarak_core::diff(root, file, mode)?,
            Self::Commit(detail) => pokarak_core::history::historical_diff(root, detail, path)?,
        };
        Ok(fingerprint(&doc))
    }
}

impl Pokarak {
    fn current_review_source(&self) -> Option<ReviewSource> {
        if self.git_history.active {
            self.git_history
                .review_data()
                .map(|(detail, _)| ReviewSource::Commit(Arc::new(detail.clone())))
        } else {
            self.snapshot().map(|snapshot| ReviewSource::Working {
                mode: snapshot.compare.clone(),
                files: snapshot.changes.clone(),
            })
        }
    }

    fn review_key(&self, path: &Path) -> Option<ReviewKey> {
        let context = if self.git_history.active {
            let (detail, _) = self.git_history.review_data()?;
            ReviewContext::Commit {
                oid: detail.commit.oid.clone(),
                parent: detail.parent.clone(),
            }
        } else {
            ReviewContext::working(&self.session()?.saved.comparison)
        };
        ReviewKey::new(self.current.as_ref()?, path, context)
    }

    pub(super) fn file_review_state(&self, path: &Path) -> ReviewState {
        let Some(key) = self.review_key(path) else {
            return ReviewState::Unchecked;
        };
        if matches!(key.context, ReviewContext::Working(_))
            && (self.code_edits.dirty(&key.root, &key.path)
                || self.code_edits.stale(&key.root, &key.path))
        {
            return ReviewState::Unchecked;
        }
        self.reviewed_files.state(&key)
    }

    pub(super) fn review_counts(&self) -> String {
        let files = if self.git_history.active {
            let Some((detail, _)) = self.git_history.review_data() else {
                return String::new();
            };
            detail.files.as_slice()
        } else {
            let Some(snapshot) = self.snapshot() else {
                return String::new();
            };
            snapshot.changes.as_slice()
        };
        let (mut reviewed, mut pending) = (0, 0);
        for file in files {
            match self.file_review_state(&file.path) {
                ReviewState::Reviewed => reviewed += 1,
                ReviewState::Pending => pending += 1,
                _ => {}
            }
        }
        if pending > 0 {
            format!("검토 {reviewed}/{} · 확인 중 {pending}", files.len())
        } else {
            format!("검토 {reviewed}/{}", files.len())
        }
    }

    pub(super) fn selected_review_status(&self) -> &'static str {
        match self
            .selected_review()
            .map(|(key, _)| self.reviewed_files.state(&key))
        {
            None => "unavailable",
            Some(ReviewState::Unchecked) => "unchecked",
            Some(ReviewState::Pending) => "pending",
            Some(ReviewState::Reviewed) => "reviewed",
        }
    }

    pub(super) fn verify_reviewed_files(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(source)) = (self.current.clone(), self.current_review_source())
        else {
            return;
        };
        let context = source.context();
        let keys = self
            .reviewed_files
            .records
            .iter()
            .filter(|record| record.key.root == root && record.key.context == context)
            .map(|record| record.key.clone())
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return;
        }
        for key in &keys {
            self.reviewed_files.verified.remove(key);
        }
        let serial = self.reviewed_files.begin(context.clone());
        let target = root.clone();
        let task = cx.background_spawn(async move {
            keys.into_iter()
                .map(|key| {
                    let result = source.stamp(&target, &key.path);
                    (key, result)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |view, cx| {
            let results = task.await;
            let _ = view.update(cx, |app, cx| {
                if app.reviewed_files.serial != serial {
                    return;
                }
                app.reviewed_files.checking = false;
                app.reviewed_files.checking_context = None;
                if app.current.as_ref() == Some(&root)
                    && app
                        .current_review_source()
                        .is_some_and(|source| source.context() == context)
                {
                    app.reviewed_files.apply_verification(results);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn selected_review(&self) -> Option<(ReviewKey, String)> {
        let root = self.current.as_ref()?;
        if self.git_history.active {
            let (_, doc) = self.git_history.review_data()?;
            let doc = doc?;
            Some((
                self.review_key(&doc.path)?,
                self.reviewed_files.cached_stamp(doc)?,
            ))
        } else {
            let session = self.session()?;
            let path = session.saved.selected.as_ref()?;
            if self.code_edits.dirty(root, path) || self.code_edits.stale(root, path) {
                return None;
            }
            let doc = session.diffs.get(path)?;
            Some((
                self.review_key(path)?,
                self.reviewed_files.cached_stamp(&doc.document)?,
            ))
        }
    }

    pub(super) fn toggle_reviewed_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, expected)) = self.selected_review() else {
            return;
        };
        if self.reviewed_files.state(&key) == ReviewState::Reviewed {
            let _ = self.reviewed_files.set(key, None);
            cx.notify();
            return;
        }
        let Some(source) = self.current_review_source() else {
            return;
        };
        let serial = self.reviewed_files.begin(key.context.clone());
        let target = key.clone();
        let task = cx.background_spawn(async move { source.stamp(&target.root, &target.path) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if app.reviewed_files.serial != serial {
                    return;
                }
                app.reviewed_files.checking = false;
                app.reviewed_files.checking_context = None;
                if app
                    .selected_review()
                    .as_ref()
                    .is_none_or(|(current, stamp)| current != &key || stamp != &expected)
                {
                    return;
                }
                match result {
                    Ok(Some(stamp)) if stamp == expected => {
                        let advance = app.prefs.option_bool(
                            app.current_project.as_deref(),
                            "review.advance",
                            false,
                        );
                        if app
                            .reviewed_files
                            .save_reviewed_and_should_advance(key, stamp, advance)
                        {
                            app.next_unreviewed_file(window, cx);
                        }
                    }
                    Ok(_) => {
                        app.reviewed_files.error = Some(
                            "파일 내용이 바뀌었습니다. 다시 읽은 후 검토 완료를 표시하세요.".into(),
                        );
                    }
                    Err(error) => {
                        app.reviewed_files.error =
                            Some(format!("검토 완료를 확인할 수 없습니다: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn reader_review_available(&self) -> bool {
        self.review_disabled_reason().is_none() && self.selected_review().is_some()
    }

    pub(super) fn reviewed_file_button(&self, cx: &mut Context<Self>) -> Button {
        let target = self.selected_review();
        let state = target
            .as_ref()
            .map(|(key, _)| self.reviewed_files.state(key))
            .unwrap_or(ReviewState::Unchecked);
        let busy = self.loading
            || self.file_loading
            || self.git_is_busy()
            || self.git_history.read_pending()
            || self.reviewed_files.checking
            || self.reviewed_files.unreadable;
        let reason = self.review_disabled_reason();
        toolbar::content_toolbar_icon(
            "toggle-file-reviewed",
            IconName::Check,
            reason.unwrap_or("파일 검토 완료 표시 전환"),
        )
        .tooltip(reason.unwrap_or("현재 파일과 비교 내용이 바뀌면 완료 표시가 해제됩니다."))
        .selected(state == ReviewState::Reviewed)
        .disabled(busy || target.is_none())
        .on_click(cx.listener(|app, _, w, cx| app.toggle_reviewed_file(w, cx)))
    }

    fn review_disabled_reason(&self) -> Option<&'static str> {
        if self.reviewed_files.unreadable {
            return Some(
                "검토 기록을 읽을 수 없습니다. 설정 폴더의 접근 권한과 기록 파일을 확인하세요.",
            );
        }
        if self.reviewed_files.checking {
            return Some(
                "파일 내용과 이전 검토 기록을 확인하고 있습니다. 완료 후 다시 시도하세요.",
            );
        }
        if self.loading || self.file_loading || self.git_history.read_pending() {
            return Some("파일을 읽고 있습니다. 로딩이 끝난 뒤 검토하세요.");
        }
        if self.git_is_busy() {
            return Some("Git 작업이 끝난 뒤 파일을 다시 확인하세요.");
        }
        let path = if self.git_history.active {
            self.git_history
                .review_data()
                .and_then(|(_, doc)| doc.map(|d| d.path.clone()))
        } else {
            self.selected()
        };
        let Some(path) = path else {
            return Some("비교할 변경 파일을 선택하세요.");
        };
        if !self.git_history.active
            && let Some(root) = &self.current
        {
            if self.code_edits.stale(root, &path) {
                return Some(
                    "디스크와 다른 초안입니다. 디스크와 비교하고 저장한 뒤 다시 검토하세요.",
                );
            }
            if self.code_edits.dirty(root, &path) {
                return Some("미저장 초안입니다. 파일을 저장한 뒤 검토 완료를 표시하세요.");
            }
        }
        let doc = if self.git_history.active {
            self.git_history.review_data().and_then(|(_, d)| d)
        } else {
            self.session()
                .and_then(|s| s.diffs.get(&path))
                .map(|d| &d.document)
        };
        if let Some(doc) = doc {
            if doc.binary {
                return Some(
                    "바이너리 파일은 텍스트 검토 표시를 지원하지 않습니다. 외부 뷰어에서 확인하세요.",
                );
            }
            if doc.truncated {
                return Some(
                    "일부만 표시한 파일입니다. 전체 내용을 확인할 수 있는 파일만 검토 완료로 표시할 수 있습니다.",
                );
            }
        }
        if self.selected_review().is_none() {
            return Some("비교할 변경사항이 없습니다. 변경 파일 목록에서 파일을 선택하세요.");
        }
        None
    }

    pub(super) fn show_unreviewed_only(&self) -> bool {
        self.session().is_some_and(|s| s.unreviewed_only)
    }

    pub(super) fn toggle_unreviewed_filter(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = &self.current
            && let Some(s) = self.sessions.get_mut(root)
        {
            s.unreviewed_only = !s.unreviewed_only;
            self.file_cursor = 0;
            cx.notify();
        }
    }

    pub(super) fn can_open_next_unreviewed(&self) -> bool {
        !self.loading && !self.file_loading && !self.git_is_busy()
    }

    pub(super) fn next_unreviewed_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.current_review_source() else {
            return;
        };
        let selected = if self.git_history.active {
            self.git_history
                .review_data()
                .and_then(|(_, doc)| doc.map(|d| d.path.clone()))
        } else {
            self.selected()
        };
        let files = source.files();
        let start = files
            .iter()
            .position(|f| Some(&f.path) == selected.as_ref())
            .map_or(0, |i| i + 1);
        let target = (0..files.len())
            .map(|i| &files[(start + i) % files.len()].path)
            .find(|p| self.file_review_state(p) != ReviewState::Reviewed)
            .cloned();
        if let Some(path) = target {
            if self.git_history.active {
                self.open_review_history_file(path, window, cx);
            } else {
                self.open_file(path, true, None, window, cx);
            }
        } else {
            self.status = "모든 변경 파일의 검토가 완료되었습니다.".into();
            cx.notify();
        }
    }

    pub(super) fn review_filter_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let only = self.show_unreviewed_only();
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1()
            .child(
                Button::new("unreviewed-only")
                    .label("미검토만")
                    .ghost()
                    .small()
                    .selected(only)
                    .on_click(cx.listener(|app, _, _, cx| app.toggle_unreviewed_filter(cx))),
            )
            .child(
                Button::new("next-unreviewed")
                    .label("다음 미검토")
                    .ghost()
                    .small()
                    .disabled(!self.can_open_next_unreviewed())
                    .on_click(cx.listener(|app, _, w, cx| app.next_unreviewed_file(w, cx))),
            )
            .into_any_element()
    }

    pub(super) fn reviewed_files_notice(&self, cx: &App) -> Option<AnyElement> {
        let has_file = if self.git_history.active {
            self.git_history
                .review_data()
                .is_some_and(|(_, doc)| doc.is_some())
        } else {
            self.selected().is_some()
        };
        let reason =
            (has_file && self.opening_path.is_none() && !self.loading && !self.file_loading)
                .then(|| self.review_disabled_reason())
                .flatten();
        let stale = self
            .current
            .as_ref()
            .zip(self.selected().as_ref())
            .is_some_and(|(root, path)| {
                !self.git_history.active && self.code_edits.stale(root, path)
            });
        global_review_notice(
            self.reviewed_files.error.as_deref(),
            self.reviewed_files.unreadable,
            stale,
            reason,
        )
        .map(|error| {
            div()
                .px_3()
                .py_1()
                .flex_none()
                .text_size(ui_text(11.))
                .text_color(Palette::new(cx).rgb(RED))
                .child(error.to_owned())
                .into_any_element()
        })
    }
}

/// Disabled states are ordinary context; only failed IO/persistence or a stale
/// draft should appear in the workspace's red error notice.
fn global_review_notice<'a>(
    error: Option<&'a str>,
    unreadable: bool,
    stale: bool,
    disabled_reason: Option<&'a str>,
) -> Option<&'a str> {
    error.or_else(|| (unreadable || stale).then_some(disabled_reason).flatten())
}

#[cfg(test)]
mod tests {
    #[test]
    fn ordinary_disabled_review_context_is_not_a_global_failure() {
        for reason in [
            "비교할 변경사항이 없습니다.",
            "파일을 읽고 있습니다.",
            "비교할 변경 파일을 선택하세요.",
            "미저장 초안입니다.",
        ] {
            assert_eq!(
                super::global_review_notice(None, false, false, Some(reason)),
                None
            );
        }
        assert_eq!(
            super::global_review_notice(Some("기록 보관 실패"), false, false, Some("정상 비활성")),
            Some("기록 보관 실패")
        );
        assert_eq!(
            super::global_review_notice(None, true, false, Some("기록 읽기 실패")),
            Some("기록 읽기 실패")
        );
        assert_eq!(
            super::global_review_notice(None, false, true, Some("디스크와 다른 초안")),
            Some("디스크와 다른 초안")
        );
    }
    use super::{ReviewContext, ReviewKey, ReviewSource, ReviewState, ReviewedFiles, fingerprint};
    use pokarak_core::{CompareMode, DiffDocument};
    use std::{fs, path::Path, process::Command};

    fn git(root: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        let output = command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "--literal-pathspecs",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Review Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-C",
            ])
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn source(root: &Path) -> ReviewSource {
        let snapshot = pokarak_core::snapshot_current(root, &CompareMode::Uncommitted).unwrap();
        ReviewSource::Working {
            mode: snapshot.compare,
            files: snapshot.changes,
        }
    }

    #[test]
    fn real_file_checkpoint_survives_restart_and_only_its_content_invalidates() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("one.txt"), "before\n").unwrap();
        fs::write(root.join("two.txt"), "other\n").unwrap();
        git(&root, &["add", "--", "."]);
        git(&root, &["commit", "-m", "base", "--"]);
        fs::write(root.join("one.txt"), "reviewed\n").unwrap();
        let key =
            ReviewKey::new(&root, Path::new("one.txt"), ReviewContext::Working(None)).unwrap();
        let stamp = source(&root).stamp(&root, &key.path).unwrap().unwrap();
        let storage = temp.path().join("session/reviewed-files.json");
        let mut state = ReviewedFiles::load_from(storage.clone());
        state.set(key.clone(), Some(stamp.clone())).unwrap();
        assert_eq!(state.state(&key), ReviewState::Reviewed);
        let mut restored = ReviewedFiles::load_from(storage);
        assert_eq!(restored.state(&key), ReviewState::Pending);
        fs::write(root.join("two.txt"), "unrelated\n").unwrap();
        restored.apply_verification(vec![(key.clone(), source(&root).stamp(&root, &key.path))]);
        assert_eq!(restored.state(&key), ReviewState::Reviewed);
        let file = fs::OpenOptions::new()
            .write(true)
            .open(root.join("one.txt"))
            .unwrap();
        let modified = file.metadata().unwrap().modified().unwrap();
        fs::write(root.join("one.txt"), "changed!\n").unwrap(); // same length, restored mtime
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        restored.apply_verification(vec![(key.clone(), source(&root).stamp(&root, &key.path))]);
        assert_eq!(restored.state(&key), ReviewState::Unchecked);
        assert!(
            ReviewedFiles::load_from(restored.path.clone())
                .records
                .is_empty()
        );
        assert!(!restored.path.starts_with(&root));
    }

    #[test]
    fn real_linked_worktrees_and_comparison_contexts_do_not_share_marks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("one.txt"), "base\n").unwrap();
        git(&root, &["add", "--", "."]);
        git(&root, &["commit", "-m", "base", "--"]);
        let worker = temp.path().join("worker");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "worker",
                "--",
                worker.to_str().unwrap(),
            ],
        );
        let worker = worker.canonicalize().unwrap();
        let path = Path::new("one.txt");
        let key = ReviewKey::new(&root, path, ReviewContext::Working(None)).unwrap();
        let mut state = ReviewedFiles::load_from(temp.path().join("session.json"));
        state.set(key.clone(), Some("a".repeat(64))).unwrap();
        for other in [
            ReviewKey::new(&worker, path, key.context.clone()).unwrap(),
            ReviewKey::new(&root, path, ReviewContext::Working(Some("main".into()))).unwrap(),
            ReviewKey::new(
                &root,
                path,
                ReviewContext::Commit {
                    oid: git(&root, &["rev-parse", "HEAD"]),
                    parent: None,
                },
            )
            .unwrap(),
        ] {
            assert_eq!(state.state(&other), ReviewState::Unchecked);
        }
    }

    #[test]
    fn failed_persistence_keeps_memory_marks_and_corrupt_file_is_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("marks.json");
        let key = ReviewKey::new(
            Path::new("/fixture"),
            Path::new("one.txt"),
            ReviewContext::Working(None),
        )
        .unwrap();
        let mut state = ReviewedFiles::load_from(path.clone());
        state.set(key.clone(), Some("a".repeat(64))).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(state.set(key.clone(), None).is_err());
        assert_eq!(state.state(&key), ReviewState::Reviewed);
        assert!(state.error.is_some());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, "unreadable old checkpoint").unwrap();
        let mut corrupt = ReviewedFiles::load_from(path.clone());
        assert!(corrupt.set(key, Some("b".repeat(64))).is_err());
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "unreadable old checkpoint"
        );
    }

    #[test]
    fn pending_checks_hide_old_marks_and_reject_binary_or_truncated_previews() {
        let temp = tempfile::tempdir().unwrap();
        let key = ReviewKey::new(
            Path::new("/fixture"),
            Path::new("one.txt"),
            ReviewContext::Working(None),
        )
        .unwrap();
        let mut state = ReviewedFiles::load_from(temp.path().join("marks.json"));
        state.set(key.clone(), Some("a".repeat(64))).unwrap();
        state.invalidate_working(&key.root);
        assert_eq!(state.state(&key), ReviewState::Pending);
        assert!(
            fingerprint(&DiffDocument {
                binary: true,
                ..Default::default()
            })
            .is_none()
        );
        assert!(
            fingerprint(&DiffDocument {
                truncated: true,
                ..Default::default()
            })
            .is_none()
        );
    }

    #[test]
    fn immutable_checks_and_cached_documents_do_not_depend_on_working_events() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = ReviewedFiles::load_from(temp.path().join("marks.json"));
        let serial = state.begin(ReviewContext::Commit {
            oid: "a".repeat(40),
            parent: None,
        });
        state.invalidate_working(Path::new("/fixture"));
        assert_eq!(state.serial, serial);
        assert!(state.checking);
        let mut document = std::sync::Arc::new(DiffDocument {
            new_text: "before".into(),
            ..Default::default()
        });
        let before = state.cached_stamp(&document).unwrap();
        assert_eq!(
            state.cached_stamp(&document).as_deref(),
            Some(before.as_str())
        );
        assert_eq!(std::sync::Arc::strong_count(&document), 1);
        std::sync::Arc::make_mut(&mut document).new_text = "after".into();
        assert_ne!(state.cached_stamp(&document).unwrap(), before);
    }
    #[test]
    fn review_advance_requires_enabled_setting_and_a_durable_successful_mark() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("marks.json");
        let key = ReviewKey::new(
            Path::new("/fixture"),
            Path::new("one.txt"),
            ReviewContext::Working(None),
        )
        .unwrap();
        let stamp = "a".repeat(64);
        let mut state = ReviewedFiles::load_from(path.clone());
        assert!(!state.save_reviewed_and_should_advance(key.clone(), stamp.clone(), false));
        assert_eq!(state.state(&key), ReviewState::Reviewed);
        assert_eq!(
            ReviewedFiles::load_from(path.clone()).stored.get(&key),
            Some(&stamp)
        );
        state.set(key.clone(), None).unwrap();
        assert!(state.save_reviewed_and_should_advance(key.clone(), stamp.clone(), true));
        assert_eq!(
            ReviewedFiles::load_from(path.clone()).stored.get(&key),
            Some(&stamp)
        );
        state.set(key.clone(), None).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(!state.save_reviewed_and_should_advance(key.clone(), stamp, true));
        assert_eq!(state.state(&key), ReviewState::Unchecked);
        assert!(state.error.is_some());
        assert!(path.is_dir());
    }
}
