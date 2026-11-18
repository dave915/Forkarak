//! Immutable native commit/file previews. File reads use retained trees, never
//! re-resolved mutable form fields or current working files.
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;
use pokarak_core::{
    comparison::RefComparison,
    history::{Commit, CommitDetail},
    operations::{preview::OperationPreview, recovery::ReflogEntry},
};
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
pub(super) enum ReviewSource {
    Operation(OperationPreview),
    Comparison(RefComparison),
    Reflog {
        entry: ReflogEntry,
        detail: CommitDetail,
    },
}
#[derive(Clone)]
pub(super) enum ReviewWrite {
    Push(network::PushTarget),
    Merge { source: String, destination: String },
    Rebase,
}
#[derive(Clone)]
pub(super) struct Review {
    pub root: PathBuf,
    pub title: String,
    pub description: String,
    pub source: ReviewSource,
    pub write: Option<ReviewWrite>,
}
impl Review {
    fn files(&self) -> &[ChangedFile] {
        match &self.source {
            ReviewSource::Operation(p) => &p.files,
            ReviewSource::Comparison(p) => &p.files,
            ReviewSource::Reflog { detail, .. } => &detail.files,
        }
    }
    fn commits(&self) -> &[Commit] {
        match &self.source {
            ReviewSource::Operation(p) => &p.commits,
            ReviewSource::Reflog { detail, .. } => std::slice::from_ref(&detail.commit),
            _ => &[],
        }
    }
    fn diff(&self, path: &Path) -> anyhow::Result<DiffDocument> {
        match &self.source {
            ReviewSource::Operation(p) => p.diff(&self.root, path),
            ReviewSource::Comparison(p) => pokarak_core::comparison::diff(&self.root, p, path),
            ReviewSource::Reflog { detail, .. } => {
                pokarak_core::history::historical_diff(&self.root, detail, path)
            }
        }
    }
}
impl Pokarak {
    pub(super) fn show_git_review(
        &mut self,
        review: Review,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.present_git_review(Arc::new(review), false, window, cx);
    }

