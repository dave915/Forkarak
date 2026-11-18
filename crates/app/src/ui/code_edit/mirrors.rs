//! A file owns its draft. Each open editor owns its selection and viewport.
use super::*;
use gpui_kit::component::input::{InputEditsEvent, Rope};
use std::{cell::Cell, rc::Rc};

pub(super) struct EditorBinding {
    _subscription: Subscription,
    // Synchronous Save/Close can precede a queued input event. Only an editor
    // that diverged from its own last displayed text may supply new bytes.
    observed: Rope,
    pending_mirrors: Rc<Cell<usize>>,
}

impl EditorBinding {
    // GPUI queues the input event during replace(). A defer queued directly
    // afterwards releases this guard before the next user edit is delivered.
    // Multiple replacements may be queued together (workspace edit and Undo).
    fn mirrored(&mut self, text: &Rope) -> impl FnOnce() + use<> {
        self.observed = text.clone();
        let pending = self.pending_mirrors.clone();
        pending.set(pending.get() + 1);
        move || pending.set(pending.get() - 1)
    }
}

impl CodeEdits {
    fn binding(&mut self, key: &FileKey, id: EntityId) -> Option<&mut EditorBinding> {
        self.bindings.get_mut(key)?.get_mut(&id)
    }

    fn capture_editor(&mut self, key: &FileKey, id: EntityId, text: &Rope) {
        if let Some(binding) = self.bindings.get_mut(key).and_then(|b| b.get_mut(&id))
            && let Some(buffer) = self.buffers.get_mut(key)
            && buffer.editing
            && &binding.observed != text
        {
            binding.observed = text.clone();
            buffer.changed(text.to_string());
        }
    }

    fn accepts_input(&mut self, key: &FileKey, id: EntityId, after: &Rope, current: &Rope) -> bool {
        let Some(binding) = self.binding(key, id) else {
            return false;
        };
        if binding.pending_mirrors.get() != 0 {
            return false;
        }
        let Some(buffer) = self.buffers.get_mut(key).filter(|b| b.editing) else {
            return false;
        };
        if after != current {
            // Type-then-undo can restore exactly the same bytes before either
            // event runs. A pending workspace transaction still saw an edit.
            buffer.revision = next_revision();
            return false;
        }
        true
    }

    fn forget_editor(&mut self, key: &FileKey, id: EntityId) {
        if let Some(editors) = self.bindings.get_mut(key) {
            editors.remove(&id);
            if editors.is_empty() {
                self.bindings.remove(key);
            }
        }
    }
}

impl Pokarak {
    pub(in crate::ui) fn capture_code_buffers(&mut self, cx: &App) {
        for (root, session) in &self.sessions {
            for (path, editor) in session.open_editors() {
                if let Some(key) = FileKey::new(root, &path) {
                    self.code_edits.capture_editor(
                        &key,
                        editor.entity_id(),
                        editor.read(cx).text(),
                    );
                }
            }
        }
    }

    pub(super) fn capture_draft_editors(&mut self, key: &FileKey, cx: &App) {
        for editor in self.bound_draft_editors(key) {
            self.code_edits
                .capture_editor(key, editor.entity_id(), editor.read(cx).text());
        }
    }

    pub(super) fn bound_draft_editors(&self, key: &FileKey) -> Vec<Entity<EditorState>> {
        let (Some(session), Some(bindings)) = (
            self.sessions.get(&key.root),
            self.code_edits.bindings.get(key),
        ) else {
            return vec![];
        };
        session
            .editors_for(&key.path)
            .into_iter()
            .filter(|editor| bindings.contains_key(&editor.entity_id()))
            .collect()
    }

    pub(in crate::ui) fn bind_code_editor(
        &mut self,
        root: &Path,
        path: &Path,
        editor: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = FileKey::new(root, path) else {
            return;
        };
        let Some(session) = self.sessions.get(root) else {
            return;
        };
        let open = session.editors_for(&key.path);
        if !open.contains(editor) {
            return;
        }
        let id = editor.entity_id();
        if self.code_edits.binding(&key, id).is_none() {
            let changed = key.clone();
            let on_change =
                cx.subscribe_in(editor, window, move |app, editor, event, window, cx| {
                    app.receive_editor_change(&changed, editor, event, window, cx);
                });
            let released = key.clone();
            let on_release = cx.observe_release(editor, move |app, _, _| {
                app.code_edits.forget_editor(&released, id);
            });
            self.code_edits
                .bindings
                .entry(key.clone())
                .or_default()
                .insert(
                    id,
                    EditorBinding {
                        _subscription: Subscription::join(on_change, on_release),
                        observed: editor.read(cx).text().clone(),
                        pending_mirrors: Default::default(),
                    },
                );
        }
        // An accepted disk refresh can change a clean draft or its eligibility
        // even when both entities and their subscriptions are reused.
        if self.code_edits.buffers.contains_key(&key) {
            self.sync_draft_editors(&key, window, cx);
        } else {
            self.sync_editor_readonly(&key, cx);
        }
    }

