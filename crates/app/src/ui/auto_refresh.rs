//! Debounced, worktree-scoped refresh scheduling. Filesystem notifications are
//! only hints; a read-only content/ref summary decides whether a reload is needed.
use super::*;
use std::time::Instant;

const QUIET: Duration = Duration::from_millis(600);
const MAX_BURST: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ticket {
    scope: u64,
    serial: u64,
}

#[derive(Default)]
pub(super) struct AutoRefresh {
    root: Option<PathBuf>,
    scope: u64,
    serial: u64,
    first_event: Option<Instant>,
    last_event: Option<Instant>,
    checking: Option<Ticket>,
    fingerprint: Option<String>,
    ready: bool,
    automatic_request: Option<u64>,
    pub(super) completed: u64,
}

impl AutoRefresh {
    pub(super) fn reset(&mut self, root: Option<PathBuf>) {
        *self = Self {
            root,
            scope: self.scope.wrapping_add(1),
            completed: self.completed,
            ..Self::default()
        };
    }

    fn queue(&mut self, now: Instant) {
        self.first_event.get_or_insert(now);
        self.last_event = Some(now);
    }

    pub(super) fn pending(&self) -> bool {
        self.first_event.is_some() || self.ready || self.checking.is_some()
    }

    fn settled(&self, now: Instant) -> bool {
        self.last_event
            .is_some_and(|last| now.duration_since(last) >= QUIET)
            || self
                .first_event
                .is_some_and(|first| now.duration_since(first) >= MAX_BURST)
    }

    fn start(&mut self, now: Instant, blocked: bool) -> Option<Ticket> {
        if blocked || self.checking.is_some() || !self.settled(now) {
            return None;
        }
        self.serial = self.serial.wrapping_add(1);
        let ticket = Ticket {
            scope: self.scope,
            serial: self.serial,
        };
        self.checking = Some(ticket);
        self.first_event = None;
        self.last_event = None;
        Some(ticket)
    }

    fn finish(&mut self, root: &Path, ticket: Ticket, fingerprint: Option<String>) -> bool {
        if self.root.as_deref() != Some(root) || self.checking != Some(ticket) {
            return false;
        }
        self.checking = None;
        if let Some(fingerprint) = fingerprint {
            // The first check also closes the race between opening the graph
            // or code snapshot and installing its filesystem watcher.
            self.ready |= self.fingerprint.as_ref() != Some(&fingerprint);
            self.fingerprint = Some(fingerprint);
        }
        true
    }

    pub(super) fn preserves_focus(&self, request: u64) -> bool {
        self.automatic_request == Some(request)
    }

    pub(super) fn installed(&mut self, request: u64) {
        if self
            .automatic_request
            .take()
            .is_some_and(|own| own == request)
        {
            self.completed = self.completed.wrapping_add(1);
        }
    }
}

fn relevant_event(event: &notify::Event, objects: &[PathBuf]) -> bool {
    !event.kind.is_access()
        && (event.need_rescan()
            || event.paths.is_empty()
            || event
                .paths
                .iter()
                .any(|path| !objects.iter().any(|dir| path.starts_with(dir))))
}

