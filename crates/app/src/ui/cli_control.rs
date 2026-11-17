use super::*;
use serde_json::{Value, json};

impl Pokarak {
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
                json!({"pid":std::process::id(),"socket":self.cli_control.as_ref().map(|s| &s.socket),"repo":self.current,"opening_repo":self.opening_path,"loading":self.loading,"file_loading":self.file_loading,"view":if self.git_history.active {"graph"} else {"code"},"file":self.selected(),"error":self.error,"projects":self.projects.iter().map(|p| &p.root).collect::<Vec<_>>(),"worktrees":self.inventory,
                    "comparison":self.session().map(|session| &session.saved.comparison),
                    "external_change":self.external_change,"auto_refresh_pending":self.auto_refresh.pending(),
                    "auto_refresh_completed":self.auto_refresh.completed,"status":self.status,
                    "reading_point":point,"draft_dirty":dirty,"history_loading":self.git_history.read_pending(),
                    "graph_selection":self.git_history.selection(),"graph_scroll_y":self.git_history.scroll_y()}),
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
