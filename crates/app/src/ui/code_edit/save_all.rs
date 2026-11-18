//! Explicit, worktree-scoped Save All. A formatter result is only a proposal
//! until both its source and the original on-disk bytes have been checked.
use super::*;
use pokarak_navigation::{DocumentSnapshot, FormattingOptions, TextEdit};

pub(in crate::ui) struct SaveAllReport {
    pub(in crate::ui) saved: Vec<PathBuf>,
    pub(in crate::ui) failures: Vec<(PathBuf, String)>,
    pub(in crate::ui) remaining_dirty: usize,
}

#[derive(Clone)]
pub(super) struct SaveTarget {
    pub(super) key: FileKey,
    revision: u64,
}

impl SaveAllReport {
    pub(super) async fn collect(
        targets: Vec<SaveTarget>,
        mut save: impl AsyncFnMut(&SaveTarget) -> Result<(), String>,
    ) -> Self {
        let mut report = Self {
            saved: vec![],
            failures: vec![],
            remaining_dirty: 0,
        };
        for target in targets {
            match save(&target).await {
                Ok(()) => report.saved.push(target.key.path),
                Err(reason) => report.failures.push((target.key.path, reason)),
            }
        }
        report
    }

    pub(super) fn summary(&self) -> String {
        if self.failures.is_empty() {
            format!("{}개 파일 저장됨", self.saved.len())
        } else {
            format!(
                "{}개 저장 · {}개 실패",
                self.saved.len(),
                self.failures.len()
            )
        }
    }

    pub(super) fn show_failures(&self, window: &mut Window, cx: &mut App) {
        let failures = self.failures.clone();
        window.open_app_dialog(cx, move |dialog, _, _| {
            dialog.title("모두 저장 실패 내역").child(
                div()
                    .id("save-all-failures")
                    .v_flex()
                    .gap_2()
                    .max_h(ui_px(340.))
                    .overflow_y_scroll()
                    .children(failures.iter().map(|(path, reason)| {
                        div()
                            .v_flex()
                            .child(path.display().to_string())
                            .child(reason.clone())
                    })),
            )
        });
    }
}

struct PendingFormat {
    source: DocumentSnapshot,
    settings: serde_json::Value,
    task: Task<Result<String, String>>,
}

impl CodeEdits {
    pub(in crate::ui) fn dirty_count(&self, root: &Path) -> usize {
        self.buffers
            .iter()
            .filter(|(k, b)| k.root == root && b.dirty())
            .count()
    }

    pub(super) fn dirty_targets(&self, root: &Path) -> Vec<SaveTarget> {
        let mut targets: Vec<_> = self
            .buffers
            .iter()
            .filter(|(key, b)| key.root == root && b.dirty())
            .map(|(key, b)| SaveTarget {
                key: key.clone(),
                revision: b.revision,
            })
            .collect();
        targets.sort_by(|a, b| a.key.path.cmp(&b.key.path));
        targets
    }

    fn target_buffer(&self, target: &SaveTarget) -> Result<&Buffer, String> {
        let buffer = self.buffers.get(&target.key).ok_or("초안이 닫혔습니다.")?;
        if buffer.saving {
            return Err("이미 저장 중입니다.".into());
        }
        if !buffer.dirty() || buffer.revision != target.revision {
            return Err("모두 저장 요청 후 초안이 변경되었습니다. 다시 저장하세요.".into());
        }
        Ok(buffer)
    }

    pub(super) fn begin_batch_write(
        &mut self,
        target: &SaveTarget,
        formatted: Option<String>,
    ) -> Result<Draft, String> {
        let buffer = self.target_buffer(target)?;
        let mut submission = buffer.draft.clone();
        if let Some(formatted) = formatted {
            submission.text = formatted;
        }
        let buffer = self.buffers.get_mut(&target.key).unwrap();
        buffer.saving = true;
        buffer.error = None;
        Ok(submission)
    }

