//! Each worktree has one or two file readers. The existing command surface
//! addresses the focused reader. Both rendering and file completion can address
//! its sibling synchronously without changing focus or duplicating the pipeline.
use super::*;
use crate::prefs::SplitView;
use gpui_kit::base::ElementExt;
use gpui_kit::component::resizable::resize_handle;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::AtomicU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
pub(super) enum ReaderGroup {
    Left,
    Right,
}

impl ReaderGroup {
    pub(super) fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Left => "editor-group-left",
            Self::Right => "editor-group-right",
        }
    }
}

pub(super) struct ReaderRuntime {
    pub(super) id: u64,
    pub(super) focus_scope: Option<(FocusHandle, Subscription)>,
    pub(super) preview_tab: Option<PathBuf>,
    pub(super) result_tab: Option<result_presentation::NavigationTab>,
    pub(super) documents: HashMap<PathBuf, Entity<EditorState>>,
    pub(super) historical_documents: HashSet<PathBuf>,
    pub(super) diffs: HashMap<PathBuf, ReviewDiff>,
    pub(super) scroll: UniformListScrollHandle,
    pub(super) history: Vec<ReadingPoint>,
    pub(super) forward: Vec<ReadingPoint>,
    pub(super) restoring: bool,
    // Compatibility with existing open/CLI/save callers: while addressed, the
    // request counter and loading flag are held by Pokarak.
    file_request: u64,
    file_loading: bool,
}

impl ReaderRuntime {
    pub(super) fn new(restoring: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            focus_scope: None,
            preview_tab: None,
            result_tab: None,
            documents: HashMap::new(),
            historical_documents: HashSet::new(),
            diffs: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            history: Vec::new(),
            forward: Vec::new(),
            restoring,
            file_request: 0,
            file_loading: false,
        }
    }
}

impl Deref for Session {
    type Target = ReaderRuntime;
    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

impl DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.reader
    }
}

fn swap_saved_reader(saved: &mut SavedView) {
    use std::mem::swap;
    let Some(peer) = saved.split.as_mut() else {
        return;
    };
    swap(&mut saved.tabs, &mut peer.tabs);
    swap(&mut saved.selected, &mut peer.selected);
    swap(&mut saved.code, &mut peer.code);
    swap(&mut saved.reader_split_diff, &mut peer.reader_split_diff);
    swap(&mut saved.positions, &mut peer.positions);
    swap(&mut saved.line, &mut peer.line);
    swap(&mut saved.column, &mut peer.column);
    swap(&mut saved.scroll_x, &mut peer.scroll_x);
    swap(&mut saved.scroll_y, &mut peer.scroll_y);
    swap(&mut saved.diff_row, &mut peer.diff_row);
}

pub(super) fn list_offset(handle: &UniformListScrollHandle) -> Option<[f32; 2]> {
    let state = handle.0.borrow();
    if state.last_item_size.is_none() || state.deferred_scroll_to_item.is_some() {
        return None;
    }
    let offset = state.base_handle.offset();
    Some([offset.x.into(), offset.y.into()])
}

pub(super) fn restore_list_offset(handle: &UniformListScrollHandle, offset: [f32; 2]) {
    let mut state = handle.0.borrow_mut();
    state.deferred_scroll_to_item = None;
    state
        .base_handle
        .set_offset(point(px(offset[0]), px(offset[1])));
}

impl Session {
    pub(super) fn saved_view(&self) -> SavedView {
        let mut saved = self.saved.clone();
        if self.active_group == ReaderGroup::Right {
            swap_saved_reader(&mut saved);
        }
        saved
    }

    pub(super) fn for_each_reader(&mut self, mut apply: impl FnMut(&mut Self)) {
        apply(self);
        if self.peer.is_some() {
            self.swap_readers();
            apply(self);
            self.swap_readers();
        }
    }

