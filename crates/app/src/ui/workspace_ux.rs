//! Worktree-scoped screen restoration and searchable native navigation.
use super::*;
use crate::prefs::UpdateView;
use crate::ui::dialogs::DialogWindowExt as _;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Screen {
    view: UpdateView,
    #[serde(default)]
    split_diff: Option<bool>,
}

fn screen_path() -> PathBuf {
    crate::prefs::config_path().with_file_name("workspace-screens.json")
}

fn read_screens() -> HashMap<PathBuf, Screen> {
    std::fs::read(screen_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(super) fn tab_label(path: &Path, tabs: &[PathBuf]) -> String {
    let name = path.file_name().unwrap_or_default();
    if tabs.iter().filter(|p| p.file_name() == Some(name)).count() < 2 {
        return name.to_string_lossy().into_owned();
    }
    // Include the shortest unique suffix, even when immediate parents match.
    let components: Vec<_> = path.components().collect();
    for count in 2..=components.len() {
        let suffix: PathBuf = components[components.len() - count..].iter().collect();
        if tabs.iter().filter(|p| p.ends_with(&suffix)).count() == 1 {
            return suffix.to_string_lossy().into_owned();
        }
    }
    path.to_string_lossy().into_owned()
}

fn review_preferences(
    remembered: Option<SavedView>,
    remembered_split: Option<bool>,
    legacy_split: bool,
    default_split: bool,
    default_hide: bool,
) -> (SavedView, bool) {
    let known = remembered.is_some();
    let mut saved = remembered.unwrap_or_default();
    if !known {
        saved.diff_hide_whitespace = default_hide;
    }
    let split = remembered_split.unwrap_or(if known { legacy_split } else { default_split });
    (saved, split)
}

impl Pokarak {
    pub(super) fn initialize_review_preferences(
        &mut self,
        root: &Path,
        remembered: Option<SavedView>,
    ) -> SavedView {
        let saved_split = read_screens()
            .get(root)
            .and_then(|screen| screen.split_diff);
        let default_split =
            self.prefs
                .option_bool(self.current_project.as_deref(), "review.split_diff", false);
        let default_hide = self.prefs.option_bool(
            self.current_project.as_deref(),
            "review.hide_whitespace",
            false,
        );
        let (saved, split) = review_preferences(
            remembered,
            saved_split,
            self.legacy_split_diff,
            default_split,
            default_hide,
        );
        self.prefs.split_diff = split;
        saved
    }

    pub(super) fn remember_workspace_screen(&self) {
        if self.opening_path.is_some() {
            return;
        }
        let Some(root) = &self.current else {
            return;
        };
        let view = if self.inline_commit_active() {
            UpdateView::Changes
        } else if self.git_history.active {
            UpdateView::Graph(self.git_history.selection())
        } else {
            UpdateView::Code
        };
        let mut screens = read_screens();
        screens.insert(
            root.clone(),
            Screen {
                view,
                split_diff: Some(self.prefs.split_diff),
            },
        );
        let path = screen_path();
        if let Some(parent) = path.parent()
            && std::fs::create_dir_all(parent).is_ok()
            && let Ok(mut file) = tempfile::NamedTempFile::new_in(parent)
            && serde_json::to_writer(file.as_file_mut(), &screens).is_ok()
        {
            let _ = file.persist(path);
        }
    }

    pub(super) fn open_workspace_screen(
        &mut self,
        restart: Option<UpdateView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let restore = self.prefs.option_bool(
            self.current_project.as_deref(),
            "workspace.restore_last",
            true,
        );
        if !restore
            && restart.is_none()
            && let Some(root) = &self.current
            && let Some(session) = self.sessions.get_mut(root)
        {
            session.saved.selected = None;
            session.saved.tabs.clear();
            session.result_tab = None;
            session.saved.positions.clear();
            session.saved.line = 0;
            session.saved.column = 0;
            session.saved.diff_row = 0;
            session.saved.scroll_x = 0.;
            session.saved.scroll_y = 0.;
            session.documents.clear();
            session.diffs.clear();
            session.history.clear();
            session.forward.clear();
            session.restoring = false;
        }
        let view = restart.or_else(|| {
            restore
                .then(|| {
                    self.current
                        .as_ref()
                        .and_then(|root| read_screens().remove(root).map(|s| s.view))
                })
                .flatten()
        });
        if let Some(view) = view {
            self.open_restored_view(Some(view), window, cx);
            return;
        }
        if self.prefs.option_string(
            self.current_project.as_deref(),
            "workspace.default_view",
            "git",
        ) == "editor"
        {
            if let Some(root) = &self.current
                && let Some(session) = self.sessions.get_mut(root)
            {
                session.saved.code = true;
                session.saved.all_files = true;
                if !restore {
                    session.saved.selected = None;
                    session.restoring = false;
                }
            }
            self.show_code_view(window, cx);
        } else {
            self.open_git_history(window, cx);
        }
    }

    pub(super) fn close_active_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_history.active {
            self.close_history_file(window, cx);
        } else if self.result_tab_active() {
            self.close_result_tab(window, cx);
        } else if let Some(path) = self.selected() {
            self.close_file(path, window, cx);
        }
    }

    pub(super) fn open_worktree_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.current.is_none() || self.opening_path.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let rows = self
            .inventory
            .iter()
            .filter(|t| !t.is_prunable)
            .map(|t| Choice {
                label: format!(
                    "{} · {}{}",
                    t.path.file_name().unwrap_or_default().to_string_lossy(),
                    t.branch,
                    if self.current.as_ref() == Some(&t.path) {
                        " · 현재 작업"
                    } else {
                        ""
                    }
                ),
                detail: t.path.display().to_string(),
                action: ChoiceAction::Worktree(t.path.clone()),
            })
            .collect();
        self.open_workspace_choices(
            "워크트리 열기",
            "워크트리 이름·브랜치·경로 검색",
            rows,
            window,
            cx,
        );
    }

    pub(super) fn open_workspace_choices(
        &self,
        title: &'static str,
        placeholder: &'static str,
        rows: Vec<Choice>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let owner = cx.weak_entity();
        let generation = self.request;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let picker = cx.new(|cx| {
            let subscription = cx.subscribe_in(
                &input,
                window,
                |this: &mut Picker, input, event: &InputEvent, w, cx| match event {
                    InputEvent::Change => {
                        let query = input.read(cx).value().to_lowercase();
                        this.matches = this
                            .rows
                            .iter()
                            .enumerate()
                            .filter(|(_, r)| {
                                format!("{} {}", r.label, r.detail)
                                    .to_lowercase()
                                    .contains(query.trim())
                            })
                            .map(|(i, _)| i)
                            .collect();
                        this.cursor = 0;
                        this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.choose(this.cursor, w, cx),
                    _ => {}
                },
            );
            Picker {
                scroll: UniformListScrollHandle::new(),
                owner,
                root,
                generation,
                matches: (0..rows.len()).collect(),
                rows,
                input: input.clone(),
                cursor: 0,
                _subscription: subscription,
            }
        });
        window.open_app_dialog(cx, move |dialog, _, _| {
            dialog
                .title(title)
                .width(ui_px(600.))
                .on_ok(|_, _, _| false)
                .child(picker.clone())
        });
        input.update(cx, |i, cx| i.focus(window, cx));
    }

    pub(super) fn visible_comparison_label(&self) -> String {
        if self.inline_commit_active() {
            return "스테이징 · HEAD ↔ Index ↔ 작업 파일".into();
        }
        if self.git_history.active {
            if let Some((detail, _)) = self.git_history.review_data() {
                return format!(
                    "커밋 비교 · {} → {} · 읽기 전용",
                    detail
                        .parent
                        .as_deref()
                        .map(|s| &s[..s.len().min(8)])
                        .unwrap_or("빈 트리"),
                    &detail.commit.oid[..detail.commit.oid.len().min(8)]
                );
            }
            return "Git 그래프 · 커밋을 선택하세요".into();
        }
        format!("코드 비교 · {}", self.applied_comparison_label())
    }
}

#[derive(Clone)]
pub(super) enum ChoiceAction {
    Worktree(PathBuf),
    Compare(String),
}
#[derive(Clone)]
pub(super) struct Choice {
    pub label: String,
    pub detail: String,
    pub action: ChoiceAction,
}
struct Picker {
    scroll: UniformListScrollHandle,
    owner: WeakEntity<Pokarak>,
    root: PathBuf,
    generation: u64,
    rows: Vec<Choice>,
    matches: Vec<usize>,
    input: Entity<InputState>,
    cursor: usize,
    _subscription: Subscription,
}
impl Picker {
    fn choose(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .matches
            .get(index)
            .and_then(|i| self.rows.get(*i))
            .cloned()
        else {
            return;
        };
        let _ = self.owner.update(cx, |app, cx| {
            if app.current.as_ref() != Some(&self.root)
                || app.request != self.generation
                || app.opening_path.is_some()
            {
                return;
            }
            window.close_dialog(cx);
            match row.action {
                ChoiceAction::Worktree(path) => {
                    if app
                        .inventory
                        .iter()
                        .any(|t| t.path == path && !t.is_prunable)
                    {
                        app.open_repository(path, window, cx);
                    }
                }
                ChoiceAction::Compare(name) => app.apply_branch_review(
                    self.root.clone(),
                    CompareMode::Branch(name),
                    window,
                    cx,
                ),
            }
        });
    }
}
impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: AnyElement = if self.matches.is_empty() {
            div()
                .p_2()
                .child("일치하는 항목이 없습니다.")
                .into_any_element()
        } else {
            let entries = Arc::new(
                self.matches
                    .iter()
                    .map(|&index| self.rows[index].clone())
                    .collect::<Vec<_>>(),
            );
            let weak = cx.weak_entity();
            let cursor = self.cursor;
            uniform_list(
                "workspace-choice-rows",
                entries.len(),
                move |range, _, _| {
                    range
                        .map(|index| {
                            let row = &entries[index];
                            let target = weak.clone();
                            Button::new(("workspace-choice", index))
                                .ghost()
                                .small()
                                .w_full()
                                .h(ui_px(48.))
                                .justify_start()
                                .selected(index == cursor)
                                .accessibility_label(format!("{} {}", row.label, row.detail))
                                .child(
                                    dialogs::choice_body()
                                        .v_flex()
                                        .child(div().text_ellipsis().child(row.label.clone()))
                                        .child(
                                            div()
                                                .text_size(ui_text(10.))
                                                .text_ellipsis()
                                                .child(row.detail.clone()),
                                        ),
                                )
                                .on_click(move |_, w, cx| {
                                    let _ = target.update(cx, |this, cx| this.choose(index, w, cx));
                                })
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.scroll)
            .h(ui_px(310.))
            .into_any_element()
        };
        div()
            .v_flex()
            .gap_2()
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let count = this.matches.len();
                if count == 0 {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "down" => this.cursor = (this.cursor + 1).min(count - 1),
                    "up" => this.cursor = this.cursor.saturating_sub(1),
                    _ => return,
                }
                this.scroll
                    .scroll_to_item(this.cursor, ScrollStrategy::Nearest);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(Input::new(&self.input))
            .child(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::{HashMap, Path, PathBuf, Screen, UpdateView, tab_label};
    use crate::prefs::SavedCommit;
    #[test]
    fn duplicate_tab_names_use_shortest_unique_suffix() {
        let tabs = vec![
            PathBuf::from("a/src/main.rs"),
            PathBuf::from("b/src/main.rs"),
            PathBuf::from("lib.rs"),
        ];
        assert_eq!(tab_label(&tabs[0], &tabs), "a/src/main.rs");
        assert_eq!(tab_label(&tabs[2], &tabs), "lib.rs");
    }
    #[test]
    fn screen_data_is_exact_worktree_scoped_and_preserves_commit_parent() {
        let mut screens = HashMap::new();
        screens.insert(
            PathBuf::from("/a"),
            Screen {
                split_diff: Some(false),
                view: UpdateView::Graph(Some(SavedCommit {
                    oid: "abc".into(),
                    parent: 1,
                    file: Some("src/lib.rs".into()),
                    code: true,
                })),
            },
        );
        screens.insert(
            PathBuf::from("/b"),
            Screen {
                view: UpdateView::Code,
                split_diff: Some(true),
            },
        );
        let data = serde_json::to_vec(&screens).unwrap();
        let restored: HashMap<PathBuf, Screen> = serde_json::from_slice(&data).unwrap();
        assert!(matches!(restored[Path::new("/b")].view, UpdateView::Code));
        assert!(
            matches!(&restored[Path::new("/a")].view,UpdateView::Graph(Some(s)) if s.parent==1 && s.code)
        );
    }
    #[test]
    fn review_defaults_apply_only_to_new_worktrees_including_false_choices() {
        use super::{SavedView, review_preferences};
        let (fresh, split) = review_preferences(None, None, false, true, true);
        assert!(split && fresh.diff_hide_whitespace);
        let (fresh, split) = review_preferences(None, None, true, false, false);
        assert!(!split && !fresh.diff_hide_whitespace);
        let (known, split) =
            review_preferences(Some(SavedView::default()), Some(false), true, true, true);
        assert!(!split && !known.diff_hide_whitespace);
        let (known, split) = review_preferences(
            Some(SavedView {
                diff_hide_whitespace: true,
                ..Default::default()
            }),
            Some(true),
            false,
            false,
            false,
        );
        assert!(split && known.diff_hide_whitespace);
    }
    #[test]
    fn legacy_worktrees_keep_old_diff_mode_and_screen_records_still_decode() {
        use super::{SavedView, review_preferences};
        let (known, split) =
            review_preferences(Some(SavedView::default()), None, true, false, true);
        assert!(split);
        assert!(!known.diff_hide_whitespace);
        let screen: Screen = serde_json::from_str(r#"{"view":"Code"}"#).unwrap();
        assert!(screen.split_diff.is_none());
        let mut screens = HashMap::new();
        screens.insert(
            PathBuf::from("/a"),
            Screen {
                view: UpdateView::Code,
                split_diff: Some(false),
            },
        );
        screens.insert(
            PathBuf::from("/b"),
            Screen {
                view: UpdateView::Code,
                split_diff: Some(true),
            },
        );
        let restored: HashMap<PathBuf, Screen> =
            serde_json::from_slice(&serde_json::to_vec(&screens).unwrap()).unwrap();
        assert_eq!(restored[Path::new("/a")].split_diff, Some(false));
        assert_eq!(restored[Path::new("/b")].split_diff, Some(true));
    }
}
