//! Immutable native commit/file previews. File reads use retained trees, never
//! re-resolved mutable form fields or current working files.
use super::*;
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
    Push(bool),
    Integrate,
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
        window.close_dialog(cx);
        let weak = cx.entity().downgrade();
        let review = Arc::new(review);
        window.open_dialog(cx, move |dialog, _, cx| {
            let busy = weak.upgrade().is_none_or(|view| {
                let app = view.read(cx);
                app.git_is_busy() || app.current.as_ref() != Some(&review.root) || app.opening_path.is_some()
            });
            let mut body = div().id("git-immutable-review").v_flex().gap_2().max_h(ui_px(450.)).overflow_y_scroll()
                .child(scope(&review.root, None)).child(review.description.clone());
            if let ReviewSource::Operation(p) = &review.source {
                body = body.child(note(&p.baseline_description))
                    .child(note(&format!("기준: {}\n대상: {}", p.base_oid.as_deref().unwrap_or("로컬에 알려진 기준 없음"), p.head_oid)));
                if p.commits_truncated { body = body.child(note("커밋 목록은 처음 200개까지만 표시합니다. 파일 목록은 두 끝점의 차이입니다.")); }
            }
            if let ReviewSource::Comparison(p) = &review.source {
                body = body.child(note(&format!("직접 비교 (공통 조상 사용 안 함)\n이전: {}\n이후: {}\n작업 파일 변경은 포함하지 않습니다.", p.left_oid, p.right_oid)));
            }
            if let ReviewSource::Reflog { entry, detail } = &review.source {
                body = body.child(note(&format!("{} · {}\n{}\n부모 기준: {}", entry.selector, entry.timestamp, entry.oid, detail.parent.as_deref().unwrap_or("최초 커밋 · 빈 트리"))));
            }
            let mut commits = div().id("git-review-commits").max_h(ui_px(125.)).overflow_y_scroll().v_flex().gap_1();
            for commit in review.commits() {
                commits = commits.child(div().child(format!("{} · {} · {}", &commit.oid[..8], commit.author, commit.subject)));
            }
            body = body.child(format!("커밋 {}개", review.commits().len())).child(commits);
            let mut files = div().id("git-review-files").max_h(ui_px(190.)).overflow_y_scroll().v_flex().gap_1();
            for (index, file) in review.files().iter().enumerate() {
                let target = weak.clone(); let source = review.clone(); let path = file.path.clone();
                files = files.child(theme::action_button(("git-review-file", index), busy).label(format!("{} · {:?}", super::super::git_commit::display_path(&file.path), file.status)).small().on_click(move |_,w,cx| {
                    let source = source.clone(); let path = path.clone();
                    let _ = target.update(cx, |app,cx| app.read_git_review_file(source, path, w,cx));
                }));
            }
            body = body.child(format!("변경 파일 {}개 · 선택하여 코드 / diff 확인", review.files().len())).child(files);
            let mut footer = div().flex().flex_wrap().justify_end().gap_2().child(super::recovery::review_close(&weak));
            if let ReviewSource::Reflog { entry, .. } = &review.source {
                let target = weak.clone(); let entry = entry.clone();
                footer = footer.child(theme::action_button("reflog-create-recovery", busy).label("복구 브랜치 만들기…").on_click(move |_,w,cx| { let _ = target.update(cx, |app,cx| app.show_recovery_branch(entry.clone(),w,cx)); }));
            }
            if let Some(write) = &review.write {
                let target = weak.clone(); let review = review.clone(); let write = write.clone();
                footer = footer.child(theme::action_button("git-review-execute", busy).label(format!("{} 실행", review.title)).primary().on_click(move |_,w,cx| {
                    let ReviewSource::Operation(preview) = &review.source else { return; };
                    let preview = preview.clone(); let write = write.clone();
                    let _ = target.update(cx, |app,cx| {
                        w.close_dialog(cx);
                        app.run_git_job(review.root.clone(), review.title.clone(), move |ops| match write { ReviewWrite::Push(upstream) => ops.execute_push_preview(&preview, upstream), ReviewWrite::Integrate => ops.execute_integration_preview(&preview) }, w,cx);
                    });
                }));
            }
            dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title(format!("{} · 대상과 변경 확인", review.title)).width(ui_px(780.)).margin_top(ui_px(12.)).child(body).footer(footer)
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
        window.open_dialog(cx, move |dialog, _, _| {
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
                                .h(ui_px(350.)),
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
                                        app.show_git_review((*review).clone(), w, cx)
                                    });
                                }),
                        ),
                )
        });
    }
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
