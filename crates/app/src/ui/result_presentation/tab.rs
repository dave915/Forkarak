//! In-memory reader tab, history and rerun at the originating source.
use super::*;

/// This tab belongs to a live worktree session. Its path never enters SavedView.
#[derive(Clone)]
pub(in crate::ui) struct NavigationTab {
    label: String,
    title: String,
    query: reader_navigation::Query,
    results: Arc<[ResultExcerpt]>,
    revisions: HashMap<PathBuf, FileRevision>,
    sources: HashMap<PathBuf, pokarak_navigation::DocumentSnapshot>,
    rows: Option<Arc<Vec<Row>>>,
    anchor: Option<Bounds<Pixels>>,
    cursor: usize,
    surface: Surface,
}

impl NavigationTab {
    pub(super) fn cache_rows(&mut self, results: &Arc<[ResultExcerpt]>, rows: &Arc<Vec<Row>>) {
        if Arc::ptr_eq(results, &self.results) {
            self.rows = Some(rows.clone());
        }
    }
}

impl Session {
    pub(super) fn forget_result_tab(&mut self) {
        self.result_tab = None;
        self.history.retain(|point| !point.result_tab);
        self.forward.retain(|point| !point.result_tab);
    }

    pub(in crate::ui) fn push_history(&mut self, point: ReadingPoint) {
        self.history.push(point);
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.forward.clear();
    }
}

impl Pokarak {
    pub(in crate::ui) fn clear_result_surface(&mut self) {
        self.results_open = false;
        self.results_root = None;
        self.results = Arc::default();
        self.result_rows = Arc::default();
        self.result_revisions.clear();
        self.restore_result_sources(HashMap::new());
        self.result_query = None;
        self.rerun_after_read = None;
        self.rerun_placement = None;
        self.result_semantic = false;
    }

    pub(in crate::ui) fn result_tab_active(&self) -> bool {
        self.results_open
            && self
                .session()
                .is_some_and(|s| s.active_group == s.focused_group)
            && self.result_surface == Surface::Editor
            && self.current.is_some()
            && self.results_root == self.current
            && !self.git_history.active
    }

    pub(in crate::ui) fn remember_result_tab(&mut self, previous: Option<ReadingPoint>) {
        let Some(query) = self.result_query.clone() else {
            return;
        };
        let root = query.source.root();
        if self.current.as_deref() != Some(root) {
            return;
        }
        let name = query
            .symbol()
            .or_else(|| query.source.path().file_name()?.to_str())
            .unwrap_or_default();
        let tab = NavigationTab {
            label: format!("{} · {name}", query.tool.title()),
            query: query.clone(),
            title: self.result_title.clone(),
            results: self.results.clone(),
            revisions: self.result_revisions.clone(),
            sources: self.semantic_result_sources(),
            rows: None,
            anchor: self.result_anchor,
            cursor: self.result_cursor,
            surface: self.result_surface,
        };
        if let Some(session) = self.sessions.get_mut(root) {
            session.forget_result_tab();
            if let Some(point) = previous.filter(|p| !p.result_tab) {
                session.push_history(point);
            }
            session.result_tab = Some(tab);
        }
    }

