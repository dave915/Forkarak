//! Width-aware ref badges and a read-only, independently hoverable overflow list.
use super::*;
use gpui_kit::component::{menu::PopupMenu, popover::Popover, scroll::ScrollableElement};

const GAP: f32 = 4.;
const PADDING: f32 = 8.;

#[derive(Clone, Default)]
struct RefHover(std::rc::Rc<std::cell::Cell<Option<(bool, usize)>>>);

impl RefHover {
    fn update(&self, source: (bool, usize), hovered: bool) -> Option<bool> {
        if hovered {
            self.0.set(Some(source));
            Some(true)
        } else if self.0.get() == Some(source) {
            self.0.set(None);
            Some(false)
        } else {
            // An old badge's exit can arrive after its same-OID neighbor's enter.
            None
        }
    }
}

struct BadgeStyle {
    width: f32,
    color: Rgba,
    full: bool,
}

#[derive(Debug, PartialEq)]
struct BadgeLayout {
    widths: Vec<f32>,
    hidden: usize,
}

/// Reserve the actual +N width before admitting each complete badge. At the
/// minimum column width a clipped first badge is useful only if it leaves a
/// readable fragment; otherwise the overflow control owns the entire cell.
fn badge_layout(widths: &[f32], available: f32, overflow: impl Fn(usize) -> f32) -> BadgeLayout {
    let total = widths.len();
    let mut used = 0.;
    let mut visible = 0;
    for (i, &width) in widths.iter().enumerate() {
        let next = used + if i == 0 { 0. } else { GAP } + width;
        let remaining = total - i - 1;
        let required = next
            + if remaining == 0 {
                0.
            } else {
                GAP + overflow(remaining)
            };
        if required <= available {
            visible = i + 1;
        }
        // Keep considering the all-visible case: removing +N can make it fit.
        used = next;
    }
    let mut inline = widths[..visible].to_vec();
    if visible == 0 && total > 1 {
        let fragment = available - GAP - overflow(total - 1);
        if fragment >= 24. {
            inline.push(widths[0].min(fragment));
            visible = 1;
        }
    }
    BadgeLayout {
        widths: inline,
        hidden: total - visible,
    }
}

fn ordered_refs(refs: &[String], current: Option<&str>) -> Vec<String> {
    let mut refs = refs.to_vec();
    refs.sort_by_key(|reference| match reference.strip_prefix("refs/heads/") {
        Some(branch) if Some(branch) == current => 0,
        None if current.is_none() && reference == "HEAD" => 0,
        Some(_) => 1,
        None => 2,
    });
    refs
}

fn ref_kind(reference: &str) -> &'static str {
    if reference.starts_with("refs/heads/") {
        "로컬 브랜치"
    } else if reference.starts_with("refs/remotes/") {
        "원격 브랜치"
    } else if reference.starts_with("refs/tags/") {
        "태그"
    } else {
        "참조"
    }
}

fn text_width(text: &str, window: &Window) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color: rgb(TEXT).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.to_owned().into(), ui_text(11.), &[run], None);
    theme::logical_px(line.width()).ceil()
}

/// Neither detail requests nor hovers invalidate this snapshot. Replacing the
/// graph rows (refresh/search/scope/worktree), even at the same path, does.
#[derive(Clone)]
struct RefScope {
    root: Option<PathBuf>,
    rows: std::sync::Weak<Vec<HistoryEntry>>,
    branch: Option<String>,
    generation: u64,
}

impl RefScope {
    fn same(&self, other: &Self) -> bool {
        self.root == other.root
            && self.rows.ptr_eq(&other.rows)
            && self.branch == other.branch
            && self.generation == other.generation
    }

    fn accepts_history(
        &self,
        ui: &HistoryUi,
        current: Option<&Path>,
        branch: Option<&str>,
        generation: u64,
    ) -> bool {
        ui.active
            && self.root.is_some()
            && current == self.root.as_deref()
            && ui.root == self.root
            && self.rows.ptr_eq(&Arc::downgrade(&ui.entries))
            && branch == self.branch.as_deref()
            && generation == self.generation
    }

    fn accepts(&self, app: &Pokarak) -> bool {
        self.accepts_history(
            &app.git_history,
            app.current.as_deref(),
            app.graph_branch(),
            app.request,
        )
    }
}

