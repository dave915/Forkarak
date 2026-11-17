//! Native anchored navigation and grouped source excerpts, shared by all placements.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    Popover,
    Files,
    Bottom,
}
impl Surface {
    pub(super) fn from_option(value: &str) -> Self {
        match value {
            "files" => Self::Files,
            "bottom" => Self::Bottom,
            _ => Self::Popover,
        }
    }
}

pub(super) enum Row {
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
fn match_range(text: &str, column: u32, query: Option<&str>) -> Option<std::ops::Range<usize>> {
    if let Some(query) = query
        && !query.is_empty()
    {
        return text.find(query).map(|at| at..at + query.len());
    }
    let start = utf16_byte(text, column);
    let end = text[start..]
        .char_indices()
        .take_while(|(_, ch)| ch.is_alphanumeric() || *ch == '_')
        .last()
        .map_or(start, |(at, ch)| start + at + ch.len_utf8());
    (end > start).then_some(start..end)
}

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
        let mut visible: BTreeMap<u32, (String, Option<usize>, u32)> = BTreeMap::new();
        let mut targets: BTreeMap<u32, Vec<(usize, u32)>> = BTreeMap::new();
        for (index, hit) in group {
            targets
                .entry(hit.line)
                .or_default()
                .push((index, hit.column));
            let start = hit.line.saturating_sub(2);
            let end = (hit.line as usize + 3).min(lines.len());
            for row in start as usize..end {
                visible
                    .entry(row as u32)
                    .or_insert_with(|| (lines[row].to_owned(), None, 0));
            }
            let text = lines
                .get(hit.line as usize)
                .map(|s| (*s).to_owned())
                .unwrap_or_else(|| hit.preview.clone());
            visible.insert(hit.line, (text, Some(index), hit.column));
        }
        let mut previous = None;
        for (line, (text, index, column)) in visible {
            if previous.is_some_and(|p| line > p + 1) {
                rows.push(Row::Gap);
            }
            previous = Some(line);
            let syntax = full_syntax
                .clone()
                .unwrap_or_else(|| Arc::new(syntax::CodeSyntax::new(&path, &text)));
            let syntax_line = if full_syntax.is_some() {
                line as usize
            } else {
                0
            };
            let candidates: Vec<_> = targets
                .get(&line)
                .map(|hits| hits.iter().map(|(i, col)| (Some(*i), *col)).collect())
                .unwrap_or_else(|| vec![(index, column)]);
            for (index, column) in candidates {
                let highlight = index.and_then(|_| match_range(&text, column, query));
                rows.push(Row::Line {
                    location: SourceLocation {
                        path: path.clone(),
                        line,
                        column,
                        preview: text.clone(),
                    },
                    text: text.clone(),
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
        let query = self
            .result_title
            .starts_with("본문 검색:")
            .then(|| self.filter.clone());
        self.result_rows = Arc::new(prepare_rows(
            &hits,
            &self.result_revisions,
            query.as_deref(),
            false,
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
            prepare_rows_with_sources(&hits, &revisions, query.as_deref(), true, &sources)
        });
        cx.spawn(async move |view, cx| {
            let rows = task.await;
            let _ = view.update(cx, |app, cx| {
                if app.current.as_ref() == Some(&root)
                    && app.navigation_request == request
                    && Arc::ptr_eq(&app.results, &results)
                    && guards.iter().all(|s| s.is_current())
                {
                    app.result_rows = Arc::new(rows);
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
        self.result_surface = surface;
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
                        if self.results.len() >= 300 {
                            " · 최대 300개 표시"
                        } else {
                            ""
                        }
                    )),
            )
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
                    .tooltip("결과 닫기 · 파일 목록으로 돌아가기")
                    .on_click(cx.listener(|app, _, w, cx| {
                        app.results_open = false;
                        app.focus.focus(w, cx);
                        cx.notify();
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
                } else {
                    "결과가 없습니다."
                })
                .into_any_element()
        } else {
            uniform_list("grouped-source-results", rows.len(), move |range, _, cx| {
                range
                    .map(|index| match &rows[index] {
                        Row::File(path) => div()
                            .h(ui_px(24.))
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
                            .h(ui_px(24.))
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
                            let hit_location = location.clone();
                            let expected = scope.clone();
                            let model = rows.clone();
                            let accessible_target = weak.clone();
                            let accessible_location = location.clone();
                            let accessible_scope = scope.clone();
                            let accessible_model = rows.clone();
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
                                    let _ = accessible_target.update(cx, |app, cx| {
                                        if app.current.as_ref() == Some(&accessible_scope)
                                            && Arc::ptr_eq(&app.result_rows, &accessible_model)
                                        {
                                            app.open_location(
                                                accessible_location.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    });
                                })
                                .h(ui_px(24.))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .font_family("JetBrains Mono")
                                .text_size(ui_text(11.))
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
                                .child(div().min_w_0().text_ellipsis().child(styled))
                                .on_click(move |_, w, cx| {
                                    let _ = target.update(cx, |app, cx| {
                                        if app.current.as_ref() == Some(&expected)
                                            && app.results_root.as_ref() == Some(&expected)
                                            && Arc::ptr_eq(&app.result_rows, &model)
                                        {
                                            app.open_location(hit_location.clone(), w, cx);
                                        }
                                    });
                                })
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            })
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
        assert_eq!(match_range("😀 total()", 3, None), Some(5..10));
        assert_eq!(match_range("한 total total", 0, Some("total")), Some(4..9));
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