    pub(super) fn finish_batch_write(
        &mut self,
        target: &SaveTarget,
        submitted: &str,
        result: Result<(), String>,
    ) -> Result<(), String> {
        let buffer = self
            .buffers
            .get_mut(&target.key)
            .ok_or("초안이 닫혔습니다.")?;
        match result {
            Ok(()) => {
                if buffer.revision == target.revision {
                    buffer.changed(submitted.to_owned());
                }
                buffer.saved(submitted);
                Ok(())
            }
            Err(reason) => {
                buffer.saving = false;
                buffer.error = Some(reason.clone());
                Err(reason)
            }
        }
    }
}

impl Draft {
    pub(super) fn write_working_file(&self) -> Result<(), String> {
        if self.key.in_nested_checkout() {
            return Err("저장 대상이 다른 워크트리에 있습니다.".into());
        }
        pokarak_core::operations::save_file(
            &self.key.root,
            &self.key.path,
            &self.expected,
            &self.text,
        )
        .map_err(|e| format!("{e:#}"))
    }
}

impl Pokarak {
    pub(in crate::ui) fn save_all_code_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let started = self.save_all_code_files_then(window, cx, |app, root, report, window, cx| {
            if app.current.as_deref() == Some(root) && app.opening_path.is_none() {
                if !report.saved.is_empty() && !app.loading {
                    app.refresh(window, cx);
                }
                app.status = report.summary();
            }
        });
        if let Err(reason) = started {
            self.status = reason;
            cx.notify();
        }
    }

    /// Completes after all formatter and guarded disk writes finish. Consumers
    /// can start a run only for the matching root with no failures or dirty files.
    pub(in crate::ui) fn save_all_code_files_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        complete: impl FnOnce(&mut Self, &Path, &SaveAllReport, &mut Window, &mut Context<Self>)
        + 'static,
    ) -> Result<(), String> {
        let root = self.current.clone().ok_or("워크트리를 선택하세요.")?;
        self.check_batch_context(&root)?;
        if self.code_edits.unreadable_storage {
            return Err(self.code_edits.storage_error.clone().unwrap_or_default());
        }
        if window.has_active_dialog(cx) {
            return Err("열려 있는 대화상자를 먼저 닫으세요.".into());
        }
        if !self.code_edits.saving_all.insert(root.clone()) {
            return Err("이 워크트리의 모두 저장이 진행 중입니다.".into());
        }
        self.capture_code_buffers(cx);
        let targets = self.code_edits.dirty_targets(&root);
        self.code_edits.save_reports.remove(&root);
        cx.spawn_in(window, async move |view, cx| {
            let mut report = SaveAllReport::collect(targets, async |target| {
                let format = view
                    .update_in(cx, |app, _, cx| app.prepare_batch_format(target, cx))
                    .map_err(|e| e.to_string())??;
                let formatted = match format {
                    None => None,
                    Some(format) => Some((format.task.await?, format.source, format.settings)),
                };
                let (submitted, write) = view
                    .update_in(cx, |app, _, cx| {
                        app.check_batch_context(&target.key.root)?;
                        app.capture_draft_editors(&target.key, cx);
                        let replacement = if let Some((text, source, settings)) = formatted {
                            if !source.is_current()
                                || !app.batch_format_trusted(&target.key.root)
                                || app.language_settings() != settings
                            {
                                return Err(
                                    "초안·신뢰·포맷 설정이 변경되었습니다. 다시 저장하세요."
                                        .to_owned(),
                                );
                            }
                            Some(text)
                        } else {
                            None
                        };
                        let submission = app.code_edits.begin_batch_write(target, replacement)?;
                        app.sync_editor_readonly(&target.key, cx);
                        let submitted = submission.text.clone();
                        let write =
                            cx.background_spawn(async move { submission.write_working_file() });
                        cx.notify();
                        Ok((submitted, write))
                    })
                    .map_err(|e| e.to_string())??;
                let result = write.await;
                view.update_in(cx, |app, window, cx| {
                    app.capture_draft_editors(&target.key, cx);
                    let result = app
                        .code_edits
                        .finish_batch_write(target, &submitted, result);
                    app.sync_draft_editors(&target.key, window, cx);
                    app.checkpoint_code_drafts(cx);
                    result
                })
                .map_err(|e| e.to_string())?
            })
            .await;
            let _ = view.update_in(cx, |app, window, cx| {
                app.code_edits.saving_all.remove(&root);
                app.capture_code_buffers(cx);
                report.remaining_dirty = app.code_edits.dirty_count(&root);
                let _ = app.code_edits.persist();
                complete(app, &root, &report, window, cx);
                app.code_edits.save_reports.insert(root, report);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }

    fn check_batch_context(&self, root: &Path) -> Result<(), String> {
        if self.current.as_deref() != Some(root)
            || self.opening_path.is_some()
            || self.loading
            || self.git_is_busy()
        {
            return Err("워크트리가 바뀌었거나 다른 작업이 진행 중입니다. 다시 저장하세요.".into());
        }
        Ok(())
    }

    fn batch_format_trusted(&self, root: &Path) -> bool {
        self.prefs
            .trusted
            .iter()
            .any(|p| p == root || Some(p) == self.current_project.as_ref())
    }

    fn prepare_batch_format(
        &mut self,
        target: &SaveTarget,
        cx: &mut Context<Self>,
    ) -> Result<Option<PendingFormat>, String> {
        let key = &target.key;
        self.check_batch_context(&key.root)?;
        self.capture_draft_editors(&target.key, cx);
        let buffer = self.code_edits.target_buffer(target)?;
        let project = self.current_project.as_deref();
        if !self
            .prefs
            .option_bool(project, "editor.format_on_save", false)
        {
            return Ok(None);
        }
        if !self.batch_format_trusted(&key.root) {
            return Err("워크트리 분석을 허용한 후 포맷하여 저장할 수 있습니다.".into());
        }
        let text = buffer.draft.text.clone();
        let language =
            pokarak_navigation::language_for_path(&key.path).unwrap_or_else(|| language(&key.path));
        if !self.managed_language_enabled(language) {
            return Err(format!("{language} 언어 지원이 꺼져 있습니다."));
        }
        let options = FormattingOptions {
            tab_size: self
                .prefs
                .option_usize(project, "editor.tab_size", 4)
                .clamp(1, 32) as u32,
            insert_spaces: self.prefs.option_bool(project, "editor.use_spaces", true),
        };
        let settings = self.language_settings();
        let config = self.configured_file_formatter(&key.path)?;
        let source = self.snapshot_for(key, text)?;
        let snapshot = source.clone();
        let task = if let Some(config) = config {
            let key = key.clone();
            cx.background_spawn(async move {
                editor_tools::formatter::run_controlled(
                    config,
                    &key.root,
                    &key.path,
                    snapshot.text(),
                    Duration::from_secs(15),
                    Some(snapshot.clone()),
                )
            })
        } else {
            if self.navigation.is_none() {
                self.enable_analysis(cx);
            }
            let navigator = self
                .navigation
                .clone()
                .ok_or("언어 서버 포맷터를 준비할 수 없습니다.")?;
            self.capture_code_buffers(cx);
            let retained = self
                .code_edits
                .analysis_drafts(&key.root)
                .into_iter()
                .map(|(path, text)| {
                    self.snapshot_for(
                        &FileKey {
                            root: key.root.clone(),
                            path,
                        },
                        text,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            cx.background_spawn(async move {
                let mut nav = navigator
                    .lock()
                    .map_err(|_| "언어 서버가 응답하지 않습니다.".to_owned())?;
                format_with_navigator(&mut nav, &snapshot, &retained, &options)
            })
        };
        Ok(Some(PendingFormat {
            source,
            settings,
            task,
        }))
    }
}

pub(super) fn format_with_navigator(
    nav: &mut Navigator,
    source: &DocumentSnapshot,
    retained: &[DocumentSnapshot],
    options: &FormattingOptions,
) -> Result<String, String> {
    for draft in retained {
        nav.retain_buffer(draft).map_err(|e| format!("{e:#}"))?;
    }
    let edits = nav
        .format_buffer(source, options)
        .map_err(|e| format!("{e:#}"))?;
    TextEdit::apply_all(source.text(), &edits.value).map_err(|e| format!("{e:#}"))
}
