//! Local sync reads and explicitly confirmed, immutable network destinations.
use super::*;
use gpui_kit::component::{checkbox::Checkbox, select::SelectEvent};
use pokarak_core::operations::{branches::BranchStart, sync::BranchSync};

type GitJob = Arc<dyn Fn(GitOperations) -> anyhow::Result<OperationOutput> + Send + Sync>;

struct JobConfirmation {
    root: PathBuf,
    label: String,
    detail: String,
    destructive: bool,
    job: GitJob,
}

pub(super) enum SyncRead {
    Loading,
    Ready(BranchSync),
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
struct NetworkDefaults {
    remote: Option<String>,
    branch: String,
    set_upstream: bool,
}

fn network_defaults(
    state: &RepositoryState,
    sync: &BranchSync,
    source: Option<&str>,
) -> NetworkDefaults {
    // branch_sync describes only the current branch. Never reuse its upstream
    // for a different source selected in the Push picker.
    let current_source = source == state.current_branch.as_deref();
    let upstream = current_source.then_some(sync.upstream.as_ref()).flatten();
    let remote = if let Some(upstream) = upstream {
        // '.' is a local tracking relationship, not a network destination.
        state
            .remotes
            .iter()
            .find(|r| r.name == upstream.remote)
            .map(|r| r.name.clone())
    } else if current_source {
        state
            .remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| (state.remotes.len() == 1).then(|| &state.remotes[0]))
            .map(|r| r.name.clone())
    } else {
        None
    };
    let branch = upstream
        .and_then(|u| u.branch.strip_prefix("refs/heads/"))
        .unwrap_or_else(|| source.unwrap_or_default())
        .to_owned();
    NetworkDefaults {
        remote,
        branch,
        set_upstream: current_source && sync.upstream.is_none(),
    }
}

fn sync_detail(sync: &BranchSync) -> String {
    let branch = sync.branch.as_deref().unwrap_or("detached HEAD");
    let upstream = sync
        .upstream
        .as_ref()
        .map(|u| {
            format!(
                "{}{} (원격 {} / {})",
                u.reference,
                if u.missing {
                    " · 추적 참조 없음"
                } else {
                    ""
                },
                u.remote,
                u.branch
            )
        })
        .unwrap_or_else(|| "미설정".into());
    let ahead = sync
        .ahead
        .map(|n| n.to_string())
        .unwrap_or_else(|| "알 수 없음".into());
    let behind = sync
        .behind
        .map(|n| n.to_string())
        .unwrap_or_else(|| "알 수 없음".into());
    let fetch = sync
        .last_fetch
        .map(|t| history::relative_time(t, SystemTime::now()))
        .unwrap_or_else(|| "기록 없음".into());
    format!(
        "현재 브랜치: {branch}\nUpstream: {upstream}\n로컬이 앞섬: {ahead} / 뒤처짐: {behind}\n이 워크트리의 마지막 Fetch: {fetch}\n로컬 참조 기준입니다. 서버의 최신 상태는 Fetch 후 확인할 수 있습니다."
    )
}

fn head(starts: &[BranchStart]) -> Option<&str> {
    starts
        .iter()
        .find(|s| s.reference == "HEAD")
        .map(|s| s.oid.as_str())
}

fn verify_network_target(
    git: &GitOperations,
    remote: &Remote,
    current: Option<&str>,
    expected_head: Option<&str>,
    source: Option<&BranchStart>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        git.remotes()?.iter().any(|r| r == remote),
        "원격 URL 또는 설정이 변경되었습니다. 대상을 다시 확인하세요."
    );
    if let Some(source) = source {
        let state = git.state()?;
        let starts = git.branch_starts()?;
        anyhow::ensure!(
            state.current_branch.as_deref() == current
                && head(&starts) == expected_head
                && starts.contains(source),
            "현재 HEAD, 브랜치 또는 선택한 로컬 커밋이 변경되었습니다. 대상을 다시 확인하세요."
        );
    }
    Ok(())
}

