//! Branch forms retain immutable sources and revalidate before explicit writes.
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;
use gpui_kit::component::{checkbox::Checkbox, select::SelectEvent};
use pokarak_core::operations::branches::BranchStart;
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
struct BranchCreation {
    root: PathBuf,
    name: String,
    start: BranchStart,
    head: String,
    current: Option<String>,
    checkout: bool,
    track: bool,
}

impl BranchCreation {
    fn detail(&self) -> String {
        format!(
            "현재 브랜치: {}\n현재 HEAD: {}\n시작 참조: {}\n시작 커밋: {}\n새 로컬 브랜치: {}\n전환: {}\n추적: {}",
            self.current.as_deref().unwrap_or("detached HEAD"),
            if self.head.is_empty() {
                "커밋 없음 (unborn HEAD)"
            } else {
                &self.head
            },
            self.start.reference,
            self.start.oid,
            self.name,
            if self.checkout {
                "생성 후 이 워크트리를 새 브랜치로 전환합니다. 덮어쓸 로컬 변경이 있으면 중단합니다."
            } else {
                "브랜치만 생성하고 현재 워크트리를 유지합니다."
            },
            if self.track {
                "선택한 원격 추적 참조의 Git fetch 매핑으로 upstream을 설정합니다."
            } else {
                "upstream을 설정하지 않습니다."
            },
        )
    }

    fn validate(&self, state: &RepositoryState, starts: &[BranchStart]) -> anyhow::Result<()> {
        anyhow::ensure!(
            state.root == self.root
                && state.current_branch == self.current
                && captured_head(starts) == self.head,
            "현재 브랜치 또는 HEAD가 변경되었습니다. 만들기 창을 다시 열어 확인하세요."
        );
        validate_new_name(state, &self.name)?;
        validate_preferred(starts, &self.start)?;
        anyhow::ensure!(
            !self.track || self.start.reference.starts_with("refs/remotes/"),
            "추적할 원격 브랜치를 선택하세요."
        );
        Ok(())
    }
}

#[derive(Clone)]
enum BranchWrite {
    Create(BranchCreation),
    Rename {
        old: String,
        new: String,
        oid: String,
    },
}

impl BranchWrite {
    fn run(self, ops: GitOperations) -> anyhow::Result<OperationOutput> {
        match self {
            Self::Create(request) => ops.create_branch_from(
                &request.name,
                &request.start,
                request.checkout,
                request.track,
                &request.head,
                request.current.as_deref(),
            ),
            Self::Rename { old, new, oid } => ops.rename_branch(&old, &new, &oid),
        }
    }
}

