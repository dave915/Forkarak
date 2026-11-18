use super::*;
use git_actions::SyncRead;

pub(super) struct SyncControl {
    pub command: &'static str,
    pub name: &'static str,
    pub count: Option<usize>,
    pub disabled_reason: Option<&'static str>,
}

impl SyncControl {
    pub fn tooltip(&self) -> String {
        if let Some(reason) = self.disabled_reason {
            return format!("{} · {reason}", self.name);
        }
        match (self.command, self.count) {
            ("pull", Some(n)) => format!("Pull · 받을 커밋 {n}개 (마지막 Fetch 기준)"),
            ("pull", None) => "Pull · 현재 브랜치에 fast-forward로 받기".into(),
            ("push", Some(n)) => format!("Push · 보낼 커밋 {n}개"),
            ("publish", _) => "게시 · 현재 브랜치를 처음 Push하고 업스트림 설정".into(),
            _ => "Push · 원격에 보내기".into(),
        }
    }

    pub fn menu_label(&self) -> String {
        // NativeMenu has no per-item tooltip. Keep the disabled reason visible there.
        if self.disabled_reason.is_some() {
            self.tooltip()
        } else {
            format!("{}…", self.name)
        }
    }
}

pub(super) struct SyncControls {
    pub pull: SyncControl,
    pub push: SyncControl,
}

impl SyncControls {
    pub fn blocked(&self, command: &str) -> bool {
        match command {
            "pull" => self.pull.disabled_reason.is_some(),
            "publish" | "push" => {
                self.push.command != command || self.push.disabled_reason.is_some()
            }
            _ => false,
        }
    }

    pub fn for_worktree(read: Option<&SyncRead>, tree: Option<&Worktree>) -> Self {
        let (sync, no_remote) = match read {
            Some(SyncRead::Ready { sync, has_remotes }) => (Some(sync), !has_remotes),
            _ => (None, false),
        };
        let no_remote = no_remote.then_some("등록된 원격 저장소가 없습니다.");
        let branch = sync.and_then(|s| s.branch.as_deref());
        let local = tree.filter(|t| !t.is_detached && Some(t.branch.as_str()) == branch);
        let publish =
            local.is_some_and(|t| !t.head.is_empty()) && sync.is_some_and(|s| s.upstream.is_none());
        let branch_reason = match read {
            None | Some(SyncRead::Loading) => Some("브랜치 동기화 정보를 확인 중입니다."),
            Some(SyncRead::Failed(_)) => Some("브랜치 동기화 정보를 읽을 수 없습니다."),
            _ if branch.is_none() => Some("detached HEAD입니다. 로컬 브랜치를 선택하세요."),
            _ if local.is_none() => Some("현재 브랜치를 확인하려면 새로고침하세요."),
            _ if local.is_some_and(|t| t.head.is_empty()) => Some("먼저 커밋을 만드세요."),
            _ => None,
        };
        let pull_reason = if publish {
            Some(if no_remote.is_some() {
                "업스트림이 없습니다. 원격을 추가한 뒤 브랜치를 게시하세요."
            } else {
                "업스트림이 없습니다. 먼저 브랜치를 게시하세요."
            })
        } else {
            no_remote.or(branch_reason)
        };
        let tracked = sync.filter(|s| local.is_some() && s.upstream.is_some());
        Self {
            pull: SyncControl {
                command: "pull",
                name: "Pull",
                count: tracked.and_then(|s| s.behind).filter(|n| *n > 0),
                disabled_reason: pull_reason,
            },
            push: SyncControl {
                command: if publish { "publish" } else { "push" },
                name: if publish { "게시" } else { "Push" },
                count: tracked.and_then(|s| s.ahead).filter(|n| *n > 0),
                // Ordinary Push lets the user choose any local source, including at detached HEAD.
                disabled_reason: no_remote,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SyncControls, SyncRead, Worktree};
    use pokarak_core::operations::sync::{BranchSync, Upstream};

    fn tree() -> Worktree {
        Worktree {
            branch: "topic".into(),
            head: "a".repeat(40),
            ..Default::default()
        }
    }

    fn read(branch: Option<&str>, has_remotes: bool) -> SyncRead {
        SyncRead::Ready {
            sync: BranchSync {
                branch: branch.map(str::to_owned),
                upstream: None,
                ahead: None,
                behind: None,
                last_fetch: None,
            },
            has_remotes,
        }
    }

    #[test]
    fn unpublished_branch_offers_publish_and_explains_disabled_pull_in_both_layouts() {
        let controls = SyncControls::for_worktree(Some(&read(Some("topic"), true)), Some(&tree()));
        assert_eq!(controls.push.name, "게시");
        assert_eq!(controls.push.menu_label(), "게시…");
        assert!(!controls.blocked("publish"));
        assert!(controls.blocked("push"));
        assert!(controls.blocked("pull"));
        assert!(controls.pull.tooltip().contains("업스트림이 없습니다"));
        assert_eq!(controls.pull.menu_label(), controls.pull.tooltip());

        let none = SyncControls::for_worktree(Some(&read(Some("topic"), false)), Some(&tree()));
        assert!(none.blocked("publish") && none.blocked("pull"));
        assert!(none.push.tooltip().contains("원격 저장소가 없습니다"));
        assert!(
            none.pull
                .tooltip()
                .contains("업스트림이 없습니다. 원격을 추가")
        );
        assert_eq!(none.pull.menu_label(), none.pull.tooltip());
    }

    #[test]
    fn detached_unknown_and_stale_branch_keep_the_explicit_push_picker() {
        let mut detached = tree();
        detached.is_detached = true;
        for (read, tree) in [
            (Some(read(None, true)), detached),
            (Some(SyncRead::Failed("unreadable".into())), tree()),
            (Some(SyncRead::Loading), tree()),
            (None, tree()),
            (Some(read(Some("different"), true)), tree()),
            (
                Some(read(Some("topic"), true)),
                Worktree {
                    head: String::new(),
                    ..tree()
                },
            ),
        ] {
            let controls = SyncControls::for_worktree(read.as_ref(), Some(&tree));
            assert_eq!(controls.push.menu_label(), "Push…");
            assert!(!controls.blocked("push"));
            assert!(controls.blocked("publish") && controls.blocked("pull"));
            assert!(!controls.pull.tooltip().contains("업스트림이 없습니다"));
        }
    }

    #[test]
    fn tracked_branch_keeps_push_pull_and_only_known_counts() {
        for missing in [false, true] {
            let read = SyncRead::Ready {
                sync: BranchSync {
                    branch: Some("topic".into()),
                    upstream: Some(Upstream {
                        reference: "refs/remotes/company/trunk".into(),
                        remote: "company".into(),
                        branch: "refs/heads/trunk".into(),
                        missing,
                    }),
                    ahead: (!missing).then_some(3),
                    behind: (!missing).then_some(2),
                    last_fetch: None,
                },
                has_remotes: true,
            };
            let controls = SyncControls::for_worktree(Some(&read), Some(&tree()));
            assert!(!controls.blocked("pull") && !controls.blocked("push"));
            assert!(controls.blocked("publish"));
            assert_eq!(controls.push.count, (!missing).then_some(3));
            assert_eq!(controls.pull.count, (!missing).then_some(2));
        }
    }
}
