use super::*;
use crate::prefs::GraphColumn;
use gpui_kit::base::ElementExt;
use gpui_kit::component::{
    resizable::{HandleEdge, resize_handle},
    scroll::ScrollableElement,
};

use GraphColumn::*;

impl GraphColumn {
    const ALL: [Self; 6] = [Refs, Graph, Message, Author, Date, Hash];

    fn label(self) -> &'static str {
        match self {
            Refs => "브랜치 / 태그",
            Graph => "그래프",
            Message => "커밋 메시지",
            Author => "작성자",
            Date => "날짜",
            Hash => "해시",
        }
    }

    fn clamp(self, width: f32) -> f32 {
        let (min, max) = match self {
            Refs => (64., 700.),
            Graph => (42., 600.),
            Message => (96., 2400.),
            Author => (48., 480.),
            Date => (48., 240.),
            Hash => (48., 300.),
        };
        width.round().clamp(min, max)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Columns {
    widths: [f32; 6],
    pub(super) lane_step: f32,
}

impl Columns {
    const GAP: f32 = 8.;
    // Two insets and a gap between each pair of columns.
    const SPACING: f32 = Self::GAP * (GraphColumn::ALL.len() + 1) as f32;

    pub(super) fn row() -> Div {
        div()
            .px(px(Self::GAP))
            .flex()
            .items_center()
            .gap(px(Self::GAP))
    }

    fn new(viewport: f32, lanes: usize, saved: Option<&SavedView>) -> Self {
        let labels = (viewport * 0.18).clamp(84., 166.).round();
        let graph = (lanes as f32 * 13. + 16.)
            .clamp(50., (viewport * 0.14).clamp(60., 150.))
            .round();
        let author = (viewport * 0.10).clamp(58., 102.).round();
        let date = (viewport * 0.10).clamp(66., 82.).round();
        let hash = (viewport * 0.10).clamp(64., 82.).round();
        let message = (viewport - Self::SPACING - labels - graph - author - date - hash).max(96.);
        let mut widths = [labels, graph, message, author, date, hash];
        for column in GraphColumn::ALL {
            if let Some(width) = saved
                .and_then(|s| s.graph_columns.get(&column))
                .filter(|w| w.is_finite())
            {
                widths[column as usize] = column.clamp(*width);
            }
        }
        let lane_step =
            ((widths[Graph as usize] - 16.) / lanes.saturating_sub(1).max(1) as f32).min(13.);
        Self { widths, lane_step }
    }

    pub(super) fn width(self, column: GraphColumn) -> f32 {
        self.widths[column as usize]
    }

    fn content_width(self) -> f32 {
        self.widths.iter().sum::<f32>() + Self::SPACING
    }
}

pub(super) struct ColumnDrag {
    column: GraphColumn,
    start_x: Pixels,
    width: f32,
    changed: bool,
}

impl Pokarak {
    pub(super) fn commit_table(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.git_history;
        let palette = Palette::new(cx);
        let lanes = ui
            .entries
            .iter()
            .map(|entry| entry.graph.columns)
            .max()
            .unwrap_or(1);
        let viewport = ui
            .table_width
            .unwrap_or(window.viewport_size().width - px(230.));
        let columns = Columns::new(viewport.into(), lanes, self.session().map(|s| &s.saved));
        let header = self.commit_header(columns, palette, cx);
        let rows = if ui.entries.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_color(palette.rgb(MUTED))
                .child(if self.loading || ui.loading {
                    "커밋을 불러오는 중…"
                } else if ui.error.is_some() {
                    "기록을 불러오지 못했습니다. 새로고침으로 다시 시도하세요."
                } else {
                    "아직 커밋이 없습니다. 미커밋 변경에서 첫 커밋을 만들 수 있습니다."
                })
                .into_any_element()
        } else {
            self.commit_rows(columns, cx).into_any_element()
        };
        let view = cx.weak_entity();
        let root = self.current.clone();
        let old_width = ui.table_width;
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .on_prepaint(move |bounds, window, cx| {
                let width = bounds.size.width;
                if width > px(0.) && old_width != Some(width) {
                    window.defer(cx, move |_, cx| {
                        let _ = view.update(cx, |app, cx| {
                            if app.current == root
                                && app.git_history.active
                                && app.git_history.table_width != Some(width)
                            {
                                app.git_history.table_width = Some(width);
                                cx.notify();
                            }
                        });
                    });
                }
            })
            .child(
                div()
                    .id("graph-columns-scroll")
                    .size_full()
                    .overflow_x_scroll()
                    .track_scroll(&ui.table_scroll)
                    .child(
                        div()
                            .v_flex()
                            .h_full()
                            .flex_none()
                            .min_w_full()
                            .w(px(columns.content_width()))
                            .child(header)
                            .child(rows),
                    ),
            )
            .horizontal_scrollbar(&ui.table_scroll)
            .into_any_element()
    }
    fn commit_header(&self, columns: Columns, palette: Palette, cx: &mut Context<Self>) -> Div {
        let mut header = Columns::row()
            .h(px(26.))
            .flex_none()
            .bg(palette.rgb(SIDE))
            .text_size(px(11.))
            .text_color(palette.rgb(MUTED));
        for column in GraphColumn::ALL {
            let width = columns.width(column);
            header = header.child(
                div()
                    .relative()
                    .h_full()
                    .w(px(width))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr(px(6.))
                            .text_ellipsis()
                            .child(column.label()),
                    )
                    .child(
                        resize_handle(("graph-column-handle", column as usize), Axis::Horizontal)
                            .inside(HandleEdge::Trailing)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |app, event: &MouseDownEvent, _, _| {
                                    app.git_history.column_drag = Some(ColumnDrag {
                                        column,
                                        start_x: event.position.x,
                                        width,
                                        changed: false,
                                    });
                                }),
                            )
                            .on_drag(column, |_, _, _, cx| cx.new(|_| EmptyView)),
                    ),
            );
        }
        let finish = |app: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
            if app
                .git_history
                .column_drag
                .take()
                .is_some_and(|drag| drag.changed)
            {
                app.persist(cx);
            }
        };
        header
            .on_drag_move(
                cx.listener(|app, event: &DragMoveEvent<GraphColumn>, _, cx| {
                    let Some(drag) = &mut app.git_history.column_drag else {
                        return;
                    };
                    let column = *event.drag(cx);
                    if drag.column != column {
                        return;
                    }
                    let width =
                        column.clamp(drag.width + f32::from(event.event.position.x - drag.start_x));
                    if let Some(session) = app
                        .current
                        .as_ref()
                        .and_then(|root| app.sessions.get_mut(root))
                        && session.saved.graph_columns.insert(column, width) != Some(width)
                    {
                        drag.changed = true;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(finish))
            .on_mouse_up_out(MouseButton::Left, cx.listener(finish))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Columns,
        GraphColumn::{Author, Graph, Message, Refs},
        SavedView,
    };

    #[test]
    fn automatic_widths_fit_the_pane_and_keep_lanes_inside_the_graph() {
        for viewport in [520., 700., 1200.] {
            for lanes in [1, 9, 84] {
                let columns = Columns::new(viewport, lanes, None);
                assert_eq!(columns.content_width(), viewport);
                assert!(columns.width(Message) >= 96.);
                assert!(columns.width(Author) >= 48.);
                assert!(8. + (lanes - 1) as f32 * columns.lane_step <= columns.width(Graph) - 7.9);
            }
        }
    }

    #[test]
    fn old_views_default_and_saved_overrides_survive_narrow_panes() {
        let mut view: SavedView = serde_json::from_str("{}").unwrap();
        assert!(view.graph_columns.is_empty());
        view.graph_columns.insert(Message, 860.);
        view.graph_columns.insert(Refs, 0.);
        let json = serde_json::to_string(&view).unwrap();
        let restored: SavedView = serde_json::from_str(&json).unwrap();
        for viewport in [580., 1280.] {
            let columns = Columns::new(viewport, 3, Some(&restored));
            assert_eq!(columns.width(Message), 860.);
            assert_eq!(columns.width(Refs), 64.);
            assert_eq!(
                columns.width(Author),
                Columns::new(viewport, 3, None).width(Author)
            );
        }
    }
}
