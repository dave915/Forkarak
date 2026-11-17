use super::*;
use pokarak_core::history::navigation::{Catalog, RefGroup, SearchResults};

#[derive(Default)]
pub(super) struct NavigationUi {
    pub(super) catalog: Option<Arc<Catalog>>,
    closed: HashSet<Group>,
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
enum Group {
    Worktrees,
    Refs(RefGroup),
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Self::Worktrees => "워크트리",
            Self::Refs(RefGroup::Local) => "로컬 브랜치",
            Self::Refs(RefGroup::Remote) => "원격 브랜치",
            Self::Refs(RefGroup::Tag) => "태그",
            Self::Refs(RefGroup::Stash) => "Stash",
        }
    }
}

impl Pokarak {
    pub(in crate::ui) fn graph_search_control(&self, cx: &mut Context<Self>) -> Button {
        Button::new("graph-search")
            .small()
            .ghost()
            .icon(IconName::Search)
            .label("검색")
            .accessibility_label("커밋 검색")
            .tooltip_with_action("커밋 검색", &SearchGitGraph, Some("GitGraph"))
            .disabled(self.git_history.nav.catalog.is_none() || self.loading || self.git_is_busy())
            .on_click(cx.listener(|app, _, w, cx| app.open_graph_search(w, cx)))
    }