    fn swap_readers(&mut self) {
        if let Some(peer) = &mut self.peer {
            std::mem::swap(&mut self.reader, peer);
            swap_saved_reader(&mut self.saved);
            self.active_group = self.active_group.other();
        }
    }

    pub(super) fn clear_reader_split(&mut self) {
        if self.active_group == ReaderGroup::Right {
            self.swap_readers();
        }
        self.peer = None;
        self.saved.split = None;
        self.focused_group = ReaderGroup::Left;
    }

    fn ensure_split(&mut self, default_side_diff: bool) {
        if self.peer.is_none() {
            self.saved
                .reader_split_diff
                .get_or_insert(default_side_diff);
            self.saved.split = Some(SplitView {
                code: self.saved.code,
                reader_split_diff: self.saved.reader_split_diff,
                ..Default::default()
            });
            self.peer = Some(ReaderRuntime::new(false));
        }
    }

    fn group_for_id(&self, id: u64) -> Option<ReaderGroup> {
        if self.reader.id == id {
            Some(self.active_group)
        } else {
            self.peer
                .as_ref()
                .filter(|p| p.id == id)
                .map(|_| self.active_group.other())
        }
    }

    pub(super) fn group_is_current(&self, group: ReaderGroup, id: u64) -> bool {
        self.group_for_id(id) == Some(group)
    }

    fn documents_by_path(&self) -> impl Iterator<Item = (&Path, &Entity<EditorState>)> {
        let root = self
            .worktree_root
            .as_ref()
            .or_else(|| self.snapshot.as_ref().map(|s| &s.worktree.path));
        std::iter::once(&self.reader)
            .chain(self.peer.iter())
            .flat_map(|reader| {
                reader
                    .documents
                    .iter()
                    .filter(|(p, _)| !reader.historical_documents.contains(*p))
            })
            .filter_map(move |(path, editor)| {
                let path = if path.is_absolute() {
                    path.strip_prefix(root?).ok()?
                } else {
                    path.as_path()
                };
                Some((path, editor))
            })
    }

    pub(super) fn editors_for(&self, path: &Path) -> Vec<Entity<EditorState>> {
        let root = self
            .worktree_root
            .as_ref()
            .or_else(|| self.snapshot.as_ref().map(|s| &s.worktree.path));
        let path = root.and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path);
        self.documents_by_path()
            .filter(|(p, _)| *p == path)
            .map(|(_, ed)| ed.clone())
            .collect()
    }

    pub(super) fn open_editors(&self) -> Vec<(PathBuf, Entity<EditorState>)> {
        self.documents_by_path()
            .map(|(path, ed)| (path.to_owned(), ed.clone()))
            .collect()
    }

    pub(super) fn capture_reader_positions(&mut self, cx: &App) {
        self.for_each_reader(|s| s.capture_addressed_positions(cx));
    }

    fn capture_addressed_positions(&mut self, cx: &App) {
        if self.reader.restoring {
            return;
        }
        for (path, doc) in &self.reader.documents {
            let editor = doc.read(cx);
            let p = editor.cursor_position();
            let saved = self.saved.positions.entry(path.clone()).or_default();
            saved.line = p.line;
            saved.column = p.character;
            // Hidden/restored editors may still have a deferred viewport.
            if editor.visible_row_range().is_some() {
                let offset = editor.scroll_offset();
                saved.scroll_x = offset.x.into();
                saved.scroll_y = offset.y.into();
            }
        }
        for (path, diff) in &self.reader.diffs {
            if let Some(offset) = diff.side_offset(cx) {
                self.saved
                    .positions
                    .entry(path.clone())
                    .or_default()
                    .side_offset = Some(offset);
            }
        }
        if let Some(path) = &self.saved.selected {
            let saved = self.saved.positions.entry(path.clone()).or_default();
            if !self.saved.code {
                saved.diff_row = split_diff::scroll_row_at(&self.reader.scroll, cx);
                if let Some(offset) = list_offset(&self.reader.scroll) {
                    saved.unified_offset = Some(offset);
                }
            }
            self.saved.diff_row = saved.diff_row;
            self.saved.line = saved.line;
            self.saved.column = saved.column;
            self.saved.scroll_x = saved.scroll_x;
            self.saved.scroll_y = saved.scroll_y;
        }
    }

    pub(super) fn history_target(
        &mut self,
        backward: bool,
        current: Option<ReadingPoint>,
    ) -> Option<ReadingPoint> {
        let target = if backward {
            self.history.pop()
        } else {
            self.forward.pop()
        }?;
        if let Some(current) = current {
            if backward {
                self.forward.push(current);
            } else {
                self.history.push(current);
            }
        }
        Some(target)
    }

    pub(super) fn remove_tab(&mut self, path: &Path) -> bool {
        self.saved.tabs.retain(|p| p != path);
        self.documents.remove(path);
        self.historical_documents.remove(path);
        self.diffs.remove(path);
        if self.preview_tab.as_deref() == Some(path) {
            self.preview_tab = None;
        }
        let selected = self.saved.selected.as_deref() == Some(path);
        if selected {
            self.saved.selected = self.saved.tabs.last().cloned();
        }
        selected
    }

    pub(super) fn finish_empty_group(&mut self) {
        // The surviving reader is already addressed. Promotion keeps its
        // actual entities and identity, including any in-flight file read.
        self.peer = None;
        self.saved.split = None;
        self.active_group = ReaderGroup::Left;
        self.focused_group = ReaderGroup::Left;
    }
}

