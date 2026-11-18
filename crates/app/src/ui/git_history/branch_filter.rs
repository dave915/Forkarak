use super::*;
use pokarak_core::history::navigation::{Catalog, GraphRef, RefGroup};

pub(super) fn branch_label(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/"))
        .unwrap_or(name)
}

impl Pokarak {
    pub(super) fn graph_branch(&self) -> Option<&str> {
        self.session()?.saved.graph_branch.as_deref()
    }

    pub(super) fn save_graph_branch(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if let Some(session) = self
            .current
            .as_ref()
            .and_then(|root| self.sessions.get_mut(root))
        {
            session.saved.graph_branch = name;
            self.persist(cx);
        }
    }

    pub(super) fn branch_filter_control(&self, cx: &mut Context<Self>) -> Button {
        let name = self
            .graph_branch()
            .map(branch_label)
            .unwrap_or("전체 브랜치");
        Button::new("graph-branch-filter")
            .ghost()
            .small()
            .max_w(ui_px(205.))
            .min_w_0()
            .selected(self.graph_branch().is_some())
            .accessibility_label(format!("커밋 그래프 브랜치 필터 · {name}"))
            .tooltip(format!("그래프에 표시할 브랜치 · {name}"))
            .disabled(self.git_history.nav.catalog.is_none() || self.loading || self.git_is_busy())
            .child(
                div()
                    .w_full()
                    .text_left()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::GitBranch).size(ui_px(14.)))
                    .child(div().min_w_0().text_ellipsis().child(name.to_owned()))
                    .child(Icon::new(IconName::ChevronDown).size(ui_px(12.))),
            )
            .on_click(cx.listener(|app, _, w, cx| app.open_branch_filter(w, cx)))
    }

    pub(in crate::ui) fn open_branch_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(root), Some(catalog)) =
            (self.current.clone(), self.git_history.nav.catalog.clone())
        else {
            return;
        };
        if self.loading
            || self.git_is_busy()
            || !self.git_history.active
            || window.has_active_dialog(cx)
        {
            return;
        }
        let owner = cx.entity().downgrade();
        let selected = self.graph_branch().map(str::to_owned);
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("로컬·원격 브랜치 이름 검색"));
        let picker = cx.new(|cx| {
            let subscription = cx.subscribe_in(
                &input,
                window,
                |picker: &mut BranchPicker, input, event: &InputEvent, w, cx| match event {
                    InputEvent::Change => picker.filter(input.read(cx).value().as_ref(), cx),
                    InputEvent::PressEnter { .. }
                        if !picker.matches.is_empty()
                            || input.read(cx).value().trim().is_empty() =>
                    {
                        picker.choose(picker.cursor, w, cx)
                    }
                    _ => {}
                },
            );
            let matches = matching_branches(&catalog.refs, "");
            let cursor = selected
                .as_ref()
                .and_then(|s| matches.iter().position(|&i| catalog.refs[i].name == *s))
                .map_or(0, |i| i + 1);
            let scroll = UniformListScrollHandle::new();
            scroll.scroll_to_item(cursor, ScrollStrategy::Nearest);
            BranchPicker {
                owner,
                root,
                catalog,
                selected,
                input: input.clone(),
                matches: matches.into(),
                cursor,
                scroll,
                _subscription: subscription,
            }
        });
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("그래프 브랜치 필터")
                .on_ok(|_, _, _| false)
                .width(ui_px(470.))
                .child(picker.clone())
        });
        input.update(cx, |input, cx| input.focus(window, cx));
    }
}

fn matching_branches(refs: &[GraphRef], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    refs.iter()
        .enumerate()
        .filter(|(_, r)| {
            matches!(r.group, RefGroup::Local | RefGroup::Remote)
                && (query.is_empty() || r.label.to_lowercase().contains(&query))
        })
        .map(|(i, _)| i)
        .collect()
}

