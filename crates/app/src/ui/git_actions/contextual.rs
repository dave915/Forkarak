//! Sidebar context actions always address a fully qualified Git ref.
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

pub(in crate::ui) fn branch_context_menu(
    weak: WeakEntity<Pokarak>,
    root: PathBuf,
    reference: String,
    current_branch: Option<String>,
    can_checkout: bool,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu {
    move |mut menu, _, cx| {
        let local = reference.strip_prefix("refs/heads/");
        let name = local
            .or_else(|| reference.strip_prefix("refs/remotes/"))
            .unwrap_or(&reference);
        let disabled = weak.upgrade().is_none_or(|view| {
            let app = view.read(cx);
            app.current.as_ref() != Some(&root) || app.git_is_busy() || app.loading
        });
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

    fn review_branch_checkout(
        &mut self,
        root: PathBuf,
        branch: String,
        current: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_read(root, |ops| ops.state(), move |app, state, window, cx| {
            if state.current_branch != current || !state.local_branches.contains(&branch) || state.checked_out_branches.contains(&branch) {
                app.error = Some("브랜치 또는 워크트리 상태가 변경되었습니다. 새로고침 후 체크아웃 대상을 다시 선택하세요.".into());
                cx.notify();
                return;
            }
            let from = current.as_deref().unwrap_or("detached HEAD");
            app.confirm_git_operation(GitConfirmation {
                root: state.root, label: "체크아웃".into(),
                detail: format!("{from} → {branch}\n이 워크트리의 파일을 선택한 브랜치로 전환합니다. 덮어쓸 로컬 변경이 있으면 중단합니다."),
                operation: Operation::CheckoutBranch {name: branch}, destructive: false,
            }, window, cx);
        }, window, cx);
    }
}