impl Pokarak {
    pub(in crate::ui) fn open_branch_create(
        &mut self,
        preferred: Option<BranchStart>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading || self.opening_path.is_some() {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        self.workflow_read(
            root,
            move |ops| {
                let state = ops.state()?;
                let starts = ops.branch_starts()?;
                if let Some(start) = &preferred {
                    validate_preferred(&starts, start)?;
                }
                Ok((state, starts, preferred))
            },
            |app, (state, starts, preferred), w, cx| {
                app.branch_create_form(state, starts, preferred, w, cx);
            },
            window,
            cx,
        );
    }

    fn branch_create_form(
        &mut self,
        state: RepositoryState,
        starts: Vec<BranchStart>,
        preferred: Option<BranchStart>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial_name = preferred
            .as_ref()
            .map(|s| suggested_name(&s.reference, &state.remotes))
            .unwrap_or_default();
        let explicit_oid = preferred
            .as_ref()
            .filter(|s| s.reference == s.oid)
            .map(|s| s.oid.clone())
            .unwrap_or_default();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial_name)
                .placeholder("feature/topic")
        });
        let explicit = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(explicit_oid)
                .placeholder("선택 사항 · 전체 40자리 또는 64자리 커밋 OID")
        });
        let selected = choice(
            starts.iter().map(|s| s.reference.clone()).collect(),
            preferred.as_ref().map(|s| s.reference.as_str()),
            window,
            cx,
        );
        for input in [&name, &explicit] {
            window
                .subscribe(input, cx, |_, event: &InputEvent, w, _| {
                    if matches!(event, InputEvent::Change) {
                        w.refresh();
                    }
                })
                .detach();
        }
        window
            .subscribe(
                &selected,
                cx,
                |_, _: &SelectEvent<SearchableVec<String>>, w, _| w.refresh(),
            )
            .detach();
        let checkout = Rc::new(Cell::new(false));
        let tracking = Rc::new(Cell::new(
            preferred
                .as_ref()
                .is_some_and(|s| s.reference.starts_with("refs/remotes/")),
        ));
        let head = captured_head(&starts);
        let state = Arc::new(state);
        let starts = Arc::new(starts);
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let source = selected_start(&starts, selected.read(cx).selected_value().map(String::as_str), explicit.read(cx).value().as_ref());
            let remote = source.as_ref().is_ok_and(|s| s.reference.starts_with("refs/remotes/"));
            let track = remote && tracking.get();
            let name_error = validate_new_name(&state, name.read(cx).value().trim()).err();
            let disabled = branch_form_disabled(&weak, &state.root, cx) || source.is_err() || name_error.is_some();
            let source_note = match &source {
                Ok(start) => format!("시작 커밋: {}", start.oid),
                Err(error) => error.to_string(),
            };
            let switch_flag = checkout.clone();
            let track_flag = tracking.clone();
            let mut body = div().id("branch-create-fields").max_h(ui_px(410.)).overflow_y_scroll().v_flex().gap_3()
                .child(scope(&state.root, state.current_branch.as_deref()))
                .child(note(&format!("현재 HEAD: {}", if head.is_empty() { "커밋 없음 (unborn HEAD)" } else { &head })))
                .child(field("새 로컬 브랜치 이름", &name))
                .child(div().v_flex().gap_2().child("시작 참조")
                    .child(Select::new(&selected).id("branch-start").w_full().cursor_pointer().disabled(!explicit.read(cx).value().trim().is_empty())))
                .child(field("전체 커밋 OID에서 시작", &explicit))
                .child(note("OID를 입력하면 시작 참조 대신 해당 커밋을 사용합니다. 원격 연결이나 Fetch는 실행하지 않습니다."))
                .child(note(&source_note))
                .child(Checkbox::new("branch-create-switch").label("생성 후 새 브랜치로 전환").checked(checkout.get())
                    .on_click(move |value, w, _| { switch_flag.set(*value); w.refresh(); }))
                .child(Checkbox::new("branch-create-track").label("선택한 원격 브랜치를 upstream으로 추적").checked(track).disabled(!remote)
                    .on_click(move |value, w, _| { track_flag.set(*value); w.refresh(); }));
            if let Some(error) = name_error {
                body = body.child(note(&error.to_string()));
            }
            if starts.is_empty() {
                body = body.child(note("이 저장소에는 선택할 브랜치나 HEAD 커밋이 없습니다. 이미 저장소에 존재하는 전체 커밋 OID만 사용할 수 있습니다."));
            }
            let (weak, state, starts, name, explicit, selected, head) = (weak.clone(), state.clone(), starts.clone(), name.clone(), explicit.clone(), selected.clone(), head.clone());
            let (checkout, tracking) = (checkout.clone(), tracking.clone());
            dialog.title("브랜치 만들기").width(ui_px(700.)).margin_top(ui_px(18.)).on_ok(|_, _, _| false)
                .child(body).footer(div().flex().justify_end().gap_2().child(close_button())
                    .child(theme::action_button("branch-create-review", disabled).label("대상 확인…").primary().on_click(move |_, w, cx| {
                        let Ok(start) = selected_start(&starts, selected.read(cx).selected_value().map(String::as_str), explicit.read(cx).value().as_ref()) else { return; };
                        let track = tracking.get() && start.reference.starts_with("refs/remotes/");
                        let request = BranchCreation { root: state.root.clone(), name: name.read(cx).value().trim().into(), start, head: head.clone(), current: state.current_branch.clone(), checkout: checkout.get(), track };
                        let _ = weak.update(cx, |app, cx| app.review_branch_creation(request, w, cx));
                    })))
        });
    }

    fn review_branch_creation(
        &mut self,
        request: BranchCreation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading || self.opening_path.is_some() {
            return;
        }
        self.workflow_read(
            request.root.clone(),
            move |ops| {
                request.validate(&ops.state()?, &ops.branch_starts()?)?;
                if request.start.reference == request.start.oid {
                    // Existing read-only core API verifies this is an available commit, including merges.
                    pokarak_core::history::commit_detail(&request.root, &request.start.oid, 0)?;
                }
                Ok(request)
            },
            |app, request, w, cx| {
                let detail = request.detail();
                app.confirm_branch_write(
                    request.root.clone(),
                    "브랜치 만들기",
                    detail,
                    BranchWrite::Create(request),
                    w,
                    cx,
                );
            },
            window,
            cx,
        );
    }

    pub(super) fn open_branch_rename(
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
                let state = ops.state()?;
                let starts = ops.branch_starts()?;
                let start = starts
                    .into_iter()
                    .find(|s| s.reference == reference)
                    .ok_or_else(|| {
                        anyhow::anyhow!("선택한 브랜치가 없습니다. 새로고침 후 다시 선택하세요.")
                    })?;
                let old = start
                    .reference
                    .strip_prefix("refs/heads/")
                    .ok_or_else(|| anyhow::anyhow!("로컬 브랜치를 선택하세요."))?
                    .to_owned();
                anyhow::ensure!(
                    state.current_branch.as_deref() == Some(&old)
                        || !state.checked_out_branches.contains(&old),
                    "다른 워크트리에서 사용 중인 브랜치는 해당 워크트리에서 이름을 변경하세요."
                );
                Ok((state, old, start.oid))
            },
            |app, (state, old, oid), w, cx| app.branch_rename_form(state, old, oid, w, cx),
            window,
            cx,
        );
    }

    fn branch_rename_form(
        &mut self,
        state: RepositoryState,
        old: String,
        oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(old.clone()));
        window
            .subscribe(&name, cx, |_, _: &InputEvent, w, _| w.refresh())
            .detach();
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let disabled = branch_form_disabled(&weak, &state.root, cx) || validate_new_name(&state, name.read(cx).value().trim()).is_err();
            let (weak, name, state, old, oid) = (weak.clone(), name.clone(), state.clone(), old.clone(), oid.clone());
            dialog.title("로컬 브랜치 이름 변경").width(ui_px(650.)).on_ok(|_, _, _| false)
                .child(div().v_flex().gap_3().child(scope(&state.root, state.current_branch.as_deref()))
                    .child(format!("브랜치: {old}\n현재 커밋: {oid}"))
                    .child(field("새 로컬 브랜치 이름", &name))
                    .child(note("로컬 브랜치와 관련 Git 설정 이름을 변경합니다. 원격 브랜치 이름은 변경하지 않습니다.")))
                .footer(div().flex().justify_end().gap_2().child(close_button())
                    .child(theme::action_button("branch-rename-review", disabled).label("변경 확인…").primary().on_click(move |_, w, cx| {
                        let new = name.read(cx).value().trim().to_owned();
                        if validate_new_name(&state, &new).is_err() { return; }
                        let detail = format!("현재 브랜치: {}\n로컬 브랜치: {old} → {new}\n확인한 커밋: {oid}\n로컬 브랜치의 reflog와 관련 Git 설정 이름도 함께 변경합니다. 원격 브랜치는 유지합니다.", state.current_branch.as_deref().unwrap_or("detached HEAD"));
                        let job = BranchWrite::Rename { old: old.clone(), new, oid: oid.clone() };
                        let _ = weak.update(cx, |app, cx| app.confirm_branch_write(state.root.clone(), "브랜치 이름 변경", detail, job, w, cx));
                    })))
        });
    }

    fn confirm_branch_write(
        &mut self,
        root: PathBuf,
        label: &'static str,
        detail: String,
        job: BranchWrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current.as_ref() != Some(&root)
            || self.loading
            || self.git_is_busy()
            || self.opening_path.is_some()
        {
            return;
        }
        let weak = cx.entity().downgrade();
        window.close_all_dialogs(cx);
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let disabled = branch_form_disabled(&weak, &root, cx);
            let (weak, root, job) = (weak.clone(), root.clone(), job.clone());
            dialog
                .title(format!("{label} 확인"))
                .width(ui_px(720.))
                .margin_top(ui_px(24.))
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .id("branch-write-detail")
                        .max_h(ui_px(350.))
                        .overflow_y_scroll()
                        .v_flex()
                        .gap_3()
                        .child(scope(&root, None))
                        .child(detail.clone()),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(close_button())
                        .child(
                            theme::action_button("branch-write-confirm", disabled)
                                .label(format!("{label} 실행"))
                                .primary()
                                .on_click(move |_, w, cx| {
                                    let job = job.clone();
                                    let _ = weak.update(cx, |app, cx| {
                                        if app.current.as_ref() != Some(&root)
                                            || app.git_is_busy()
                                            || app.loading
                                        {
                                            return;
                                        }
                                        w.close_all_dialogs(cx);
                                        app.run_git_job(
                                            root.clone(),
                                            label.into(),
                                            move |ops| job.run(ops),
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

fn branch_form_disabled(weak: &WeakEntity<Pokarak>, root: &Path, cx: &App) -> bool {
    weak.upgrade().is_none_or(|view| {
        let app = view.read(cx);
        app.current.as_deref() != Some(root)
            || app.loading
            || app.git_is_busy()
            || app.opening_path.is_some()
    })
}

fn captured_head(starts: &[BranchStart]) -> String {
    starts
        .iter()
        .find(|s| s.reference == "HEAD")
        .map(|s| s.oid.clone())
        .unwrap_or_default()
}

fn validate_preferred(starts: &[BranchStart], preferred: &BranchStart) -> anyhow::Result<()> {
    if preferred.reference == preferred.oid {
        explicit_start(&preferred.oid)?;
    } else {
        anyhow::ensure!(
            starts.contains(preferred),
            "선택한 시작 참조가 변경되었거나 없어졌습니다. 새로고침 후 다시 선택하세요."
        );
    }
    Ok(())
}

fn selected_start(
    starts: &[BranchStart],
    selected: Option<&str>,
    explicit: &str,
) -> anyhow::Result<BranchStart> {
    if !explicit.trim().is_empty() {
        return explicit_start(explicit.trim());
    }
    starts
        .iter()
        .find(|s| Some(s.reference.as_str()) == selected)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!("시작 참조 또는 저장소에 존재하는 전체 커밋 OID를 선택하세요.")
        })
}

fn explicit_start(oid: &str) -> anyhow::Result<BranchStart> {
    anyhow::ensure!(
        matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()),
        "전체 40자리 또는 64자리 커밋 OID를 입력하세요."
    );
    let oid = oid.to_ascii_lowercase();
    Ok(BranchStart {
        reference: oid.clone(),
        oid,
    })
}

fn validate_new_name(state: &RepositoryState, name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!name.is_empty(), "새 브랜치 이름을 입력하세요.");
    anyhow::ensure!(
        !state
            .local_branches
            .iter()
            .chain(&state.checked_out_branches)
            .any(|b| b == name),
        "이미 존재하거나 워크트리에서 사용 중인 이름입니다."
    );
    Ok(())
}

fn suggested_name(reference: &str, remotes: &[Remote]) -> String {
    let Some(remote_ref) = reference.strip_prefix("refs/remotes/") else {
        return String::new();
    };
    // Longest configured remote prefix preserves nested names and avoids guessing tracking configuration.
    remotes
        .iter()
        .filter_map(|remote| {
            remote_ref
                .strip_prefix(&format!("{}/", remote.name))
                .map(|branch| (remote.name.len(), branch))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, branch)| branch.to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        BranchCreation, BranchStart, Remote, RepositoryState, captured_head, explicit_start,
        selected_start, suggested_name, validate_preferred,
    };

    fn start(reference: &str, letter: char) -> BranchStart {
        BranchStart {
            reference: reference.into(),
            oid: letter.to_string().repeat(40),
        }
    }

    fn state() -> RepositoryState {
        RepositoryState {
            root: "/repo/worktree".into(),
            current_branch: Some("main".into()),
            local_branches: vec!["main".into()],
            checked_out_branches: vec!["main".into(), "occupied".into()],
            remotes: vec![],
            local_identity: Default::default(),
            effective_identity: Default::default(),
        }
    }

    fn request() -> BranchCreation {
        BranchCreation {
            root: state().root,
            name: "feature/new".into(),
            start: start("refs/remotes/origin/feature/deep", 'b'),
            head: "a".repeat(40),
            current: Some("main".into()),
            checkout: false,
            track: true,
        }
    }

    #[test]
    fn remote_name_suggestions_preserve_slashes_and_configured_remote_prefixes() {
        let remotes = ["origin", "upstream", "team", "team/nested"].map(|name| Remote {
            name: name.into(),
            fetch_urls: vec![],
            push_urls: vec![],
        });
        assert_eq!(
            suggested_name("refs/remotes/origin/feature/a/b", &remotes),
            "feature/a/b"
        );
        assert_eq!(
            suggested_name("refs/remotes/upstream/feature/a/b", &remotes),
            "feature/a/b"
        );
        assert_eq!(
            suggested_name("refs/remotes/team/nested/topic/sub", &remotes),
            "topic/sub"
        );
        assert_eq!(suggested_name("refs/remotes/unknown/topic", &remotes), "");
        assert_eq!(suggested_name("refs/heads/topic/sub", &remotes), "");
    }

    #[test]
    fn preferred_ref_must_still_exist_at_the_same_commit() {
        let selected = start("refs/remotes/origin/topic", 'a');
        assert!(validate_preferred(std::slice::from_ref(&selected), &selected).is_ok());
        assert!(validate_preferred(&[start(&selected.reference, 'b')], &selected).is_err());
        assert!(validate_preferred(&[], &selected).is_err());
        assert!(selected_start(&[], Some(&selected.reference), "").is_err());
    }

    #[test]
    fn explicit_commit_overrides_ref_and_requires_a_full_hash() {
        let starts = [start("HEAD", 'a')];
        let explicit =
            selected_start(&starts, Some("HEAD"), &format!(" {} ", "B".repeat(40))).unwrap();
        assert_eq!(explicit.oid, "b".repeat(40));
        assert_eq!(explicit.reference, explicit.oid);
        assert!(validate_preferred(&[], &explicit).is_ok());
        assert!(selected_start(&starts, Some("HEAD"), "HEAD~1").is_err());
        assert!(selected_start(&starts, Some("HEAD"), "abcd1234").is_err());
        assert!(explicit_start(&"g".repeat(40)).is_err());
        assert!(explicit_start(&"c".repeat(64)).is_ok());
        assert_eq!(
            selected_start(&starts, Some("HEAD"), " ").unwrap(),
            starts[0]
        );
    }

    #[test]
    fn creation_guards_source_head_current_worktree_and_destination() {
        let request = request();
        let mut starts = vec![start("HEAD", 'a'), request.start.clone()];
        assert!(request.validate(&state(), &starts).is_ok());
        starts[1].oid = "c".repeat(40);
        assert!(request.validate(&state(), &starts).is_err());
        starts[1] = request.start.clone();
        starts[0].oid = "c".repeat(40);
        assert!(request.validate(&state(), &starts).is_err());
        starts[0] = start("HEAD", 'a');
        for changed in [
            RepositoryState {
                current_branch: Some("other".into()),
                ..state()
            },
            RepositoryState {
                root: "/other".into(),
                ..state()
            },
            RepositoryState {
                local_branches: vec![request.name.clone()],
                ..state()
            },
            RepositoryState {
                checked_out_branches: vec![request.name.clone()],
                ..state()
            },
        ] {
            assert!(request.validate(&changed, &starts).is_err());
        }
    }

    #[test]
    fn unborn_and_detached_states_do_not_invent_head_or_tracking() {
        assert!(captured_head(&[]).is_empty());
        assert!(selected_start(&[], None, "").is_err());
        let mut request = request();
        request.start = explicit_start(&"b".repeat(40)).unwrap();
        request.head.clear();
        request.track = false;
        assert!(request.validate(&state(), &[]).is_ok());
        assert!(request.validate(&state(), &[start("HEAD", 'a')]).is_err());
        request.current = None;
        let detached = RepositoryState {
            current_branch: None,
            ..state()
        };
        assert!(request.validate(&detached, &[]).is_ok());
        request.track = true;
        assert!(request.validate(&detached, &[]).is_err());
    }

    #[test]
    fn confirmation_names_immutable_source_destination_and_each_effect() {
        let mut request = request();
        let detail = request.detail();
        for value in [
            &request.start.reference,
            &request.start.oid,
            &request.head,
            &request.name,
        ] {
            assert!(detail.contains(value));
        }
        assert!(detail.contains("현재 브랜치: main"));
        assert!(detail.contains("현재 워크트리를 유지"));
        assert!(detail.contains("fetch 매핑"));
        request.checkout = true;
        request.track = false;
        let detail = request.detail();
        assert!(detail.contains("워크트리를 새 브랜치로 전환"));
        assert!(detail.contains("upstream을 설정하지 않습니다"));
        request.head.clear();
        assert!(request.detail().contains("unborn HEAD"));
    }
}
