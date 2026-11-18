use super::*;
use gpui_kit::component::tooltip::Tooltip;
use std::{collections::BTreeMap, ffi::OsString};

#[derive(Clone, Debug)]
pub(super) struct FileRow {
    pub path: PathBuf,
    pub label: String,
    pub depth: usize,
    pub directory: bool,
    pub expanded: bool,
    pub change: Option<ChangedFile>,
}

#[derive(Default)]
struct FileTree {
    rows: Vec<FileRow>,
    // Counts and controls must include files hidden by collapsed ancestors.
    file_count: usize,
    directories: HashSet<PathBuf>,
}

fn file_mode_labels(snapshot: Option<&Snapshot>, all: bool, visible: usize) -> [String; 2] {
    let Some(snapshot) = snapshot else {
        return ["변경".into(), "전체".into()];
    };
    let label = |name: &str, selected: bool, total: usize| {
        if selected && visible < total {
            format!("{name} {visible}/{total}")
        } else {
            format!("{name} {total}")
        }
    };
    [
        label("변경", !all, snapshot.changes.len()),
        label("전체", all, snapshot.files.len()),
    ]
}

#[derive(Default)]
struct Node {
    children: BTreeMap<OsString, Node>,
    file: bool,
    change: Option<ChangedFile>,
}

fn build_tree(
    snapshot: &Snapshot,
    all_files: bool,
    filter: &str,
    collapsed: &HashSet<PathBuf>,
) -> FileTree {
    let changes: HashMap<_, _> = snapshot.changes.iter().map(|f| (&f.path, f)).collect();
    // A map deduplicates entries and gives filtered paths a stable order too.
    let paths: BTreeMap<_, _> = if all_files {
        snapshot
            .files
            .iter()
            .map(|p| (p, changes.get(p).copied()))
            .collect()
    } else {
        snapshot
            .changes
            .iter()
            .map(|f| (&f.path, Some(f)))
            .collect()
    };
    let mut tree = FileTree::default();
    let mut root = Node::default();
    let query = filter.to_lowercase();
    for (path, change) in paths {
        if !query.is_empty() && !path.to_string_lossy().to_lowercase().contains(&query) {
            continue;
        }
        tree.file_count += 1;
        if !query.is_empty() {
            // Search reveals the full path without touching the remembered tree.
            tree.rows.push(FileRow {
                path: path.clone(),
                label: path.to_string_lossy().into_owned(),
                depth: 0,
                directory: false,
                expanded: false,
                change: change.cloned(),
            });
            continue;
        }
        let mut node = &mut root;
        let mut directory = PathBuf::new();
        let parts: Vec<_> = path.components().collect();
        for (index, part) in parts.iter().enumerate() {
            node = node
                .children
                .entry(part.as_os_str().to_owned())
                .or_default();
            directory.push(part.as_os_str());
            if index + 1 < parts.len() {
                tree.directories.insert(directory.clone());
            }
        }
        node.file = true;
        node.change = change.cloned();
    }
    if query.is_empty() {
        append_children(&root, Path::new(""), 0, collapsed, &mut tree.rows);
    }
    tree
}