const GROUP_MIN: f32 = 360.;
const DIVIDER: f32 = 1.;

fn visible_ratio(width: f32, preference: f32) -> Option<f32> {
    let available = width - DIVIDER;
    if available < GROUP_MIN * 2. {
        return None;
    }
    let min = GROUP_MIN / available;
    Some(if preference.is_finite() {
        preference.clamp(min, 1. - min)
    } else {
        0.5
    })
}

impl Pokarak {
    pub(super) fn session_mut(&mut self) -> Option<&mut Session> {
        self.current.as_ref().and_then(|r| self.sessions.get_mut(r))
    }

    pub(super) fn reader_load_pending(&self) -> bool {
        self.file_loading
            || self
                .session()
                .and_then(|s| s.peer.as_ref())
                .is_some_and(|p| p.file_loading)
    }

    pub(super) fn reader_split_diff(&self) -> bool {
        if !self.git_history.active
            && let Some(split) = self.session().and_then(|s| s.saved.reader_split_diff)
        {
            split
        } else {
            self.prefs.split_diff
        }
    }

    pub(super) fn address_reader(&mut self, group: ReaderGroup) -> bool {
        let Some(session) = self.current.as_ref().and_then(|r| self.sessions.get_mut(r)) else {
            return false;
        };
        if session.active_group == group {
            return true;
        }
        if session.peer.is_none() {
            return false;
        }
        session.reader.file_request = self.file_request;
        session.reader.file_loading = self.file_loading;
        session.swap_readers();
        self.file_request = session.reader.file_request;
        self.file_loading = session.reader.file_loading;
        true
    }

    /// Run synchronous work for a specific live reader, then restore the
    /// command target. A retired group ID cannot address a replacement group.
    pub(super) fn with_reader<R>(
        &mut self,
        id: u64,
        work: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        let session = self.session()?;
        let original = session.active_group;
        let group = session.group_for_id(id)?;
        self.address_reader(group);
        let result = work(self);
        self.address_reader(original);
        Some(result)
    }

