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
            dialog.title(label).width(px(590.)).child(body).footer(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(close_button())
                    .child(
                        Button::new("integration-review")
                            .label("대상 확인…")
                            .primary()
                            .disabled(empty)
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
        let label = action_label(kind);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx,move |dialog,_,_| {
            let weak=weak.clone();let target=target.clone();
            let current=&target.current.branch;
            let source=target.label();
            let description=if kind==IntegrationKind::Merge {
                format!("{source}의 변경을 현재 브랜치 {current}에 병합합니다. Fast-forward가 가능하면 브랜치만 이동합니다.")
            } else {
                format!("{current}의 고유 커밋을 {source} 위에 다시 적용합니다. 다시 적용된 커밋의 해시가 바뀝니다.")
            };
            let body=div().v_flex().gap_2().child(scope(&target.current.root,Some(current)))
                .child(format!("현재 HEAD   {}    ·    선택한 브랜치   {source}  {}",&target.current.head[..8],&target.oid[..8]))
                .child(description).child(note("충돌이 생기면 ‘Stash / 충돌 해결’에서 해결한 뒤 계속하거나 중단할 수 있습니다."));
            let root=target.current.root.clone();
            dialog.title(format!("{label} 확인")).width(px(620.)).child(body)
                .footer(div().flex().justify_end().gap_2().child(close_button()).child(Button::new("integration-confirm").label(format!("{label} 실행")).primary().on_click(move |_,w,cx|{
                    let target=target.clone();
                    let _=weak.update(cx,|app,cx|{w.close_dialog(cx);app.run_git_job(root.clone(),label.into(),move |ops|ops.integrate_branch(kind,&target),w,cx);});
                })))
        });
    }
}

fn action_label(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Merge => "Merge",
        IntegrationKind::Rebase => "Rebase",
    }
}