#[derive(Clone)]
pub(super) struct RefBadges {
    owner: WeakEntity<Pokarak>,
    scope: RefScope,
    current: Option<String>,
    occupied: HashSet<String>,
    hover: RefHover,
}

impl RefBadges {
    pub(super) fn new(app: &Pokarak, owner: WeakEntity<Pokarak>) -> Self {
        Self {
            owner,
            scope: RefScope {
                root: app.git_history.root.clone(),
                rows: Arc::downgrade(&app.git_history.entries),
                branch: app.graph_branch().map(str::to_owned),
                generation: app.request,
            },
            current: app
                .active_worktree()
                .filter(|t| !t.is_detached && !t.head.is_empty())
                .map(|t| t.branch.clone()),
            occupied: app
                .inventory
                .iter()
                .filter(|t| !t.is_detached)
                .map(|t| t.branch.clone())
                .collect(),
            hover: RefHover::default(),
        }
    }

    fn hover(&self, oid: &str, source: (bool, usize), hovered: bool, cx: &mut App) {
        let Some(hovered) = self.hover.update(source, hovered) else {
            return;
        };
        let _ = self.owner.update(cx, |app, cx| {
            if self.scope.accepts(app) {
                // Refs at the same commit intentionally trace the same lineage.
                app.hover_graph_branch(oid, hovered, cx);
            }
        });
    }

    fn clear_hover(&self, oid: &str, cx: &mut App) {
        self.hover.0.set(None);
        let _ = self.owner.update(cx, |app, cx| {
            if app.current == self.scope.root && app.git_history.root == self.scope.root {
                app.hover_graph_branch(oid, false, cx);
            }
        });
    }

    fn badge(
        &self,
        oid: &str,
        reference: &str,
        index: usize,
        style: BadgeStyle,
        palette: Palette,
    ) -> AnyElement {
        let BadgeStyle { width, color, full } = style;
        let label = format!(
            "{} · {} · 커밋 {}",
            ref_kind(reference),
            visible(reference),
            short(oid)
        );
        let hover = self.clone();
        let select = self.clone();
        let tip = oid.to_owned();
        let jump = tip.clone();
        let button = Button::new(("graph-ref", index))
            .ghost()
            .small()
            .w(ui_px(width))
            .h(ui_px(if full { 28. } else { 22. }))
            .flex_none()
            .px(ui_px(4.))
            .border_0()
            .rounded(ui_px(3.))
            .bg(palette.graph_badge_fill(color, full))
            .text_color(color)
            .tooltip(label.clone())
            .accessibility_label(label)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(ui_text(11.))
                    .text_ellipsis()
                    .child(if full {
                        format!("{} · {}", ref_kind(reference), visible(reference))
                    } else {
                        visible(reference_name(reference))
                    }),
            )
            .on_click(move |_, w, cx| {
                cx.stop_propagation();
                let _ = select.owner.update(cx, |app, cx| {
                    if select.scope.accepts(app) && !history_disabled(app) {
                        app.reveal_history_commit(jump.clone(), w, cx);
                    }
                });
            });
        let row = div()
            .id(("graph-ref-context", index))
            .flex_none()
            // Button's managed tooltip owns its hover listener. Keep graph
            // highlighting on the wrapper so each element has one listener.
            .on_hover(move |hovered, _, cx| hover.hover(&tip, (full, index), *hovered, cx))
            .child(button);
        if (reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/"))
            && let Some(root) = &self.scope.root
        {
            let badges = self.clone();
            let build = crate::ui::git_actions::branch_context_menu(
                self.owner.clone(),
                root.clone(),
                reference.to_owned(),
                self.current.clone(),
                reference
                    .strip_prefix("refs/heads/")
                    .is_some_and(|name| !self.occupied.contains(name)),
            );
            return row
                .context_menu(move |menu, window, cx| {
                    let Some(owner) = badges
                        .owner
                        .upgrade()
                        .filter(|o| badges.scope.accepts(o.read(cx)))
                    else {
                        return menu.label("브랜치 목록이 변경되었습니다. 다시 열어 주세요.");
                    };
                    let scope = badges.scope.clone();
                    cx.observe_in(&owner, window, move |_: &mut PopupMenu, app, _, cx| {
                        if !scope.accepts(app.read(cx)) {
                            cx.emit(DismissEvent);
                        }
                    })
                    .detach();
                    build(menu, window, cx)
                })
                .into_any_element();
        }
        row.into_any_element()
    }