    fn receive_editor_change(
        &mut self,
        key: &FileKey,
        editor: &Entity<EditorState>,
        event: &InputEditsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = editor.entity_id();
        if !self.bound_draft_editors(key).contains(editor) {
            return;
        }
        let Some((_, after)) = event.edits().last() else {
            return;
        };
        let state = editor.read(cx);
        if !self.code_edits.accepts_input(key, id, after, state.text()) {
            return;
        }
        let active = self.current.as_ref() == Some(&key.root)
            && self.active_editor().as_ref() == Some(editor);
        let previous = &self.code_edits.buffers[key].draft.text;
        let typed =
            if active && self.navigation.is_some() && state.focus_handle(cx).is_focused(window) {
                language_tools::on_type::inserted_character(
                    previous,
                    &after.to_string(),
                    state.cursor(),
                )
            } else {
                None
            };
        self.code_edits.capture_editor(key, id, after);
        self.sync_draft_editors(key, window, cx);
        if active {
            self.status = self.code_edits.buffers[key].status().into();
            self.schedule_input_assistance(key, editor, typed, window, cx);
            self.refresh_inline_tools(&key.root, &key.path, editor, cx);
        }
        self.pin_preview(&key.root, &key.path);
        self.checkpoint_code_drafts(cx);
        cx.notify();
    }

    pub(super) fn sync_editor_readonly(&self, key: &FileKey, cx: &mut Context<Self>) {
        let readonly = !self.code_edits.can_edit(&key.root, &key.path);
        for editor in self.bound_draft_editors(key) {
            editor.update(cx, |state, cx| state.set_readonly(readonly, cx));
        }
    }

    pub(super) fn sync_draft_editors(
        &mut self,
        key: &FileKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editors = self.bound_draft_editors(key);
        let readonly = !self.code_edits.can_edit(&key.root, &key.path);
        let active = self
            .active_editor()
            .filter(|_| self.current.as_ref() == Some(&key.root));
        let mut refreshed = None;
        let (Some(buffer), Some(bindings)) = (
            self.code_edits.buffers.get(key),
            self.code_edits.bindings.get_mut(key),
        ) else {
            return;
        };
        let text = &buffer.draft.text;
        for editor in editors {
            let binding = bindings.get_mut(&editor.entity_id()).unwrap();
            editor.update(cx, |state, cx| {
                state.set_readonly(readonly, cx);
                if state.text() != text.as_str() {
                    replace_displayed_text(state, text, window, cx);
                    let delivered = binding.mirrored(state.text());
                    cx.defer(move |_| delivered());
                    if active.as_ref() == Some(&editor) {
                        refreshed = Some(editor.clone());
                    }
                } else {
                    binding.observed = state.text().clone();
                }
            });
        }
        if let Some(editor) = refreshed {
            self.refresh_inline_tools(&key.root, &key.path, &editor, cx);
        }
    }
}

fn replace_displayed_text(
    state: &mut EditorState,
    text: &str,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) {
    let old = state.value();
    let (range, replacement) = workspace::changed_range(&old, text);
    let selected = state.selected_range();
    let start = state.text().offset_to_position(selected.start);
    let end = state.text().offset_to_position(selected.end);
    let reversed = state.is_selection_reversed();
    let scroll = state.scroll_offset();
    state.set_selected_range(range, cx);
    state.replace(replacement.to_owned(), window, cx);
    let start = state.text().position_to_offset(&start);
    let end = state.text().position_to_offset(&end);
    state.set_selected_range_with_direction(start..end, reversed, cx);
    state.set_scroll_offset(scroll, cx);
}

#[cfg(test)]
#[path = "tests/mirrors.rs"]
mod tests;
