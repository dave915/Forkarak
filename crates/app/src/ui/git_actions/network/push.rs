use super::super::{
    compact,
    recovery::review_close,
    review::{Review, ReviewSource, ReviewWrite},
};
use super::*;
use gpui_kit::component::select::SelectEvent;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui::git_actions) struct PushTarget {
    pub local: String,
    pub remote: String,
    pub branch: String,
    pub set_upstream: bool,
}

impl PushTarget {
    pub(super) fn configured(state: &RepositoryState, sync: &BranchSync) -> Option<Self> {
        let local = state.current_branch.as_ref()?;
        let upstream = sync.upstream.as_ref()?;
        if sync.branch.as_ref() != Some(local) || upstream.missing || upstream.remote == "." {
            return None;
        }
        let defaults = network_defaults(state, sync, Some(local));
        let remote = defaults.remote?;
        (!defaults.branch.is_empty()).then(|| Self {
            local: local.clone(),
            remote,
            branch: defaults.branch,
            set_upstream: false,
        })
    }

    pub(super) fn review(
        &self,
        ops: &GitOperations,
        state: &RepositoryState,
        starts: &[BranchStart],
    ) -> anyhow::Result<Review> {
        let config = state
            .remotes
            .iter()
            .find(|r| r.name == self.remote)
            .ok_or_else(|| anyhow::anyhow!("원격 저장소를 다시 선택하세요."))?;
        let reference = format!("refs/heads/{}", self.local);
        let selected = starts
            .iter()
            .find(|s| s.reference == reference)
            .ok_or_else(|| anyhow::anyhow!("보낼 브랜치의 커밋을 확인할 수 없습니다."))?;
        let preview = ops.preview_push(&self.remote, &self.local, &self.branch)?;
        verify_network_target(
            ops,
            config,
            state.current_branch.as_deref(),
            head(starts),
            Some(selected),
        )?;
        anyhow::ensure!(
            preview.head_oid == selected.oid,
            "보낼 커밋이 바뀌었습니다. 다시 확인하세요."
        );
        Ok(Review {
            root: state.root.clone(),
            title: "Push".into(),
            description: format!(
                "Push URL · {}\n업스트림 · {}",
                config.push_urls.join("\n"),
                upstream_label(self.set_upstream)
            ),
            source: ReviewSource::Operation(preview),
            write: Some(ReviewWrite::Push(self.clone())),
        })
    }

    pub(in crate::ui::git_actions) fn destination(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }
}

impl Pokarak {
    pub(in crate::ui::git_actions) fn open_branch_push(
        &mut self,
        root: PathBuf,
        edit: Option<PushTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let use_upstream = edit.is_none();
        self.workflow_read(root, move |ops| {
            let state = ops.state()?;
            let sync = ops.branch_sync()?;
            let starts = ops.branch_starts()?;
            let review = if use_upstream {
                PushTarget::configured(&state, &sync).map(|target| target.review(&ops, &state, &starts))
            } else { None };
            Ok((state, sync, starts, review))
        }, move |app, (state, sync, starts, review), w, cx| {
            let notice = match review {
                Some(Ok(review)) if matches!(&review.source, ReviewSource::Operation(p) if p.remote_state_known) => {
                    app.show_git_review(review, w, cx);
                    return;
                }
                Some(Ok(_)) => Some("로컬에서 원격 기준을 확인할 수 없습니다.".to_owned()),
                Some(Err(error)) => Some(format!("기본 대상 확인 실패 · {error}")),
                None => None,
            };
            app.branch_push_form(state, sync, starts, edit, notice, w, cx);
        }, window, cx);
    }

