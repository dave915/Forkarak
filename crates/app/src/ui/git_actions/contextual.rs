//! Sidebar context actions always address a fully qualified Git ref.
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use pokarak_core::operations::branches::BranchStart;

#[derive(Clone)]
pub(super) struct SelectedBranch {
    pub root: PathBuf,
    pub start: BranchStart,
    pub current: Option<String>,
}

impl SelectedBranch {
    pub(super) fn local_name(
        &self,
        state: &RepositoryState,
        starts: &[BranchStart],
    ) -> anyhow::Result<&str> {
        let name = self
            .start
            .reference
            .strip_prefix("refs/heads/")
            .ok_or_else(|| anyhow::anyhow!("로컬 브랜치를 선택하세요."))?;
        anyhow::ensure!(
            state.root == self.root
                && state.current_branch == self.current
                && state.local_branches.iter().any(|b| b == name)
                && starts.contains(&self.start),
            "워크트리 또는 선택한 브랜치가 변경되었습니다. 새로고침 후 다시 선택하세요."
        );
        Ok(name)
    }
}

pub(in crate::ui) fn branch_context_menu(
    weak: WeakEntity<Pokarak>,
    root: PathBuf,
    reference: String,
    oid: String,
    current_branch: Option<String>,
    can_checkout: bool,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + Clone {
    move |mut menu, _, cx| {
        let local = reference.strip_prefix("refs/heads/");
        let name = local
            .or_else(|| reference.strip_prefix("refs/remotes/"))
            .unwrap_or(&reference);
        let disabled = branch_forms::branch_form_disabled(&weak, &root, cx);
        menu = menu.label(format!(
            "{}  {name}",
            if local.is_some() { "로컬" } else { "원격" }
        ));
        if let Some(current) = &current_branch {
            let selected_current = local == Some(current);
            for (kind, label) in [
                (
                    IntegrationKind::Merge,
                    format!("Merge  {name} → {current}…"),
                ),
                (
                    IntegrationKind::Rebase,
                    format!("Rebase  {current} onto {name}…"),
                ),
            ] {
                let root = root.clone();
                let reference = reference.clone();
                let current = current.clone();
                let weak = weak.clone();
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .disabled(disabled || selected_current)
                        .on_click(move |_, w, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                app.review_branch_integration(
                                    kind,
                                    root.clone(),
                                    reference.clone(),
                                    current.clone(),
                                    w,
                                    cx,
                                );
                            });
                        }),
                );
            }
        }
        if let Some(local) = local {
            let selected = SelectedBranch {
                root: root.clone(),
                start: BranchStart {
                    reference: reference.clone(),
                    oid: oid.clone(),
                },
                current: current_branch.clone(),
            };
            let push = selected.clone();
            let owner = weak.clone();
            menu = menu.item(PopupMenuItem::new("Push…").disabled(disabled).on_click(
                move |_, w, cx| {
                    let _ = owner.update(cx, |app, cx| {
                        app.open_selected_branch_push(push.clone(), w, cx)
                    });
                },
            ));
            let occupied = !can_checkout || Some(local) == current_branch.as_deref();
            let owner = weak.clone();
            menu = menu.item(
                PopupMenuItem::new(if occupied {
                    "로컬 브랜치 삭제 · 워크트리에서 사용 중"
                } else {
                    "로컬 브랜치 삭제…"
                })
                .disabled(disabled || occupied)
                .on_click(move |_, w, cx| {
                    let _ = owner.update(cx, |app, cx| {
                        app.review_branch_delete(selected.clone(), w, cx)
                    });
                }),
            );
            let rename_root = root.clone();
            let rename_ref = reference.clone();
            let rename_weak = weak.clone();
            menu = menu.item(
                PopupMenuItem::new(format!("{local} 이름 변경…"))
                    .disabled(disabled)
                    .on_click(move |_, w, cx| {
                        let _ = rename_weak.update(cx, |app, cx| {
                            app.open_branch_rename(rename_root.clone(), rename_ref.clone(), w, cx);
                        });
                    }),
            );
            let branch = local.to_owned();
            let weak = weak.clone();
            let root = root.clone();
            let current = current_branch.clone();
            menu = menu.item(
                PopupMenuItem::new(format!("{branch} 체크아웃…"))
                    .disabled(disabled || !can_checkout || Some(local) == current_branch.as_deref())
                    .on_click(move |_, w, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.review_branch_checkout(
                                root.clone(),
                                branch.clone(),
                                current.clone(),
                                w,
                                cx,
                            );
                        });
                    }),
            );
        } else if reference.starts_with("refs/remotes/") {
            let target_root = root.clone();
            let target_ref = reference.clone();
            let target_weak = weak.clone();
            menu = menu.item(
                PopupMenuItem::new("로컬 추적 브랜치 만들기…")
                    .disabled(disabled)
                    .on_click(move |_, w, cx| {
                        let _ = target_weak.update(cx, |app, cx| {
                            app.review_remote_branch_create(
                                target_root.clone(),
                                target_ref.clone(),
                                w,
                                cx,
                            );
                        });
                    }),
            );
        }
        let reference = reference.clone();
        let root = root.clone();
        let weak = weak.clone();
        menu.separator().item(
            PopupMenuItem::new("그래프에서 보기")
                .disabled(disabled)
                .on_click(move |_, w, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        if app.current.as_ref() == Some(&root) && !app.loading && !app.git_is_busy()
                        {
                            app.jump_to_graph_reference(&reference, w, cx);
                        }
                    });
                }),
        )
    }
}