    pub(in crate::ui) fn scroll_to_result_cursor(&self) {
        if let Some(row) = self.result_rows.iter().position(
            |row| matches!(row, Row::Line {hit: Some(hit), ..} if *hit == self.result_cursor),
        ) {
            self.result_scroll
                .scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    pub(in crate::ui) fn remember_result_cursor(&mut self) {
        if let Some(tab) = self
            .current
            .as_ref()
            .and_then(|r| self.sessions.get_mut(r))
            .and_then(|s| s.result_tab.as_mut())
            && Arc::ptr_eq(&tab.results, &self.results)
        {
            tab.cursor = self.result_cursor;
        }
    }

    pub(in crate::ui) fn activate_result_tab(
        &mut self,
        record_history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.session().and_then(|s| s.result_tab.clone()) else {
            return;
        };
        if self.opening_path.is_some() || self.git_history.active {
            return;
        }
        if record_history
            && !self.result_tab_active()
            && tab.surface == Surface::Editor
            && let Some(point) = self.reading_point(cx)
            && let Some(session) = self.sessions.get_mut(tab.query.source.root())
        {
            session.push_history(point);
        }
        self.navigation_request += 1;
        self.rerun_after_read = None;
        self.results_root = self.current.clone();
        self.result_query = Some(tab.query);
        self.result_title = tab.title;
        self.results = tab.results;
        self.result_cursor = tab.cursor;
        self.result_surface = tab.surface;
        self.result_semantic = true;
        self.result_revisions = tab.revisions;
        self.restore_result_sources(tab.sources);
        self.result_skipped_paths.clear();
        self.results_open = true;
        self.analysis_feedback.output_open = false;
        self.result_anchor = tab.anchor;
        if let Some(rows) = tab.rows {
            self.result_rows = rows;
        } else {
            self.prepare_result_rows(cx);
        }
        self.scroll_to_result_cursor();
        if !self.semantic_result_sources_current(cx) {
            self.status = "저장된 탐색 결과입니다. 현재 소스에서 다시 탐색하세요.".into();
        }
        self.results_focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::ui) fn result_tab_button(
        &self,
        tab: &NavigationTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = tab.results.clone();
        let closing = model.clone();
        div()
            .flex_none()
            .flex()
            .items_center()
            .child(
                Button::new("navigation-result-tab")
                    .label(tab.label.clone())
                    .max_w(ui_px(230.))
                    .tooltip(tab.query.source.path().display().to_string())
                    .ghost()
                    .small()
                    .selected(self.result_tab_active())
                    .on_click(cx.listener(move |app, _, w, cx| {
                        if app.is_current_tab(&model) {
                            app.activate_result_tab(true, w, cx);
                        }
                    })),
            )
            .child(
                Button::new("close-navigation-tab")
                    .icon(toolbar::toolbar_glyph(IconName::X))
                    .ghost()
                    .small()
                    .accessibility_label("탐색 결과 탭 닫기")
                    .on_click(cx.listener(move |app, _, w, cx| {
                        if app.is_current_tab(&closing) {
                            app.close_result_tab(w, cx);
                        }
                    })),
            )
    }

    fn is_current_tab(&self, model: &Arc<[ResultExcerpt]>) -> bool {
        self.opening_path.is_none()
            && self
                .session()
                .and_then(|s| s.result_tab.as_ref())
                .is_some_and(|tab| Arc::ptr_eq(model, &tab.results))
    }

    pub(in crate::ui) fn close_result_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.current.as_ref().and_then(|r| self.sessions.get_mut(r)) {
            let visible = session
                .result_tab
                .as_ref()
                .is_some_and(|tab| Arc::ptr_eq(&tab.results, &self.results))
                && self.results_open;
            session.forget_result_tab();
            if visible {
                self.dismiss_results(window, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::ui) fn dismiss_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.results_open = false;
        self.navigation_request += 1;
        if let Some(editor) = self
            .active_editor()
            .filter(|_| self.session().is_some_and(|s| s.saved.code))
        {
            editor.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn rerun_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.result_query.clone() else {
            return;
        };
        if self.current.as_deref() != Some(query.source.root())
            || self.loading
            || self.opening_path.is_some()
            || self.file_loading
            || self.git_history.active
            || self.snapshot().is_none()
        {
            return;
        }
        let Ok(path) = query.source.path().strip_prefix(query.source.root()) else {
            return;
        };
        self.open_file(path.to_owned(), true, None, window, cx);
        self.rerun_after_read = Some((self.file_request, query));
    }

    pub(in crate::ui) fn run_result_query(
        &mut self,
        query: reader_navigation::Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(root), Some(path), Some(editor)) =
            (&self.current, self.selected(), self.active_editor())
        else {
            return;
        };
        if !query.matches_origin(root, &path, editor.read(cx).value().as_ref()) {
            self.error = Some(
                "탐색을 시작한 소스가 변경되었습니다. 코드에서 심볼을 다시 선택하세요.".into(),
            );
            cx.notify();
            return;
        }
        let Some(position) =
            reader_navigation::editor_position(editor.read(cx).text(), query.position)
        else {
            return;
        };
        if let Some(session) = self.sessions.get_mut(root) {
            session.saved.code = true;
        }
        editor.update(cx, |e, cx| e.set_cursor_position(position, window, cx));
        let previous_request = self.navigation_request;
        self.request_language(query.tool, Some(query.position), window, cx);
        if self.navigating && self.navigation_request != previous_request {
            self.rerun_placement = Some((self.navigation_request, self.result_surface));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Arc, HashMap, NavigationKind, NavigationTab, Position, ReadingPoint, Row, SavedView,
        Session, Surface, language_tools, reader_navigation,
    };

    #[test]
    fn only_the_owning_worktree_and_result_set_keep_tab_rows_and_history() {
        let temp = tempfile::tempdir().unwrap();
        let make_session = |name: &str| {
            let root = temp.path().canonicalize().unwrap().join(name);
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("same.ts"), "value()").unwrap();
            let query = reader_navigation::Query {
                source: pokarak_navigation::DocumentSnapshot::new(
                    root,
                    "same.ts".into(),
                    "value()".into(),
                    1,
                )
                .unwrap(),
                position: Position::new(0, 0),
                tool: language_tools::Tool::Navigate(NavigationKind::References),
            };
            let mut session = Session::new(SavedView::default());
            session.result_tab = Some(NavigationTab {
                label: "사용처 · value".into(),
                title: "사용처".into(),
                query,
                results: Vec::new().into(),
                revisions: HashMap::new(),
                sources: HashMap::new(),
                rows: None,
                anchor: None,
                cursor: 0,
                surface: Surface::Editor,
            });
            session
        };
        let mut first = make_session("first");
        let second = make_session("second");
        let tab = first.result_tab.as_mut().unwrap();
        let rows = Arc::new(vec![Row::Gap]);
        tab.cache_rows(&second.result_tab.as_ref().unwrap().results, &rows);
        assert!(tab.rows.is_none());
        tab.cache_rows(&tab.results.clone(), &rows);
        assert!(Arc::ptr_eq(tab.rows.as_ref().unwrap(), &rows));
        let file = ReadingPoint {
            path: "same.ts".into(),
            result_tab: false,
            split_diff: false,
            unified_offset: None,
            side_offset: None,
            code: true,
            row: 0,
            line: 0,
            column: 3,
            scroll_x: 0.,
            scroll_y: 0.,
        };
        let results = ReadingPoint {
            result_tab: true,
            ..file.clone()
        };
        first.push_history(file.clone());
        first.push_history(results.clone());
        first.forward.push(results);
        first.forget_result_tab();
        assert!(first.result_tab.is_none());
        assert!(second.result_tab.is_some());
        assert_eq!(first.history.len(), 1);
        assert_eq!(first.history[0].column, file.column);
        assert!(first.forward.is_empty());
        assert!(first.saved.tabs.is_empty());
        assert!(second.saved.tabs.is_empty());
    }
}
