//! Expected-content ignore editing; matching previews run in core's isolated
//! shadow repository. Merely opening/editing this view writes no source files.
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;
use pokarak_core::operations::ignore::{IgnorePreview, apply_gitignore, preview_gitignore};

fn exact_rule(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if text
        .chars()
        .any(|ch| ch == '\n' || ch == '\r' || ch == '\0')
    {
        return None;
    }
    let mut rule = String::from("/");
    for ch in text.chars() {
        if matches!(ch, '\\' | '*' | '?' | '[' | ']' | ' ' | '#' | '!') {
            rule.push('\\');
        }
        rule.push(ch);
    }
    Some(rule)
}
fn read_rules(root: &Path) -> anyhow::Result<Option<String>> {
    let path = root.join(".gitignore");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file()
                    && !metadata.is_symlink()
                    && metadata.len() <= pokarak_core::MAX_TEXT_BYTES as u64,
                "Choose a regular .gitignore file up to 2 MiB"
            );
            Ok(Some(std::fs::read_to_string(path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
impl Pokarak {
    pub(super) fn open_ignore_picker(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let read_root = root.clone();
        self.workflow_read(root.clone(),move |_| {
            let snapshot = pokarak_core::snapshot_current_inventory(&read_root,&CompareMode::Uncommitted)?;
            let files: Vec<_> = snapshot.changes.into_iter().filter(|f| f.status == FileStatus::Untracked).collect();
            anyhow::Ok((files,read_rules(&read_root)?))
        },move |_,(files,expected),w,cx| {
            let weak = cx.entity().downgrade();
            w.close_dialog(cx);
            w.open_app_dialog(cx,move |dialog,_,_| {
                let mut body = div().id("ignore-untracked-list").v_flex().gap_2().max_h(ui_px(340.)).overflow_y_scroll().child(scope(&root,None)).child(note("미추적 파일을 선택하여 저장소 루트 .gitignore에 규칙을 작성하세요. 추적 중인 파일은 이 작업으로 추적 해제되지 않습니다."));
                if files.is_empty() { body = body.child(note("미추적 파일이 없습니다.")); }
                for (index,file) in files.iter().enumerate() {
                    let target = weak.clone(); let root = root.clone(); let path = file.path.clone(); let expected = expected.clone();
                    body = body.child(Button::new(("ignore-untracked-file",index)).label(super::super::git_commit::display_path(&path)).small().on_click(move |_,w,cx| {
                        let _ = target.update(cx, |app,cx| app.show_ignore_editor(root.clone(),path.clone(),expected.clone(),w,cx));
                    }));
                }
                dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title("제외할 미추적 파일 선택").width(ui_px(740.)).child(body).footer(close_row())
            });
        },window,cx);
    }
    fn show_ignore_editor(
        &mut self,
        root: PathBuf,
        selected: PathBuf,
        expected: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut replacement = expected.clone().unwrap_or_default();
        if let Some(rule) = exact_rule(&selected) {
            if !replacement.is_empty() && !replacement.ends_with('\n') {
                replacement.push('\n');
            }
            replacement.push_str(&rule);
            replacement.push('\n');
        }
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .default_value(replacement)
        });
        let weak = cx.entity().downgrade();
        window.close_dialog(cx);
        window.open_app_dialog(cx,move |dialog,_,_| {
            let target = weak.clone(); let root = root.clone(); let expected = expected.clone(); let editor_for_action = editor.clone();
            dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title(".gitignore 편집").width(ui_px(780.)).margin_top(ui_px(12.)).child(div().v_flex().gap_2().child(scope(&root,None)).child(format!("선택한 미추적 경로: {}",super::super::git_commit::display_path(&selected)))
                .child(note("루트 .gitignore의 전체 내용을 편집합니다. 폴더 전체를 제외하려면 /폴더/ 규칙을 입력하세요. 다음 화면에서 실제 영향 경로를 확인한 뒤 적용합니다."))
                .child(Editor::new(&editor).aria_label(".gitignore 전체 규칙").h(ui_px(280.))))
                .footer(div().flex().justify_end().gap_2().child(super::recovery::review_close(&weak)).child(Button::new("ignore-preview").label("적용 범위 미리보기…").primary().on_click(move |_,w,cx| {
                    let replacement = editor_for_action.read(cx).value().to_string(); let read_root = root.clone(); let expected = expected.clone();
                    let _ = target.update(cx, |app,cx| app.workflow_read(root.clone(),move |_| preview_gitignore(&read_root,Path::new(".gitignore"),expected.as_deref(),&replacement),move |app,preview,w,cx| app.show_ignore_preview(preview,w,cx),w,cx));
                })))
        });
    }
    fn show_ignore_preview(
        &mut self,
        preview: IgnorePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .default_value(preview.replacement.clone())
        });
        editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
        window.close_dialog(cx);
        window.open_app_dialog(cx,move |dialog,_,cx| {
            let target = weak.clone(); let preview_for_action = preview.clone();
            let disabled = weak.upgrade().is_none_or(|view| { let app = view.read(cx); app.git_is_busy() || app.current.as_ref() != Some(&preview.root) });
            let mut paths = div().id("ignore-affected-paths").v_flex().gap_1().max_h(ui_px(150.)).overflow_y_scroll();
            for (label,files) in [("새로 제외",&preview.newly_ignored),("제외 해제",&preview.newly_unignored)] {
                paths = paths.child(format!("{label}: {}개",files.len()));
                for path in files { paths = paths.child(super::super::git_commit::display_path(path)); }
            }
            dialog.on_close({ let owner = weak.clone(); move |_,_,cx| { let _ = owner.update(cx, |app,_| app.invalidate_workflow_read()); } }).title(".gitignore 적용 범위 확인").width(ui_px(800.)).margin_top(ui_px(12.)).child(div().v_flex().gap_2().child(scope(&preview.root,None)).child(note("추적 중인 파일은 그대로 추적합니다. 확인한 규칙 파일 또는 미추적 경로 집합이 바뀌면 적용을 중단합니다."))
                .child(Editor::new(&editor).aria_label("적용할 .gitignore 내용").h(ui_px(180.))).child(paths))
                .footer(div().flex().justify_end().gap_2().child(super::recovery::review_close(&weak)).child(theme::action_button("ignore-apply",disabled).label("이 내용으로 .gitignore 저장").primary().on_click(move |_,w,cx| {
                    let preview = preview_for_action.clone(); let _ = target.update(cx, |app,cx| { w.close_dialog(cx); app.run_git_job(preview.root.clone(),".gitignore 저장".into(),move |_| {
                        apply_gitignore(&preview)?;
                        Ok(OperationOutput { root: preview.root, stdout: ".gitignore 저장 완료".into(), stderr: String::new(), truncated: false })
                    },w,cx); });
                })))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::exact_rule;
    use std::path::Path;
    #[test]
    fn untracked_names_become_literal_rules_and_newlines_require_manual_rules() {
        assert_eq!(
            exact_rule(Path::new("a*[?]/#new file")),
            Some("/a\\*\\[\\?\\]/\\#new\\ file".into())
        );
        assert!(exact_rule(Path::new("a\nb")).is_none());
    }
}
