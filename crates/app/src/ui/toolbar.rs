use super::*;
use git_actions::Page;
use gpui_kit::component::TitleBar;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};

impl Pokarak {
    pub(super) fn render_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let width = window.viewport_size().width;
        let compact = width < ui_px(1180.);
        let has_projects = !self.projects.is_empty();
        let content = div()
            .absolute()
            .inset_0()
            .min_w_0()
            .px_2()
            .flex()
            .items_center()
            .gap(ui_px(4.))
            .when(!has_projects, |bar| {
                bar.child(
                    div()
                        .text_size(ui_text(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Forkarak"),
                )
            })
            .when(has_projects, |bar| {
                bar.child(self.project_tabs(window, cx)).child(
                    Button::new("add-project")
                        .icon(IconName::Plus)
                        .ghost()
                        .small()
                        .accessibility_label("프로젝트 열기")
                        .tooltip_with_action("프로젝트 열기", &OpenRepository, None)
                        .on_click(cx.listener(|app, _, w, cx| app.choose_repository(w, cx))),
                )
            })
            .when(self.current.is_some(), |bar| {
                bar.child(if compact {
                    self.compact_git_toolbar(cx)
                } else {
                    self.git_toolbar(cx).into_any_element()
                })
            })
            .child(div().flex_1())
            .when(self.current.is_some(), |bar| {
                let bar = bar
                    .when(!self.git_history.active, |bar| {
                        bar.child(self.reading_navigation(cx))
                    })
                    .child(self.workspace_modes(compact, cx))
                    .child(self.pane_toggles(cx));
                #[cfg(target_os = "macos")]
                let bar =
                    bar.child(
                        toolbar_icon("open-terminal", IconName::SquareTerminal, "워크트리 터미널")
                            .disabled(self.opening_path.is_some())
                            .tooltip_with_action(
                                "워크트리 터미널 표시 / 숨기기 · zsh",
                                &OpenWorktreeTerminal,
                                None,
                            )
                            .selected(self.session().is_some_and(|s| s.terminal_visible))
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.open_worktree_terminal(window, cx)
                            })),
                    );
                bar.child(
                    toolbar_icon("refresh", IconName::RefreshCw, "저장소 새로고침")
                        .loading(self.loading)
                        .disabled(self.loading || self.git_is_busy())
                        .tooltip_with_action("저장소 새로고침", &Refresh, None)
                        .on_click(cx.listener(|app, _, w, cx| app.refresh(w, cx))),
                )
            })
            .child(
                toolbar_icon("settings", IconName::Settings, "설정")
                    .tooltip_with_action("설정", &OpenSettings, None)
                    .on_click(cx.listener(|app, _, w, cx| app.open_settings(w, cx))),
            );

        let title = TitleBar::new()
            .h(ui_px(36.))
            .pl(if window.is_fullscreen() {
                px(0.)
            } else {
                px(80.)
            })
            .bg(palette.rgb(SIDE))
            .border_b_0()
            .child(div().relative().size_full().child(content));
        div().flex_none().child(title).into_any_element()
    }

    fn compact_git_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("compact-git-toolbar")
            .flex_none()
            .flex()
            .items_center()
            .gap(ui_px(2.))
            .h(ui_px(28.))
            .child(
                toolbar_icon(
                    "stage-commit",
                    IconName::GitCommitHorizontal,
                    "Stage / 커밋",
                )
                .tooltip_with_action("Stage / 커밋", &OpenGitCommit, None)
                .disabled(self.loading || self.git_is_busy())
                .on_click(cx.listener(|a, _, w, cx| a.open_git_commit(w, cx))),
            )
            .child(
                toolbar_icon("compact-git-more", IconName::Ellipsis, "Git 작업 더 보기")
                    .tooltip("Git 작업 더 보기 · 브랜치, Stash, Fetch, Pull, Push")
                    .on_click(cx.listener(|a, _, w, cx| a.show_compact_git_menu(w, cx))),
            )
            .into_any_element()
    }

    fn show_compact_git_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::native_menu::NativeMenu;
        let busy = self.loading || self.git_is_busy();
        let branch = self
            .session()
            .is_some_and(|s| matches!(s.saved.comparison, CompareMode::Branch(_)));
        let mut menu = NativeMenu::new();
        for (label, command) in [
            ("워크트리 선택…  ⌘⇧J", "worktree"),
            ("브랜치 전체 변경 비교…", "compare"),
            ("HEAD로 돌아가기", "head"),
            ("그래프 브랜치 필터…", "graph-filter"),
            ("커밋 검색…  ⌘F", "graph-search"),
            (self.graph_detail_label(), "graph-detail"),
            ("브랜치 만들기…", "create"),
            ("체크아웃…", "checkout"),
            ("Merge…", "merge"),
            ("Rebase…", "rebase"),
            ("Stash…", "stash"),
            ("Pop…", "pop"),
            ("Fetch…", "fetch"),
            ("Pull…", "pull"),
            ("Push…", "push"),
            ("Git 작업…", "actions"),
            ("Git 작업 기록…", "log"),
        ] {
            menu = menu.menu_with_disabled(
                label,
                (busy && !matches!(command, "log" | "graph-detail"))
                    || (command.starts_with("graph-") && !self.git_history.active)
                    || (command == "head" && !branch),
                Box::new(RunToolbar {
                    root: self.current.clone(),
                    generation: self.request,
                    command: command.into(),
                }),
            );
        }
        menu.show(window.mouse_position(), window, cx);
    }

    pub(super) fn run_toolbar_command(
        &mut self,
        action: &RunToolbar,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current != action.root
            || self.request != action.generation
            || self.opening_path.is_some()
        {
            return;
        }
        match action.command.as_str() {
            "graph-filter" if self.git_history.active => self.open_branch_filter(w, cx),
            "graph-search" if self.git_history.active => self.open_graph_search(w, cx),
            "graph-detail" if self.git_history.active => {
                let collapsed = self
                    .session()
                    .is_some_and(|s| s.saved.graph_detail_collapsed);
                self.set_graph_detail_collapsed(!collapsed, cx);
            }
            "worktree" => self.open_worktree_picker(w, cx),
            "compare" => self.open_branch_review(w, cx),
            "head" => {
                if self
                    .session()
                    .is_some_and(|s| matches!(s.saved.comparison, CompareMode::Branch(_)))
                    && let Some(root) = self.current.clone()
                {
                    self.apply_branch_review(root, CompareMode::Uncommitted, w, cx);
                }
            }
            "create" => self.load_git_page(Some(Page::Create), w, cx),
            "checkout" => self.load_git_page(Some(Page::Checkout), w, cx),
            "merge" => self.load_git_page(Some(Page::Merge), w, cx),
            "rebase" => self.load_git_page(Some(Page::Rebase), w, cx),
            "fetch" => self.load_git_page(Some(Page::Fetch), w, cx),
            "pull" => self.load_git_page(Some(Page::Pull), w, cx),
            "push" => self.load_git_page(Some(Page::Push), w, cx),
            "stash" => self.open_git_workflow(w, cx),
            "pop" => self.open_stash_pop(w, cx),
            "actions" => self.open_git_actions(w, cx),
            "log" => self.open_git_log(w, cx),
            _ => {}
        }
    }

    fn graph_detail_label(&self) -> &'static str {
        if self
            .session()
            .is_some_and(|s| s.saved.graph_detail_collapsed)
        {
            "그래프 상세 펼치기"
        } else {
            "그래프 상세 접기"
        }
    }

    fn git_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.loading || self.git_is_busy();
        let branch = self
            .session()
            .is_some_and(|s| matches!(s.saved.comparison, CompareMode::Branch(_)));
        let weak = cx.entity().downgrade();
        let root = self.current.clone();
        let generation = self.request;
        let palette = Palette::new(cx);
        let log_weak = weak.clone();
        let log_root = root.clone();
        let graph = self.git_history.active;
        let detail_label = self.graph_detail_label();
        div()
            .id("git-toolbar")
            .flex_none()
            .flex()
            .items_center()
            .h(ui_px(28.))
            .px(ui_px(3.))
            .gap(ui_px(2.))
            .rounded(ui_px(6.))
            .bg(palette.rgb(PANEL))
            .child(
                toolbar_icon(
                    "stage-commit",
                    IconName::GitCommitHorizontal,
                    "Stage / 커밋",
                )
                .tooltip_with_action("Stage / 커밋", &OpenGitCommit, None)
                .disabled(busy)
                .on_click(cx.listener(|app, _, w, cx| app.open_git_commit(w, cx))),
            )
            .child(
                toolbar_icon("toolbar-branch", IconName::GitBranch, "브랜치")
                    .tooltip("브랜치 · 만들기, 체크아웃, Merge, Rebase")
                    .disabled(busy)
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                        for (page, label) in [
                            (Page::Create, "브랜치 만들기…"),
                            (Page::Checkout, "체크아웃…"),
                            (Page::Merge, "Merge…"),
                            (Page::Rebase, "Rebase…"),
                        ] {
                            let weak = weak.clone();
                            let root = root.clone();
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                                    let _ = weak.update(cx, |app, cx| {
                                        if app.current == root {
                                            app.load_git_page(Some(page), w, cx);
                                        }
                                    });
                                }));
                        }
                        for (label, command) in [
                            ("워크트리 선택…  ⌘⇧J", "worktree"),
                            ("브랜치 전체 변경 비교…", "compare"),
                            ("HEAD로 돌아가기", "head"),
                        ] {
                            let weak = weak.clone();
                            let action = RunToolbar {
                                root: root.clone(),
                                generation,
                                command: command.into(),
                            };
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .disabled(command == "head" && !branch)
                                    .on_click(move |_, w, cx| {
                                        let _ = weak.update(cx, |app, cx| {
                                            app.run_toolbar_command(&action, w, cx);
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                toolbar_icon("toolbar-stash", IconName::Archive, "Stash")
                    .tooltip_with_action("Stash · 변경 보관 및 복원", &OpenGitWorkflow, None)
                    .disabled(busy)
                    .on_click(cx.listener(|app, _, w, cx| app.open_git_workflow(w, cx))),
            )
            .child(
                toolbar_icon("toolbar-stash-pop", IconName::ArchiveRestore, "Pop")
                    .tooltip("Pop · 보관한 Stash 적용 후 제거…")
                    .disabled(busy)
                    .on_click(cx.listener(|app, _, w, cx| app.open_stash_pop(w, cx))),
            )
            .children(
                [
                    (
                        Page::Fetch,
                        "Fetch",
                        IconName::CloudDownload,
                        "Fetch · 원격 브랜치 가져오기",
                    ),
                    (
                        Page::Pull,
                        "Pull",
                        IconName::ArrowDownToLine,
                        "Pull · 현재 브랜치에 fast-forward로 받기",
                    ),
                    (
                        Page::Push,
                        "Push",
                        IconName::ArrowUpFromLine,
                        "Push · 원격에 보내기",
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (page, name, icon, tooltip))| {
                    toolbar_icon(name, icon, name)
                        .when(index == 0, |button| button.ml(ui_px(6.)))
                        .tooltip(tooltip)
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |app, _, w, cx| app.load_git_page(Some(page), w, cx)),
                        )
                }),
            )
            .child(
                toolbar_icon("git-actions", IconName::Ellipsis, "Git 작업 더 보기")
                    .tooltip("Git 작업 더 보기")
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                        for (label, command) in [
                            ("그래프 브랜치 필터…", "graph-filter"),
                            ("커밋 검색…  ⌘F", "graph-search"),
                            (detail_label, "graph-detail"),
                        ] {
                            let target = log_weak.clone();
                            let action = RunToolbar {
                                root: log_root.clone(),
                                generation,
                                command: command.into(),
                            };
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .disabled(!graph || (busy && command != "graph-detail"))
                                    .on_click(move |_, w, cx| {
                                        let _ = target.update(cx, |app, cx| {
                                            app.run_toolbar_command(&action, w, cx);
                                        });
                                    }),
                            );
                        }
                        let actions = log_weak.clone();
                        let actions_root = log_root.clone();
                        let logs = log_weak.clone();
                        let logs_root = log_root.clone();
                        menu.item(PopupMenuItem::new("Git 작업…").disabled(busy).on_click(
                            move |_, w, cx| {
                                let _ = actions.update(cx, |app, cx| {
                                    if app.current == actions_root {
                                        app.open_git_actions(w, cx);
                                    }
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Git 작업 기록…").on_click(move |_, w, cx| {
                                let _ = logs.update(cx, |app, cx| {
                                    if app.current == logs_root {
                                        app.open_git_log(w, cx);
                                    }
                                });
                            }),
                        )
                    }),
            )
    }

    fn workspace_modes(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let graph = self.git_history.active;
        div()
            .ml_1()
            .flex_none()
            .flex()
            .items_center()
            .gap(ui_px(2.))
            .p(ui_px(2.))
            .rounded(ui_px(5.))
            .bg(palette.rgb(BG))
            .child(
                Button::new("view-graph")
                    .icon(IconName::GitGraph)
                    .accessibility_label("Git 그래프")
                    .when(!compact, |button| button.label("Git 그래프"))
                    .ghost()
                    .compact()
                    .small()
                    .selected(graph)
                    .toggled(graph)
                    .disabled(
                        self.opening_path.is_some()
                            || self.git_is_busy()
                            || (self.loading && graph),
                    )
                    .tooltip_with_action("Git 그래프", &OpenGitHistory, None)
                    .on_click(cx.listener(|app, _, w, cx| {
                        if !app.git_history.active {
                            app.open_git_history(w, cx);
                        }
                    })),
            )
            .child(
                Button::new("view-code")
                    .icon(IconName::CodeXml)
                    .accessibility_label("코드 보기")
                    .tooltip("코드 보기")
                    .when(!compact, |button| button.label("코드 보기"))
                    .ghost()
                    .compact()
                    .small()
                    .selected(!graph)
                    .toggled(!graph)
                    .disabled(self.loading)
                    .on_click(cx.listener(|app, _, w, cx| app.show_code_view(w, cx))),
            )
    }

    fn reading_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session();
        div()
            .flex_none()
            .flex()
            .items_center()
            .child(
                Button::new("back")
                    .icon(IconName::ArrowLeft)
                    .accessibility_label("뒤로")
                    .tooltip_with_action("뒤로", &Back, None)
                    .small()
                    .ghost()
                    .disabled(session.is_none_or(|s| s.history.is_empty()))
                    .on_click(cx.listener(|app, _, w, cx| app.navigate_history(true, w, cx))),
            )
            .child(
                Button::new("forward")
                    .icon(IconName::ArrowRight)
                    .accessibility_label("앞으로")
                    .tooltip_with_action("앞으로", &Forward, None)
                    .small()
                    .ghost()
                    .disabled(session.is_none_or(|s| s.forward.is_empty()))
                    .on_click(cx.listener(|app, _, w, cx| app.navigate_history(false, w, cx))),
            )
    }
}

pub(super) fn toolbar_icon(id: &'static str, icon: IconName, name: &'static str) -> Button {
    Button::new(id)
        .icon(icon)
        .accessibility_label(name)
        .ghost()
        .small()
        .w(ui_px(28.))
        .h(ui_px(26.))
        .flex_none()
}

#[derive(Action, Clone, PartialEq, serde::Deserialize)]
#[action(namespace = forkarak_toolbar, no_json)]
pub(super) struct RunToolbar {
    root: Option<PathBuf>,
    generation: u64,
    command: String,
}