    fn present_git_review(
        &mut self,
        review: Arc<Review>,
        show_details: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        let weak = cx.entity().downgrade();
        let expanded = Rc::new(Cell::new(show_details));
        window.open_app_dialog(cx, move |dialog, _, cx| {
            let busy = super::branch_forms::branch_form_disabled(&weak, &review.root, cx);
            let compact = matches!(review.write, Some(ReviewWrite::Push(_) | ReviewWrite::Merge { .. }));
            let mut body = div().v_flex().min_w_0().gap_2();
            let mut footer = div().flex().items_center().justify_end().gap_2();
            if let (Some(write), ReviewSource::Operation(p)) = (&review.write, &review.source) && compact {
                match write {
                    ReviewWrite::Push(push) => {
                        body = body.child(compact::row("보낼 브랜치", push.local.clone(), cx))
                            .child(compact::row("받을 브랜치", push.destination(), cx))
                            .when(push.set_upstream, |body| body.child(note("Push 후 받을 브랜치를 업스트림으로 설정합니다.")));
                        let owner = weak.clone();
                        let root = review.root.clone();
                        let push = push.clone();
                        footer = footer.child(theme::action_button("push-change-destination", busy)
                            .label("대상 변경…").on_click(move |_, w, cx| {
                                let _ = owner.update(cx, |app, cx| app.open_branch_push(root.clone(), Some(push.clone()), w, cx));
                            })).child(div().flex_1());
                    }
                    ReviewWrite::Merge { source, destination } => {
                        body = body.child(compact::row("가져올 브랜치", source.clone(), cx))
                            .child(compact::row("병합할 위치", destination.clone(), cx));
                    }
                    ReviewWrite::Rebase => unreachable!(),
                }
                body = body.child(compact::worktree(&review.root, cx))
                    .child(div().border_t_1().border_color(Palette::new(cx).rgb(LINE)).pt_2()
                        .child(preview_summary(write, p)))
                    .child(div().flex().child(compact::disclosure("git-review-details", "커밋·파일 상세".into(), &expanded)));
                if expanded.get() {
                    body = body.child(scope(&review.root, None)).child(note(&review.description))
                        .child(note(&format!("소스 커밋 · {}\n대상 기준 · {}", p.head_oid, p.base_oid.as_deref().unwrap_or("알 수 없음"))));
                }
            } else {
                body = body.child(scope(&review.root, None)).child(review.description.clone());
                match &review.source {
                    ReviewSource::Operation(p) => {
                        body = body.child(note(&p.baseline_description))
                            .child(note(&format!("기준: {}\n대상: {}", p.base_oid.as_deref().unwrap_or("로컬에 알려진 기준 없음"), p.head_oid)));
                    }
                    ReviewSource::Comparison(p) => {
                        body = body.child(note(&format!("직접 비교 (공통 조상 사용 안 함)\n이전: {}\n이후: {}\n작업 파일 변경은 포함하지 않습니다.", p.left_oid, p.right_oid)));
                    }
                    ReviewSource::Reflog { entry, detail } => {
                        body = body.child(note(&format!("{} · {}\n{}\n부모 기준: {}", entry.selector, entry.timestamp, entry.oid, detail.parent.as_deref().unwrap_or("최초 커밋 · 빈 트리"))));
                    }
                }
            }
            if !compact || expanded.get() {
                body = body.child(review_lists(&review, &weak, compact, busy, cx));
            }

            footer = footer.child(super::recovery::review_close(&weak));
            if let ReviewSource::Reflog { entry, .. } = &review.source {
                let target = weak.clone(); let entry = entry.clone();
                footer = footer.child(theme::action_button("reflog-create-recovery", busy).label("복구 브랜치 만들기…").on_click(move |_,w,cx| { let _ = target.update(cx, |app,cx| app.show_recovery_branch(entry.clone(),w,cx)); }));
            }
            if let Some(write) = &review.write {
                let target = weak.clone(); let review = review.clone(); let write = write.clone();
                let label = match &write {
                    ReviewWrite::Push(push) if push.set_upstream => "게시",
                    ReviewWrite::Push(_) => "푸시",
                    ReviewWrite::Merge { .. } => "병합",
                    ReviewWrite::Rebase => "Rebase 실행",
                };
                footer = footer.child(theme::action_button("git-review-execute", busy).label(label).primary().on_click(move |_,w,cx| {
                    let ReviewSource::Operation(preview) = &review.source else { return; };
                    let preview = preview.clone(); let write = write.clone();
                    let _ = target.update(cx, |app,cx| {
                        w.close_dialog(cx);
                        app.run_git_job(review.root.clone(), review.title.clone(), move |ops| match write {
                            ReviewWrite::Push(push) => ops.execute_push_preview(&preview, push.set_upstream),
                            ReviewWrite::Merge { .. } | ReviewWrite::Rebase => ops.execute_integration_preview(&preview),
                        }, w,cx);
                    });
                }));
            }
            let dialog = if compact {
                compact::dialog(dialog, &weak).title(format!("{} 확인", review.title))
            } else {
                dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } })
                    .title(format!("{} · 대상과 변경 확인", review.title)).width(ui_px(780.)).margin_top(ui_px(12.))
            };
            dialog.child(body.id("git-immutable-review").max_h(ui_px(410.)).overflow_y_scroll()).footer(footer)
        });
    }

    fn read_git_review_file(
        &mut self,
        review: Arc<Review>,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = review.clone();
        let file = path.clone();
        self.workflow_read(
            review.root.clone(),
            move |_| source.diff(&file),
            move |app, doc, w, cx| app.show_git_review_file(review, path, doc, w, cx),
            window,
            cx,
        );
    }
    fn show_git_review_file(
        &mut self,
        review: Arc<Review>,
        path: PathBuf,
        doc: DiffDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editors = [doc.patch, doc.old_text, doc.new_text].map(|text| {
            let editor = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language("plaintext")
                    .default_value(text)
            });
            editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
            editor
        });
        let selected = Rc::new(Cell::new(0));
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_app_dialog(cx, move |dialog, window, _| {
            let mut tabs = div().flex().gap_2();
            for (index, label) in ["Diff", "이전 코드", "이후 코드"].into_iter().enumerate()
            {
                let selected = selected.clone();
                let notify = weak.clone();
                tabs = tabs.child(
                    Button::new(("git-review-code", index))
                        .label(label)
                        .selected(selected.get() == index)
                        .small()
                        .on_click(move |_, _, cx| {
                            selected.set(index);
                            let _ = notify.update(cx, |_, cx| cx.notify());
                        }),
                );
            }
            let back = weak.clone();
            let review = review.clone();
            dialog
                .on_close({
                    let owner = weak.clone();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |app, _| app.invalidate_workflow_read());
                    }
                })
                .title(format!(
                    "{} · 읽기 전용",
                    super::super::git_commit::display_path(&path)
                ))
                .width(ui_px(860.))
                .margin_top(ui_px(12.))
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(scope(&review.root, None))
                        .child(tabs)
                        .when(doc.binary, |d| {
                            d.child(note(
                                "바이너리 파일입니다. 텍스트 코드를 표시할 수 없습니다.",
                            ))
                        })
                        .when(doc.truncated, |d| {
                            d.child(note(
                                "파일이 표시 한도를 초과했습니다. 코드 / diff 일부만 표시합니다.",
                            ))
                        })
                        .child(
                            Editor::new(&editors[selected.get()])
                                .aria_label("변경 코드 읽기 전용")
                                .h(ui_px(350.)
                                    .min(window.viewport_size().height - ui_px(220.))
                                    .max(ui_px(60.))),
                        ),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(super::recovery::review_close(&weak))
                        .child(
                            Button::new("git-review-back")
                                .label("대상 목록으로")
                                .on_click(move |_, w, cx| {
                                    let _ = back.update(cx, |app, cx| {
                                        app.present_git_review(review.clone(), true, w, cx)
                                    });
                                }),
                        ),
                )
        });
    }
}