    fn branch_push_form(
        &mut self,
        state: RepositoryState,
        sync: BranchSync,
        starts: Vec<BranchStart>,
        edit: Option<PushTarget>,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = Arc::new(state);
        let starts = Arc::new(starts);
        let local = edit
            .as_ref()
            .map(|t| t.local.as_str())
            .or(state.current_branch.as_deref());
        let defaults = match &edit {
            Some(t) => NetworkDefaults {
                remote: Some(t.remote.clone()),
                branch: t.branch.clone(),
                set_upstream: t.set_upstream,
            },
            None => network_defaults(&state, &sync, local),
        };
        let source = optional_choice(state.local_branches.clone(), local, window, cx);
        let remote = optional_choice(
            state.remotes.iter().map(|r| r.name.clone()).collect(),
            defaults.remote.as_deref(),
            window,
            cx,
        );
        let destination = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(defaults.branch)
                .placeholder("브랜치 이름")
        });
        let upstream = cx.new(|_| defaults.set_upstream);
        let options = Rc::new(Cell::new(false));
        let notice = Rc::new(std::cell::RefCell::new(notice));
        let source_sub = cx.subscribe_in(&source, window, {
            let (state, remote, destination, upstream) = (
                state.clone(),
                remote.clone(),
                destination.clone(),
                upstream.clone(),
            );
            move |app: &mut Pokarak, _, event: &SelectEvent<SearchableVec<String>>, w, cx| {
                let SelectEvent::Confirm(local) = event;
                app.invalidate_workflow_read();
                let defaults = network_defaults(&state, &sync, local.as_deref());
                destination.update(cx, |input, cx| input.set_value(defaults.branch, w, cx));
                remote.update(cx, |input, cx| match defaults.remote {
                    Some(name) => input.set_selected_value(&name, w, cx),
                    None => input.set_selected_index(None, w, cx),
                });
                upstream.update(cx, |value, cx| {
                    *value = defaults.set_upstream;
                    cx.notify();
                });
                w.refresh();
            }
        });
        let dest_sub = cx.subscribe_in(&destination, window, {
            let notice = notice.clone();
            move |app: &mut Pokarak, _, event: &InputEvent, w, _| {
                if matches!(event, InputEvent::Change) {
                    notice.take();
                    app.invalidate_workflow_read();
                    w.refresh();
                }
            }
        });
        let remote_sub = cx.subscribe_in(&remote, window, {
            let notice = notice.clone();
            move |app: &mut Pokarak, _, _: &SelectEvent<SearchableVec<String>>, w, _| {
                notice.take();
                app.invalidate_workflow_read();
                w.refresh();
            }
        });
        let upstream_sub = cx.observe(&upstream, |app, _, _| app.invalidate_workflow_read());
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let _subscriptions = (&source_sub, &dest_sub, &remote_sub, &upstream_sub);
            let tracking = *upstream.read(cx);
            let selected_local = source.read(cx).selected_value();
            let selected_remote = remote.read(cx).selected_value();
            let branch = destination.read(cx).value();
            let push = selected_local
                .zip(selected_remote)
                .filter(|_| !branch.trim().is_empty())
                .map(|(local, remote)| PushTarget {
                    local: local.clone(),
                    remote: remote.clone(),
                    branch: branch.trim().to_owned(),
                    set_upstream: tracking,
                });
            let mut body = div()
                .v_flex()
                .gap_2()
                .child(compact::row(
                    "보낼 브랜치",
                    Select::new(&source)
                        .id("push-source")
                        .accessibility_label("보낼 로컬 브랜치")
                        .placeholder("로컬 브랜치 선택")
                        .w_full(),
                    cx,
                ))
                .child(compact::row(
                    "받을 브랜치",
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap_1()
                        .child(
                            div().flex_1().min_w_0().child(
                                Select::new(&remote)
                                    .id("push-remote")
                                    .accessibility_label("받을 원격 저장소")
                                    .placeholder("원격 선택")
                                    .w_full(),
                            ),
                        )
                        .child("/")
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&destination)
                                    .id("push-destination")
                                    .aria_label("받을 원격 브랜치"),
                            ),
                        ),
                    cx,
                ))
                .child(compact::worktree(&state.root, cx))
                .child(div().flex().child(compact::disclosure(
                    "push-options",
                    format!("추적 옵션 · {}", upstream_label(tracking)),
                    &options,
                )));
            if options.get() {
                body = body.child(check_field(
                    "push-upstream",
                    "Push 후 받을 브랜치를 업스트림으로 설정",
                    &upstream,
                    cx,
                ));
            }
            if state.remotes.is_empty() {
                body = body.child(note("먼저 원격 저장소를 추가하세요."));
            } else if state.local_branches.is_empty() {
                body = body.child(note("보낼 로컬 브랜치와 커밋이 필요합니다."));
            } else if let Some(message) = notice.borrow().as_deref() {
                body = body.child(note(message));
            }
            let (owner, state, starts) = (weak.clone(), state.clone(), starts.clone());
            let disabled = push.is_none()
                || super::super::branch_forms::branch_form_disabled(&weak, &state.root, cx);
            compact::dialog(dialog, &weak)
                .title("Push 대상")
                .child(body)
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(review_close(&weak))
                        .child(
                            theme::action_button("git-network-next", disabled)
                                .primary()
                                .label("미리보기")
                                .on_click(move |_, w, cx| {
                                    let Some(push) = push.clone() else {
                                        return;
                                    };
                                    let (state, starts) = (state.clone(), starts.clone());
                                    let _ = owner.update(cx, |app, cx| {
                                        app.workflow_read(
                                            state.root.clone(),
                                            move |ops| push.review(&ops, &state, &starts),
                                            |app, review, w, cx| app.show_git_review(review, w, cx),
                                            w,
                                            cx,
                                        )
                                    });
                                }),
                        ),
                )
        });
    }
}

fn upstream_label(set_upstream: bool) -> &'static str {
    if set_upstream {
        "받을 브랜치로 설정"
    } else {
        "기존 설정 유지"
    }
}