struct BranchPicker {
    owner: WeakEntity<Pokarak>,
    root: PathBuf,
    catalog: Arc<Catalog>,
    selected: Option<String>,
    input: Entity<InputState>,
    matches: Arc<Vec<usize>>,
    cursor: usize,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl BranchPicker {
    fn filter(&mut self, query: &str, cx: &mut Context<Self>) {
        self.matches = matching_branches(&self.catalog.refs, query).into();
        self.cursor = usize::from(!query.trim().is_empty() && !self.matches.is_empty());
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn choose(&self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let name = if row == 0 {
            None
        } else {
            let Some(&index) = self.matches.get(row - 1) else {
                return;
            };
            Some(self.catalog.refs[index].name.clone())
        };
        window.close_dialog(cx);
        let _ = self.owner.update(cx, |app, cx| {
            if app.current.as_ref() != Some(&self.root)
                || app.loading
                || app.git_is_busy()
                || !app.git_history.active
                || !app
                    .git_history
                    .nav
                    .catalog
                    .as_ref()
                    .is_some_and(|c| Arc::ptr_eq(c, &self.catalog))
                || app.graph_branch() == name.as_deref()
            {
                return;
            }
            app.save_graph_branch(name, cx);
            app.open_git_history(window, cx);
        });
    }
}

impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::new(cx);
        let weak = cx.entity().downgrade();
        let matches = self.matches.clone();
        let catalog = self.catalog.clone();
        let selected = self.selected.clone();
        let cursor = self.cursor;
        let count = matches.len() + 1;
        let list = uniform_list("graph-branch-options", count, move |range, _, _| {
            range
                .map(|i| {
                    let branch = i.checked_sub(1).map(|row| &catalog.refs[matches[row]]);
                    let label = branch.map_or("전체 브랜치", |r| &r.label);
                    let group = branch.map(|r| {
                        if r.group == RefGroup::Local {
                            "로컬"
                        } else {
                            "원격"
                        }
                    });
                    let current = branch.map(|r| r.name.as_str()) == selected.as_deref();
                    let weak = weak.clone();
                    Button::new(("graph-branch-option", i))
                        .small()
                        .ghost()
                        .w_full()
                        .h(ui_px(29.))
                        .selected(i == cursor)
                        .accessibility_label(format!(
                            "{} {label}{}",
                            group.unwrap_or(""),
                            if current { " · 적용 중" } else { "" }
                        ))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_ellipsis()
                                        .child(label.to_owned()),
                                )
                                .when_some(group, |r, g| {
                                    r.child(
                                        div()
                                            .text_color(palette.rgb(MUTED))
                                            .text_size(ui_text(11.))
                                            .child(g),
                                    )
                                })
                                .when(current, |r| {
                                    r.child(
                                        Icon::new(IconName::Check)
                                            .size(ui_px(13.))
                                            .text_color(palette.rgb(BLUE)),
                                    )
                                }),
                        )
                        .on_click(move |_, w, cx| {
                            let _ = weak.update(cx, |picker, cx| picker.choose(i, w, cx));
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .h(ui_px((count.min(9) * 29) as f32));
        div()
            .v_flex()
            .gap_2()
            .capture_key_down(cx.listener(|picker, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "down" => picker.cursor = (picker.cursor + 1).min(picker.matches.len()),
                    "up" => picker.cursor = picker.cursor.saturating_sub(1),
                    _ => return,
                }
                picker
                    .scroll
                    .scroll_to_item(picker.cursor, ScrollStrategy::Nearest);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(Input::new(&self.input).small())
            .child(list)
            .when(self.matches.is_empty(), |r| {
                r.child(
                    div()
                        .text_color(palette.rgb(MUTED))
                        .child("일치하는 브랜치가 없습니다."),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{GraphRef, RefGroup, matching_branches};

    #[test]
    fn filter_matches_local_and_remote_display_names_case_insensitively() {
        let refs = [
            (RefGroup::Local, "Feature/Release"),
            (RefGroup::Local, "other"),
            (RefGroup::Remote, "origin/FEATURE/release"),
            (RefGroup::Tag, "release"),
            (RefGroup::Stash, "stash release"),
        ]
        .map(|(group, label)| GraphRef {
            group,
            label: label.into(),
            name: String::new(),
            oid: String::new(),
        });
        assert_eq!(matching_branches(&refs, " ReLeAsE "), [0, 2]);
        assert_eq!(matching_branches(&refs, "origin/"), [2]);
        assert_eq!(matching_branches(&refs, ""), [0, 1, 2]);
        assert!(matching_branches(&refs, "no-match").is_empty());
    }
}