fn preview_summary(write: &ReviewWrite, preview: &OperationPreview) -> String {
    let commits = preview.commits.len();
    let files = preview.files.len();
    let count = if preview.commits_truncated {
        format!("{commits}개 초과")
    } else {
        format!("{commits}개")
    };
    match write {
        ReviewWrite::Push(_) if !preview.remote_state_known => format!(
            "원격 기준을 알 수 없습니다. 소스 전체를 표시합니다.\n커밋 {count} · 파일 {files}개"
        ),
        ReviewWrite::Push(_) => format!("로컬 원격 추적 기준 · 커밋 {count} · 변경 파일 {files}개"),
        ReviewWrite::Merge { .. } => format!("가져올 커밋 {count} · 브랜치 간 파일 차이 {files}개"),
        ReviewWrite::Rebase => unreachable!(),
    }
}

fn review_lists(
    review: &Arc<Review>,
    weak: &WeakEntity<Pokarak>,
    compact: bool,
    busy: bool,
    cx: &App,
) -> Div {
    let mut lists = div().v_flex().min_w_0().gap_1();
    if !matches!(&review.source, ReviewSource::Comparison(_)) {
        let truncated = matches!(&review.source, ReviewSource::Operation(p) if p.commits_truncated);
        lists = lists.child(if truncated {
            format!("커밋 · 처음 {}개 표시", review.commits().len())
        } else {
            format!("커밋 {}개", review.commits().len())
        });
        let mut commits = div()
            .id("git-review-commits")
            .max_h(ui_px(125.))
            .overflow_y_scroll()
            .v_flex()
            .gap_1();
        for commit in review.commits() {
            let line = if compact {
                div()
                    .v_flex()
                    .child(format!("{} · {}", commit.subject, commit.author))
                    .child(note(&commit.oid).text_color(Palette::new(cx).rgb(MUTED)))
            } else {
                div().child(format!(
                    "{} · {} · {}",
                    &commit.oid[..8],
                    commit.author,
                    commit.subject
                ))
            };
            commits = commits.child(line.flex_none());
        }
        lists = lists.child(commits);
    }
    lists = lists.child(format!(
        "파일 {}개 · 선택하여 Diff 보기",
        review.files().len()
    ));
    let mut files = div()
        .id("git-review-files")
        .max_h(ui_px(170.))
        .overflow_y_scroll()
        .v_flex()
        .gap_1();
    for (index, file) in review.files().iter().enumerate() {
        let target = weak.clone();
        let source = review.clone();
        let path = file.path.clone();
        let display = super::super::git_commit::display_path(&path);
        let display = match &file.old_path {
            Some(old) if old != &path => format!(
                "{} → {display}",
                super::super::git_commit::display_path(old)
            ),
            _ => display,
        };
        let (status, _) = file_status(&file.status);
        let label = format!("{status}  {display}");
        files = files.child(
            theme::action_button(("git-review-file", index), busy)
                .accessibility_label(label.clone())
                .tooltip(label.clone())
                .child(dialogs::choice_body().text_ellipsis().child(label))
                .small()
                .flex_none()
                .on_click(move |_, w, cx| {
                    let _ = target.update(cx, |app, cx| {
                        app.read_git_review_file(source.clone(), path.clone(), w, cx)
                    });
                }),
        );
    }
    lists.child(files)
}

#[cfg(test)]
mod tests {
    use super::{Review, ReviewSource};
    use std::{fs, process::Command};
    #[test]
    fn review_routes_exact_pinned_comparison_and_rejects_other_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", temp.path().join("no-global"))
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-b", "main", "--", "."]);
        fs::write(root.join("file"), "base\n").unwrap();
        git(&["add", "--", "file"]);
        git(&["commit", "-m", "base"]);
        git(&["checkout", "-b", "left"]);
        fs::write(root.join("file"), "left\n").unwrap();
        git(&["commit", "-a", "-m", "left"]);
        git(&["checkout", "main"]);
        fs::write(root.join("file"), "right\n").unwrap();
        git(&["commit", "-a", "-m", "right"]);
        let root = root.canonicalize().unwrap();
        let comparison = pokarak_core::comparison::compare_refs(&root, "left", "main").unwrap();
        let mut review = Review {
            root: root.clone(),
            title: "compare".into(),
            description: String::new(),
            source: ReviewSource::Comparison(comparison),
            write: None,
        };
        // Moving the named reference and changing current source cannot retarget
        // the review; no merge-base/current-source fallback is allowed.
        git(&["branch", "-f", "--", "left", "main"]);
        fs::write(root.join("file"), "working\n").unwrap();
        let diff = review.diff(std::path::Path::new("file")).unwrap();
        assert_eq!(diff.old_text, "left\n");
        assert_eq!(diff.new_text, "right\n");
        let other = temp.path().join("other");
        git(&[
            "worktree",
            "add",
            "--detach",
            "--",
            other.to_str().unwrap(),
            "HEAD",
        ]);
        review.root = other;
        assert!(review.diff(std::path::Path::new("file")).is_err());
    }
}