    pub(in crate::ui) fn open_graph_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_history.active
            || self.git_is_busy()
            || self.loading
            || window.has_active_dialog(cx)
        {
            return;
        }
        let (Some(root), Some(catalog)) =
            (self.current.clone(), self.git_history.nav.catalog.clone())
        else {
            return;
        };
        let owner = cx.entity().downgrade();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("해시·커밋 메시지·작성자"));
        let picker = cx.new(|cx| {
            let subscription = cx.subscribe_in(
                &input,
                window,
                |picker: &mut CommitSearch, input, event: &InputEvent, w, cx| match event {
                    InputEvent::Change => picker.search(input.read(cx).value().to_string(), cx),
                    InputEvent::PressEnter { .. } if !picker.busy => {
                        if let Some(commit) = picker.results.commits.first() {
                            picker.select(commit.oid.clone(), w, cx);
                        }
                    }
                    _ => {}
                },
            );
            CommitSearch {
                owner,
                root,
                catalog,
                input: input.clone(),
                _subscription: subscription,
                pending: None,
                results: SearchResults::default(),
                error: None,
                query: String::new(),
                busy: false,
            }
        });
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("커밋 검색")
                .width(px(620.))
                .margin_top(px(28.))
                .child(picker.clone())
        });
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn jump_to_graph_oid(&mut self, oid: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.git_is_busy() || !self.git_history.active {
            return;
        }
        if let Some(i) = self
            .git_history
            .entries
            .iter()
            .position(|row| row.commit.oid == oid)
        {
            self.git_history
                .scroll
                .scroll_to_item_strict_with_offset(i, ScrollStrategy::Top, 3);
            self.reveal_history_commit(oid, window, cx);
            return;
        }
        let Some(catalog) = self.git_history.nav.catalog.clone() else {
            return;
        };
        let target = oid.clone();
        self.history_read(
            move |root| catalog.around(root, &target),
            move |app, (page, i), w, cx| {
                app.git_history.entries = page.entries.into();
                app.git_history.cursor = page.next;
                app.git_history.limit_reached = page.limit_reached;
                app.git_history.shallow = page.shallow;
                app.git_history
                    .scroll
                    .scroll_to_item_strict_with_offset(i, ScrollStrategy::Top, 3);
                app.reveal_history_commit(oid, w, cx);
            },
            window,
            cx,
        );
    }

    pub(in crate::ui) fn render_graph_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let Some(snapshot) = self.snapshot() else {
            return div().size_full().bg(palette.rgb(SIDE)).into_any_element();
        };
        let catalog = self.git_history.nav.catalog.clone();
        let trees = snapshot.worktrees.clone();
        let root = snapshot.worktree.path.clone();
        let current = (!snapshot.worktree.is_detached && !snapshot.worktree.head.is_empty())
            .then(|| snapshot.worktree.branch.clone());
        let occupied: HashSet<_> = trees
            .iter()
            .filter(|t| !t.is_detached)
            .map(|t| t.branch.clone())
            .collect();
        let selected = self
            .git_history
            .detail
            .as_ref()
            .map(|d| d.commit.oid.clone());
        let scope = root.clone();
        let weak = cx.entity().downgrade();
        let sections = [
            Group::Worktrees,
            Group::Refs(RefGroup::Local),
            Group::Refs(RefGroup::Remote),
            Group::Refs(RefGroup::Tag),
            Group::Refs(RefGroup::Stash),
        ];
        // Only range/section metadata and visible rows are constructed per frame.
        let spans: Vec<_> = sections
            .into_iter()
            .map(|section| {
                let count = match section {
                    Group::Worktrees => trees.len(),
                    Group::Refs(group) => catalog.as_ref().map_or(0, |c| c.group(group).len()),
                };
                (
                    section,
                    count,
                    self.git_history.nav.closed.contains(&section),
                )
            })
            .collect();
        let rows = spans
            .iter()
            .map(|(_, n, closed)| if *closed { 1 } else { 1 + n })
            .sum();
        let list = uniform_list("graph-ref-list", rows, move |range, _, _| {
            range
                .filter_map(|index| {
                    let mut offset = index;
                    for &(group, count, closed) in &spans {
                        if offset == 0 {
                            let target = weak.clone();
                            let header = Button::new(("graph-ref-section", index))
                                .ghost()
                                .small()
                                .justify_start()
                                .flex_1()
                                .min_w_0()
                                .icon(if closed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .label(format!("{}  {count}", group.title()))
                                .accessibility_label(format!(
                                    "{} {count}개 {}",
                                    group.title(),
                                    if closed { "펼치기" } else { "접기" }
                                ))
                                .on_click(move |_, _, cx| {
                                    let _ = target.update(cx, |app, cx| {
                                        if !app.git_history.nav.closed.remove(&group) {
                                            app.git_history.nav.closed.insert(group);
                                        }
                                        cx.notify();
                                    });
                                });
                            let target = weak.clone();
                            return Some(
                                div()
                                    .h(px(30.))
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .child(header)
                                    .when(group == Group::Worktrees, |row| {
                                        row.child(
                                            Button::new("graph-manage-worktrees")
                                                .icon(IconName::Settings)
                                                .ghost()
                                                .xsmall()
                                                .accessibility_label("워크트리 관리")
                                                .tooltip("워크트리 생성·정리")
                                                .on_click(move |_, w, cx| {
                                                    let _ = target.update(cx, |app, cx| {
                                                        app.open_worktree_manager(w, cx)
                                                    });
                                                }),
                                        )
                                    })
                                    .into_any_element(),
                            );
                        }
                        offset -= 1;
                        if closed {
                            continue;
                        }
                        if offset >= count {
                            offset -= count;
                            continue;
                        }
                        let tree = (group == Group::Worktrees).then(|| &trees[offset]);
                        let reference = match group {
                            Group::Refs(g) => {
                                catalog.as_ref().and_then(|cat| cat.group(g).get(offset))
                            }
                            Group::Worktrees => None,
                        };
                        let (label, tip, oid, icon) = if let Some(tree) = tree {
                            let name = tree.path.file_name().unwrap_or_default().to_string_lossy();
                            (
                                format!("{}  ·  {name}", tree.branch),
                                tree.path.display().to_string(),
                                tree.head.clone(),
                                IconName::FolderGit2,
                            )
                        } else {
                            let r = reference?;
                            let icon = match r.group {
                                RefGroup::Local => IconName::GitBranch,
                                RefGroup::Remote => IconName::Cloud,
                                RefGroup::Tag => IconName::Tag,
                                RefGroup::Stash => IconName::Archive,
                            };
                            (
                                r.label.clone(),
                                format!("{}  ·  {}", r.name, short(&r.oid)),
                                r.oid.clone(),
                                icon,
                            )
                        };
                        let target = weak.clone();
                        let select_scope = scope.clone();
                        let jump = oid.clone();
                        let selected = selected.as_ref() == Some(&oid);
                        let mut button = Button::new(("graph-reference", index))
                            .ghost()
                            .small()
                            .w_full()
                            .h(px(29.))
                            .justify_start()
                            .accessibility_label(format!("{} {label} 커밋으로 이동", group.title()))
                            .tooltip(tip)
                            .disabled(oid.is_empty() || catalog.is_none())
                            .selected(selected)
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .pl_3()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Icon::new(icon).size(px(14.)).text_color(
                                        palette.rgb(if selected { BLUE } else { MUTED }),
                                    ))
                                    .child(div().min_w_0().text_ellipsis().child(label)),
                            )
                            .on_click(move |_, w, cx| {
                                let _ = target.update(cx, |app, cx| {
                                    if app.current.as_ref() == Some(&select_scope) {
                                        app.jump_to_graph_oid(jump.clone(), w, cx);
                                    }
                                });
                            });
                        if reference
                            .is_some_and(|r| matches!(r.group, RefGroup::Local | RefGroup::Remote))
                        {
                            let target = weak.clone();
                            button = button.on_hover(move |hovered, _, cx| {
                                let _ = target.update(cx, |app, cx| {
                                    app.hover_graph_branch(&oid, *hovered, cx)
                                });
                            });
                        }
                        let row = div().h(px(30.)).w_full().px_1().child(button);
                        if let Some(r) = reference
                            .filter(|r| matches!(r.group, RefGroup::Local | RefGroup::Remote))
                        {
                            let can_checkout =
                                r.group == RefGroup::Local && !occupied.contains(&r.label);
                            return Some(
                                row.context_menu(crate::ui::git_actions::branch_context_menu(
                                    weak.clone(),
                                    root.clone(),
                                    r.name.clone(),
                                    current.clone(),
                                    can_checkout,
                                ))
                                .into_any_element(),
                            );
                        }
                        if let Some(t) = tree {
                            let target = weak.clone();
                            let path = t.path.clone();
                            let disabled = t.is_prunable;
                            return Some(
                                row.context_menu(move |menu, _, _| {
                                    let target = target.clone();
                                    let path = path.clone();
                                    menu.item(
                                        PopupMenuItem::new("이 워크트리 열기")
                                            .disabled(disabled)
                                            .on_click(move |_, w, cx| {
                                                let _ = target.update(cx, |app, cx| {
                                                    app.open_repository(path.clone(), w, cx)
                                                });
                                            }),
                                    )
                                })
                                .into_any_element(),
                            );
                        }
                        return Some(row.into_any_element());
                    }
                    None
                })
                .collect::<Vec<_>>()
        });
        div()
            .size_full()
            .v_flex()
            .bg(palette.rgb(SIDE))
            .pt_1()
            .child(list.flex_1().min_h_0())
            .into_any_element()
    }
}