    pub(super) fn activate_reader_group(
        &mut self,
        group: ReaderGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.git_history.active || self.opening_path.is_some() || window.has_active_dialog(cx) {
            return false;
        }
        if group == ReaderGroup::Right
            && self
                .session()
                .and_then(|s| s.reader_width)
                .is_some_and(|w| visible_ratio(w, 0.5).is_none())
        {
            return false;
        }
        if !self.address_reader(group) {
            return false;
        }
        let session = self.session_mut().unwrap();
        if session.focused_group != group {
            session.focused_group = group;
            self.navigation_request += 1;
            self.pending_history = None;
            self.pending_location_revision = None;
            self.clear_result_surface();
            self.invalidate_language_view(cx);
            cx.notify();
        }
        true
    }

    pub(super) fn focus_reader_group(
        &mut self,
        group: ReaderGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.activate_reader_group(group, window, cx) {
            self.focus_addressed_reader(window, cx);
        }
    }

    pub(super) fn focus_addressed_reader(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session().is_some_and(|s| s.saved.code)
            && let Some(editor) = self.active_editor()
        {
            editor.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    pub(super) fn split_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected() {
            self.open_in_group(ReaderGroup::Right, path, window, cx);
        }
    }

    pub(super) fn open_to_side(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.session() {
            self.open_in_group(session.active_group.other(), path, window, cx);
        }
    }

    fn open_in_group(
        &mut self,
        group: ReaderGroup,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_history.active
            || self.loading
            || self.opening_path.is_some()
            || window.has_active_dialog(cx)
        {
            return;
        }
        self.capture_positions(cx);
        let split_mode = self.reader_split_diff();
        let Some(session) = self.session_mut() else {
            return;
        };
        let source = session.saved.positions.get(&path).cloned();
        session.ensure_split(split_mode);
        let original = session.focused_group;
        let visible = self.activate_reader_group(group, window, cx);
        self.address_reader(group);
        let previous = self.reading_point(cx).filter(|p| p.path != path);
        let session = self.session_mut().unwrap();
        if let Some(previous) = previous {
            session.push_history(previous);
        }
        if let Some(source) = source {
            session
                .saved
                .positions
                .entry(path.clone())
                .or_insert(source);
        }
        self.open_file(path, false, None, window, cx);
        if !visible {
            self.address_reader(original);
        }
        self.persist(cx);
    }

    pub(super) fn move_to_other(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_history.active
            || self.loading
            || self.opening_path.is_some()
            || window.has_active_dialog(cx)
            || !self.session().is_some_and(|s| s.saved.tabs.contains(&path))
        {
            return;
        }
        self.capture_positions(cx);
        let group = self.session().unwrap().active_group;
        self.open_in_group(group.other(), path.clone(), window, cx);
        self.address_reader(group);
        self.close_file(path, window, cx);
        if self.session().is_some_and(|s| s.peer.is_some()) {
            self.focus_reader_group(group.other(), window, cx);
            let focused = self.session().unwrap().focused_group;
            self.address_reader(focused);
        }
        self.persist(cx);
    }
}

#[derive(Clone)]
struct GroupDivider {
    root: PathBuf,
    reader: u64,
}

impl Render for GroupDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl Pokarak {
    pub(super) fn render_editor_groups(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(session) = self.session() else {
            return self.render_welcome(cx);
        };
        let group = session.active_group;
        if session.peer.is_some() {
            self.session_mut().unwrap().for_each_reader(|s| {
                if s.focus_scope.is_none() {
                    let handle = cx.focus_handle();
                    let id = s.id;
                    let observed = handle.clone();
                    let subscription = cx.on_focus_in(&handle, window, move |app, window, cx| {
                        if observed.contains_focused(window, cx)
                            && let Some(group) = app.session().and_then(|s| s.group_for_id(id))
                        {
                            app.activate_reader_group(group, window, cx);
                        }
                    });
                    s.focus_scope = Some((handle, subscription));
                }
            });
        }
        let session = self.session().unwrap();
        let split = session.saved.split.as_ref();
        let width = session.reader_width.unwrap_or(0.);
        let ratio = split.and_then(|s| visible_ratio(width, s.ratio));
        let unsplit = split.is_none();
        let hidden = !unsplit && ratio.is_none();
        let root = self.current.clone().unwrap();
        let observed_root = root.clone();
        self.address_reader(ReaderGroup::Left);
        let left_id = self.session().unwrap().id;
        let left = self.render_reader(ReaderGroup::Left, unsplit, hidden, window, cx);
        let right = ratio.map(|_| {
            self.address_reader(ReaderGroup::Right);
            self.render_reader(ReaderGroup::Right, true, false, window, cx)
        });
        self.address_reader(group);
        let measured = cx.weak_entity();
        let mut panels = div()
            .id("editor-groups")
            .relative()
            .flex()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .on_prepaint(move |bounds, window, cx| {
                let width = theme::logical_px(bounds.size.width);
                let _ = measured.update(cx, |app, cx| {
                    if app.current.as_ref() != Some(&observed_root) {
                        return;
                    }
                    let Some(session) = app.session_mut() else {
                        return;
                    };
                    if session.reader_width == Some(width) {
                        return;
                    }
                    session.reader_width = Some(width);
                    if session.focused_group == ReaderGroup::Right
                        && visible_ratio(width, 0.5).is_none()
                    {
                        app.focus_reader_group(ReaderGroup::Left, window, cx);
                    }
                    cx.notify();
                });
            });
        if let (Some(ratio), Some(right)) = (ratio, right) {
            let drag = GroupDivider {
                root,
                reader: left_id,
            };
            panels = panels
                .on_drag_move(
                    cx.listener(|app, event: &DragMoveEvent<GroupDivider>, _, cx| {
                        let drag = event.drag(cx);
                        if app.current.as_ref() != Some(&drag.root) {
                            return;
                        }
                        let Some(session) = app.session_mut() else {
                            return;
                        };
                        if !session.group_is_current(ReaderGroup::Left, drag.reader) {
                            return;
                        }
                        let width = theme::logical_px(event.bounds.size.width);
                        let x = theme::logical_px(event.event.position.x - event.bounds.left());
                        if let Some(ratio) = visible_ratio(width, x / (width - DIVIDER))
                            && let Some(split) = &mut session.saved.split
                        {
                            split.ratio = ratio;
                            session.reader_resizing = true;
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_group_resize))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::finish_group_resize))
                .child(
                    div()
                        .flex_none()
                        .min_w_0()
                        .h_full()
                        .w(ui_px((width - DIVIDER) * ratio))
                        .child(left),
                )
                .child(
                    div()
                        .flex_none()
                        .h_full()
                        .w(ui_px(DIVIDER))
                        .bg(Palette::new(cx).rgb(LINE)),
                )
                .child(div().flex_1().min_w_0().h_full().child(right))
                .child(
                    div()
                        .id("editor-divider-semantics")
                        .role(Role::Splitter)
                        .accessibility_id("editor-group-divider")
                        .aria_label("편집기 그룹 너비 조절")
                        .absolute()
                        .top_0()
                        .left(ui_px((width - DIVIDER) * ratio))
                        .h_full()
                        .w(ui_px(DIVIDER))
                        .child(
                            resize_handle("editor-group-divider", Axis::Horizontal)
                                .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())),
                        ),
                );
        } else {
            panels = panels.child(left);
        }
        panels.into_any_element()
    }

    fn finish_group_resize(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self
            .session_mut()
            .is_some_and(|s| std::mem::take(&mut s.reader_resizing))
        {
            self.persist(cx);
        }
    }
}