impl Pokarak {
    pub(in crate::ui) fn refresh_branch_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.current.clone() else {
            return;
        };
        if self.active_worktree_has_git_job() {
            return;
        }
        self.git_ui.sync_request += 1;
        let token = self.git_ui.sync_request;
        let generation = self.request;
        self.git_ui.sync.insert(root.clone(), SyncRead::Loading);
        let target = root.clone();
        let task = cx.background_spawn(async move { GitOperations::open(&target)?.branch_sync() });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, _, cx| {
                if app.git_ui.sync_request != token
                    || app.current.as_ref() != Some(&root)
                    || app.request != generation
                    || app.opening_path.is_some()
                    || app.active_worktree_has_git_job()
                {
                    return;
                }
                let value = match result {
                    Ok(sync) => SyncRead::Ready(sync),
                    Err(error) => SyncRead::Failed(format!("{error:#}")),
                };
                app.git_ui.sync.insert(root, value);
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::ui) fn git_sync_button(&self, cx: &mut Context<Self>) -> Button {
        let metadata = self
            .current
            .as_ref()
            .and_then(|root| self.git_ui.sync.get(root));
        let fallback = self
            .active_worktree()
            .map(|tree| {
                if tree.is_detached {
                    "detached HEAD"
                } else {
                    tree.branch.as_str()
                }
            })
            .unwrap_or("브랜치 알 수 없음");
        let (label, tooltip) = match metadata {
            Some(SyncRead::Ready(sync)) => {
                let branch = sync.branch.as_deref().unwrap_or("detached HEAD");
                let upstream = sync
                    .upstream
                    .as_ref()
                    .map(|u| {
                        u.reference
                            .strip_prefix("refs/remotes/")
                            .or_else(|| u.reference.strip_prefix("refs/heads/"))
                            .unwrap_or(&u.reference)
                    })
                    .unwrap_or("upstream 없음");
                let count =
                    |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
                (
                    format!(
                        "{branch} → {upstream}  ↑{} ↓{}",
                        count(sync.ahead),
                        count(sync.behind)
                    ),
                    sync_detail(sync),
                )
            }
            Some(SyncRead::Failed(error)) => (
                format!("{fallback} · 동기화 ?"),
                format!("동기화 메타데이터 읽기 실패\n{error}"),
            ),
            _ => (
                format!(
                    "{fallback} · {}",
                    if self.active_worktree_has_git_job() {
                        "동기화 갱신 대기"
                    } else {
                        "동기화 확인 중"
                    }
                ),
                "로컬 Git 참조를 읽고 있습니다. 네트워크 연결은 하지 않습니다.".into(),
            ),
        };
        theme::action_button("git-sync-status", false)
            .label(label)
            .small()
            .max_w(ui_px(270.))
            .overflow_hidden()
            .tooltip(tooltip)
            .on_click(cx.listener(|app, _, w, cx| app.show_sync_details(w, cx)))
    }