struct CommitSearch {
    owner: WeakEntity<Pokarak>,
    root: PathBuf,
    catalog: Arc<Catalog>,
    input: Entity<InputState>,
    _subscription: Subscription,
    pending: Option<Task<()>>,
    results: SearchResults,
    error: Option<String>,
    query: String,
    busy: bool,
}

impl CommitSearch {
    fn search(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query.trim().to_owned();
        self.pending = None;
        self.error = None;
        self.results = SearchResults::default();
        self.busy = !self.query.is_empty();
        if self.busy {
            let query = self.query.clone();
            let root = self.root.clone();
            let catalog = self.catalog.clone();
            let executor = cx.background_executor().clone();
            self.pending = Some(cx.spawn(async move |this, cx| {
                executor.timer(Duration::from_millis(200)).await;
                let needle = query.clone();
                let result = executor
                    .spawn(async move { catalog.search(&root, &needle) })
                    .await;
                let _ = this.update(cx, |picker, cx| {
                    if picker.query != query {
                        return;
                    }
                    picker.busy = false;
                    match result {
                        Ok(rows) => picker.results = rows,
                        Err(e) => picker.error = Some(format!("{e:#}")),
                    }
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn select(&self, oid: String, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        let _ = self.owner.update(cx, |app, cx| {
            if app.current.as_ref() == Some(&self.root)
                && app
                    .git_history
                    .nav
                    .catalog
                    .as_ref()
                    .is_some_and(|cat| Arc::ptr_eq(cat, &self.catalog))
            {
                app.jump_to_graph_oid(oid, window, cx);
            }
        });
    }
}

impl Render for CommitSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let mut rows = div()
            .v_flex()
            .id("graph-search-results")
            .max_h(px(278.))
            .overflow_y_scroll();
        for (i, commit) in self.results.commits.iter().enumerate() {
            let oid = commit.oid.clone();
            rows = rows.child(
                Button::new(("graph-search-result", i))
                    .ghost()
                    .small()
                    .w_full()
                    .h(px(32.))
                    .justify_start()
                    .flex_none()
                    .accessibility_label(format!(
                        "{} {} {}",
                        short(&oid),
                        visible(&commit.subject),
                        visible(&commit.author)
                    ))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_family("JetBrains Mono")
                                    .text_color(palette.rgb(BLUE))
                                    .child(short(&oid).to_owned()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .child(visible(&commit.subject)),
                            )
                            .child(
                                div()
                                    .max_w(px(110.))
                                    .text_color(palette.rgb(MUTED))
                                    .text_ellipsis()
                                    .child(visible(&commit.author)),
                            ),
                    )
                    .on_click(
                        cx.listener(move |picker, _, w, cx| picker.select(oid.clone(), w, cx)),
                    ),
            );
        }
        let note = self.error.clone().or_else(|| {
            if self.busy {
                Some("검색 중…".into())
            } else if self.query.is_empty() {
                None
            } else if self.results.commits.is_empty() {
                Some("일치하는 커밋이 없습니다.".into())
            } else if self.results.truncated {
                Some("처음 50개 결과 · 검색어로 범위를 좁힐 수 있습니다.".into())
            } else {
                None
            }
        });
        div()
            .v_flex()
            .gap_2()
            .child(Input::new(&self.input).small())
            .when_some(note, |d, n| {
                d.child(div().text_color(palette.rgb(MUTED)).child(n))
            })
            .child(rows)
    }
}
