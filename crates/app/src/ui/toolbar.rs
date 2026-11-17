use super::*;
use gpui_kit::component::{
    TitleBar,
    tab::{Tab, TabBar},
};

impl Pokarak {
    pub(super) fn render_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Palette::new(cx);
        let compact = window.viewport_size().width < px(1100.);
        let has_projects = !self.projects.is_empty();
        let content = div()
            .size_full()
            .min_w_0()
            .px_2()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                Icon::new(IconName::Utensils)
                    .data(include_bytes!("../../../../assets/icons/spork.svg").as_slice())
                    .size(px(22.))
                    .text_color(palette.rgb(BLUE)),
            )
            .when(!has_projects, |bar| {
                bar.child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("포카락"),
                )
                .child(div().flex_1())
            })
            .when(has_projects, |bar| {
                bar.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.project_tabs(window, cx)),
                )
                .child(
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
                bar.child(divider(palette))
                    .child(self.workspace_modes(compact, cx))
                    .when(!self.git_history.active, |bar| {
                        bar.child(self.reading_navigation(cx))
                    })
                    .child(divider(palette))
                    .child(
                        Button::new("stage-commit")
                            .label("Stage / 커밋")
                            .when(!compact, |button| {
                                button.icon(IconName::GitCommitHorizontal)
                            })
                            .small()
                            .disabled(self.loading || self.git_is_busy())
                            .tooltip_with_action("변경사항 스테이징 및 커밋", &OpenGitCommit, None)
                            .on_click(cx.listener(|app, _, w, cx| app.open_git_commit(w, cx))),
                    )
                    .child(
                        Button::new("git-actions")
                            .label(if self.git_is_busy() {
                                "Git 작업 중…"
                            } else {
                                "Git 작업"
                            })
                            .icon(IconName::GitBranch)
                            .ghost()
                            .small()
                            .disabled(self.loading || self.git_is_busy())
                            .tooltip("브랜치 작업·원격 동기화·저장소 설정")
                            .on_click(cx.listener(|app, _, w, cx| app.open_git_actions(w, cx))),
                    )
                    .child(divider(palette))
                    .child(
                        Button::new("refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .small()
                            .loading(self.loading)
                            .disabled(self.loading || self.git_is_busy())
                            .accessibility_label("저장소 새로고침")
                            .tooltip_with_action("저장소 새로고침", &Refresh, None)
                            .on_click(cx.listener(|app, _, w, cx| app.refresh(w, cx))),
                    )
            })
            .child(
                Button::new("settings")
                    .icon(IconName::Settings)
                    .ghost()
                    .small()
                    .accessibility_label("설정")
                    .tooltip_with_action("설정", &OpenSettings, None)
                    .on_click(cx.listener(|app, _, w, cx| app.open_settings(w, cx))),
            );

        TitleBar::new()
            .h(px(48.))
            .pl(if window.is_fullscreen() {
                px(0.)
            } else {
                px(80.)
            })
            .bg(palette.rgb(SIDE))
            .border_b_1()
            .border_color(palette.rgb(LINE))
            .child(content)
            .into_any_element()
    }

    fn project_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> TabBar {
        let palette = Palette::new(cx);
        let active = self
            .projects
            .iter()
            .position(|p| self.current_project.as_ref() == Some(&p.root));
        let state = window.use_keyed_state("project-tab-reveal", cx, |_, _| {
            (None::<usize>, ScrollHandle::new())
        });
        let scroll = state.read(cx).1.clone();
        if state.read(cx).0 != active {
            state.update(cx, |state, _| state.0 = active);
            if let Some(index) = active {
                scroll.scroll_to_item(index);
            }
        }

        TabBar::new("project-tabs")
            .underline()
            .small()
            .min_w_0()
            .w_full()
            .menu(self.projects.len() > 1)
            .max_width(px(200.))
            .track_scroll(&scroll)
            .when_some(active, |bar, index| bar.selected_index(index))
            .children(self.projects.iter().enumerate().map(|(index, project)| {
                let target = project.last_worktree.clone();
                let close = project.root.clone();
                let path = project.root.to_string_lossy().into_owned();
                let label = project
                    .root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                // Tab::icon renders an icon-only tab. The prefix lets the label
                // determine the width while keeping both actions visible.
                Tab::new()
                    .min_w(px(112.))
                    .prefix(
                        Icon::new(IconName::FolderGit2).size(px(14.)).text_color(
                            palette.rgb(if active == Some(index) { BLUE } else { MUTED }),
                        ),
                    )
                    .label(label.clone())
                    .aria_label(format!("프로젝트 {path}"))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(w, cx)
                    })
                    .suffix(
                        Button::new(("close-project", index))
                            .icon(
                                Icon::new(IconName::X)
                                    .size(px(13.))
                                    .text_color(palette.rgb(MUTED)),
                            )
                            .ghost()
                            .xsmall()
                            .accessibility_label(format!("{label} 프로젝트 닫기"))
                            .tooltip(format!("{label} 프로젝트 닫기"))
                            .on_click(cx.listener(move |app, _, w, cx| {
                                cx.stop_propagation();
                                app.close_project(close.clone(), w, cx);
                            })),
                    )
                    .on_click(cx.listener(move |app, _, w, cx| {
                        if app.current.as_ref() != Some(&target) {
                            app.open_repository(target.clone(), w, cx);
                        }
                    }))
            }))
    }

    fn workspace_modes(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let graph = self.git_history.active;
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(6.))
            .bg(palette.rgb(BG))
            .border_1()
            .border_color(palette.rgb(LINE))
            .child(
                Button::new("view-graph")
                    .label("Git 그래프")
                    .when(!compact, |button| button.icon(IconName::GitGraph))
                    .ghost()
                    .compact()
                    .small()
                    .selected(graph)
                    .toggled(graph)
                    .disabled(self.loading || self.git_is_busy())
                    .tooltip_with_action("Git 그래프", &OpenGitHistory, None)
                    .on_click(cx.listener(|app, _, w, cx| {
                        if !app.git_history.active {
                            app.open_git_history(w, cx);
                        }
                    })),
            )
            .child(
                Button::new("view-code")
                    .label("코드 보기")
                    .when(!compact, |button| button.icon(IconName::CodeXml))
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

fn divider(palette: Palette) -> Div {
    div().w(px(1.)).h(px(18.)).flex_none().bg(palette.rgb(LINE))
}