    fn show_sync_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current.clone() else {
            return;
        };
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let detail = weak
                .upgrade()
                .filter(|view| view.read(cx).current.as_ref() == Some(&root))
                .map(|view| match view.read(cx).git_ui.sync.get(&root) {
                    Some(SyncRead::Ready(sync)) => sync_detail(sync),
                    Some(SyncRead::Failed(error)) => format!("메타데이터 읽기 실패\n{error}"),
                    _ => "로컬 Git 참조 확인 중…".into(),
                })
                .unwrap_or_else(|| "워크트리가 바뀌었습니다. 창을 닫고 다시 확인하세요.".into());
            let mut actions = div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap_2()
                .child(close_button());
            for (page, label) in [
                (Page::Fetch, "Fetch…"),
                (Page::Pull, "Pull…"),
                (Page::Push, "Push…"),
                (Page::Log, "작업 기록…"),
            ] {
                let target = weak.clone();
                let root = root.clone();
                actions = actions.child(theme::action_button(label, false).label(label).on_click(
                    move |_, w, cx| {
                        let _ = target.update(cx, |app, cx| {
                            if app.current.as_ref() == Some(&root) {
                                w.close_dialog(cx);
                                if matches!(page, Page::Log) {
                                    app.open_git_log(w, cx);
                                } else {
                                    app.load_git_page(Some(page), w, cx);
                                }
                            }
                        });
                    },
                ));
            }
            dialog
                .title("브랜치 동기화 상태")
                .width(ui_px(620.))
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(scope(&root, None))
                        .child(detail),
                )
                .footer(actions)
        });
    }

    pub(super) fn show_network_form(
        &mut self,
        state: Arc<RepositoryState>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workflow_read(
            state.root.clone(),
            |git| Ok((git.state()?, git.branch_sync()?, git.branch_starts()?)),
            move |app, (state, sync, starts), w, cx| {
                app.network_form(Arc::new(state), sync, Arc::new(starts), page, w, cx)
            },
            window,
            cx,
        );
    }

    fn network_form(
        &mut self,
        state: Arc<RepositoryState>,
        sync: BranchSync,
        starts: Arc<Vec<BranchStart>>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fetch = matches!(page, Page::Fetch);
        let push = matches!(page, Page::Push);
        let title = if fetch {
            "Fetch"
        } else if push {
            "Push"
        } else {
            "Pull (ff-only)"
        };
        let defaults = network_defaults(&state, &sync, state.current_branch.as_deref());
        let remote_values = state.remotes.iter().map(|r| r.name.clone()).collect();
        let remote = optional_choice(remote_values, defaults.remote.as_deref(), window, cx);
        let source = optional_choice(
            state.local_branches.clone(),
            state.current_branch.as_deref(),
            window,
            cx,
        );
        let destination = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(defaults.branch)
                .placeholder("원격 브랜치 이름")
        });
        let upstream = cx.new(|_| defaults.set_upstream);
        let subscription = if push {
            let state = state.clone();
            let sync = sync.clone();
            let destination = destination.clone();
            let remote = remote.clone();
            let upstream = upstream.clone();
            Some(cx.subscribe_in(
                &source,
                window,
                move |_: &mut Pokarak, _, event: &SelectEvent<SearchableVec<String>>, w, cx| {
                    let SelectEvent::Confirm(selected) = event;
                    let defaults = network_defaults(&state, &sync, selected.as_deref());
                    destination.update(cx, |input, cx| input.set_value(defaults.branch, w, cx));
                    remote.update(cx, |input, cx| {
                        if let Some(remote) = defaults.remote {
                            input.set_selected_value(&remote, w, cx);
                        } else {
                            input.set_selected_index(None, w, cx);
                        }
                    });
                    upstream.update(cx, |value, cx| {
                        *value = defaults.set_upstream;
                        cx.notify();
                    });
                    cx.notify();
                },
            ))
        } else {
            None
        };
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let _keep_subscription = &subscription;
            let mut body = div().id("git-network-form").max_h(ui_px(340.)).overflow_y_scroll().v_flex().gap_3().child(scope(&state.root, state.current_branch.as_deref())).child(note(&sync_detail(&sync))).child(select_field("원격 저장소", &remote));
            if push {
                body = body.child(select_field("보낼 로컬 브랜치", &source)).child(field("보낼 원격 브랜치", &destination)).child(check_field("push-upstream", "이 로컬 브랜치의 upstream 설정 (-u)", &upstream, cx)).child(note("현재 브랜치는 upstream을 기본값으로 씁니다. 다른 로컬 브랜치를 고르면 원격 선택과 목적지를 초기화합니다. 해당 브랜치의 추적 상태는 확인되지 않았으므로 직접 검토하세요."));
            } else if !fetch { body = body.child(field("가져올 원격 브랜치", &destination)).child(note("Fast-forward만 적용합니다. 이력이 갈라졌거나 로컬 변경을 덮어쓰면 중단합니다.")); }
            else { body = body.child(note("원격 추적 참조만 가져옵니다. 작업 파일과 현재 브랜치는 유지합니다.")); }
            if sync.upstream.as_ref().is_some_and(|u| u.remote == ".") { body = body.child(note("현재 upstream은 로컬 브랜치입니다. 네트워크 원격을 직접 선택하세요.")); }
            if state.remotes.is_empty() { body = body.child(note("등록된 원격이 없습니다. Git 작업의 원격 저장소 관리에서 추가하세요.")); }
            if !fetch && head(&starts).is_none() { body = body.child(note("현재 HEAD에 커밋이 없습니다. 먼저 커밋을 만들거나 기존 브랜치를 확인하세요.")); }
            let mut buttons = div().flex().flex_wrap().justify_end().gap_2().child(close_button());
            for prune in if fetch { vec![true, false] } else { vec![false] } {
                let target = weak.clone(); let state = state.clone(); let starts = starts.clone(); let remote = remote.clone(); let source = source.clone(); let destination = destination.clone(); let upstream = upstream.clone();
                let disabled = state.remotes.is_empty()
                    || remote.read(cx).selected_value().is_none()
                    || (!fetch && destination.read(cx).value().trim().is_empty())
                    || (push && source.read(cx).selected_value().is_none())
                    || (!fetch && !push && (state.current_branch.is_none() || head(&starts).is_none()));
                buttons = buttons.child(theme::action_button(if prune { "git-fetch-prune" } else { "git-network-next" }, disabled).label(if prune { "Fetch + 원격 추적 정리…" } else { "대상 확인…" }).on_click(move |_, w, cx| {
                    let Some(remote_name) = remote.read(cx).selected_value().cloned() else { return; };
                    let Some(config) = state.remotes.iter().find(|r| r.name == remote_name).cloned() else { return; };
                    let branch = destination.read(cx).value().trim().to_owned();
                    let local = source.read(cx).selected_value().cloned();
                    let track = *upstream.read(cx);
                    let expected_current = state.current_branch.clone(); let expected_head = head(&starts).map(str::to_owned);
                    let urls = if push { config.push_urls.join("\n") } else { config.fetch_urls.join("\n") };
                    let (label, detail, job): (String, String, GitJob) = if fetch {
                        let label = if prune { "Fetch + 원격 추적 정리" } else { "Fetch" }.to_owned();
                        let detail = format!("원격: {remote_name}\nFetch URL:\n{urls}\n{}", if prune { "사라진 원격 추적 참조를 정리합니다 (--prune)." } else { "원격 추적 참조를 갱신합니다. Prune은 하지 않습니다." });
                        let job = Arc::new(move |git: GitOperations| { verify_network_target(&git, &config, None, None, None)?; git.execute(&Operation::Fetch { remote: remote_name.clone(), prune }) });
                        (label, detail, job)
                    } else {
                        if branch.is_empty() { return; }
                        let Some(local) = (if push { local } else { state.current_branch.clone() }) else { return; };
                        let Some(selected) = starts.iter().find(|s| s.reference == format!("refs/heads/{local}")).cloned() else { return; };
                        let detail = format!("원격: {remote_name}\n{} URL:\n{urls}\n로컬: {local} @ {}\n원격 브랜치: {branch}\n{}", if push { "Push" } else { "Fetch" }, selected.oid, if push { format!("이 브랜치 하나만 보냅니다. 강제 푸시와 태그 전송은 하지 않습니다.\nUpstream 설정: {}", if track { "설정 / 기존 값 변경 (-u)" } else { "유지" }) } else { "원격 → 현재 로컬 브랜치에 fast-forward만 적용합니다.".into() });
                        let job = Arc::new(move |git: GitOperations| {
                            verify_network_target(&git, &config, expected_current.as_deref(), expected_head.as_deref(), Some(&selected))?;
                            if push { git.push_branch(&remote_name, &local, &branch, track) }
                            else { git.execute(&Operation::Pull { remote: remote_name.clone(), remote_branch: branch.clone(), local_branch: local.clone() }) }
                        });
                        (title.into(), detail, job)
                    };
                    let _ = target.update(cx, |app, cx| app.confirm_git_job(JobConfirmation { root: state.root.clone(), label, detail, destructive: prune, job }, w, cx));
                }));
            }
            dialog.title(title).width(ui_px(660.)).margin_top(ui_px(12.)).child(body).footer(buttons)
        });
    }

    fn confirm_git_job(
        &mut self,
        confirmation: JobConfirmation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let JobConfirmation {
            root,
            label,
            detail,
            destructive,
            job,
        } = confirmation;
        window.close_dialog(cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let target = weak.clone(); let root_for_action = root.clone(); let label_for_action = label.clone(); let job = job.clone();
            dialog.title(format!("{label} 확인")).width(ui_px(660.)).margin_top(ui_px(16.)).child(div().id("git-job-confirm-detail").max_h(ui_px(310.)).overflow_y_scroll().v_flex().gap_3().child(scope(&root, None)).child(detail.clone()).child(note("Git hooks와 기존 인증 설정을 사용합니다. 확인 중 대상 커밋이나 원격 설정이 바뀌면 중단합니다."))).footer(div().flex().justify_end().gap_2().child(close_button()).child(theme::action_button("git-confirm", false).label(label.clone()).when(destructive, |b| b.danger()).when(!destructive, |b| b.primary()).on_click(move |_, w, cx| {
                let job = job.clone(); let _ = target.update(cx, |app, cx| { w.close_dialog(cx); app.run_git_job(root_for_action.clone(), label_for_action.clone(), move |git| job(git), w, cx); });
            })))
        });
    }
}

