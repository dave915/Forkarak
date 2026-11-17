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
        let selected = choice(
            choices.iter().map(|c| c.label().to_owned()).collect(),
            None,
            window,
            cx,
        );
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let choices = Arc::new(choices.clone());
            let empty = choices.is_empty();
            let weak = weak.clone();
            let selected = selected.clone();
            let mut body = div().v_flex().gap_3();
            if let Some(first) = choices.first() {
                body = body
                    .child(scope(&first.current.root, Some(&first.current.branch)))
                    .child(select_field(
                        if kind == IntegrationKind::Merge {
                            "가져와서 병합할 브랜치"
                        } else {
                            "현재 브랜치의 새 기준"
                        },
                        &selected,
                    ));
            } else {
                body = body.child(note("선택할 다른 로컬 또는 원격 추적 브랜치가 없습니다."));
            }
            dialog.title(label).width(ui_px(590.)).child(body).footer(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(close_button())
                    .child(
                        theme::action_button("integration-review", empty)
                            .label("대상 확인…")
                            .primary()
                            .on_click(move |_, w, cx| {
                                let Some(item) = selected
                                    .read(cx)
                                    .selected_value()
                                    .and_then(|name| choices.iter().find(|c| c.label() == name))
                                    .cloned()
                                else {
                                    return;
                                };
                                let _ = weak.update(cx, |app, cx| {
                                    w.close_dialog(cx);
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
        let description = format!(
            "현재 브랜치: {} @ {}\n선택: {} @ {}\n충돌이 생기면 작업 상태에서 해결한 뒤 계속하거나 중단하세요.",
            target.current.branch,
            target.current.head,
            target.label(),
            target.oid
        );
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
                        write: Some(super::review::ReviewWrite::Integrate),
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

fn action_label(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Merge => "Merge",
        IntegrationKind::Rebase => "Rebase",
    }
}
