//! Explicit, read-only aggregate branch review, independent of graph filtering.
use super::*;

fn baseline_label(mode: &CompareMode) -> String {
    match mode {
        CompareMode::Uncommitted => "HEAD → 현재 파일".into(),
        CompareMode::Branch(base) => format!("공통 조상({base}, HEAD) → 현재 파일"),
    }
}

struct ComparisonRequest {
    root: PathBuf,
    generation: u64,
}

impl ComparisonRequest {
    fn accepts(&self, current: Option<&Path>, generation: u64, opening: bool) -> bool {
        current == Some(self.root.as_path()) && self.generation == generation && !opening
    }
}

impl Pokarak {
    pub(super) fn applied_comparison_label(&self) -> String {
        baseline_label(
            &self
                .session()
                .map(|session| session.saved.comparison.clone())
                .unwrap_or_default(),
        )
    }

    pub(super) fn open_branch_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.git_is_busy() || window.has_active_dialog(cx) {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        let Some(rows) = self.comparison_branch_choices() else {
            self.status = "브랜치 목록을 읽는 중입니다. 잠시 후 다시 시도하세요.".into();
            cx.notify();
            return;
        };
        let _ = root;
        self.open_workspace_choices(
            "브랜치 전체 변경 비교",
            "로컬·원격 브랜치 검색",
            rows,
            window,
            cx,
        );
    }

    pub(super) fn apply_branch_review(
        &mut self,
        root: PathBuf,
        compare: CompareMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current.as_ref() != Some(&root)
            || self.opening_path.is_some()
            || self.loading
            || self.git_is_busy()
        {
            return;
        }
        self.persist(cx);
        self.request += 1;
        self.file_request += 1;
        self.navigation_request += 1;
        self.navigating = false;
        self.file_loading = false;
        self.pending_cli_file = None;
        self.checking_changes = false;
        self.count_pending = false;
        self.loading = true;
        self.error = None;
        self.status = format!("비교 중 · {}", baseline_label(&compare));
        let request = ComparisonRequest {
            root: root.clone(),
            generation: self.request,
        };
        let task = cx.background_spawn(async move {
            pokarak_core::snapshot_current_inventory(&root, &compare)
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if !request.accepts(
                    app.current.as_deref(),
                    app.request,
                    app.opening_path.is_some(),
                ) {
                    return;
                }
                app.loading = false;
                match result {
                    Ok(snapshot) => {
                        // Neither the selected baseline nor the existing reader is
                        // replaced before successful validation of this exact root.
                        app.leave_git_graph();
                        app.external_change = false;
                        app.install_snapshot(snapshot, window, cx);
                        app.persist(cx);
                    }
                    Err(error) => {
                        app.error = Some(format!(
                            "비교 기준을 적용하지 못했습니다. 이전 비교를 유지합니다.\n{error:#}"
                        ));
                        app.status = "브랜치 비교 실패 · 이전 비교 유지".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{ComparisonRequest, baseline_label};
    use pokarak_core::CompareMode;
    use std::path::Path;

    #[test]
    fn late_comparison_cannot_install_into_a_different_or_reopened_worktree() {
        let request = ComparisonRequest {
            root: "/fixture/worktree-a".into(),
            generation: 9,
        };
        assert!(request.accepts(Some(Path::new("/fixture/worktree-a")), 9, false));
        assert!(!request.accepts(Some(Path::new("/fixture/worktree-b")), 9, false));
        assert!(!request.accepts(Some(Path::new("/fixture/worktree-a")), 10, false));
        assert!(!request.accepts(Some(Path::new("/fixture/worktree-a")), 9, true));
        assert!(!request.accepts(None, 9, false));
    }

    #[test]
    fn applied_label_distinguishes_merge_base_from_the_branch_tip_and_graph_filter() {
        assert_eq!(
            baseline_label(&CompareMode::Uncommitted),
            "HEAD → 현재 파일"
        );
        assert_eq!(
            baseline_label(&CompareMode::Branch("origin/main".into())),
            "공통 조상(origin/main, HEAD) → 현재 파일"
        );
    }
}