pub(super) fn optional_choice(
    values: Vec<String>,
    selected: Option<&str>,
    window: &mut Window,
    cx: &mut Context<Pokarak>,
) -> Choice {
    let index = selected
        .and_then(|s| values.iter().position(|v| v == s))
        .map(IndexPath::new);
    cx.new(|cx| SelectState::new(SearchableVec::new(values), index, window, cx).searchable(true))
}

pub(super) fn check_field(
    id: &'static str,
    label: &'static str,
    state: &Entity<bool>,
    cx: &mut App,
) -> Checkbox {
    let target = state.clone();
    Checkbox::new(id)
        .label(label)
        .checked(*state.read(cx))
        .on_click(move |checked, window, cx| {
            target.update(cx, |value, cx| {
                *value = *checked;
                cx.notify();
            });
            window.refresh();
        })
}

#[cfg(test)]
mod tests {
    use super::{network_defaults, sync_detail};
    use pokarak_core::operations::{
        Identity, Remote, RepositoryState,
        sync::{BranchSync, Upstream},
    };
    fn data() -> (RepositoryState, BranchSync) {
        (
            RepositoryState {
                root: "/fixture".into(),
                current_branch: Some("main".into()),
                local_branches: vec!["main".into(), "unpublished".into()],
                checked_out_branches: vec!["main".into()],
                remotes: vec![Remote {
                    name: "origin".into(),
                    fetch_urls: vec!["/fixture/origin.git".into()],
                    push_urls: vec!["/fixture/origin.git".into()],
                }],
                local_identity: Identity::default(),
                effective_identity: Identity::default(),
            },
            BranchSync {
                branch: Some("main".into()),
                upstream: Some(Upstream {
                    reference: "refs/remotes/origin/trunk".into(),
                    remote: "origin".into(),
                    branch: "refs/heads/trunk".into(),
                    missing: false,
                }),
                ahead: Some(1),
                behind: Some(2),
                last_fetch: None,
            },
        )
    }
    #[test]
    fn differently_named_upstream_and_changing_source_never_retargets_trunk() {
        let (state, sync) = data();
        let initial = network_defaults(&state, &sync, Some("main"));
        assert_eq!(initial.remote.as_deref(), Some("origin"));
        assert_eq!(initial.branch, "trunk");
        assert!(!initial.set_upstream);
        let changed = network_defaults(&state, &sync, Some("unpublished"));
        assert_eq!(changed.remote, None);
        assert_eq!(changed.branch, "unpublished");
        assert!(!changed.set_upstream);
    }
    #[test]
    fn local_upstream_does_not_guess_network_remote_and_unknown_stays_unknown() {
        let (state, mut sync) = data();
        sync.upstream.as_mut().unwrap().remote = ".".into();
        sync.ahead = None;
        sync.behind = None;
        assert_eq!(network_defaults(&state, &sync, Some("main")).remote, None);
        let detail = sync_detail(&sync);
        assert!(detail.contains("알 수 없음"));
        assert!(detail.contains("기록 없음"));
        sync.upstream = None;
        assert!(network_defaults(&state, &sync, Some("main")).set_upstream);
    }
    fn fixture_git(root: &std::path::Path, global: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-C",
            ])
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    #[test]
    fn confirmation_rejects_external_source_commit_and_remote_url_edits() {
        use pokarak_core::operations::GitOperations;
        use std::fs;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let global = temp.path().join("empty-global");
        fs::create_dir(&root).unwrap();
        fs::write(&global, "").unwrap();
        fixture_git(&root, &global, &["init", "-b", "main"]);
        fixture_git(
            &root,
            &global,
            &["config", "--local", "user.name", "Fixture"],
        );
        fixture_git(
            &root,
            &global,
            &["config", "--local", "user.email", "fixture@example.invalid"],
        );
        fixture_git(&root, &global, &["commit", "--allow-empty", "-m", "seed"]);
        fixture_git(&root, &global, &["branch", "unpublished"]);
        let origin = temp.path().join("origin.git");
        fixture_git(
            &root,
            &global,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let git = GitOperations::open(&root).unwrap();
        let remote = git.remotes().unwrap().remove(0);
        let starts = git.branch_starts().unwrap();
        let expected_head = super::head(&starts);
        let source = starts
            .iter()
            .find(|s| s.reference == "refs/heads/unpublished")
            .unwrap();
        super::verify_network_target(&git, &remote, Some("main"), expected_head, Some(source))
            .unwrap();
        // Move only the selected source; the current main HEAD stays unchanged.
        let tree = fixture_git(&root, &global, &["rev-parse", "HEAD^{tree}"]);
        let next = fixture_git(
            &root,
            &global,
            &["commit-tree", &tree, "-p", &source.oid, "-m", "external"],
        );
        fixture_git(
            &root,
            &global,
            &["update-ref", "refs/heads/unpublished", &next],
        );
        assert!(
            super::verify_network_target(&git, &remote, Some("main"), expected_head, Some(source))
                .is_err()
        );
        fixture_git(
            &root,
            &global,
            &["update-ref", "refs/heads/unpublished", &source.oid],
        );
        fixture_git(
            &root,
            &global,
            &[
                "remote",
                "set-url",
                "origin",
                temp.path().join("changed.git").to_str().unwrap(),
            ],
        );
        assert!(
            super::verify_network_target(&git, &remote, Some("main"), expected_head, Some(source))
                .is_err()
        );
        assert!(
            !origin.exists(),
            "read-only guards must not contact or create a remote"
        );
    }
}
