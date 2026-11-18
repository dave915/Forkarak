//! A language-server edit is one transaction over the current worktree's drafts.
use super::*;
use pokarak_navigation::{TextEdit, WorkspaceEdit};

const STALE: &str = "편집 중 내용이 바뀌어 적용하지 않았습니다. 다시 시도하세요";

pub(super) struct Replacement {
    key: FileKey,
    before: String,
    after: String,
    disk: FileRevision,
    locations: usize,
}

struct UndoFile {
    key: FileKey,
    before: String,
    revision: u64,
    disk: FileRevision,
}

pub(super) struct Transaction {
    id: u64,
    label: String,
    files: Vec<UndoFile>,
    locations: usize,
}

impl FileKey {
    pub(super) fn in_nested_checkout(&self) -> bool {
        self.path
            .ancestors()
            .skip(1)
            .take_while(|p| !p.as_os_str().is_empty())
            .any(|parent| self.root.join(parent).join(".git").exists())
    }
}

impl CodeEdits {
    pub(super) fn prepare_workspace_edit(
        &self,
        root: &Path,
        edit: &WorkspaceEdit,
    ) -> Result<Vec<Replacement>, String> {
        if self.unreadable_storage {
            return Err(self.storage_error.clone().unwrap_or_default());
        }
        let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
        let mut seen = HashSet::new();
        let mut replacements = Vec::new();
        for file in &edit.files {
            let relative = file
                .path
                .strip_prefix(&canonical_root)
                .map_err(|_| "현재 워크트리 밖의 파일은 수정할 수 없습니다.")?;
            let key = FileKey::new(root, relative).ok_or("올바르지 않은 편집 경로입니다.")?;
            if !seen.insert(key.clone()) {
                return Err(format!("중복된 편집 대상: {}", key.path.display()));
            }
            // Recheck a preview's target. A directory may now be a link or a
            // nested checkout, even though the server returned a canonical path.
            if file.path.canonicalize().ok().as_ref() != Some(&file.path)
                || !file.path.is_file()
                || relative
                    .components()
                    .any(|p| p.as_os_str().eq_ignore_ascii_case(".git"))
                || key.in_nested_checkout()
            {
                return Err(format!(
                    "수정할 수 없는 워크트리 파일: {}",
                    key.path.display()
                ));
            }
            let disk = file_revision(&file.path).ok_or(STALE)?;
            let buffer = self.buffers.get(&key);
            if buffer.is_some_and(|b| b.saving) {
                return Err(format!("{} 저장 중입니다.", key.path.display()));
            }
            let before = match buffer.filter(|b| b.dirty()) {
                Some(b) => b.draft.text.clone(),
                None => pokarak_core::read_file(root, &key.path).map_err(|e| format!("{e:#}"))?,
            };
            if before != file.original {
                return Err(STALE.into());
            }
            let after = TextEdit::apply_all(&before, &file.edits).map_err(|e| format!("{e:#}"))?;
            if after.len() > pokarak_core::MAX_TEXT_BYTES || after.contains('\0') {
                return Err(format!(
                    "{}: 2 MiB 이하의 텍스트 파일만 편집할 수 있습니다.",
                    key.path.display()
                ));
            }
            if after != before {
                replacements.push(Replacement {
                    key,
                    before,
                    after,
                    disk,
                    locations: file.edits.len(),
                });
            }
        }
        if replacements.is_empty() {
            return Err("적용할 변경이 없습니다.".into());
        }
        Ok(replacements)
    }

    pub(super) fn install_workspace_edit(
        &mut self,
        root: &Path,
        label: &str,
        replacements: Vec<Replacement>,
    ) {
        let mut transaction = Transaction {
            id: next_revision(),
            label: label.into(),
            locations: 0,
            files: Vec::with_capacity(replacements.len()),
        };
        for change in replacements {
            let key = change.key;
            let buffer = self.buffers.entry(key.clone()).or_insert_with(|| {
                Buffer::new(Draft {
                    key: key.clone(),
                    expected: change.before.clone(),
                    text: change.before.clone(),
                })
            });
            if !buffer.dirty() {
                buffer.draft.expected = change.before.clone();
                buffer.stale = false;
            }
            buffer.changed(change.after);
            transaction.locations += change.locations;
            transaction.files.push(UndoFile {
                key,
                before: change.before,
                revision: buffer.revision,
                disk: change.disk,
            });
        }
        self.transactions.insert(root.to_owned(), transaction);
        self.save_reports.remove(root);
    }

    fn can_undo_transaction(&self, transaction: &Transaction) -> bool {
        transaction.files.iter().all(|f| {
            self.buffers
                .get(&f.key)
                .is_some_and(|b| !b.saving && b.revision == f.revision)
        })
    }

    pub(super) fn undo_transaction(&mut self, root: &Path) -> Result<Vec<FileKey>, String> {
        let transaction = self
            .transactions
            .get(root)
            .ok_or("되돌릴 수정이 없습니다.")?;
        if !self.can_undo_transaction(transaction)
            || transaction.files.iter().any(|f| {
                f.key.in_nested_checkout() || file_revision(&root.join(&f.key.path)) != Some(f.disk)
            })
        {
            return Err("적용 후 파일이 변경되어 수정 전체를 되돌릴 수 없습니다.".into());
        }
        let transaction = self.transactions.remove(root).unwrap();
        Ok(transaction
            .files
            .into_iter()
            .map(|f| {
                self.buffers.get_mut(&f.key).unwrap().changed(f.before);
                f.key
            })
            .collect())
    }
}