    pub(super) fn cell(
        &self,
        commit: &Commit,
        width: f32,
        color: Rgba,
        window: &mut Window,
        cx: &mut App,
    ) -> Div {
        let hover = window.use_keyed_state(
            SharedString::from(format!("graph-ref-hover-{}", commit.oid)),
            cx,
            |_, cx| {
                let badges = self.clone();
                let oid = commit.oid.clone();
                cx.on_release(move |hover: &mut RefHover, cx| {
                    if hover.0.get().is_some() {
                        badges.clear_hover(&oid, cx);
                    }
                })
                .detach();
                RefHover::default()
            },
        );
        let badges = Self {
            hover: hover.read(cx).clone(),
            ..self.clone()
        };
        let refs = ordered_refs(&commit.refs, badges.current.as_deref());
        let widths: Vec<_> = refs
            .iter()
            .map(|r| text_width(&visible(reference_name(r)), window) + PADDING)
            .collect();
        // The trigger uses the same font, padding and border-free geometry.
        let overflow_width = |count| text_width(&format!("+{count}"), window) + PADDING;
        let layout = badge_layout(&widths, width, overflow_width);
        let mut cell = div()
            .w(ui_px(width))
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap(ui_px(GAP));
        for (i, &width) in layout.widths.iter().enumerate() {
            cell = cell.child(badges.badge(
                &commit.oid,
                &refs[i],
                i,
                BadgeStyle {
                    width,
                    color,
                    full: false,
                },
                Palette::new(cx),
            ));
        }
        if layout.hidden > 0 {
            let trigger_width = overflow_width(layout.hidden).min(width);
            let popup = window.use_keyed_state(
                SharedString::from(format!("graph-ref-overflow-{}", commit.oid)),
                cx,
                |window, cx| RefOverflow::new(badges.clone(), commit.clone(), window, cx),
            );
            popup.update(cx, |popup, cx| {
                let context_changed =
                    !popup.badges.scope.same(&self.scope) || popup.badges.current != self.current;
                let changed = context_changed
                    || popup.hidden != layout.hidden
                    || popup.width != trigger_width
                    || popup.color != color;
                if context_changed {
                    popup.badges.clear_hover(&popup.commit.oid, cx);
                    popup.open = false;
                    popup.badges = badges.clone();
                    popup.commit = commit.clone();
                }
                popup.hidden = layout.hidden;
                popup.width = trigger_width;
                popup.color = color;
                if changed {
                    cx.notify();
                }
            });
            cell = cell.child(popup);
        }
        cell
    }
}

struct RefOverflow {
    badges: RefBadges,
    commit: Commit,
    hidden: usize,
    width: f32,
    color: Rgba,
    open: bool,
    scroll: UniformListScrollHandle,
    horizontal: ScrollHandle,
    _owner_subscription: Option<Subscription>,
}

impl RefOverflow {
    fn new(badges: RefBadges, commit: Commit, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = badges.owner.upgrade().map(|owner| {
            cx.observe_in(&owner, window, |popup: &mut Self, owner, _, cx| {
                if popup.open && !popup.badges.scope.accepts(owner.read(cx)) {
                    popup.open = false;
                    popup.badges.clear_hover(&popup.commit.oid, cx);
                    cx.notify();
                }
            })
        });
        // Virtual rows may unmount with a popup open. Drop its hover as well.
        cx.on_release(|popup, cx| {
            if popup.open {
                popup.badges.clear_hover(&popup.commit.oid, cx);
            }
        })
        .detach();
        Self {
            badges,
            commit,
            hidden: 0,
            width: 0.,
            color: rgb(TEXT),
            open: false,
            scroll: UniformListScrollHandle::new(),
            horizontal: ScrollHandle::new(),
            _owner_subscription: subscription,
        }
    }
}

