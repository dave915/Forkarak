use super::*;

/// The tip follows its first parent at a merge. Between commits the same path
/// can move sideways as unrelated lanes start or end.
#[derive(Clone, Copy, Default)]
pub(super) struct Route {
    incoming: Option<usize>,
    outgoing: Option<(usize, usize)>,
    pub node: bool,
}

#[derive(Default)]
pub(super) struct BranchTrace {
    track: usize,
    start: usize,
    rows: Vec<Route>,
}

impl BranchTrace {
    pub fn from_tip(entries: &[HistoryEntry], oid: &str) -> Option<Self> {
        let start = entries.iter().position(|entry| entry.commit.oid == oid)?;
        let tip = &entries[start].graph;
        let mut lane = tip.lane;
        let mut rows = Vec::new();
        for entry in &entries[start..] {
            let row = &entry.graph;
            let outgoing = row.edges.iter().copied().find(|(from, _)| *from == lane);
            rows.push(Route {
                incoming: (!rows.is_empty()).then_some(lane),
                outgoing,
                node: lane == row.lane,
            });
            let Some((_, next)) = outgoing else { break };
            lane = next;
        }
        Some(Self {
            track: tip.track,
            start,
            rows,
        })
    }

    pub fn row(&self, index: usize) -> Route {
        index
            .checked_sub(self.start)
            .and_then(|i| self.rows.get(i))
            .copied()
            .unwrap_or_default()
    }
}

pub(super) fn track_color(track: usize, palette: Palette) -> Rgba {
    const COLORS: [u32; 8] = [
        BLUE, GREEN, 0xd59ede, 0xe4bb79, 0x7ac8ce, RED, 0xb0a9ef, 0xa8c483,
    ];
    palette.rgb(COLORS[track % COLORS.len()])
}

pub(super) fn graph_view(
    row: GraphRow,
    width: f32,
    step: f32,
    index: usize,
    trace: Option<&BranchTrace>,
    palette: Palette,
) -> impl IntoElement {
    let route = trace.map(|trace| (trace.row(index), track_color(trace.track, palette)));
    canvas(
        move |_, _, _| row,
        move |bounds, row, window, _| {
            let x = |lane: usize| bounds.left() + px(8. + lane as f32 * step);
            let top = bounds.top();
            let middle = top + bounds.size.height * 0.5;
            let bottom = bounds.bottom();
            let stroke = |from: Point<Pixels>,
                          to: Point<Pixels>,
                          track: usize,
                          highlighted: bool,
                          window: &mut Window| {
                let color = match route {
                    Some((_, active)) if highlighted => active,
                    Some(_) => track_color(track, palette).opacity(0.24),
                    None => track_color(track, palette),
                };
                let mut path = PathBuilder::stroke(px(if highlighted { 2.8 } else { 1.8 }));
                path.move_to(from);
                let bend = (from.y + to.y) * 0.5;
                path.cubic_bezier_to(to, point(from.x, bend), point(to.x, bend));
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            };
            for (lane, &track) in row.incoming_tracks.iter().enumerate() {
                stroke(
                    point(x(lane), top),
                    point(x(lane), middle),
                    track,
                    route.is_some_and(|(r, _)| r.incoming == Some(lane)),
                    window,
                );
            }
            for &(from, to) in &row.edges {
                stroke(
                    point(x(from), middle),
                    point(x(to), bottom),
                    row.outgoing_tracks[to],
                    route.is_some_and(|(r, _)| r.outgoing == Some((from, to))),
                    window,
                );
            }
            let color = match route {
                Some((r, color)) if r.node => color,
                Some(_) => track_color(row.track, palette).opacity(0.35),
                None => track_color(row.track, palette),
            };
            let radius = px(4.);
            window.paint_quad(quad(
                Bounds::new(
                    point(x(row.lane) - radius, middle - radius),
                    size(radius * 2., radius * 2.),
                ),
                radius,
                palette.rgb(BG),
                px(1.8),
                color,
                BorderStyle::Solid,
            ));
        },
    )
    .w(px(width))
    .h(px(30.))
    .flex_none()
}

pub(super) fn reference_name(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/"))
        .or_else(|| name.strip_prefix("refs/tags/"))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::{BranchTrace, Commit, GraphRow, HistoryEntry};

    #[test]
    fn hover_follows_first_parent_through_lane_shifts() {
        let row = |oid: &str, lane, edges: Vec<(usize, usize)>| HistoryEntry {
            commit: Commit {
                oid: oid.into(),
                parents: Vec::new(),
                author: String::new(),
                date: String::new(),
                subject: String::new(),
                refs: Vec::new(),
            },
            graph: GraphRow {
                lane,
                edges,
                ..GraphRow::default()
            },
        };
        let entries = [
            row("other tip", 0, vec![(0, 0)]),
            row("branch", 1, vec![(0, 0), (1, 1), (1, 2)]),
            row("other root", 0, vec![(1, 0), (2, 1)]),
            row("merged side", 1, vec![(0, 0), (1, 0)]),
            row("first parent", 0, vec![]),
        ];
        let path = BranchTrace::from_tip(&entries, "branch").unwrap();
        assert!(!path.row(0).node);
        assert_eq!(path.row(1).outgoing, Some((1, 1)));
        assert_eq!(path.row(2).outgoing, Some((1, 0)));
        assert_eq!(path.row(3).incoming, Some(0));
        assert!(!path.row(3).node);
        assert!(path.row(4).node);
        assert_eq!(path.row(4).outgoing, None);
    }
}