impl Pokarak {
    fn review_remote_branch_create(
        &mut self,
        root: PathBuf,
        reference: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading || self.opening_path.is_some() {
            return;
        }
        self.workflow_read(
            root,
            move |ops| {
                ops.branch_starts()?.into_iter()
                    .find(|start| start.reference == reference && start.reference.starts_with("refs/remotes/"))
                    .ok_or_else(|| anyhow::anyhow!("선택한 원격 참조가 변경되었거나 없어졌습니다. 새로고침 후 다시 선택하세요."))
            },
            |app, start, w, cx| app.open_branch_create(Some(start), w, cx),
            window,
            cx,
        );
    }

    fn review_branch_integration(
        &mut self,
        kind: IntegrationKind,
        root: PathBuf,
        reference: String,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_read(root, |ops| ops.integration_choices(), move |app, choices, window, cx| {
            let selected = choices.into_iter().find(|choice| choice.reference == reference && choice.current.branch == current);
            if let Some(selected) = selected {
                app.confirm_integration(kind, selected, window, cx);
            } else {
                app.error = Some("현재 브랜치 또는 선택한 브랜치가 변경되었습니다. 새로고침 후 다시 선택하세요.".into());
                cx.notify();
            }
        }, window, cx);
    }

    pub(in crate::ui) fn review_branch_checkout(
        &mut self,
        root: PathBuf,
        branch: String,
        current: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_read(
            root,
            |ops| ops.state(),
            move |app, state, window, cx| {
                if state.current_branch != current
                    || !state.local_branches.contains(&branch)
                    || state.checked_out_branches.contains(&branch)
                {
                    app.error = Some("브랜치 또는 워크트리 상태가 변경되었습니다. 새로고침 후 체크아웃 대상을 다시 선택하세요.".into());
                    cx.notify();
                    return;
                }
                app.confirm_branch_checkout(state.root, current, branch, window, cx);
            },
            window,
            cx,
        );
    }

    fn confirm_branch_checkout(
        &self,
        root: PathBuf,
        current: Option<String>,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        let weak = cx.entity().downgrade();
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let target = weak.clone();
            let action_root = root.clone();
            let from = current.clone();
            let to = branch.clone();
            let confirm = theme::action_button("git-confirm", false)
                .label("체크아웃")
                .primary()
                .on_click(move |_, window, cx| {
                    let _ = target.update(cx, |app, cx| {
                        window.close_dialog(cx);
                        let from = from.clone();
                        let to = to.clone();
                        app.run_git_job(
                            action_root.clone(),
                            "체크아웃".into(),
                            move |git| {
                                anyhow::ensure!(
                                    git.state()?.current_branch == from,
                                    "현재 브랜치가 변경되었습니다. 체크아웃 대상을 다시 확인하세요."
                                );
                                anyhow::ensure!(
                                    git.in_progress()?.is_none(),
                                    "진행 중인 Git 작업을 먼저 완료하거나 중단하세요."
                                );
                                git.execute(&Operation::CheckoutBranch { name: to })
                            },
                            window,
                            cx,
                        );
                    });
                });
            compact::dialog(dialog, &weak)
                .title("브랜치를 체크아웃할까요?")
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(compact::row(
                            "현재 브랜치",
                            current.clone().unwrap_or_else(|| "detached HEAD".into()),
                            cx,
                        ))
                        .child(compact::row("전환할 브랜치", branch.clone(), cx))
                        .child(compact::worktree(&root, cx))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(close_button())
                                .child(confirm),
                        ),
                )
        });
    }
}
