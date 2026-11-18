//! Native anchored navigation and grouped source excerpts, shared by all placements.
mod tab;
use super::*;
use std::collections::BTreeMap;
pub(super) use tab::NavigationTab;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Surface {
    Editor,
    Popover,
    Files,
    Bottom,
}
impl Surface {
    pub(super) fn from_option(value: &str) -> Self {
        match value {
            "editor" => Self::Editor,
            "files" => Self::Files,
            "bottom" => Self::Bottom,
            _ => Self::Popover,
        }
    }

    // Side and bottom panels can stay visible beside a direct definition jump.
    pub(super) fn visible_on_arrival(self, foreground: bool) -> bool {
        foreground || matches!(self, Self::Files | Self::Bottom)
    }

    pub(super) fn for_results(prefs: &Preferences, project: Option<&Path>, semantic: bool) -> Self {
        let placement = prefs.option_string(
            project,
            "navigation.results_placement",
            if semantic { "editor" } else { "popover" },
        );
        match (Self::from_option(&placement), semantic) {
            (Self::Editor, false) => Self::Popover,
            (surface, _) => surface,
        }
    }
}

pub(super) enum Row {
    Section(reader_navigation::ResultGroup, usize),
    File(PathBuf),
    Gap,
    Line {
        location: SourceLocation,
        text: String,
        hit: Option<usize>,
        highlight: Option<std::ops::Range<usize>>,
        syntax: Arc<syntax::CodeSyntax>,
        syntax_line: usize,
    },
}

fn utf16_byte(text: &str, column: u32) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units >= column {
            return offset;
        }
        units += ch.len_utf16() as u32;
    }
    text.len()
}
fn match_range(text: &str, start: usize, query: Option<&str>) -> Option<std::ops::Range<usize>> {
    if let Some(query) = query
        && !query.is_empty()
    {
        return text
            .get(start..)?
            .starts_with(query)
            .then_some(start..start + query.len());
    }
    let end = text[start..]
        .char_indices()
        .take_while(|(_, ch)| ch.is_alphanumeric() || matches!(ch, '_' | '$'))
        .last()
        .map_or(start, |(at, ch)| start + at + ch.len_utf8());
    (end > start).then_some(start..end)
}

#[cfg(test)]
fn prepare_rows(
    hits: &[SourceLocation],
    revisions: &HashMap<PathBuf, FileRevision>,
    query: Option<&str>,
    context: bool,
) -> Vec<Row> {
    prepare_rows_with_sources(hits, revisions, query, context, &HashMap::new())
}

