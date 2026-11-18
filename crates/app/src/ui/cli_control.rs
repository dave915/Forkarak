use super::*;
use serde_json::{Value, json};

pub(super) struct Opening {
    target: crate::cli::OpenTarget,
    phase: Phase,
    pending: Option<crate::control::Pending>,
    deadline: std::time::Instant,
}
enum Phase {
    Ready,
    Project(u64),
    File {
        project: u64,
        reader: u64,
        file: u64,
        snapshot: bool,
    },
}
impl Pokarak {
    pub(super) fn cli_opens_project_file(&self, root: &Path, generation: u64) -> bool {
        self.cli_open.as_ref().is_some_and(|opening| {
            opening.target.repo == root
                && opening.target.path.is_some()
                && matches!(opening.phase, Phase::Project(request) if request == generation)
        })
    }
    pub(crate) fn queue_cli_targets(&mut self, targets: Vec<crate::cli::OpenTarget>) {
        self.cli_targets = targets.into();
    }
    fn start_cli_file(
        &mut self,
        opening: &mut Opening,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = opening.target.path.clone() else {
            return;
        };
        if let Some(session) = self.sessions.get_mut(&opening.target.repo) {
            session.saved.code = true;
            // A stale deletion must never route an existing live path to historical text.
            if session.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .changes
                    .iter()
                    .any(|file| file.path == path && file.status == FileStatus::Deleted)
            }) {
                session.snapshot = None;
            }
        }
        self.pending_location_revision = None;
        self.open_file(path, true, None, window, cx);
        opening.phase = Phase::File {
            project: self.request,
            reader: self.session().expect("CLI file has an open session").id,
            file: self.file_request,
            snapshot: self.pending_cli_file.is_some(),
        };
    }
    fn advance_cli_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cli_open.is_none()
            && !self.loading
            && !self.file_loading
            && let Some(target) = self.cli_targets.pop_front()
        {
            self.cli_open = Some(Opening {
                target,
                phase: Phase::Ready,
                pending: None,
                deadline: std::time::Instant::now() + Duration::from_secs(30),
            });
        }
        let Some(mut opening) = self.cli_open.take() else {
            return;
        };
        let result = (|| -> Result<Option<Value>, String> {
            if std::time::Instant::now() >= opening.deadline {
                return Err("File open timed out; inspect ctl gui status before retrying".into());
            }
            match opening.phase {
                Phase::Ready => {
                    if self.loading || self.file_loading {
                        return Ok(None);
                    }
                    if self.current.as_ref() != Some(&opening.target.repo) {
                        self.open_repository(opening.target.repo.clone(), window, cx);
                        opening.phase = Phase::Project(self.request);
                        return Ok(None);
                    }
                    if opening.target.path.is_some() {
                        self.start_cli_file(&mut opening, window, cx);
                        return Ok(None);
                    }
                }
                Phase::Project(generation) => {
                    if self.request != generation {
                        return Err("Project changed while CLI open was pending".into());
                    }
                    if self.loading {
                        return Ok(None);
                    }
                    if self.current.as_ref() != Some(&opening.target.repo) || self.error.is_some() {
                        return Err(self
                            .error
                            .clone()
                            .unwrap_or_else(|| "Requested worktree did not open".into()));
                    }
                    if opening.target.path.is_some() {
                        self.start_cli_file(&mut opening, window, cx);
                        return Ok(None);
                    }
                }
                Phase::File {
                    project,
                    reader,
                    file,
                    snapshot,
                } => {
                    if self.request != project
                        || self.session().is_none_or(|s| s.id != reader)
                        || self.current.as_ref() != Some(&opening.target.repo)
                        || self.git_history.active
                    {
                        return Err(
                            "Worktree/source changed while CLI file open was pending".into()
                        );
                    }
                    let expected = file + u64::from(snapshot);
                    if snapshot && self.pending_cli_file.is_some() && self.file_request == file {
                        if self.loading {
                            return Ok(None);
                        }
                        return Err(self
                            .error
                            .clone()
                            .unwrap_or_else(|| "Requested snapshot was not installed".into()));
                    }
                    if self.file_request != expected {
                        return Err("File changed while CLI open was pending".into());
                    }
                    if self.loading || self.file_loading {
                        return Ok(None);
                    }
                    if self.selected() != opening.target.path || self.error.is_some() {
                        return Err(self
                            .error
                            .clone()
                            .unwrap_or_else(|| "Requested file did not open".into()));
                    }
                    let path = opening.target.path.as_ref().unwrap();
                    let editor = self
                        .sessions
                        .get(&opening.target.repo)
                        .and_then(|session| session.documents.get(path))
                        .cloned()
                        .ok_or("Requested file has no readable editor")?;
                    if let Some(location) = opening.target.location {
                        let (line, column) = location.utf16(editor.read(cx).value().as_ref())?;
                        let position = reader_navigation::editor_position(
                            editor.read(cx).text(),
                            Position::new(line, column),
                        )
                        .ok_or("Location is outside the displayed source")?;
                        editor.update(cx, |editor, cx| {
                            editor.set_cursor_position(position, window, cx)
                        });
                    }
                    self.pin_preview(&opening.target.repo, path);
                    editor.read(cx).focus_handle(cx).focus(window, cx);
                    self.persist(cx);
                }
            }
            if opening
                .pending
                .as_ref()
                .is_some_and(|pending| pending.request.activate)
            {
                window.activate_window();
                cx.activate(true);
            }
            Ok(Some(
                json!({"opened":true,"repo":opening.target.repo,"file":opening.target.path,
                "view":if self.git_history.active {"graph"} else {"code"},"reading_point":self.reading_point(cx).map(|point|
                json!({"line":point.line,"column":point.column}))}),
            ))
        })();
        match result {
            Ok(None) => self.cli_open = Some(opening),
            done => {
                let done = done.map(|value| value.unwrap());
                if let Err(error) = &done {
                    self.error = Some(error.clone());
                    self.cli_targets.clear();
                }
                if let Some(pending) = opening.pending {
                    let _ = pending.reply.try_send(done);
                }
                cx.notify();
            }
        }
    }

    pub(super) fn install_cli_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match crate::control::Server::start() {
            Ok(server) => self.cli_control = Some(server),
            Err(error) => {
                eprintln!("Forkarak GUI control unavailable: {error:#}");
                return;
            }
        }
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |view, cx| {
            loop {
                executor.timer(Duration::from_millis(50)).await;
                if view
                    .update_in(cx, |this, window, cx| this.pump_cli_control(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    fn pump_cli_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.advance_cli_open(window, cx);
        // Bound work per frame; all path/contents validation happened off-thread.
        for _ in 0..4 {
            let pending = self
                .cli_control
                .as_ref()
                .and_then(|s| s.receiver.try_recv().ok());
            let Some(pending) = pending else {
                break;
            };
            if std::time::Instant::now() >= pending.deadline {
                continue;
            }
            if pending.request.action == "open-target" {
                if self.cli_open.is_some() || !self.cli_targets.is_empty() {
                    let _ = pending.reply.try_send(Err(
                        "A CLI open is already pending; retry after checking status".into(),
                    ));
                } else if let Some(target) = pending.request.target.clone() {
                    self.cli_open = Some(Opening {
                        target,
                        phase: Phase::Ready,
                        deadline: pending.deadline,
                        pending: Some(pending),
                    });
                }
                continue;
            }
            let response = self.apply_cli_control(pending.request, window, cx);
            let _ = pending.reply.try_send(response);
        }
    }
    fn apply_cli_control(
        &mut self,
        request: crate::control::Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        if request.action == "status" {
            let point = self.reading_point(cx).map(|point| {
                json!({
                    "line":point.line,"column":point.column,"scroll_x":point.scroll_x,
                    "scroll_y":point.scroll_y,"diff_row":point.row
                })
            });
            let dirty = self
                .current
                .as_ref()
                .zip(self.selected())
                .is_some_and(|(root, path)| self.code_edits.dirty(root, &path));
            return Ok(
                json!({"pid":std::process::id(),"session_dir":crate::prefs::explicit_session_dir(),"cli_open_pending":self.cli_open.is_some() || !self.cli_targets.is_empty(),"socket":self.cli_control.as_ref().map(|s| &s.socket),"repo":self.current,"opening_repo":self.opening_path,"loading":self.loading,"file_loading":self.file_loading,"view":if self.git_history.active {"graph"} else {"code"},"file":self.selected(),"error":self.error,"projects":self.projects.iter().map(|p| &p.root).collect::<Vec<_>>(),"project_worktrees":self.projects.iter().flat_map(|p| &p.worktrees).collect::<Vec<_>>(),"file_tabs":self.session().map(|session| &session.saved.tabs),"worktrees":self.inventory,
                    "comparison":self.session().map(|session| &session.saved.comparison),
                    "external_change":self.external_change,"auto_refresh_pending":self.auto_refresh.pending(),
                    "auto_refresh_completed":self.auto_refresh.completed,"status":self.status,
                    "reading_point":point,"draft_dirty":dirty,"history_loading":self.git_history.read_pending(),
                    "graph_selection":self.git_history.selection(),"graph_scroll_y":self.git_history.scroll_y(),
                    "file_review_state":self.selected_review_status(),"review_counts":self.review_counts()}),
            );
        }
        if self.loading {
            return Err("Project is loading; wait for gui status loading=false".into());
        }
        let repo = request.repo.ok_or("Missing repo")?;
        match request.action.as_str() {
            "open-project" => self.open_repository(repo, window, cx),
            "select-worktree" => {
                if !self
                    .inventory
                    .iter()
                    .any(|t| t.path == repo && !t.is_prunable)
                {
                    return Err(
                        "Worktree is not in the active project's inventory; use open-project"
                            .into(),
                    );
                }
                self.open_repository(repo, window, cx);
            }
            _ => {
                if self.current.as_ref() != Some(&repo) {
                    return Err(
                        "GUI worktree differs from --repo; select it explicitly first".into(),
                    );
                }
                match request.action.as_str() {
                    "open-file" => {
                        if self.file_loading {
                            return Err("A file is loading; wait for gui status".into());
                        }
                        self.open_file(
                            request.path.ok_or("Missing file path")?,
                            true,
                            Some(Position::new(request.line.saturating_sub(1), 0)),
                            window,
                            cx,
                        );
                    }
                    "view" if request.mode.as_deref() == Some("graph") => {
                        self.open_git_history(window, cx)
                    }
                    "view" if request.mode.as_deref() == Some("code") => {
                        self.show_code_view(window, cx)
                    }
                    "refresh" => self.refresh(window, cx),
                    _ => return Err("Unsupported GUI action".into()),
                }
            }
        }
        cx.notify();
        Ok(
            json!({"accepted":true,"action":request.action,"loading":self.loading,"file_loading":self.file_loading}),
        )
    }
}