fn append_children(
    parent: &Node,
    path: &Path,
    depth: usize,
    collapsed: &HashSet<PathBuf>,
    rows: &mut Vec<FileRow>,
) {
    // Directories first, then files; each group is ordered by its actual name.
    for directory in [true, false] {
        for (name, node) in &parent.children {
            if (directory && node.children.is_empty()) || (!directory && !node.file) {
                continue;
            }
            let path = path.join(name);
            let expanded = directory && !collapsed.contains(&path);
            rows.push(FileRow {
                path: path.clone(),
                label: name.to_string_lossy().into_owned(),
                depth,
                directory,
                expanded,
                change: if directory { None } else { node.change.clone() },
            });
            if expanded {
                append_children(node, &path, depth + 1, collapsed, rows);
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TreeMove {
    cursor: usize,
    toggle: bool,
}

fn tree_move(rows: &[FileRow], cursor: usize, key: &str) -> Option<TreeMove> {
    let cursor = cursor.min(rows.len().checked_sub(1)?);
    let row = &rows[cursor];
    let mut result = TreeMove {
        cursor,
        toggle: false,
    };
    match key {
        "down" => result.cursor = (cursor + 1).min(rows.len() - 1),
        "up" => result.cursor = cursor.saturating_sub(1),
        "home" => result.cursor = 0,
        "end" => result.cursor = rows.len() - 1,
        "left" if row.directory && row.expanded => result.toggle = true,
        "left" => {
            if let Some(parent) = row.path.parent() {
                result.cursor = rows
                    .iter()
                    .position(|r| r.directory && r.path == parent)
                    .unwrap_or(cursor);
            }
        }
        "right" if row.directory && !row.expanded => result.toggle = true,
        "right" if row.directory => {
            if rows.get(cursor + 1).is_some_and(|r| r.depth > row.depth) {
                result.cursor += 1;
            }
        }
        "right" => {}
        "enter" => result.toggle = row.directory,
        _ => return None,
    }
    Some(result)
}

// Reanchor the tree cursor after hiding its descendants. This never changes the
// open file or editor: their selection belongs to SavedView, independently.
fn visible_cursor(rows: &[FileRow], previous: Option<&Path>, fallback: usize) -> usize {
    let mut path = previous;
    while let Some(candidate) = path {
        if let Some(index) = rows.iter().position(|r| r.path == candidate) {
            return index;
        }
        path = candidate.parent();
    }
    fallback.min(rows.len().saturating_sub(1))
}

pub(super) fn skipped_paths_notice(id: &'static str, paths: &[PathBuf]) -> Option<Button> {
    let first = paths.first()?;
    let count = paths.len();
    let label = format!("권한으로 제외된 파일 {count}개");
    let tooltip = format!(
        "{}{} · 클릭하면 제외된 경로를 모두 복사합니다",
        first.display(),
        if count == 1 {
            String::new()
        } else {
            format!(" 외 {}개", count - 1)
        }
    );
    let paths = paths
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    Some(
        Button::new(id)
            .small()
            .ghost()
            .icon(IconName::Copy)
            .accessibility_label(format!("{label} · 경로 복사"))
            .label(label)
            .tooltip(tooltip)
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(paths.clone()))
            }),
    )
}

impl Pokarak {
    fn file_tree(&self) -> FileTree {
        let Some(session) = self.session() else {
            return FileTree::default();
        };
        let Some(snapshot) = &session.snapshot else {
            return FileTree::default();
        };
        let filtered;
        let snapshot = if session.unreviewed_only && !session.saved.all_files {
            filtered = Snapshot {
                changes: snapshot
                    .changes
                    .iter()
                    .filter(|f| {
                        self.file_review_state(&f.path) != reviewed_files::ReviewState::Reviewed
                    })
                    .cloned()
                    .collect(),
                ..snapshot.clone()
            };
            &filtered
        } else {
            snapshot
        };
        build_tree(
            snapshot,
            session.saved.all_files,
            if self.text_search { "" } else { &self.filter },
            &session.saved.collapsed_folders,
        )
    }

    pub(super) fn file_rows(&self) -> Vec<FileRow> {
        self.file_tree().rows
    }

    pub(super) fn toggle_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(session) = self
            .current
            .as_ref()
            .and_then(|root| self.sessions.get_mut(root))
        {
            let collapsed = &mut session.saved.collapsed_folders;
            if !collapsed.remove(path) {
                collapsed.insert(path.to_path_buf());
            }
            let rows = self.file_rows();
            self.file_cursor = visible_cursor(&rows, Some(path), self.file_cursor);
            self.persist(cx);
            cx.notify();
        }
    }

    fn set_folders_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let tree = self.file_tree();
        let previous = tree.rows.get(self.file_cursor).map(|r| r.path.clone());
        if let Some(session) = self
            .current
            .as_ref()
            .and_then(|root| self.sessions.get_mut(root))
        {
            if expanded {
                session
                    .saved
                    .collapsed_folders
                    .retain(|p| !tree.directories.contains(p));
            } else {
                session.saved.collapsed_folders.extend(tree.directories);
            }
            self.file_cursor =
                visible_cursor(&self.file_rows(), previous.as_deref(), self.file_cursor);
            self.persist(cx);
            cx.notify();
        }
    }

    fn handle_tree_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Only this focus handle owns tree keys. Inputs and editors keep every
        // cursor key, even if the tree still has a highlighted/open file.
        if !self.file_tree_focus.is_focused(window) || event.keystroke.modifiers.modified() {
            return;
        }
        if event.keystroke.key == "escape" {
            self.focus.focus(window, cx);
        } else {
            let rows = self.file_rows();
            let Some(movement) = tree_move(&rows, self.file_cursor, &event.keystroke.key) else {
                return;
            };
            self.file_cursor = movement.cursor;
            if movement.toggle || event.keystroke.key == "enter" {
                self.activate_row(rows[self.file_cursor].clone(), window, cx);
            }
            self.file_scroll
                .scroll_to_item(self.file_cursor, ScrollStrategy::Nearest);
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The worktree this file tree reads, as a switcher. It keeps worktree
    /// switching in view when Code view hides the Git sidebar.
    fn files_worktree_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tree = self.active_worktree()?;
        let palette = Palette::new(cx);
        let name = tree
            .path
            .file_name()
            .unwrap_or(tree.path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let branch = if tree.is_detached {
            "detached HEAD".to_owned()
        } else {
            tree.branch.clone()
        };
        let label = format!("{branch} · {name}");
        let others = self.inventory.len().saturating_sub(1);
        Some(
            div()
                .flex_none()
                .px_2()
                .pt_2()
                .child(
                    theme::action_button("files-worktree-picker", self.opening_path.is_some())
                        .small()
                        .w_full()
                        .accessibility_label(format!("워크트리 {label} · 다른 워크트리 선택"))
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::FolderGit2)
                                        .size(ui_px(14.))
                                        .text_color(palette.rgb(BLUE)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_left()
                                        .text_ellipsis()
                                        .child(label),
                                )
                                .when(others > 0, |row| {
                                    row.child(
                                        div()
                                            .flex_none()
                                            .text_size(ui_text(11.))
                                            .text_color(palette.rgb(MUTED))
                                            .child(format!("+{others}")),
                                    )
                                })
                                .child(Icon::new(IconName::ChevronDown).size(ui_px(13.))),
                        )
                        .on_click(cx.listener(|app, _, w, cx| app.open_worktree_picker(w, cx)))
                        .shortcut_tooltip(
                            format!("워크트리 선택 · {}", tree.path.display()),
                            &ChooseWorktree,
                            None,
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_files(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.results_open
            && self.result_surface == result_presentation::Surface::Files
            && !self.analysis_feedback.output_open
        {
            return self.render_results(cx);
        }
        let palette = Palette::new(cx);
        let all = self.session().is_some_and(|s| s.saved.all_files);
        let selected_file = self.selected();
        let keyboard_index = (self.file_tree_focus.is_focused(window)
            || (self.search_input.focus_handle(cx).is_focused(window) && !self.text_search))
            .then_some(self.file_cursor);
        let tree = self.file_tree();
        let count = tree.file_count;
        let snapshot = self.snapshot();
        let [changed_label, all_label] = file_mode_labels(snapshot, all, count);
        let has_folders = !tree.directories.is_empty();
        let filtering = !self.text_search && !self.filter.is_empty();
        let rows = Arc::new(tree.rows);
        let reviewed = rows
            .iter()
            .filter(|row| !row.directory)
            .map(|row| (row.path.clone(), self.file_review_state(&row.path)))
            .collect::<HashMap<_, _>>();
        let weak = cx.entity().downgrade();
        div()
            .size_full()
            .min_w_0()
            .v_flex()
            .bg(palette.rgb(SIDE))
            .overflow_hidden()
            .when(!self.sidebar_shown(window), |pane| {
                pane.children(self.files_worktree_picker(cx))
            })
            .child(
                div()
                    .h(ui_px(34.))
                    .flex_none()
                    .px(ui_px(4.))
                    .flex()
                    .items_center()
                    .gap(ui_px(2.))
                    .children(
                        [
                            (false, "changes", changed_label),
                            (true, "all-files", all_label),
                        ]
                        .map(|(all_files, id, label)| {
                            Button::new(id)
                                .tooltip(label.clone())
                                .label(label)
                                .ghost()
                                .small()
                                .flex_initial()
                                .min_w(ui_px(38.))
                                .px(ui_px(3.))
                                .selected(all == all_files)
                                .toggled(all == all_files)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(root) = &this.current {
                                        this.sessions.get_mut(root).unwrap().saved.all_files =
                                            all_files;
                                    }
                                    this.file_cursor = 0;
                                    this.persist(cx);
                                    cx.notify();
                                }))
                        }),
                    )
                    .when(
                        !all && snapshot.is_some_and(|s| !s.changes.is_empty()),
                        |row| {
                            let label = self.review_counts();
                            row.child(
                                div()
                                    .id("files-review-count")
                                    .flex_1()
                                    .min_w(ui_px(40.))
                                    .text_right()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text(11.))
                                    .text_color(palette.rgb(MUTED))
                                    .child(label.clone())
                                    .tooltip(move |w, cx| Tooltip::new(label.clone()).build(w, cx)),
                            )
                        },
                    ),
            )
            .children(snapshot.and_then(|snapshot| {
                skipped_paths_notice("copy-skipped-files", &snapshot.skipped_paths)
            }))
            .when(!all, |d| d.child(self.review_filter_controls(cx)))
            .child(
                div()
                    .flex_none()
                    .px(ui_px(4.))
                    .pb_2()
                    .flex()
                    .items_center()
                    .gap(ui_px(2.))
                    .child(
                        Input::new(&self.search_input)
                            .small()
                            .flex_1()
                            .min_w_0()
                            .prefix(IconName::Search),
                    )
                    .child(
                        Button::new("text-search")
                            .icon(IconName::TextSearch)
                            .ghost()
                            .with_size(ui_px(24.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.focus_search(true, window, cx)
                            }))
                            .shortcut_tooltip("본문 검색", &FindText, None),
                    )
                    .child(
                        Button::new("expand-all-folders")
                            .icon(IconName::ChevronsUpDown)
                            .accessibility_label("모든 폴더 펼치기")
                            .ghost()
                            .with_size(ui_px(24.))
                            .disabled(!has_folders || filtering)
                            .tooltip("모든 폴더 펼치기 · 트리에서 → 펼치기 / Enter 전환")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_folders_expanded(true, cx);
                                this.file_tree_focus.focus(window, cx);
                            })),
                    )
                    .child(
                        Button::new("collapse-all-folders")
                            .icon(IconName::ChevronsDownUp)
                            .accessibility_label("모든 폴더 접기")
                            .ghost()
                            .with_size(ui_px(24.))
                            .disabled(!has_folders || filtering)
                            .tooltip("모든 폴더 접기 · 트리에서 ← 접기 / Enter 전환")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_folders_expanded(false, cx);
                                this.file_tree_focus.focus(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("file-tree")
                    .key_context("FileTree")
                    .track_focus(&self.file_tree_focus)
                    .role(accesskit::Role::Tree)
                    .aria_label("파일 트리")
                    .focus_visible(|s| s.border_1().border_color(palette.rgb(BLUE)))
                    .on_key_down(cx.listener(Self::handle_tree_key))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.file_tree_focus.focus(window, cx);
                            cx.notify();
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(if count == 0 {
                        div()
                            .p_4()
                            .text_color(palette.rgb(MUTED))
                            .child(if filtering {
                                "일치하는 파일이 없습니다."
                            } else if all {
                                "표시할 파일이 없습니다."
                            } else {
                                "변경된 파일이 없습니다. 전체 파일에서 코드를 읽을 수 있습니다."
                            })
                            .into_any_element()
                    } else {
                        uniform_list("files", rows.len(), move |range, _, cx| {
                            range
                                .map(|ix| {
                                    let row = rows[ix].clone();
                                    let target = weak.clone();
                                    let a11y_target = weak.clone();
                                    let a11y_row = row.clone();
                                    let selected =
                                        keyboard_index.map(|index| ix == index).unwrap_or_else(
                                            || selected_file.as_ref() == Some(&row.path),
                                        );
                                    let status =
                                        row.change.as_ref().map(|c| file_status(&c.status));
                                    div()
                                        .id(("file", ix))
                                        .w_full()
                                        .min_w_0()
                                        .role(accesskit::Role::TreeItem)
                                        .aria_label(row.path.to_string_lossy().to_string())
                                        .aria_level(row.depth + 1)
                                        .aria_selected(selected)
                                        .when(row.directory, |d| d.aria_expanded(row.expanded))
                                        .on_a11y_action(
                                            accesskit::Action::Click,
                                            move |_, window, cx| {
                                                let _ = a11y_target.update(cx, |this, cx| {
                                                    this.file_cursor = ix;
                                                    this.file_tree_focus.focus(window, cx);
                                                    this.activate_row(a11y_row.clone(), window, cx);
                                                });
                                            },
                                        )
                                        .h(ui_px(29.))
                                        .px_2()
                                        .pl(ui_px(8. + row.depth as f32 * 16.))
                                        .flex()
                                        .items_center()
                                        .gap(ui_px(4.))
                                        .cursor_pointer()
                                        .bg(if selected {
                                            palette.selected()
                                        } else {
                                            palette.rgb(SIDE)
                                        })
                                        .hover(|s| {
                                            s.bg(if selected {
                                                palette.selected_hover()
                                            } else {
                                                palette.hover()
                                            })
                                        })
                                        .child(
                                            div()
                                                .w(ui_px(16.))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .when(row.directory, |d| {
                                                    d.child(
                                                        Icon::new(if row.expanded {
                                                            IconName::ChevronDown
                                                        } else {
                                                            IconName::ChevronRight
                                                        })
                                                        .size(ui_px(14.))
                                                        .text_color(palette.rgb(MUTED)),
                                                    )
                                                }),
                                        )
                                        .child(if row.directory {
                                            Icon::new(if row.expanded {
                                                IconName::FolderOpen
                                            } else {
                                                IconName::Folder
                                            })
                                            .size(ui_px(16.))
                                            .text_color(palette.rgb(MUTED))
                                            .into_any_element()
                                        } else {
                                            file_icons::icon_for_path(&row.path, cx)
                                                .into_any_element()
                                        })
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .child(row.label.clone()),
                                        )
                                        .when(
                                            reviewed.get(&row.path)
                                                == Some(&reviewed_files::ReviewState::Reviewed),
                                            |row| {
                                                row.child(
                                                    div()
                                                        .flex_none()
                                                        .text_color(palette.rgb(GREEN))
                                                        .child("✓"),
                                                )
                                            },
                                        )
                                        .when(
                                            reviewed.get(&row.path)
                                                == Some(&reviewed_files::ReviewState::Pending),
                                            |row| {
                                                row.child(
                                                    div()
                                                        .flex_none()
                                                        .text_color(palette.rgb(MUTED))
                                                        .child("…"),
                                                )
                                            },
                                        )
                                        .when_some(status, |d, (label, color)| {
                                            d.child(
                                                div()
                                                    .flex_none()
                                                    .text_size(ui_text(10.))
                                                    .text_color(palette.rgb(color))
                                                    .child(label),
                                            )
                                        })
                                        .on_click(move |_, window, cx| {
                                            let _ = target.update(cx, |this, cx| {
                                                this.file_cursor = ix;
                                                this.file_tree_focus.focus(window, cx);
                                                this.activate_row(row.clone(), window, cx);
                                            });
                                        })
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>()
                        })
                        .track_scroll(&self.file_scroll)
                        .h_full()
                        .into_any_element()
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileTree, TreeMove, build_tree, file_mode_labels, tree_move, visible_cursor};
    use crate::prefs::{Preferences, SavedView};
    use pokarak_core::{ChangedFile, CompareMode, FileStatus, Snapshot};
    use std::{
        collections::HashSet,
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=File Tree Test",
                "-c",
                "user.email=tree@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        for name in [
            "src/nested/deep.rs",
            "src/main.rs",
            "docs/guide.md",
            "deleted/old.rs",
            "README.md",
        ] {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "original\n").unwrap();
        }
        fs::create_dir(dir.path().join("empty")).unwrap();
        git(dir.path(), &["add", "--", "."]);
        git(dir.path(), &["commit", "-qm", "fixture"]);
        fs::write(dir.path().join("src/main.rs"), "changed\n").unwrap();
        fs::write(dir.path().join("src/nested/deep.rs"), "changed\n").unwrap();
        fs::write(dir.path().join("docs/new.md"), "new\n").unwrap();
        fs::remove_file(dir.path().join("deleted/old.rs")).unwrap();
        dir
    }

    fn paths(tree: &FileTree) -> Vec<String> {
        tree.rows
            .iter()
            .map(|r| r.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn mode_buttons_show_both_totals_and_the_active_filtered_count() {
        let fixture = fixture();
        let snapshot = pokarak_core::snapshot(fixture.path(), &CompareMode::Uncommitted).unwrap();
        let collapsed = HashSet::from([PathBuf::from("docs")]);
        for (all, query, expected) in [
            (false, "", ["변경 4", "전체 5"]),
            (true, "", ["변경 4", "전체 5"]),
            (false, "docs/", ["변경 1/4", "전체 5"]),
            (true, "docs/", ["변경 4", "전체 2/5"]),
            (false, "missing", ["변경 0/4", "전체 5"]),
        ] {
            let tree = build_tree(&snapshot, all, query, &collapsed);
            assert_eq!(
                file_mode_labels(Some(&snapshot), all, tree.file_count),
                expected
            );
        }
        assert_eq!(file_mode_labels(None, false, 0), ["변경", "전체"]);
        assert_eq!(
            file_mode_labels(Some(&Snapshot::default()), false, 0),
            ["변경 0", "전체 0"]
        );
    }

    #[test]
    fn real_snapshot_has_stable_hierarchy_in_both_modes_and_retains_status() {
        let fixture = fixture();
        let snapshot = pokarak_core::snapshot(fixture.path(), &CompareMode::Uncommitted).unwrap();
        let changed = build_tree(&snapshot, false, "", &HashSet::new());
        assert_eq!(
            paths(&changed),
            [
                "deleted",
                "deleted/old.rs",
                "docs",
                "docs/new.md",
                "src",
                "src/nested",
                "src/nested/deep.rs",
                "src/main.rs"
            ]
        );
        assert_eq!(changed.file_count, 4);
        let removed = changed
            .rows
            .iter()
            .find(|r| r.path == Path::new("deleted/old.rs"))
            .unwrap();
        assert_eq!(removed.change.as_ref().unwrap().status, FileStatus::Deleted);
        assert_eq!(removed.depth, 1);
        let all = build_tree(&snapshot, true, "", &HashSet::new());
        assert!(paths(&all).contains(&"docs/guide.md".into()));
        assert!(!paths(&all).contains(&"empty".into()));
        let mut shuffled = snapshot.clone();
        shuffled.files.reverse();
        shuffled.changes.reverse();
        assert_eq!(
            paths(&all),
            paths(&build_tree(&shuffled, true, "", &HashSet::new()))
        );
        assert_eq!(
            paths(&changed),
            paths(&build_tree(&shuffled, false, "", &HashSet::new()))
        );
    }

    #[test]
    fn collapsed_tree_keeps_count_and_search_reveals_paths_then_restores_collapse() {
        let fixture = fixture();
        let snapshot = pokarak_core::snapshot(fixture.path(), &CompareMode::Uncommitted).unwrap();
        for all in [false, true] {
            let open = build_tree(&snapshot, all, "", &HashSet::new());
            let collapsed = open.directories.clone();
            let closed = build_tree(&snapshot, all, "", &collapsed);
            assert_eq!(closed.file_count, open.file_count);
            assert!(
                closed
                    .rows
                    .iter()
                    .filter(|r| r.directory)
                    .all(|r| !r.expanded)
            );
            assert!(!paths(&closed).contains(&"src/nested/deep.rs".into()));
            let revealed = build_tree(&snapshot, all, "DEEP.RS", &collapsed);
            assert_eq!(paths(&revealed), ["src/nested/deep.rs"]);
            assert_eq!(revealed.rows[0].label, "src/nested/deep.rs");
            assert!(!revealed.rows[0].directory);
            assert_eq!(
                paths(&closed),
                paths(&build_tree(&snapshot, all, "", &collapsed))
            );
        }
    }

    #[test]
    fn saved_collapse_round_trips_per_worktree_without_changing_open_file() {
        let fixture = fixture();
        let second = fixture.path().with_extension("linked");
        git(
            fixture.path(),
            &["worktree", "add", "-qb", "second", second.to_str().unwrap()],
        );
        let first_root = fs::canonicalize(fixture.path()).unwrap();
        let second_root = fs::canonicalize(&second).unwrap();
        let mut prefs = Preferences::default();
        let selected = PathBuf::from("src/nested/deep.rs");
        prefs.worktrees.insert(
            first_root.clone(),
            SavedView {
                selected: Some(selected.clone()),
                tabs: vec![selected.clone()],
                collapsed_folders: HashSet::from([
                    PathBuf::from("src"),
                    PathBuf::from("src/nested"),
                ]),
                ..SavedView::default()
            },
        );
        prefs
            .worktrees
            .insert(second_root.clone(), SavedView::default());
        // The same serialization used by session.json, in an external directory.
        let config = tempfile::tempdir().unwrap();
        let saved = config.path().join("session.json");
        fs::write(&saved, serde_json::to_vec(&prefs).unwrap()).unwrap();
        let restored: Preferences = serde_json::from_slice(&fs::read(saved).unwrap()).unwrap();
        let first = &restored.worktrees[&first_root];
        let second_view = &restored.worktrees[&second_root];
        assert_eq!(first.selected.as_ref(), Some(&selected));
        assert_eq!(first.tabs, [selected]);
        assert!(second_view.collapsed_folders.is_empty());
        let snapshot = pokarak_core::snapshot(&first_root, &CompareMode::Uncommitted).unwrap();
        assert!(
            !paths(&build_tree(&snapshot, true, "", &first.collapsed_folders))
                .contains(&"src/main.rs".into())
        );
        let snapshot = pokarak_core::snapshot(&second_root, &CompareMode::Uncommitted).unwrap();
        assert!(
            paths(&build_tree(
                &snapshot,
                true,
                "",
                &second_view.collapsed_folders
            ))
            .contains(&"src/main.rs".into())
        );
        git(
            fixture.path(),
            &["worktree", "remove", "--", second.to_str().unwrap()],
        );
    }

    #[test]
    fn legacy_saved_view_defaults_to_expanded_folders() {
        let view: SavedView =
            serde_json::from_str(r#"{"selected":"src/main.rs","all_files":true}"#).unwrap();
        assert!(view.collapsed_folders.is_empty());
        assert_eq!(view.selected.as_deref(), Some(Path::new("src/main.rs")));
    }

    #[test]
    fn tree_keyboard_expands_collapses_and_moves_to_parent_or_first_child() {
        let snapshot = Snapshot {
            files: vec![
                "src/nested/deep.rs".into(),
                "src/main.rs".into(),
                "README.md".into(),
            ],
            ..Snapshot::default()
        };
        let open = build_tree(&snapshot, true, "", &HashSet::new());
        assert_eq!(
            tree_move(&open.rows, 0, "right"),
            Some(TreeMove {
                cursor: 1,
                toggle: false
            })
        );
        assert_eq!(
            tree_move(&open.rows, 0, "left"),
            Some(TreeMove {
                cursor: 0,
                toggle: true
            })
        );
        assert_eq!(
            tree_move(&open.rows, 0, "enter"),
            Some(TreeMove {
                cursor: 0,
                toggle: true
            })
        );
        assert_eq!(
            tree_move(&open.rows, 2, "left"),
            Some(TreeMove {
                cursor: 1,
                toggle: false
            })
        );
        assert_eq!(
            tree_move(&open.rows, 2, "right"),
            Some(TreeMove {
                cursor: 2,
                toggle: false
            })
        );
        assert_eq!(
            tree_move(&open.rows, 3, "enter"),
            Some(TreeMove {
                cursor: 3,
                toggle: false
            })
        );
        let closed = build_tree(&snapshot, true, "", &HashSet::from([PathBuf::from("src")]));
        assert_eq!(
            tree_move(&closed.rows, 0, "right"),
            Some(TreeMove {
                cursor: 0,
                toggle: true
            })
        );
        assert_eq!(
            tree_move(&closed.rows, 0, "left"),
            Some(TreeMove {
                cursor: 0,
                toggle: false
            })
        );
        assert_eq!(
            visible_cursor(&closed.rows, Some(Path::new("src/nested/deep.rs")), 3),
            0
        );
        assert_eq!(
            tree_move(&closed.rows, usize::MAX, "down"),
            Some(TreeMove {
                cursor: 1,
                toggle: false
            })
        );
        assert_eq!(tree_move(&[], 0, "left"), None);
        assert_eq!(tree_move(&open.rows, 0, "a"), None);
    }

    #[test]
    fn changed_file_to_directory_transition_keeps_both_entries() {
        let snapshot = Snapshot {
            changes: vec![
                ChangedFile {
                    path: "src".into(),
                    status: FileStatus::Deleted,
                    ..ChangedFile::default()
                },
                ChangedFile {
                    path: "src/new.rs".into(),
                    status: FileStatus::Untracked,
                    ..ChangedFile::default()
                },
            ],
            ..Snapshot::default()
        };
        let tree = build_tree(&snapshot, false, "", &HashSet::new());
        assert_eq!(paths(&tree), ["src", "src/new.rs", "src"]);
        assert!(tree.rows[0].directory);
        assert!(!tree.rows[2].directory);
        assert_eq!(
            tree.rows[2].change.as_ref().unwrap().status,
            FileStatus::Deleted
        );
    }
}