pub(super) fn place_cursor(
    editor: &mut EditorState,
    position: Position,
    focus: bool,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) {
    if focus {
        editor.set_cursor_position(position, window, cx);
    } else {
        let byte = editor.text().position_to_offset(&position);
        editor.set_selected_range(byte..byte, cx);
    }
}

impl Pokarak {
    pub(super) fn activate_tab_group(
        &mut self,
        group: ReaderGroup,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.session()
            .is_some_and(|s| s.group_is_current(group, id))
            && self.activate_reader_group(group, window, cx)
    }

    pub(super) fn reload_peer_reader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_history.active {
            return;
        }
        if let Some(id) = self.session().and_then(|s| s.peer.as_ref()).map(|p| p.id) {
            self.with_reader(id, |app| {
                if let Some(path) = app.selected() {
                    app.open_file(path, false, None, window, cx);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Path, PathBuf, ReaderGroup, ReadingPoint, SavedPosition, SavedView, Session, visible_ratio,
    };
    use serde_json::json;

    fn legacy_reader() -> SavedView {
        serde_json::from_value(json!({
            "tabs": ["src/main.rs", "src/lib.rs"], "selected": "src/main.rs",
            "code": false, "line": 27, "column": 8, "diff_row": 12,
            "scroll_x": -31.5, "scroll_y": -450.25,
            "positions": { "src/main.rs": { "line": 27, "column": 8, "diff_row": 12 } },
            "hide_sidebar": true
        }))
        .unwrap()
    }

    fn reading(path: &str, line: u32, split: bool) -> ReadingPoint {
        ReadingPoint {
            path: path.into(),
            line,
            column: 7,
            scroll_x: -14.,
            scroll_y: -240.,
            code: false,
            split_diff: split,
            row: 19,
            result_tab: false,
            unified_offset: Some([-7.5, -309.25]),
            side_offset: Some([19.5, -419.]),
        }
    }

    #[test]
    fn legacy_reader_round_trips_and_saving_from_right_keeps_left_fields() {
        let mut session = Session::new(legacy_reader());
        assert!(session.saved_view().split.is_none());
        assert!(session.peer.is_none());
        assert_eq!(session.active_group, ReaderGroup::Left);
        let left = session.id;
        session.ensure_split(true);
        session.saved.split.as_mut().unwrap().ratio = 0.63;
        session.swap_readers();
        session.focused_group = ReaderGroup::Right;
        assert_ne!(left, session.id);
        session.saved.tabs = vec!["tests/reader.rs".into()];
        session.saved.selected = session.saved.tabs.first().cloned();
        session.saved.line = 91;
        session.saved.column = 14;
        session.saved.reader_split_diff = Some(false);
        session.saved.positions.insert(
            "tests/reader.rs".into(),
            SavedPosition {
                line: 91,
                column: 14,
                scroll_x: -19.5,
                scroll_y: -349.25,
                unified_offset: Some([-7.5, -309.25]),
                side_offset: Some([19.5, -419.]),
                ..Default::default()
            },
        );
        let json = serde_json::to_value(session.saved_view()).unwrap();
        assert_eq!(json["selected"], "src/main.rs");
        assert_eq!(json["line"], 27);
        assert_eq!(json["column"], 8);
        assert_eq!(json["diff_row"], 12);
        assert_eq!(json["scroll_x"], -31.5);
        assert_eq!(json["reader_split_diff"], true);
        assert_eq!(json["split"]["selected"], "tests/reader.rs");
        assert_eq!(json["split"]["reader_split_diff"], false);
        assert!(json.get("active_group").is_none() && json.get("focused_group").is_none());
        let mut restored = Session::new(serde_json::from_value(json.clone()).unwrap());
        assert_eq!(restored.focused_group, ReaderGroup::Left);
        assert!(restored.restoring);
        restored.swap_readers();
        assert!(restored.restoring);
        assert_eq!(restored.saved.tabs, [PathBuf::from("tests/reader.rs")]);
        assert_eq!(
            restored.saved.positions[Path::new("tests/reader.rs")].unified_offset,
            Some([-7.5, -309.25])
        );
        assert_eq!(
            restored.saved.positions[Path::new("tests/reader.rs")].side_offset,
            Some([19.5, -419.])
        );
        assert_eq!(restored.saved.split.as_ref().unwrap().ratio, 0.63);
        assert_eq!(serde_json::to_value(restored.saved_view()).unwrap(), json);
    }

    #[test]
    fn definition_history_and_preview_belong_to_the_originating_group() {
        let mut session = Session::new(legacy_reader());
        session.ensure_split(false);
        let caller = reading("src/main.rs", 17, true);
        let definition = reading("src/lib.rs", 28, false);
        session.push_history(caller.clone());
        session.preview_tab = Some(definition.path.clone());
        session.saved.selected = Some(definition.path.clone());
        session.swap_readers();
        session.saved.tabs = vec!["tests/main.rs".into()];
        session.saved.selected = session.saved.tabs.first().cloned();
        session.preview_tab = session.saved.selected.clone();
        assert!(session.history_target(true, None).is_none());
        session.swap_readers();
        assert_eq!(session.preview_tab.as_ref(), Some(&definition.path));
        let back = session
            .history_target(true, Some(definition.clone()))
            .unwrap();
        assert_eq!(back.path, caller.path);
        assert_eq!((back.line, back.column, back.scroll_y), (17, 7, -240.));
        assert!(back.split_diff);
        let forward = session.history_target(false, Some(back)).unwrap();
        assert_eq!(forward.path, definition.path);
        assert!(!forward.split_diff);
        assert_eq!(forward.unified_offset, Some([-7.5, -309.25]));
        session.swap_readers();
        assert_eq!(
            session.saved.selected.as_deref(),
            Some(Path::new("tests/main.rs"))
        );
        assert_eq!(session.saved.tabs, [PathBuf::from("tests/main.rs")]);
        assert_eq!(session.preview_tab, session.saved.selected);
        assert!(session.forward.is_empty() && session.history.is_empty());
    }

    #[test]
    fn closing_either_last_tab_keeps_survivor_and_rejects_retired_reader() {
        for closing in [ReaderGroup::Left, ReaderGroup::Right] {
            let mut session = Session::new(legacy_reader());
            session.saved.tabs = vec!["src/main.rs".into()];
            session.ensure_split(false);
            session.swap_readers();
            session.saved.tabs = vec!["tests/main.rs".into()];
            session.saved.selected = session.saved.tabs.first().cloned();
            if closing == ReaderGroup::Left {
                session.swap_readers();
            }
            let retired = session.id;
            let survivor = session.peer.as_ref().unwrap().id;
            let path = session.saved.selected.clone().unwrap();
            assert!(session.remove_tab(&path));
            assert!(session.saved.tabs.is_empty() && session.saved.selected.is_none());
            session.swap_readers();
            let tabs = session.saved.tabs.clone();
            session.finish_empty_group();
            assert_eq!(session.group_for_id(retired), None);
            assert_eq!(session.group_for_id(survivor), Some(ReaderGroup::Left));
            assert_eq!(session.focused_group, ReaderGroup::Left);
            assert!(session.saved.split.is_none());
            assert_eq!(session.saved.tabs, tabs);
            assert_eq!(session.saved.selected, tabs.first().cloned());
            session.ensure_split(false);
            assert!(session.group_for_id(retired).is_none());
        }
    }

    #[test]
    fn actual_reader_width_clamps_both_groups_without_overwriting_user_ratio() {
        let mut session = Session::new(legacy_reader());
        session.ensure_split(false);
        let split = session.saved.split.as_mut().unwrap();
        split.ratio = 0.7;
        for (width, expected) in [
            (650., None),
            (720., None),
            (721., Some(0.5)),
            (1601., Some(0.7)),
        ] {
            assert_eq!(visible_ratio(width, split.ratio), expected);
        }
        assert_eq!(visible_ratio(801., split.ratio), Some(0.55));
        assert_eq!(split.ratio, 0.7);
    }
}
