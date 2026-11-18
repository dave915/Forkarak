use super::*;
use pokarak_core::operations::integration::BranchIntegration;
pub(in crate::ui) use pokarak_core::operations::integration::IntegrationKind;

impl Pokarak {
    pub(in crate::ui) fn open_integration(
        &mut self,
        kind: IntegrationKind,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.current.clone() else {
            return;
        };
        self.workflow_read(
            root,
            |ops| ops.integration_choices(),
            move |app, choices, w, cx| {
                if let Some(branch) = branch {
                    // rev-parse's short refnames can include the namespace when a name is ambiguous.
                    let mut matches = choices.iter().filter(|c| {
                        c.label() == branch
                            || c.reference.strip_prefix("refs/heads/") == Some(&branch)
                            || c.reference.strip_prefix("refs/remotes/") == Some(&branch)
                    });
                    if let Some(selected) = matches.next()
                        && matches.next().is_none()
                    {
                        app.confirm_integration(kind, selected.clone(), w, cx);
                        return;
                    }
                }
                app.integration_form(kind, choices, w, cx);
            },
            window,
            cx,
        );
    }

    fn integration_form(
        &mut self,
        kind: IntegrationKind,
        choices: Vec<BranchIntegration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = action_label(kind);
        let merge = kind == IntegrationKind::Merge;
        let choices = Arc::new(choices);
        let names: Vec<_> = choices
            .iter()
            .map(|item| {
                if merge {
                    merge_source(item)
                } else {
                    item.label().to_owned()
                }
            })
            .collect();
        let selected = choice(names.clone(), None, window, cx);
        let weak = cx.entity().downgrade();
        let changed = cx.subscribe_in(
            &selected,
            window,
            |app: &mut Self,
             _,
             _: &gpui_kit::component::select::SelectEvent<SearchableVec<String>>,
             w,
             _| {
                app.invalidate_workflow_read();
                w.refresh();
            },
        );
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let _subscription = &changed;
            let mut body = div().v_flex().gap_2();
            let mut disabled = true;
            if let Some(first) = choices.first() {
                disabled =
                    super::branch_forms::branch_form_disabled(&weak, &first.current.root, cx);
                if merge {
                    body = body
                        .child(compact::row(
                            "가져올 브랜치",
                            Select::new(&selected)
                                .id("merge-source")
                                .accessibility_label("가져올 브랜치")
                                .w_full(),
                            cx,
                        ))
                        .child(compact::row(
                            "병합할 위치",
                            first.current.branch.clone(),
                            cx,
                        ))
                        .child(compact::worktree(&first.current.root, cx));
                } else {
                    body = body
                        .child(scope(&first.current.root, Some(&first.current.branch)))
                        .child(select_field("현재 브랜치의 새 기준", &selected));
                }
            } else {
                body = body.child(note("선택할 다른 브랜치가 없습니다."));
            }
            let selected_item = selected
                .read(cx)
                .selected_value()
                .and_then(|name| names.iter().position(|s| s == name))
                .map(|index| choices[index].clone());
            let owner = weak.clone();
            let dialog = if merge {
                compact::dialog(dialog, &weak)
            } else {
                dialog.width(ui_px(590.))
            };
            dialog.title(label).child(body).footer(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(super::recovery::review_close(&weak))
                    .child(
                        theme::action_button(
                            "integration-review",
                            disabled || selected_item.is_none(),
                        )
                        .label(if merge {
                            "미리보기"
                        } else {
                            "대상 확인…"
                        })
                        .primary()
                        .on_click(move |_, w, cx| {
                            let Some(item) = selected_item.clone() else {
                                return;
                            };
                            let _ = owner.update(cx, |app, cx| {
                                if !merge {
                                    w.close_dialog(cx);
                                }
                                app.confirm_integration(kind, item, w, cx);
                            });
                        }),
                    ),
            )
        });
    }

    pub(super) fn confirm_integration(
        &mut self,
        kind: IntegrationKind,
        target: BranchIntegration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = target.current.root.clone();
        let label = action_label(kind).to_owned();
        let (write, description) = if kind == IntegrationKind::Merge {
            (
                super::review::ReviewWrite::Merge {
                    source: merge_source(&target),
                    destination: target.current.branch.clone(),
                },
                format!(
                    "가져올 참조 · {}\n받을 참조 · refs/heads/{}",
                    target.reference, target.current.branch
                ),
            )
        } else {
            (
                super::review::ReviewWrite::Rebase,
                format!(
                    "현재 브랜치: {} @ {}\n선택: {} @ {}\n충돌이 생기면 작업 상태에서 해결한 뒤 계속하거나 중단하세요.",
                    target.current.branch,
                    target.current.head,
                    target.label(),
                    target.oid
                ),
            )
        };
        self.workflow_read(
            root.clone(),
            move |ops| ops.preview_integration(kind, &target),
            move |app, preview, w, cx| {
                app.show_git_review(
                    super::review::Review {
                        root,
                        title: label,
                        description,
                        source: super::review::ReviewSource::Operation(preview),
                        write: Some(write),
                    },
                    w,
                    cx,
                );
            },
            window,
            cx,
        );
    }
}

fn merge_source(target: &BranchIntegration) -> String {
    if let Some(name) = target.reference.strip_prefix("refs/heads/") {
        format!("{name} · 로컬")
    } else if let Some(name) = target.reference.strip_prefix("refs/remotes/") {
        format!("{name} · 원격 추적")
    } else {
        target.reference.clone()
    }
}

fn action_label(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Merge => "Merge",
        IntegrationKind::Rebase => "Rebase",
    }
}