fn prepare_rows_with_sources(
    hits: &[SourceLocation],
    revisions: &HashMap<PathBuf, FileRevision>,
    query: Option<&str>,
    context: bool,
    sources: &HashMap<PathBuf, pokarak_navigation::DocumentSnapshot>,
) -> Vec<Row> {
    let mut groups: BTreeMap<PathBuf, Vec<(usize, &SourceLocation)>> = BTreeMap::new();
    for (index, hit) in hits.iter().enumerate() {
        groups
            .entry(hit.path.clone())
            .or_default()
            .push((index, hit));
    }
    let mut rows = vec![];
    let mut context_budget = 8 * 1024 * 1024;
    for (path, group) in groups {
        rows.push(Row::File(path.clone()));
        let source = context
            .then(|| {
                if let Some(snapshot) = sources.get(&path) {
                    // Never read disk for a path whose semantic positions came
                    // from a retained draft, even after that snapshot expires.
                    return snapshot.is_current().then(|| snapshot.text().to_owned());
                }
                let expected = revisions.get(&path)?;
                if expected.0 > 2 * 1024 * 1024
                    || expected.0 > context_budget
                    || file_revision(&path).as_ref() != Some(expected)
                {
                    return None;
                }
                let source = std::fs::read_to_string(&path).ok()?;
                if file_revision(&path).as_ref() != Some(expected) {
                    return None;
                }
                context_budget -= expected.0;
                Some(source)
            })
            .flatten();
        let full_syntax = source
            .as_deref()
            .map(|text| Arc::new(syntax::CodeSyntax::new(&path, text)));
        let lines: Vec<_> = source
            .as_deref()
            .map(|s| s.lines().collect())
            .unwrap_or_default();
        // Each row keeps its own search hit, including its provider-supplied excerpt.
        let mut visible: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (index, hit) in group {
            let end = (hit.line as usize + 3).min(lines.len());
            for row in hit.line.saturating_sub(2) as usize..end {
                visible.entry(row as u32).or_default();
            }
            visible.entry(hit.line).or_default().push(index);
        }
        let mut previous = None;
        for (line, targets) in visible {
            if previous.is_some_and(|p| line > p + 1) {
                rows.push(Row::Gap);
            }
            previous = Some(line);
            let source_line = lines.get(line as usize).copied();
            let context_only = targets.is_empty().then_some(None);
            for index in targets.into_iter().map(Some).chain(context_only) {
                let hit = index.map(|i| &hits[i]);
                let column = hit.map_or(0, |hit| hit.column);
                let text = source_line
                    .or_else(|| hit.map(|hit| hit.preview.as_str()))
                    .unwrap_or_default();
                let syntax = full_syntax
                    .clone()
                    .unwrap_or_else(|| Arc::new(syntax::CodeSyntax::new(&path, text)));
                let syntax_line = if full_syntax.is_some() {
                    line as usize
                } else {
                    0
                };
                let highlight = hit.and_then(|_| {
                    let mut start = utf16_byte(text, column);
                    if source_line.is_none() && query.is_some() {
                        // Core literal excerpts retain up to 61 Unicode characters before the hit.
                        if let Some((end, _)) = text.char_indices().nth(61) {
                            start = start.min(end);
                        }
                    }
                    match_range(text, start, query)
                });
                rows.push(Row::Line {
                    location: SourceLocation {
                        path: path.clone(),
                        line,
                        column,
                        preview: text.to_owned(),
                    },
                    text: text.to_owned(),
                    hit: index,
                    highlight,
                    syntax: syntax.clone(),
                    syntax_line,
                });
            }
        }
    }
    rows
}

fn prepare_grouped_rows(
    hits: &[SourceLocation],
    groups: &[Option<reader_navigation::ResultGroup>],
    revisions: &HashMap<PathBuf, FileRevision>,
    query: Option<&str>,
    context: bool,
    sources: &HashMap<PathBuf, pokarak_navigation::DocumentSnapshot>,
) -> Vec<Row> {
    let mut rows = vec![];
    let mut start = 0;
    while start < hits.len() {
        let group = groups[start];
        let end = (start + 1..hits.len())
            .find(|i| groups[*i] != group)
            .unwrap_or(hits.len());
        if let Some(group) = group {
            rows.push(Row::Section(group, end - start));
        }
        let mut section =
            prepare_rows_with_sources(&hits[start..end], revisions, query, context, sources);
        for row in &mut section {
            if let Row::Line {
                hit: Some(index), ..
            } = row
            {
                *index += start;
            }
        }
        rows.extend(section);
        start = end;
    }
    rows
}