impl Render for RefOverflow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let badges = self.badges.clone();
        let commit = self.commit.clone();
        let refs = ordered_refs(&commit.refs, badges.current.as_deref());
        let scroll = self.scroll.clone();
        let horizontal = self.horizontal.clone();
        let color = self.color;
        let content_width = refs
            .iter()
            .map(|r| {
                text_width(&format!("{} · {}", ref_kind(r), visible(r)), window) + PADDING + 4.
            })
            .fold(0., f32::max);
        let width = ui_px(480.).min((window.viewport_size().width - ui_px(48.)).max(ui_px(160.)));
        Popover::new("graph-refs-popup")
            .open(self.open)
            .trigger(Button::new("graph-refs-more").ghost().small().w(ui_px(self.width)).h(ui_px(22.)).px(ui_px(4.)).border_0()
                .accessibility_label(format!("표시하지 못한 참조 {}개 · 커밋 {}의 브랜치·태그 {}개 모두 보기", self.hidden, short(&self.commit.oid), self.commit.refs.len()))
                .tooltip("모든 브랜치·태그 보기 · 각 항목을 가리키면 그래프 강조, 브랜치 우클릭으로 병합")
                .child(div().text_size(ui_text(11.)).child(format!("+{}", self.hidden)))
                .on_click(|_, _, cx| cx.stop_propagation()))
            .on_open_change(move |open, _, cx| {
                let _ = weak.update(cx, |popup, cx| {
                    popup.open = *open && popup.badges.owner.upgrade().is_some_and(|o| popup.badges.scope.accepts(o.read(cx)));
                    if !popup.open { popup.badges.clear_hover(&popup.commit.oid, cx); }
                    cx.notify();
                });
            })
            .content(move |_, _, _| {
                let badges = badges.clone();
                let oid = commit.oid.clone();
                let refs = refs.clone();
                let count = refs.len();
                let list_width = content_width.max(theme::logical_px(width));
                let list = uniform_list("graph-overflow-refs", count, move |range, _, cx| {
                    let palette = Palette::new(cx);
                    range.map(|i| {
                        let style = BadgeStyle { width: list_width, color, full: true };
                        badges.badge(&oid, &refs[i], i, style, palette)
                    }).collect()
                }).w(ui_px(list_width)).h(ui_px((count.min(10) * 28) as f32)).track_scroll(&scroll);
                div().v_flex().gap_1().w(width)
                    .child(div().text_size(ui_text(12.)).child(format!("{} · 브랜치 / 태그 {}개", short(&commit.oid), count)))
                    .child(div().id("graph-refs-horizontal").w_full().overflow_x_scroll().track_scroll(&horizontal)
                        .child(list).horizontal_scrollbar(&horizontal))
                    .child(div().text_size(ui_text(11.)).child("가리키기: 그래프 강조 · 브랜치 우클릭: 현재 브랜치로 병합 · Esc: 닫기"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BadgeLayout, GAP, HistoryUi, RefHover, RefScope, badge_layout, ordered_refs, ref_kind,
    };
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    #[test]
    fn widening_reveals_more_refs_and_removes_overflow_when_all_fit() {
        let widths = [48., 72., 60.];
        let more = |_| 24.;
        assert_eq!(
            badge_layout(&widths, 80., more),
            BadgeLayout {
                widths: vec![48.],
                hidden: 2
            }
        );
        assert_eq!(
            badge_layout(&widths, 152., more),
            BadgeLayout {
                widths: vec![48., 72.],
                hidden: 1
            }
        );
        // At 188 the final badge fits because +1 no longer consumes space.
        assert_eq!(
            badge_layout(&widths, 188., more),
            BadgeLayout {
                widths: widths.to_vec(),
                hidden: 0
            }
        );
    }

    #[test]
    fn same_commit_alias_hover_ignores_the_previous_badges_late_exit() {
        let hover = RefHover::default();
        assert_eq!(hover.update((false, 0), true), Some(true));
        assert_eq!(hover.update((false, 1), true), Some(true));
        assert_eq!(hover.update((false, 0), false), None);
        assert_eq!(hover.update((true, 0), true), Some(true));
        assert_eq!(hover.update((false, 1), false), None);
        assert_eq!(hover.update((true, 0), false), Some(false));
    }

    #[test]
    fn minimum_width_long_single_ref_and_empty_rows_keep_overflow_reachable() {
        assert_eq!(
            badge_layout(&[], 64., |_| 24.),
            BadgeLayout {
                widths: vec![],
                hidden: 0
            }
        );
        assert_eq!(
            badge_layout(&[400.], 64., |_| 24.),
            BadgeLayout {
                widths: vec![],
                hidden: 1
            }
        );
        assert_eq!(
            badge_layout(&[400., 50.], 64., |_| 24.),
            BadgeLayout {
                widths: vec![36.],
                hidden: 1
            }
        );
        assert_eq!(
            badge_layout(&[400., 50.], 40., |_| 24.),
            BadgeLayout {
                widths: vec![],
                hidden: 2
            }
        );
    }

    #[test]
    fn measured_glyph_widths_font_sizes_and_overflow_digit_changes_are_respected() {
        // Equal character counts need not have equal widths (iii vs WWW).
        assert_eq!(badge_layout(&[20., 50., 20.], 75., |_| 24.).hidden, 2);
        assert_eq!(badge_layout(&[20., 20., 20.], 75., |_| 24.).hidden, 0);
        for font_scale in [0.85, 1., 1.4] {
            let widths = vec![40. * font_scale; 15];
            for available in 64..=700 {
                let more = |n| {
                    if n >= 10 {
                        32. * font_scale
                    } else {
                        24. * font_scale
                    }
                };
                let layout = badge_layout(&widths, available as f32, more);
                let used = layout.widths.iter().sum::<f32>()
                    + GAP * layout.widths.len().saturating_sub(1) as f32
                    + if layout.hidden > 0 {
                        more(layout.hidden) + if layout.widths.is_empty() { 0. } else { GAP }
                    } else {
                        0.
                    };
                assert!(used <= available as f32 + 0.001);
                assert_eq!(layout.widths.len() + layout.hidden, widths.len());
            }
        }
    }

    #[test]
    fn refs_are_preserved_including_tags_remotes_and_same_tip_aliases() {
        let refs: Vec<String> = [
            "refs/tags/v1",
            "refs/remotes/origin/feature",
            "refs/heads/feature",
            "refs/heads/alias",
        ]
        .map(str::to_owned)
        .into();
        let ordered = ordered_refs(&refs, None);
        let attached = ordered_refs(&refs, Some("alias"));
        assert_eq!(attached[0], "refs/heads/alias");
        assert_eq!(attached[1], "refs/heads/feature");
        let mut head_refs = refs.clone();
        head_refs.push("HEAD".to_owned());
        assert_eq!(ordered_refs(&head_refs, None)[0], "HEAD");
        assert_eq!(
            ordered_refs(&head_refs, Some("feature"))[0],
            "refs/heads/feature"
        );
        assert_eq!(
            ordered,
            [
                "refs/heads/feature",
                "refs/heads/alias",
                "refs/tags/v1",
                "refs/remotes/origin/feature"
            ]
        );
        assert_eq!(ordered.len(), refs.len());
        assert_eq!(ref_kind(&ordered[2]), "태그");
        assert_eq!(ref_kind(&ordered[3]), "원격 브랜치");
    }

    #[test]
    fn popup_scope_rejects_worktree_scope_refresh_and_code_view_changes() {
        let root = PathBuf::from("/fixture/main");
        let mut ui = HistoryUi {
            active: true,
            root: Some(root.clone()),
            ..Default::default()
        };
        let scope = RefScope {
            root: ui.root.clone(),
            rows: Arc::downgrade(&ui.entries),
            branch: Some("refs/heads/main".into()),
            generation: 7,
        };
        let accepts = |ui: &HistoryUi, path: &Path, branch, generation| {
            scope.accepts_history(ui, Some(path), branch, generation)
        };
        assert!(accepts(&ui, &root, Some("refs/heads/main"), 7));
        assert!(!accepts(
            &ui,
            Path::new("/fixture/other"),
            Some("refs/heads/main"),
            7
        ));
        assert!(!accepts(&ui, &root, Some("refs/heads/feature"), 7));
        assert!(!accepts(&ui, &root, Some("refs/heads/main"), 8));
        ui.active = false;
        assert!(!accepts(&ui, &root, Some("refs/heads/main"), 7));
        ui.active = true;
        ui.entries = Arc::new(Vec::new());
        assert!(!accepts(&ui, &root, Some("refs/heads/main"), 7));
    }
}