impl Pokarak {
    pub(super) fn bound_draft_editor(&self, key: &FileKey) -> Option<Entity<EditorState>> {
        let (id, _) = self.code_edits.bindings.get(key)?;
        let documents = &self.sessions.get(&key.root)?.documents;
        if !self.code_edits.buffers.get(key)?.editing {
            return None;
        }
        documents
            .get(&key.path)
            .filter(|e| e.entity_id() == *id)
            .or_else(|| {
                documents
                    .get(&key.root.join(&key.path))
                    .filter(|e| e.entity_id() == *id)
            })
            .cloned()
    }

    pub(in crate::ui) fn apply_workspace_edit(
        &mut self,
        root: &Path,
        edit: &WorkspaceEdit,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.current.as_deref() != Some(root)
            || self.git_history.active
            || self.git_is_busy()
            || self.loading
            || self.file_loading
            || self.opening_path.is_some()
        {
            return Err(STALE.into());
        }
        self.capture_code_buffers(cx);
        let replacements = self.code_edits.prepare_workspace_edit(root, edit)?;
        if replacements.iter().any(|f| {
            self.bound_draft_editor(&f.key)
                .is_some_and(|editor| editor.read(cx).value().as_ref() != f.before)
        }) {
            return Err(STALE.into());
        }
        if replacements
            .iter()
            .any(|f| file_revision(&root.join(&f.key.path)) != Some(f.disk))
        {
            return Err(STALE.into());
        }
        let keys: Vec<_> = replacements.iter().map(|f| f.key.clone()).collect();
        self.code_edits
            .install_workspace_edit(root, label, replacements);
        for key in keys {
            self.sync_draft_editor(&key, window, cx);
            self.pin_preview(root, &key.path);
        }
        self.checkpoint_code_drafts(cx);
        cx.notify();
        Ok(())
    }

    pub(in crate::ui) fn undo_workspace_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.current.clone() else {
            return;
        };
        if self.git_is_busy() || self.loading || self.opening_path.is_some() {
            return;
        }
        self.capture_code_buffers(cx);
        match self.code_edits.undo_transaction(&root) {
            Ok(keys) => {
                for key in keys {
                    self.sync_draft_editor(&key, window, cx);
                }
                self.checkpoint_code_drafts(cx);
                self.status = "초안 수정을 되돌렸습니다.".into();
            }
            Err(e) => self.status = e,
        }
        cx.notify();
    }

    pub(in crate::ui) fn workspace_edit_status(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let root = self.current.as_ref()?;
        let transaction = self.code_edits.transactions.get(root);
        let saving = self.code_edits.saving_all.contains(root);
        let report = self.code_edits.save_reports.get(root);
        if transaction.is_none() && !saving && report.is_none() {
            return None;
        }
        let mut row = div().flex().items_center().gap_1().text_size(ui_text(11.));
        if let Some(batch) = transaction {
            let id = batch.id;
            let owner = root.clone();
            let save_owner = root.clone();
            row = row
                .child(format!(
                    "{} · {}개 파일 {}곳",
                    batch.label,
                    batch.files.len(),
                    batch.locations
                ))
                .child(
                    Button::new("undo-workspace-edit")
                        .label("되돌리기")
                        .small()
                        .ghost()
                        .disabled(saving || !self.code_edits.can_undo_transaction(batch))
                        .on_click(cx.listener(move |a, _, w, cx| {
                            if a.current.as_ref() == Some(&owner)
                                && a.code_edits
                                    .transactions
                                    .get(&owner)
                                    .is_some_and(|b| b.id == id)
                            {
                                a.undo_workspace_edit(w, cx);
                            }
                        })),
                )
                .child(
                    Button::new("save-workspace-edits")
                        .label("모두 저장")
                        .small()
                        .ghost()
                        .disabled(saving || self.code_edits.dirty_count(root) == 0)
                        .on_click(cx.listener(move |a, _, w, cx| {
                            if a.current.as_ref() == Some(&save_owner)
                                && a.code_edits
                                    .transactions
                                    .get(&save_owner)
                                    .is_some_and(|b| b.id == id)
                            {
                                a.save_all_code_files(w, cx);
                            }
                        })),
                );
        }
        if saving {
            row = row.child("모두 저장 중…");
        } else if let Some(report) = report {
            row = row.child(report.summary());
            if !report.failures.is_empty() {
                let owner = root.clone();
                row = row.child(
                    Button::new("save-all-details")
                        .label("실패 내역")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |a, _, w, cx| {
                            if a.current.as_ref() == Some(&owner)
                                && let Some(report) = a.code_edits.save_reports.get(&owner)
                            {
                                report.show_failures(w, cx);
                            }
                        })),
                );
            }
        }
        Some(row.into_any_element())
    }

    pub(super) fn sync_draft_editor(
        &mut self,
        key: &FileKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.bound_draft_editor(key) else {
            return;
        };
        let text = &self.code_edits.buffers[key].draft.text;
        editor.update(cx, |e, cx| {
            let old = e.value();
            if old.as_ref() == text {
                return;
            }
            let cursor = e.cursor_position();
            let scroll = e.scroll_offset();
            // One native range replacement gives one undo step even for a
            // multi-location rename. Preserve the unaffected prefix and suffix.
            let (range, replacement) = changed_range(&old, text);
            e.set_selected_range(range, cx);
            e.replace(replacement.to_owned(), window, cx);
            e.set_cursor_position(cursor, window, cx);
            e.set_scroll_offset(scroll, cx);
        });
        self.refresh_inline_tools(&key.root, &key.path, &editor, cx);
    }
}

pub(super) fn changed_range<'a>(before: &str, after: &'a str) -> (std::ops::Range<usize>, &'a str) {
    let start = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let suffix = before[start..]
        .chars()
        .rev()
        .zip(after[start..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    (
        start..before.len() - suffix,
        &after[start..after.len() - suffix],
    )
}