fn merged_highlights(
    text: &str,
    syntax: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    matched: Option<std::ops::Range<usize>>,
    tint: Hsla,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let mut boundaries = vec![0, text.len()];
    for (range, _) in &syntax {
        if text.get(range.clone()).is_some() {
            boundaries.extend([range.start, range.end]);
        }
    }
    if let Some(range) = &matched {
        boundaries.extend([range.start, range.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
        .windows(2)
        .filter_map(|pair| {
            let range = pair[0]..pair[1];
            if range.is_empty() {
                return None;
            }
            let mut style = syntax
                .iter()
                .find(|(span, _)| span.start <= range.start && span.end >= range.end)
                .map(|(_, style)| *style)
                .unwrap_or_default();
            if matched
                .as_ref()
                .is_some_and(|m| m.start <= range.start && m.end >= range.end)
            {
                style.background_color = Some(tint);
                style.font_weight = Some(FontWeight::BOLD);
            }
            Some((range, style))
        })
        .collect()
}

impl Pokarak {
    pub(super) fn prepare_result_rows(&mut self, cx: &mut Context<Self>) {
        let hits: Vec<_> = self.results.iter().map(|r| r.location.clone()).collect();
        let groups: Vec<_> = self.results.iter().map(|r| r.group).collect();
        let query = self
            .result_title
            .strip_prefix("본문 검색: ")
            .map(str::to_owned);
        self.result_rows = Arc::new(prepare_grouped_rows(
            &hits,
            &groups,
            &self.result_revisions,
            query.as_deref(),
            false,
            &HashMap::new(),
        ));
        let Some(root) = self.current.clone() else {
            return;
        };
        let request = self.navigation_request;
        let results = self.results.clone();
        let revisions = self.result_revisions.clone();
        let sources = if self.result_semantic {
            self.semantic_result_sources()
        } else {
            HashMap::new()
        };
        let guards: Vec<_> = sources.values().cloned().collect();
        let task = cx.background_spawn(async move {
            prepare_grouped_rows(&hits, &groups, &revisions, query.as_deref(), true, &sources)
        });
        cx.spawn(async move |view, cx| {
            let rows = task.await;
            let _ = view.update(cx, |app, cx| {
                if app.current.as_ref() == Some(&root)
                    && app.navigation_request == request
                    && Arc::ptr_eq(&app.results, &results)
                    && guards.iter().all(|s| s.is_current())
                    && (!app.result_semantic || app.semantic_result_sources_current(cx))
                {
                    app.result_rows = Arc::new(rows);
                    if let Some(tab) = app
                        .sessions
                        .get_mut(&root)
                        .and_then(|s| s.result_tab.as_mut())
                    {
                        tab.cache_rows(&results, &app.result_rows);
                    }
                    app.scroll_to_result_cursor();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn move_results(&mut self, surface: Surface, window: &mut Window, cx: &mut Context<Self>) {
        if self.results_root != self.current {
            return;
        }
        let previous = self.reading_point(cx);
        let was_editor = self.result_tab_active();
        self.result_surface = surface;
        if surface == Surface::Editor && !was_editor {
            self.remember_result_tab(previous);
        } else if was_editor
            && surface != Surface::Editor
            && let Some(session) = self.current.as_ref().and_then(|r| self.sessions.get_mut(r))
        {
            session.forget_result_tab();
        }
        if surface == Surface::Files
            && let Some(root) = &self.current
            && let Some(s) = self.sessions.get_mut(root)
        {
            s.saved.hide_files = false;
        }
        self.results_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn result_popover(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.results_open
            || self.result_surface != Surface::Popover
            || self.results_root != self.current
            || self.analysis_feedback.output_open
        {
            return None;
        }
        let anchor = self.result_anchor.unwrap_or(Bounds::new(
            point(ui_px(250.), ui_px(140.)),
            size(ui_px(1.), ui_px(1.)),
        ));
        Some(
            deferred(
                anchored()
                    .attach(anchor, Anchor::BottomLeft)
                    .snap_to_window()
                    .child(
                        div()
                            .id("anchored-navigation-results")
                            // Occlude the editor's hitbox, including its capture-phase
                            // scroll handling. Bubble only after the inner list scrolls,
                            // consuming wheel input over the header and list boundaries.
                            .occlude()
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .w(ui_px(650.).min(window.viewport_size().width - ui_px(32.)))
                            .h(ui_px(360.).min(window.viewport_size().height - ui_px(90.)))
                            .border_1()
                            .border_color(Palette::new(cx).rgb(LINE))
                            .shadow_lg()
                            .child(self.render_results(cx))
                            .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                                app.results_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .with_priority(100)
            .into_any_element(),
        )
    }

    pub(super) fn render_result_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let weak = cx.weak_entity();
        let rows = self.result_rows.clone();
        let root = self.current.clone().unwrap_or_default();
        let scope = root.clone();
        let cursor = self.result_cursor;
        let failed = self.result_title == "본문 검색 실패";
        let in_editor = self.result_surface == Surface::Editor;
        let font_size = if in_editor {
            ui_px(self.prefs.display.code_font_size)
        } else {
            ui_text(11.)
        };
        let row_height = ui_px(24.).max(theme::code_row_height(font_size));
        let widest = rows
            .iter()
            .enumerate()
            .max_by_key(|(_, row)| match row {
                Row::File(path) => path.strip_prefix(&root).unwrap_or(path).as_os_str().len(),
                Row::Line { text, .. } => text.len() + 8,
                _ => 0,
            })
            .map(|(index, _)| index);
        let mut header = div()
            .flex_none()
            .min_h(ui_px(32.))
            .px_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_text(11.))
                    .child(format!(
                        "{} · {}{}",
                        self.result_title,
                        self.results.len(),
                        if !self.result_semantic && self.results.len() >= 300 {
                            " · 최대 300개 표시"
                        } else {
                            ""
                        }
                    )),
            )
            .when(self.result_semantic, |header| {
                header
                    .child(
                        Button::new("rerun-navigation")
                            .icon(IconName::RotateCw)
                            .ghost()
                            .xsmall()
                            .tooltip("원래 심볼에서 다시 탐색")
                            .accessibility_label("다시 탐색")
                            .disabled(self.loading || self.file_loading || self.navigating)
                            .on_click(cx.listener(|app, _, w, cx| app.rerun_results(w, cx))),
                    )
                    .child(
                        Button::new("editor-results")
                            .label("편집기 탭")
                            .ghost()
                            .xsmall()
                            .selected(self.result_surface == Surface::Editor)
                            .on_click(cx.listener(|app, _, w, cx| {
                                app.move_results(Surface::Editor, w, cx)
                            })),
                    )
            })
            .child(
                Button::new("pin-result-files")
                    .label("파일 영역에 고정")
                    .ghost()
                    .xsmall()
                    .selected(self.result_surface == Surface::Files)
                    .on_click(cx.listener(|app, _, w, cx| app.move_results(Surface::Files, w, cx))),
            )
            .child(
                Button::new("result-bottom")
                    .label("하단")
                    .ghost()
                    .xsmall()
                    .selected(self.result_surface == Surface::Bottom)
                    .on_click(
                        cx.listener(|app, _, w, cx| app.move_results(Surface::Bottom, w, cx)),
                    ),
            )
            .child(
                Button::new("result-popup")
                    .label("팝오버")
                    .ghost()
                    .xsmall()
                    .selected(self.result_surface == Surface::Popover)
                    .on_click(
                        cx.listener(|app, _, w, cx| app.move_results(Surface::Popover, w, cx)),
                    ),
            )
            .child(
                Button::new("close-results")
                    .icon(IconName::X)
                    .ghost()
                    .xsmall()
                    .tooltip("결과 닫기")
                    .on_click(cx.listener(|app, _, w, cx| {
                        if app.result_tab_active() {
                            app.close_result_tab(w, cx);
                        } else {
                            app.dismiss_results(w, cx);
                        }
                    })),
            );
        if failed {
            header = header.child(
                Button::new("retry-text-search")
                    .label("다시 시도")
                    .small()
                    .on_click(cx.listener(|app, _, w, cx| app.search_contents(w, cx))),
            );
        }
        let body = if rows.is_empty() {
            div()
                .p_3()
                .child(if failed {
                    "검색하지 못했습니다. 오류를 확인하고 다시 시도하세요."
                } else if !self.result_skipped_paths.is_empty() {
                    "읽을 수 있는 파일에서 일치하는 결과를 찾지 못했습니다."
                } else {
                    "결과가 없습니다."
                })
                .into_any_element()
        } else {
            uniform_list("grouped-source-results", rows.len(), move |range, _, cx| {
                range
                    .map(|index| match &rows[index] {
                        Row::Section(group, count) => div()
                            .h(row_height)
                            .px_2()
                            .bg(palette.rgb(PANEL))
                            .text_size(ui_text(11.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{} · {}개", group.label(), count))
                            .into_any_element(),
                        Row::File(path) => div()
                            .h(row_height)
                            .px_2()
                            .bg(palette.rgb(PANEL))
                            .text_size(ui_text(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(
                                path.strip_prefix(&root)
                                    .unwrap_or(path)
                                    .display()
                                    .to_string(),
                            )
                            .into_any_element(),
                        Row::Gap => div()
                            .h(row_height)
                            .px_2()
                            .text_color(palette.rgb(MUTED))
                            .child("⋯")
                            .into_any_element(),
                        Row::Line {
                            location,
                            text,
                            hit,
                            highlight,
                            syntax,
                            syntax_line,
                        } => {
                            let target = weak.clone();
                            let expected = scope.clone();
                            let model = rows.clone();
                            // Pointer and accessibility activation must validate the same rendered row.
                            let open = move |window: &mut Window, cx: &mut App| {
                                let _ = target.update(cx, |app, cx| {
                                    if app.opening_path.is_some()
                                        || !app.results_open
                                        || app.current.as_ref() != Some(&expected)
                                        || !Arc::ptr_eq(&app.result_rows, &model)
                                    {
                                        return;
                                    }
                                    if let Row::Line { location, hit, .. } = &model[index] {
                                        if let Some(hit) = hit {
                                            app.result_cursor = *hit;
                                            app.remember_result_cursor();
                                        }
                                        app.open_location(location.clone(), window, cx);
                                    }
                                });
                            };
                            let accessible_open = open.clone();
                            let spans = merged_highlights(
                                text,
                                syntax.spans(*syntax_line, cx),
                                highlight.clone(),
                                palette.rgb(0x395674).into(),
                            );
                            let styled = StyledText::new(text.clone()).with_highlights(spans);
                            div()
                                .id(("result-source-line", index))
                                .role(accesskit::Role::Button)
                                .aria_label(format!(
                                    "{}:{}:{}",
                                    location.path.display(),
                                    location.line + 1,
                                    location.column + 1
                                ))
                                .on_a11y_action(accesskit::Action::Click, move |_, window, cx| {
                                    accessible_open(window, cx);
                                })
                                .h(row_height)
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .font_family("JetBrains Mono")
                                .min_w_full()
                                .text_size(font_size)
                                .bg(if *hit == Some(cursor) {
                                    palette.rgb(PANEL)
                                } else {
                                    palette.rgb(SIDE)
                                })
                                .cursor_pointer()
                                .child(
                                    div()
                                        .w(ui_px(38.))
                                        .flex_none()
                                        .text_right()
                                        .text_color(palette.rgb(MUTED))
                                        .child((location.line + 1).to_string()),
                                )
                                .child(div().whitespace_nowrap().flex_none().child(styled))
                                .on_click(move |_, w, cx| open(w, cx))
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .with_width_from_item(widest)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .track_scroll(&self.result_scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        div()
            .id("results-panel")
            .size_full()
            .v_flex()
            .track_focus(&self.results_focus)
            .bg(palette.rgb(SIDE))
            .child(header)
            .children(file_tree::skipped_paths_notice(
                "copy-skipped-search",
                &self.result_skipped_paths,
            ))
            .child(
                div()
                    .px_2()
                    .flex_none()
                    .text_size(ui_text(10.))
                    .text_color(palette.rgb(MUTED))
                    .child(if self.result_semantic {
                        "언어 분석 결과 · 파일별 소스 문맥"
                    } else {
                        "텍스트 후보 · 의미상 사용처가 아닐 수 있습니다"
                    }),
            )
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_navigation_and_text_search_respect_independent_and_explicit_placements() {
        use super::Preferences;
        use std::path::Path;
        let mut prefs = Preferences::default();
        let project = Path::new("/fixture");
        assert_eq!(Surface::for_results(&prefs, None, true), Surface::Editor);
        assert_eq!(Surface::for_results(&prefs, None, false), Surface::Popover);
        prefs.set_option(
            None,
            "navigation.results_placement",
            serde_json::json!("files"),
        );
        for (key, placement) in [
            ("popover", Surface::Popover),
            ("bottom", Surface::Bottom),
            ("files", Surface::Files),
            ("editor", Surface::Editor),
        ] {
            prefs.set_option(
                Some(project),
                "navigation.results_placement",
                serde_json::json!(key),
            );
            assert_eq!(Surface::for_results(&prefs, Some(project), true), placement);
            assert_eq!(Surface::for_results(&prefs, None, true), Surface::Files);
            assert_eq!(
                Surface::for_results(&prefs, Some(project), false),
                if placement == Surface::Editor {
                    Surface::Popover
                } else {
                    placement
                }
            );
            assert!(placement.visible_on_arrival(true));
            assert_eq!(
                placement.visible_on_arrival(false),
                matches!(placement, Surface::Files | Surface::Bottom)
            );
        }
    }

    #[test]
    fn definitions_and_usages_in_the_same_file_keep_distinct_sections_and_global_hit_indices() {
        use super::reader_navigation::ResultGroup;
        let path = std::path::PathBuf::from("/fixture/Java.java");
        let hits = vec![
            super::SourceLocation {
                path: path.clone(),
                line: 1,
                column: 4,
                preview: "greet".into(),
            },
            super::SourceLocation {
                path: path.clone(),
                line: 3,
                column: 4,
                preview: "greet".into(),
            },
            super::SourceLocation {
                path,
                line: 3,
                column: 15,
                preview: "greet".into(),
            },
        ];
        let groups = [
            Some(ResultGroup::Definition),
            Some(ResultGroup::Usages),
            Some(ResultGroup::Usages),
        ];
        let rows = super::prepare_grouped_rows(
            &hits,
            &groups,
            &Default::default(),
            None,
            false,
            &Default::default(),
        );
        assert!(matches!(
            rows[0],
            super::Row::Section(ResultGroup::Definition, 1)
        ));
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, super::Row::Section(ResultGroup::Usages, 2)))
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, super::Row::File(_)))
                .count(),
            2
        );
        let indices: Vec<_> = rows
            .iter()
            .filter_map(|r| {
                if let super::Row::Line { hit, .. } = r {
                    *hit
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(indices, [0, 1, 2]);
    }
    #[test]
    fn semantic_excerpts_use_retained_source_and_never_fall_back_to_disk_for_it() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let path = root.join("draft.ts");
        std::fs::write(&path, "const disk = 0;\ndisk();").unwrap();
        let snapshot = pokarak_navigation::DocumentSnapshot::new(
            root.clone(),
            path.clone(),
            "// retained context\nconst dirty = 1;\ndirty();".into(),
            1,
        )
        .unwrap();
        let hit = super::SourceLocation {
            path: path.clone(),
            line: 2,
            column: 0,
            preview: "dirty();".into(),
        };
        let sources = std::collections::HashMap::from([(path.clone(), snapshot.clone())]);
        let revisions = super::result_revisions(std::slice::from_ref(&hit));
        let rows = super::prepare_rows_with_sources(
            std::slice::from_ref(&hit),
            &revisions,
            None,
            true,
            &sources,
        );
        let text = rows
            .iter()
            .filter_map(|r| {
                if let super::Row::Line { text, .. } = r {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("retained context"));
        assert!(text.contains("const dirty"));
        assert!(!text.contains("disk"));
        snapshot.invalidate();
        let rows = super::prepare_rows_with_sources(&[hit], &revisions, None, true, &sources);
        assert!(
            rows.iter()
                .all(|r| !matches!(r,super::Row::Line{text,..} if text.contains("disk")))
        );
    }
    use super::{HashMap, Row, SourceLocation, Surface, match_range, prepare_rows};
    #[test]
    fn grouped_excerpts_merge_nearby_context_and_mark_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(
            &path,
            "one\ntwo\nfn total() {}\nfour\ntotal();\nsix\nseven\neight\nnine\nten\nlast\n",
        )
        .unwrap();
        let hits = vec![
            SourceLocation {
                path: path.clone(),
                line: 2,
                column: 3,
                preview: "fn total() {}".into(),
            },
            SourceLocation {
                path: path.clone(),
                line: 4,
                column: 0,
                preview: "total();".into(),
            },
            SourceLocation {
                path: path.clone(),
                line: 10,
                column: 0,
                preview: "last".into(),
            },
        ];
        let revisions = super::result_revisions(&hits);
        let rows = prepare_rows(&hits, &revisions, None, true);
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::File(_))).count(), 1);
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Gap)).count(), 1);
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, Row::Line { hit: Some(_), .. }))
                .count(),
            3
        );
        assert!(
            rows.iter()
                .any(|r| matches!(r,Row::Line{text,hit:None,..} if text=="four"))
        );
    }
    #[test]
    fn stale_sources_use_provider_preview_without_reading_new_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "old").unwrap();
        let hit = SourceLocation {
            path: path.clone(),
            line: 0,
            column: 0,
            preview: "old".into(),
        };
        let hits = vec![hit];
        let revisions = super::result_revisions(&hits);
        std::fs::write(&path, "changed and longer").unwrap();
        let rows = prepare_rows(&hits, &revisions, None, true);
        assert!(matches!(&rows[1],Row::Line{text,..} if text=="old"));
        assert_eq!(rows.len(), 2);
    }
    #[test]
    fn unicode_match_highlights_are_valid_utf8_ranges() {
        assert_eq!(match_range("😀 total()", 5, None), Some(5..10));
        assert_eq!(
            match_range("한 total total", 10, Some("total")),
            Some(10..15)
        );
        assert!(matches!(Surface::from_option("bottom"), Surface::Bottom));
        assert!(matches!(Surface::from_option("bad"), Surface::Popover));
        let _ = HashMap::<String, String>::new();
    }
    #[test]
    fn chooser_keeps_multiple_targets_on_the_same_source_line() {
        let hits = vec![
            SourceLocation {
                path: "a.rs".into(),
                line: 3,
                column: 0,
                preview: "one two".into(),
            },
            SourceLocation {
                path: "a.rs".into(),
                line: 3,
                column: 4,
                preview: "one two".into(),
            },
        ];
        let rows = prepare_rows(&hits, &HashMap::new(), None, false);
        let targets: Vec<_> = rows
            .iter()
            .filter_map(|r| {
                if let Row::Line {
                    location,
                    hit: Some(index),
                    ..
                } = r
                {
                    Some((*index, location.column))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(targets, vec![(0, 0), (1, 4)]);
    }
    #[test]
    fn repeated_literal_matches_keep_their_columns_and_own_clipped_excerpts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q", "--template=", "--"])
                .arg(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap()
                .success()
        );
        let line = format!("😀 token + token {}token + token", "한😀".repeat(40));
        std::fs::write(root.join("sample.txt"), &line).unwrap();
        let hits = pokarak_core::search(&root, "token", 10).unwrap();
        let ranges = line
            .match_indices("token")
            .map(|(i, _)| i..i + 5)
            .collect::<Vec<_>>();
        assert_eq!(hits.len(), ranges.len());
        for context in [false, true] {
            let rows = prepare_rows(
                &hits,
                &super::result_revisions(&hits),
                Some("token"),
                context,
            );
            let mut checked = 0;
            for row in rows {
                if let Row::Line {
                    text,
                    hit: Some(i),
                    highlight,
                    ..
                } = row
                {
                    let range = highlight.unwrap();
                    assert_eq!(&text[range.clone()], "token");
                    let prefix = &line[..ranges[i].start];
                    let shown = if context {
                        prefix.len()
                    } else {
                        prefix.chars().rev().take(61).map(char::len_utf8).sum()
                    };
                    assert_eq!(range.start, shown);
                    checked += 1;
                }
            }
            assert_eq!(checked, ranges.len());
        }
    }

    #[test]
    fn source_and_match_styles_form_nonoverlapping_utf8_runs() {
        let text = "한 total()";
        let spans = super::merged_highlights(
            text,
            vec![(4..9, super::HighlightStyle::default())],
            Some(4..9),
            super::rgb(0x395674).into(),
        );
        assert_eq!(
            spans.iter().map(|(r, _)| r.len()).sum::<usize>(),
            text.len()
        );
        assert!(
            spans
                .windows(2)
                .all(|pair| pair[0].0.end == pair[1].0.start)
        );
        assert!(spans.iter().all(|(r, _)| text.get(r.clone()).is_some()));
        assert!(
            spans
                .iter()
                .any(|(r, s)| *r == (4..9) && s.background_color.is_some())
        );
    }
}
