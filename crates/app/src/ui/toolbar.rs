use super::*;
use git_actions::Page;
use gpui_kit::component::TitleBar;
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::native_menu::NativeMenu;
mod sync;
use sync::{SyncControl, SyncControls};

impl Pokarak {
    pub(super) fn render_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let width = window.viewport_size().width;
        let compact = width < ui_px(1240.).max(ui_text(1100.));
        let narrow = width < ui_px(900.).max(ui_text(805.));
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
                    toolbar_icon("add-project", IconName::Plus, "프로젝트 열기")
                        .on_click(cx.listener(|app, _, w, cx| app.choose_repository(w, cx)))
                        .shortcut_tooltip("프로젝트 열기", &OpenRepository, None),
                )
            })
            .child(div().flex_1())
            .when(self.current.is_some(), |bar| {
                if narrow {
                    return bar.child(self.view_options());
                }
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
                            .selected(self.session().is_some_and(|s| s.terminal_visible))
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.open_worktree_terminal(window, cx)
                            }))
                            .shortcut_tooltip(
                                "워크트리 터미널 표시 / 숨기기 · zsh",
                                &OpenWorktreeTerminal,
                                None,
                            ),
                    );
                bar.child(
                    toolbar_icon("refresh", IconName::RefreshCw, "저장소 새로고침")
                        .loading(self.loading)
                        .disabled(self.loading || self.git_is_busy())
                        .on_click(cx.listener(|app, _, w, cx| app.refresh(w, cx)))
                        .shortcut_tooltip("저장소 새로고침", &Refresh, None),
                )
            })
            .child(
                toolbar_icon("settings", IconName::Settings, "설정")
                    .on_click(cx.listener(|app, _, w, cx| app.open_settings(w, cx)))
                    .shortcut_tooltip("설정", &OpenSettings, None),
            );

        let title = TitleBar::new()
            .h(ui_px(42.))
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
                content_toolbar_icon(
                    "stage-commit",
                    IconName::GitCommitHorizontal,
                    "Stage / 커밋",
                )
                .disabled(self.loading || self.git_is_busy())
                .on_click(cx.listener(|a, _, w, cx| a.open_git_commit(w, cx)))
                .shortcut_tooltip("Stage / 커밋", &OpenGitCommit, None),
            )
            .child(
                content_toolbar_icon("compact-git-more", IconName::Ellipsis, "Git 작업 더 보기")
                    .tooltip("Git 작업 더 보기 · 브랜치, Stash, 원격 동기화")
                    .on_click(cx.listener(|a, _, w, cx| a.show_compact_git_menu(w, cx))),
            )
            .into_any_element()
    }

    fn show_compact_git_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.loading || self.git_is_busy();
        let branch = self
            .session()
            .is_some_and(|s| matches!(s.saved.comparison, CompareMode::Branch(_)));
        let mut menu = NativeMenu::new();
        let sync = SyncControls::for_worktree(self.active_sync_read(), self.active_worktree());
        let pull = sync.pull.menu_label();
        let push = sync.push.menu_label();
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
            (pull.as_str(), sync.pull.command),
            (push.as_str(), sync.push.command),
            ("Git 작업…", "actions"),
            ("Git 작업 기록…", "log"),
        ] {
            menu = menu.menu_with_disabled(
                keycaps::menu_label(label, &self.prefs, self.current_project.as_deref()),
                (busy && !matches!(command, "log" | "graph-detail"))
                    || (command.starts_with("graph-") && !self.git_history.active)
                    || (command == "head" && !branch)
                    || sync.blocked(command),
                Box::new(RunToolbar {
                    root: self.current.clone(),
                    generation: self.request,
                    command: command.into(),
                }),
            );
        }
        menu.show(window.mouse_position(), window, cx);
    }

    pub(super) fn file_review_menu_button(&self, cx: &mut Context<Self>) -> Button {
        let label = if self.snapshot().is_some() {
            self.review_counts()
        } else {
            "검토".into()
        };
        let only = self.show_unreviewed_only();
        let detail = if only {
            format!("{label} · 검토 메뉴 · 미검토만 표시 중")
        } else {
            format!("{label} · 검토 메뉴 · 미검토만 / 다음 미검토")
        };
        Button::new("files-review-count")
            .label(label)
            .accessibility_label(detail.clone())
            .tooltip(detail)
            .ghost()
            .xsmall()
            .h(ui_px(24.))
            .flex_initial()
            .ml_auto()
            .min_w(ui_px(68.))
            .px(ui_px(2.))
            .dropdown_caret(true)
            .selected(only)
            .when(only, |button| button.icon(IconName::ListFilter))
            .on_click(cx.listener(|app, event: &ClickEvent, window, cx| {
                let action = |command: &str| {
                    Box::new(RunToolbar {
                        root: app.current.clone(),
                        generation: app.request,
                        command: command.into(),
                    })
                };
                NativeMenu::new()
                    .menu_with_check(
                        "미검토만",
                        app.show_unreviewed_only(),
                        action("unreviewed-only"),
                    )
                    .menu_with_disabled(
                        "다음 미검토",
                        !app.can_open_next_unreviewed(),
                        action("next-unreviewed"),
                    )
                    .show(event.position(), window, cx);
            }))
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
        let changed_files =
            !self.git_history.active && self.session().is_some_and(|s| !s.saved.all_files);
        let sync = SyncControls::for_worktree(self.active_sync_read(), self.active_worktree());
        if sync.blocked(&action.command) {
            return;
        }
        match action.command.as_str() {
            "unreviewed-only" if changed_files => self.toggle_unreviewed_filter(cx),
            "next-unreviewed" if changed_files && self.can_open_next_unreviewed() => {
                self.next_unreviewed_file(w, cx)
            }
            "graph-filter" if self.git_history.active => self.open_branch_filter(w, cx),
            "graph-search" if self.git_history.active => self.open_graph_search(w, cx),
            "graph-detail" if self.git_history.active => {
                let collapsed = self
                    .session()
                    .is_some_and(|s| s.saved.graph_detail_collapsed);
                self.set_graph_detail_collapsed(!collapsed, cx);
            }
            "code" => self.show_code_view(w, cx),
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
            "publish" => {
                if let Some(tree) = self.active_worktree() {
                    self.open_publish_branch(tree.path.clone(), tree.branch.clone(), w, cx);
                }
            }
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

    /// Git actions shown in the content header: the Graph header row in Git mode
    /// and the right end of the file tab row in Code mode. The project titlebar
    /// keeps only project tabs and workspace controls.
    pub(in crate::ui) fn git_action_group(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if compact {
            return self.compact_git_toolbar(cx);
        }
        let busy = self.loading || self.git_is_busy();
        let branch = self
            .session()
            .is_some_and(|s| matches!(s.saved.comparison, CompareMode::Branch(_)));
        let weak = cx.entity().downgrade();
        let root = self.current.clone();
        let generation = self.request;
        let palette = Palette::new(cx);
        let graph = self.git_history.active;
        let detail_label = self.graph_detail_label();
        let (target, target_detail) = self.git_sync_summary();
        let sync = SyncControls::for_worktree(self.active_sync_read(), self.active_worktree());
        let changes = match self.change_count {
            Some((count, _)) if count > 0 => Some(count),
            _ => None,
        };
        let commit = theme::action_button("stage-commit", busy)
            .icon(IconName::GitCommitHorizontal)
            .label(match changes {
                Some(count) => format!("커밋 {count}"),
                None => "커밋".into(),
            })
            .accessibility_label(match changes {
                Some(count) => format!("Stage / 커밋 · 변경 파일 {count}개"),
                None => "Stage / 커밋".into(),
            })
            .small()
            .flex_none()
            .on_click(cx.listener(|app, _, w, cx| app.open_git_commit(w, cx)))
            .shortcut_tooltip("Stage / 커밋", &OpenGitCommit, None);
        let sync_button = |id: &'static str, icon: IconName, control: SyncControl| {
            let accessible = match control.count {
                Some(n) => format!("{} · {n}개", control.name),
                None => control.name.to_owned(),
            };
            let action = RunToolbar {
                root: self.current.clone(),
                generation,
                command: control.command.into(),
            };
            let tooltip = control.tooltip();
            let button = Button::new(id)
                .icon(icon)
                .when(control.command == "publish", |button| {
                    button.label(control.name)
                })
                .when_some(control.count, |button, n| button.label(n.to_string()))
                .accessibility_label(accessible)
                .ghost()
                .small()
                .h(ui_px(26.))
                .flex_none()
                .disabled(busy || control.disabled_reason.is_some())
                .on_click(
                    cx.listener(move |app, _, w, cx| app.run_toolbar_command(&action, w, cx)),
                );
            if control.disabled_reason.is_some() {
                // The disabled control stays inert. Its containing hitbox owns the hover.
                div()
                    .id((id, 0usize))
                    .flex_none()
                    .aria_description(tooltip.clone())
                    .child(button)
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(w, cx)
                    })
                    .into_any_element()
            } else {
                button.tooltip(tooltip).into_any_element()
            }
        };
        div()
            .id("git-action-group")
            .flex_none()
            .flex()
            .items_center()
            .gap(ui_px(6.))
            .child(commit)
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(ui_px(28.))
                    .px(ui_px(3.))
                    .gap(ui_px(2.))
                    .rounded(ui_px(6.))
                    .bg(palette.rgb(PANEL))
                    .child(
                        div()
                            .id("git-action-target")
                            .flex()
                            .items_center()
                            .gap(ui_px(5.))
                            .pl(ui_px(6.))
                            .pr(ui_px(4.))
                            .max_w(ui_px(180.))
                            .overflow_hidden()
                            .child(
                                Icon::new(IconName::GitBranch)
                                    .size(ui_px(13.))
                                    .text_color(palette.rgb(BLUE)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .text_size(ui_text(12.))
                                    .child(target),
                            )
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(target_detail.clone())
                                    .build(window, cx)
                            }),
                    )
                    .child(
                        content_toolbar_icon("toolbar-fetch", IconName::CloudDownload, "Fetch")
                            .tooltip("Fetch · 원격 브랜치 가져오기")
                            .disabled(busy)
                            .on_click(cx.listener(|app, _, w, cx| {
                                app.load_git_page(Some(Page::Fetch), w, cx)
                            })),
                    )
                    .child(sync_button(
                        "toolbar-pull",
                        IconName::ArrowDownToLine,
                        sync.pull,
                    ))
                    .child(sync_button(
                        "toolbar-push",
                        IconName::ArrowUpFromLine,
                        sync.push,
                    ))
                    .child(
                        content_toolbar_icon("toolbar-stash", IconName::Archive, "Stash")
                            .disabled(busy)
                            .on_click(cx.listener(|app, _, w, cx| app.open_git_workflow(w, cx)))
                            .shortcut_tooltip("Stash · 변경 보관 및 복원", &OpenGitWorkflow, None),
                    )
                    .child(
                        content_toolbar_icon("git-actions", IconName::Ellipsis, "Git 작업 더 보기")
                            .tooltip("Git 작업 더 보기 · 브랜치, Pop, Merge/Rebase, 작업 기록")
                            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                                let add =
                                    |menu: gpui_kit::component::menu::PopupMenu,
                                     label: &'static str,
                                     command: &'static str| {
                                        let target = weak.clone();
                                        let action = RunToolbar {
                                            root: root.clone(),
                                            generation,
                                            command: command.into(),
                                        };
                                        let disabled = (busy
                                            && !matches!(command, "log" | "graph-detail"))
                                            || (command.starts_with("graph-") && !graph)
                                            || (command == "head" && !branch);
                                        menu.item(
                                            keycaps::popup_item(label).disabled(disabled).on_click(
                                                move |_, w, cx| {
                                                    let _ = target.update(cx, |app, cx| {
                                                        app.run_toolbar_command(&action, w, cx);
                                                    });
                                                },
                                            ),
                                        )
                                    };
                                for (label, command) in [
                                    ("브랜치 만들기…", "create"),
                                    ("체크아웃…", "checkout"),
                                    ("Merge…", "merge"),
                                    ("Rebase…", "rebase"),
                                ] {
                                    menu = add(menu, label, command);
                                }
                                menu = menu.separator();
                                menu = add(menu, "Pop · 보관한 Stash 적용 후 제거…", "pop");
                                menu = menu.separator();
                                for (label, command) in [
                                    ("워크트리 선택…  ⌘⇧J", "worktree"),
                                    ("브랜치 전체 변경 비교…", "compare"),
                                    ("HEAD로 돌아가기", "head"),
                                ] {
                                    menu = add(menu, label, command);
                                }
                                if graph {
                                    menu = menu.separator();
                                    for (label, command) in [
                                        ("그래프 브랜치 필터…", "graph-filter"),
                                        ("커밋 검색…  ⌘F", "graph-search"),
                                        (detail_label, "graph-detail"),
                                    ] {
                                        menu = add(menu, label, command);
                                    }
                                }
                                menu = menu.separator();
                                menu = add(menu, "Git 작업…", "actions");
                                add(menu, "Git 작업 기록…", "log")
                            }),
                    ),
            )
            .into_any_element()
    }

    // Keep the active project visible when the window is narrow or zoomed in.
    fn view_options(&self) -> impl IntoElement {
        let graph = self.git_history.active;
        let pending = self.opening_path.is_some() || self.session().is_none();
        let can_go_back = self.session().is_some_and(|s| !s.history.is_empty());
        let can_go_forward = self.session().is_some_and(|s| !s.forward.is_empty());
        let busy = self.loading || self.git_is_busy();
        let focus = self.focus.clone();
        let show_code = RunToolbar {
            root: self.current.clone(),
            generation: self.request,
            command: "code".into(),
        };
        let icon = if graph {
            IconName::GitGraph
        } else {
            IconName::CodeXml
        };
        toolbar_button("toolbar-view-options", icon, "보기 옵션")
            .label("보기")
            .tooltip("Git 그래프, 코드 보기 및 패널")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let menu = menu
                    .action_context(focus.clone())
                    .menu_with_check_and_disabled(
                        "Git 그래프",
                        graph,
                        Box::new(OpenGitHistory),
                        graph || pending || busy,
                    )
                    .menu_with_check_and_disabled(
                        "코드 보기",
                        !graph,
                        Box::new(show_code.clone()),
                        !graph || pending || busy,
                    )
                    .separator()
                    .menu_with_disabled("사이드바 표시 / 숨기기", Box::new(ToggleSidebar), pending)
                    .menu_with_disabled(
                        "파일 목록 표시 / 숨기기",
                        Box::new(ToggleFiles),
                        graph || pending,
                    );
                #[cfg(target_os = "macos")]
                let menu = menu.menu_with_disabled(
                    "워크트리 터미널",
                    Box::new(OpenWorktreeTerminal),
                    pending,
                );
                menu.separator()
                    .when(!graph, |menu| {
                        menu.menu_with_disabled("뒤로", Box::new(Back), !can_go_back)
                            .menu_with_disabled("앞으로", Box::new(Forward), !can_go_forward)
                    })
                    .menu_with_disabled("저장소 새로고침", Box::new(Refresh), busy)
            })
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
                toolbar_icon("view-graph", IconName::GitGraph, "Git 그래프")
                    .when(!compact, |button| {
                        button.w_auto().px(ui_px(7.)).label("Git 그래프")
                    })
                    .selected(graph)
                    .toggled(graph)
                    .disabled(
                        self.opening_path.is_some()
                            || self.git_is_busy()
                            || (self.loading && graph),
                    )
                    .on_click(cx.listener(|app, _, w, cx| {
                        if !app.git_history.active {
                            app.open_git_history(w, cx);
                        }
                    }))
                    .shortcut_tooltip("Git 그래프", &OpenGitHistory, None),
            )
            .child(
                toolbar_icon("view-code", IconName::CodeXml, "코드 보기")
                    .tooltip("코드 보기")
                    .when(!compact, |button| {
                        button.w_auto().px(ui_px(7.)).label("코드 보기")
                    })
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
                toolbar_icon("back", IconName::ArrowLeft, "뒤로")
                    .disabled(session.is_none_or(|s| s.history.is_empty()))
                    .on_click(cx.listener(|app, _, w, cx| app.navigate_history(true, w, cx)))
                    .shortcut_tooltip("뒤로", &Back, None),
            )
            .child(
                toolbar_icon("forward", IconName::ArrowRight, "앞으로")
                    .disabled(session.is_none_or(|s| s.forward.is_empty()))
                    .on_click(cx.listener(|app, _, w, cx| app.navigate_history(false, w, cx)))
                    .shortcut_tooltip("앞으로", &Forward, None),
            )
    }
}

pub(super) fn toolbar_icon(id: &'static str, icon: IconName, name: &'static str) -> Button {
    toolbar_button(id, icon, name).w(ui_px(32.)).px_0()
}

fn toolbar_button(id: &'static str, icon: IconName, name: &'static str) -> Button {
    Button::new(id)
        .icon(icon)
        .accessibility_label(name)
        .ghost()
        .with_size(ui_px(28.))
        .h(ui_px(32.))
        .px(ui_px(5.))
        .flex_none()
}

fn content_toolbar_icon(id: &'static str, icon: IconName, name: &'static str) -> Button {
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
