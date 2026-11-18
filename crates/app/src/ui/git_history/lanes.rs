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
    palette.graph_track(track)
}

fn inactive_color(color: Rgba, palette: Palette, node: bool) -> Rgba {
    if palette.is_dark() {
        color.opacity(if node { 0.35 } else { 0.24 })
    } else {
        // Width/shape carry the hover emphasis. Fading a dark hue on a light
        // selected row would erase unrelated lanes and their branch identities.
        color
    }
}

fn node_shape(track: usize, palette: Palette) -> (f32, bool) {
    if palette.is_dark() {
        (4., false)
    } else {
        // Independent cues for neighboring hues and line crossings: round or
        // square, hollow or filled, always keyed by track rather than column.
        (if track % 2 == 0 { 4. } else { 1. }, track % 4 >= 2)
    }
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
            let x = |lane: usize| bounds.left() + ui_px(8. + lane as f32 * step);
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
                    Some(_) => inactive_color(track_color(track, palette), palette, false),
                    None => track_color(track, palette),
                };
                let width = if highlighted { 2.8 } else { 1.8 };
                let bend = (from.y + to.y) * 0.5;
                // A narrow knockout keeps crossing strokes separable without
                // moving lanes or changing edges. Preserve the dark rendering.
                let halo = (!palette.is_dark()).then_some((width + 2., palette.rgb(BG)));
                for (width, color) in [halo, Some((width, color))].into_iter().flatten() {
                    let mut path = PathBuilder::stroke(ui_px(width));
                    path.move_to(from);
                    path.cubic_bezier_to(to, point(from.x, bend), point(to.x, bend));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
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
                Some(_) => inactive_color(track_color(row.track, palette), palette, true),
                None => track_color(row.track, palette),
            };
            let radius = ui_px(4.);
            let (corner, filled) = node_shape(row.track, palette);
            window.paint_quad(quad(
                Bounds::new(
                    point(x(row.lane) - radius, middle - radius),
                    size(radius * 2., radius * 2.),
                ),
                ui_px(corner),
                if filled { color } else { palette.rgb(BG) },
                ui_px(1.8),
                color,
                BorderStyle::Solid,
            ));
        },
    )
    .w(ui_px(width))
    .h(ui_px(30.))
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
    use super::{
        BranchTrace, Commit, GraphRow, HistoryEntry, inactive_color, node_shape, track_color,
    };
    use crate::theme::Palette;

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

    #[test]
    fn real_merge_fixture_retains_track_identity_and_first_parent_hover() {
        use std::{
            collections::{HashMap, HashSet},
            io::Write,
            process::{Command, Stdio},
        };
        let fixture = tempfile::tempdir().unwrap();
        let git = |args: &[&str], input: &str| {
            let mut child = Command::new("git")
                .arg("-C")
                .arg(fixture.path())
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Graph Fixture")
                .env("GIT_AUTHOR_EMAIL", "graph@example.invalid")
                .env("GIT_COMMITTER_NAME", "Graph Fixture")
                .env("GIT_COMMITTER_EMAIL", "graph@example.invalid")
                .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+0000")
                .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+0000")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-b", "main"], "");
        let tree = git(&["mktree"], "");
        let commit = |message: &str, parents: &[&str]| {
            let mut args = vec!["commit-tree", tree.as_str()];
            for parent in parents {
                args.extend(["-p", parent]);
            }
            git(&args, message)
        };
        let root = commit("root", &[]);
        let first_parent = commit("main before merge", &[&root]);
        let mut sides = Vec::new();
        for index in 0..7 {
            let oid = commit(&format!("side {index}"), &[&root]);
            git(
                &["update-ref", &format!("refs/heads/side-{index}"), &oid],
                "",
            );
            sides.push(oid);
        }
        let first_merge = commit("merge side-0", &[&first_parent, &sides[0]]);
        let merge = commit("merge side-1", &[&first_merge, &sides[1]]);
        git(&["update-ref", "refs/heads/main", &merge], "");
        let page = pokarak_core::history::first_page(fixture.path(), 100).unwrap();
        assert_eq!(page.entries.len(), 11);
        assert!(page.entries.iter().any(|e| e.commit.parents.len() == 2));
        let trace = BranchTrace::from_tip(&page.entries, &merge).unwrap();
        let hovered: Vec<_> = page
            .entries
            .iter()
            .enumerate()
            .filter(|(index, _)| trace.row(*index).node)
            .map(|(_, e)| e.commit.oid.as_str())
            .collect();
        assert_eq!(hovered, [&merge, &first_merge, &first_parent, &root]);

        let light = Palette::for_test(false);
        let mut positions: HashMap<usize, HashSet<usize>> = HashMap::new();
        for entry in &page.entries {
            let row = &entry.graph;
            positions.entry(row.track).or_default().insert(row.lane);
            for (lane, track) in row.incoming_tracks.iter().enumerate() {
                positions.entry(*track).or_default().insert(lane);
                let color = track_color(*track, light);
                assert_eq!(inactive_color(color, light, false), color);
                assert_eq!(inactive_color(color, light, true), color);
                assert_eq!(color.a, 1.);
            }
        }
        assert!(positions.len() >= 8);
        assert!(
            positions.values().any(|lanes| lanes.len() > 1),
            "fixture must exercise lane shifts"
        );
        let shapes: HashSet<_> = (0..4)
            .map(|track| {
                let (corner, filled) = node_shape(track, light);
                (corner as u8, filled)
            })
            .collect();
        assert_eq!(shapes.len(), 4);
        let dark = Palette::for_test(true);
        assert!((inactive_color(track_color(0, dark), dark, false).a - 0.24).abs() < 0.001);
        assert_eq!(node_shape(3, dark), (4., false));
    }
}
