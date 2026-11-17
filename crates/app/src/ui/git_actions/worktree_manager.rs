use super::*;
use pokarak_core::operations::worktrees::WorktreeBranch;

impl Pokarak {
    pub(in crate::ui) fn open_worktree_manager(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_git_page(Some(Page::Worktrees), window, cx);
    }

    pub(super) fn show_worktree_manager(
        &mut self,
        state: Arc<RepositoryState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        let trees = snapshot.worktrees.clone();
        let head = snapshot.worktree.head.clone();
        let available: Vec<String> = state
            .local_branches
            .iter()
            .filter(|b| !trees.iter().any(|t| !t.is_detached && t.branch == **b))
            .cloned()
            .collect();
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let palette = Palette::new(cx);
            let mut list = div()
                .id("manage-worktree-list")
                .v_flex()
                .max_h(px(205.))
                .overflow_y_scroll();
            for (index, tree) in trees.iter().enumerate() {
                let title = if tree.is_detached {
                    format!("detached · {}", short_head(&tree.head))
                } else {
                    tree.branch.clone()
                };
                let status = if tree.is_main {
                    "기본"
                } else if tree.is_locked {
                    "잠김"
                } else if tree.is_prunable {
                    "경로 없음"
                } else if tree.path == state.root {
                    "현재"
                } else {
                    ""
                };
                let button = Button::new(("remove-worktree", index))
                    .label(if tree.is_prunable {
                        "등록 정리…"
                    } else {
                        "제거…"
                    })
                    .small()
                    .ghost()
                    .disabled(
                        tree.is_main
                            || tree.is_locked
                            || tree.path == state.root
                            || tree.changed_files > 0,
                    )
                    .tooltip(if tree.changed_files > 0 {
                        "변경사항을 먼저 정리하세요"
                    } else {
                        "연결된 워크트리 제거"
                    });
                let target = tree.clone();
                let weak = weak.clone();
                let root = state.root.clone();
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py_1()
                        .border_b_1()
                        .border_color(palette.rgb(LINE))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .v_flex()
                                .child(format!("{title}   {status}"))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(palette.rgb(MUTED))
                                        .text_ellipsis()
                                        .child(tree.path.display().to_string()),
                                ),
                        )
                        .child(button.on_click(move |_, w, cx| {
                            let target = target.clone();
                            let msg = format!(
                                "{}\n{}  ·  {}\n{}",
                                target.path.display(),
                                target.branch,
                                short_head(&target.head),
                                if target.is_prunable {
                                    "사라진 폴더의 Git 등록을 정리합니다."
                                } else {
                                    "이 워크트리 폴더를 삭제합니다. 브랜치와 커밋은 보존합니다."
                                }
                            );
                            let _ = weak.update(cx, |app, cx| {
                                app.confirm_worktree_remove(root.clone(), target, msg, w, cx)
                            });
                        })),
                );
            }
            let mut buttons = div().flex().gap_2();
            for new_branch in [true, false] {
                let root = state.root.clone();
                let head = head.clone();
                let available = available.clone();
                let weak = weak.clone();
                buttons = buttons.child(
                    Button::new(if new_branch {
                        "new-worktree-branch"
                    } else {
                        "existing-worktree-branch"
                    })
                    .label(if new_branch {
                        "새 브랜치로 생성…"
                    } else {
                        "기존 브랜치로 생성…"
                    })
                    .small()
                    .disabled(head.is_empty() || (!new_branch && available.is_empty()))
                    .on_click(move |_, w, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            w.close_dialog(cx);
                            app.worktree_create_form(
                                root.clone(),
                                head.clone(),
                                new_branch,
                                available.clone(),
                                w,
                                cx,
                            );
                        });
                    }),
                );
            }
            dialog
                .title("워크트리 관리")
                .width(px(640.))
                .child(div().v_flex().gap_3().child(buttons).child(list))
                .footer(close_row())
        });
    }

    fn confirm_worktree_remove(
        &mut self,
        root: PathBuf,
        tree: pokarak_core::Worktree,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let tree = tree.clone();
            let root = root.clone();
            dialog
                .title("워크트리 제거 확인")
                .width(px(590.))
                .child(message.clone())
                .footer(
                    div()
                        .flex()
                        .gap_2()
                        .justify_end()
                        .child(close_button())
                        .child(
                            Button::new("confirm-worktree-remove")
                                .label("워크트리 제거")
                                .danger()
                                .on_click(move |_, w, cx| {
                                    let tree = tree.clone();
                                    let _ = weak.update(cx, |app, cx| {
                                        w.close_dialog(cx);
                                        app.run_git_job(
                                            root.clone(),
                                            "워크트리 제거".into(),
                                            move |ops| ops.remove_worktree(&tree),
                                            w,
                                            cx,
                                        );
                                    });
                                }),
                        ),
                )
        });
    }

    fn worktree_create_form(
        &mut self,
        root: PathBuf,
        head: String,
        new_branch: bool,
        available: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent = root.parent().unwrap_or(&root);
        let example = parent.join("review").display().to_string();
        let path = cx.new(|cx| InputState::new(window, cx).placeholder(example));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("feature/review"));
        for input in [&path, &name] {
            window
                .subscribe(input, cx, |_, event: &InputEvent, window, _| {
                    if matches!(event, InputEvent::Change) {
                        window.refresh();
                    }
                })
                .detach();
        }
        let branch = choice(available, None, window, cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (weak, path, name, branch, root, head) = (
                weak.clone(),
                path.clone(),
                name.clone(),
                branch.clone(),
                root.clone(),
                head.clone(),
            );
            let value = path.read(cx).value();
            let disabled = !Path::new(value.as_ref()).is_absolute()
                || (new_branch && name.read(cx).value().trim().is_empty());
            dialog
                .title("워크트리 생성")
                .width(px(630.))
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(if new_branch {
                            field("새 브랜치 이름", &name)
                        } else {
                            select_field("사용할 로컬 브랜치", &branch)
                        })
                        .when(new_branch, |d| {
                            d.child(note(&format!("시작 커밋  {}", short_head(&head))))
                        })
                        .child(field("새 폴더의 절대 경로", &path)),
                )
                .footer(
                    div()
                        .flex()
                        .gap_2()
                        .justify_end()
                        .child(close_button())
                        .child(
                            Button::new("confirm-worktree-create")
                                .label("워크트리 생성")
                                .primary()
                                .disabled(disabled)
                                .on_click(move |_, w, cx| {
                                    let destination = PathBuf::from(path.read(cx).value().as_ref());
                                    let branch = if new_branch {
                                        WorktreeBranch::New(name.read(cx).value().trim().into())
                                    } else if let Some(name) = branch.read(cx).selected_value() {
                                        WorktreeBranch::Existing(name.clone())
                                    } else {
                                        return;
                                    };
                                    let head = head.clone();
                                    let _ = weak.update(cx, |app, cx| {
                                        w.close_dialog(cx);
                                        app.run_git_job(
                                            root.clone(),
                                            "워크트리 생성".into(),
                                            move |ops| {
                                                ops.create_worktree(&destination, &branch, &head)
                                            },
                                            w,
                                            cx,
                                        );
                                    });
                                }),
                        ),
                )
        });
    }
}

fn short_head(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}