impl Pokarak {
    pub(super) fn watch(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.watcher = None;
        self.changed = Arc::new(AtomicBool::new(false));
        self.auto_refresh.reset(Some(root.clone()));
        self.auto_refresh.queue(Instant::now());
        let scope = self.changed.clone();
        let changed = scope.clone();
        let task = cx.background_spawn(async move {
            let paths = pokarak_core::repository_watch_paths(&root)?;
            let objects = paths
                .iter()
                .filter(|path| *path != &root)
                .map(|path| path.join("objects"))
                .collect::<Vec<_>>();
            let mut watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if match &event {
                        Ok(event) => relevant_event(event, &objects),
                        Err(_) => true,
                    } {
                        changed.store(true, Ordering::Relaxed);
                    }
                })?;
            let mut installed = Vec::<PathBuf>::new();
            for path in paths {
                // Avoid duplicate recursive watches, including the main repo's
                // .git directory, while retaining external linked-worktree dirs.
                if !installed.iter().any(|parent| path.starts_with(parent)) {
                    watcher.watch(&path, RecursiveMode::Recursive)?;
                    installed.push(path);
                }
            }
            anyhow::Ok(watcher)
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |app, cx| {
                if !Arc::ptr_eq(&app.changed, &scope) {
                    return;
                }
                match result {
                    Ok(watcher) => app.watcher = Some(watcher),
                    Err(error) => {
                        app.error = Some(format!(
                            "자동갱신 감시를 시작할 수 없습니다. 수동 갱신을 사용하세요. {error:#}"
                        ));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn auto_refresh_blocked(&self, window: &mut Window, cx: &mut App) -> bool {
        let editing = self
            .current
            .as_ref()
            .zip(self.selected())
            .is_some_and(|(root, path)| {
                self.code_edits.dirty(root, &path)
                    && self.code_edits.editing(root, &path)
                    && self
                        .active_editor()
                        .is_some_and(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
            });
        self.opening_path.is_some()
            || self.loading
            || self.file_loading
            || self.navigating
            || self.git_is_busy()
            || self.commit_defers_auto_refresh(window, cx)
            || self.git_history.read_pending()
            || window.has_active_dialog(cx)
            || editing
            || self
                .current
                .as_ref()
                .is_some_and(|root| self.code_edits.saving_in(root))
    }

    pub(super) fn auto_refresh_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let now = Instant::now();
        if self.changed.swap(false, Ordering::Relaxed) {
            self.reviewed_files.invalidate_working(&root);
            self.auto_refresh.queue(now);
        }
        let blocked = self.auto_refresh_blocked(window, cx);
        if !blocked && self.auto_refresh.ready && self.auto_refresh.checking.is_none() {
            self.auto_refresh.ready = false;
            self.refresh(window, cx);
            self.auto_refresh.automatic_request = Some(self.request);
            return;
        }
        let Some(ticket) = self.auto_refresh.start(now, blocked) else {
            return;
        };
        self.check_code_disk_changes(cx);
        self.verify_reviewed_files(cx);
        self.checking_changes = true;
        self.count_pending = self.git_history.active;
        let generation = self.request;
        let target = root.clone();
        let task =
            cx.background_spawn(async move { pokarak_core::worktree_change_summary(&target) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |app, cx| {
                let fingerprint = result
                    .as_ref()
                    .ok()
                    .map(|(_, fingerprint)| fingerprint.clone());
                if !app.auto_refresh.finish(&root, ticket, fingerprint) {
                    return;
                }
                app.checking_changes = false;
                app.count_pending = false;
                if app.request != generation {
                    // Navigation or a manual operation superseded this check.
                    app.auto_refresh.queue(Instant::now());
                    return;
                }
                match result {
                    Ok((count, fingerprint)) => {
                        app.change_count = Some(count);
                        app.graph_fingerprint = Some(fingerprint);
                        app.external_change |= app.auto_refresh.ready;
                    }
                    Err(error) => {
                        // Keep the previous view and retry only after another
                        // event or explicit refresh, never on every timer tick.
                        app.error = Some(format!(
                            "자동갱신 확인 실패. 현재 화면을 유지합니다. {error:#}"
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn check_graph_changes(&mut self, _cx: &mut Context<Self>) {
        self.auto_refresh.queue(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoRefresh, MAX_BURST, QUIET, relevant_event};
    use std::{
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    fn state(now: Instant) -> AutoRefresh {
        let mut state = AutoRefresh::default();
        state.reset(Some(PathBuf::from("/fixture/main")));
        state.queue(now);
        state
    }

    #[test]
    fn burst_waits_for_quiet_and_preserves_pending_while_blocked() {
        let now = Instant::now();
        let mut state = state(now);
        state.queue(now + Duration::from_millis(400));
        assert!(
            state
                .start(now + Duration::from_millis(650), false)
                .is_none()
        );
        assert!(state.start(now + Duration::from_secs(2), true).is_none());
        assert!(state.pending());
        assert!(state.start(now + Duration::from_secs(2), false).is_some());
    }

    #[test]
    fn sustained_notifications_are_bounded_without_duplicate_checks() {
        let now = Instant::now();
        let mut state = state(now);
        for tick in 1..30 {
            state.queue(now + Duration::from_millis(tick * 100));
        }
        let ticket = state.start(now + MAX_BURST, false).unwrap();
        assert!(state.start(now + MAX_BURST, false).is_none());
        state.queue(now + MAX_BURST); // An event arrives during the check.
        assert!(state.finish(Path::new("/fixture/main"), ticket, Some("one".into())));
        assert!(state.first_event.is_some());
        assert!(state.ready);
    }

    #[test]
    fn worktree_reentry_rejects_old_same_root_response() {
        let now = Instant::now();
        let mut state = state(now);
        let old = state.start(now + QUIET, false).unwrap();
        state.reset(Some(PathBuf::from("/fixture/worker")));
        state.reset(Some(PathBuf::from("/fixture/main")));
        state.queue(now);
        let current = state.start(now + QUIET, false).unwrap();
        assert!(!state.finish(Path::new("/fixture/main"), old, Some("old".into())));
        assert_eq!(state.checking, Some(current));
        assert!(state.finish(Path::new("/fixture/main"), current, Some("new".into())));
    }

    #[test]
    fn unchanged_content_does_not_reload_and_failures_do_not_spin() {
        let now = Instant::now();
        let mut state = state(now);
        let first = state.start(now + QUIET, false).unwrap();
        state.finish(Path::new("/fixture/main"), first, Some("one".into()));
        state.ready = false;
        state.queue(now + Duration::from_secs(1));
        let same = state.start(now + Duration::from_secs(2), false).unwrap();
        state.finish(Path::new("/fixture/main"), same, Some("one".into()));
        assert!(!state.ready);
        state.queue(now + Duration::from_secs(3));
        let failed = state.start(now + Duration::from_secs(4), false).unwrap();
        state.finish(Path::new("/fixture/main"), failed, None);
        assert!(!state.pending());
        assert!(state.start(now + Duration::from_secs(8), false).is_none());
    }

    #[test]
    fn object_churn_is_ignored_but_refs_rescans_and_errors_can_refresh() {
        let objects = vec![PathBuf::from("/fixture/common.git/objects")];
        let object =
            notify::Event::new(notify::EventKind::Any).add_path(objects[0].join("ab/object"));
        assert!(!relevant_event(&object, &objects));
        let reference = notify::Event::new(notify::EventKind::Any)
            .add_path(PathBuf::from("/fixture/common.git/refs/heads/main"));
        assert!(relevant_event(&reference, &objects));
        assert!(relevant_event(
            &notify::Event::new(notify::EventKind::Any),
            &objects
        ));
    }
}
