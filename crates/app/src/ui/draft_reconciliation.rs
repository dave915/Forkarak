//! Explicit reconciliation changes only retained drafts, never working files.
use super::*;
use crate::ui::dialogs::DialogWindowExt as _;

#[derive(Clone)]
pub(super) struct DraftComparison {
    pub(super) root: PathBuf,
    pub(super) path: PathBuf,
    pub(super) original: String,
    pub(super) draft: String,
    disk: Result<String, String>,
}

impl DraftComparison {
    pub(super) fn read(root: PathBuf, path: PathBuf, original: String, draft: String) -> Self {
        let disk = read_disk(&root, &path);
        Self {
            root,
            path,
            original,
            draft,
            disk,
        }
    }

    pub(super) fn validate_disk(&self, fresh: &str) -> Result<(), String> {
        match &self.disk {
            Ok(disk) if disk == fresh => Ok(()),
            Ok(_) => Err("비교를 연 뒤 디스크가 다시 변경되었습니다. 결과는 유지했습니다. 디스크를 다시 읽고 확인하세요.".into()),
            Err(error) => Err(format!("디스크를 확인할 수 없어 초안에 반영하지 않았습니다: {error}")),
        }
    }
}

fn read_disk(root: &Path, path: &Path) -> Result<String, String> {
    pokarak_core::read_file(root, path).map_err(|error| format!("{error:#}"))
}

pub(super) fn open(
    owner: WeakEntity<Pokarak>,
    comparison: DraftComparison,
    window: &mut Window,
    cx: &mut Context<Pokarak>,
) {
    let panel = cx.new(|cx| Reconciliation::new(owner, comparison, window, cx));
    window.open_app_dialog(cx, move |dialog, window, _| {
        dialog
            .title("외부 수정과 초안 조정")
            .width(ui_px(980.).min(window.viewport_size().width - ui_px(24.)))
            .margin_top(ui_px(12.))
            .overlay_closable(false)
            .keyboard(false)
            .close_button(false)
            .on_ok(|_, _, _| false)
            .child(panel.clone())
    });
}

struct Reconciliation {
    owner: WeakEntity<Pokarak>,
    comparison: DraftComparison,
    original: Entity<EditorState>,
    draft: Entity<EditorState>,
    disk: Entity<EditorState>,
    result: Entity<EditorState>,
    busy: bool,
    error: Option<String>,
}

impl Reconciliation {
    fn new(
        owner: WeakEntity<Pokarak>,
        comparison: DraftComparison,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let lang = language(&comparison.path);
        let mut editor = |text: String, readonly: bool| {
            let editor = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(lang)
                    .line_number(true)
                    .soft_wrap(false)
                    .default_value(text)
            });
            editor.update(cx, |editor, cx| editor.set_readonly(readonly, cx));
            editor
        };
        Self {
            owner,
            original: editor(comparison.original.clone(), true),
            draft: editor(comparison.draft.clone(), true),
            disk: editor(
                comparison
                    .disk
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|error| format!("읽기 불가: {error}")),
                true,
            ),
            result: editor(comparison.draft.clone(), false),
            error: comparison.disk.as_ref().err().cloned(),
            comparison,
            busy: false,
        }
    }

    fn reload_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let root = self.comparison.root.clone();
        let path = self.comparison.path.clone();
        let task = cx.background_spawn(async move { read_disk(&root, &path) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |panel, window, cx| {
                panel.busy = false;
                panel.error = result.as_ref().err().cloned();
                let text = result
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|error| format!("읽기 불가: {error}"));
                panel.comparison.disk = result;
                panel
                    .disk
                    .update(cx, |editor, cx| editor.set_value(text, window, cx));
                // Preserve all user input in the editable result.
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.comparison.disk.is_err() {
            return;
        }
        let comparison = self.comparison.clone();
        let result = self.result.read(cx).value().to_string();
        let owner = self.owner.clone();
        self.busy = true;
        self.error = None;
        self.result
            .update(cx, |editor, cx| editor.set_readonly(true, cx));
        let target = comparison.clone();
        let task = cx.background_spawn(async move { read_disk(&target.root, &target.path) });
        cx.spawn_in(window, async move |view, cx| {
            let fresh_disk = task.await;
            let _ = view.update_in(cx, |panel, window, cx| {
                let applied = (|| -> Result<(), String> {
                    if panel.result.read(cx).value().as_ref() != result {
                        return Err(
                            "결과에 새 입력이 있습니다. 내용을 확인하고 다시 반영하세요.".into(),
                        );
                    }
                    let fresh = fresh_disk?;
                    comparison.validate_disk(&fresh)?;
                    owner
                        .update(cx, |app, cx| {
                            app.apply_draft_comparison(&comparison, result, &fresh, window, cx)
                        })
                        .map_err(|_| {
                            "워크트리 창이 닫혔습니다. 결과를 복사해 두세요.".to_string()
                        })?
                })();
                panel.busy = false;
                panel
                    .result
                    .update(cx, |editor, cx| editor.set_readonly(false, cx));
                match applied {
                    Ok(()) => window.close_dialog(cx),
                    Err(error) => panel.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for Reconciliation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = ((f32::from(window.viewport_size().height) - 260.) / 2.).clamp(72., 180.);
        let save_help = self.owner.upgrade().map(|owner| {
            let app = owner.read(cx);
            keycaps::explanation(
                "반영은 초안만 변경합니다. 파일 저장은 반영 후 ⌘S로 직접 실행하세요.",
                &app.prefs,
                app.current_project.as_deref(),
                Palette::new(cx),
            )
        });
        let pane = |label: &'static str, editor: &Entity<EditorState>, readonly: bool| {
            div()
                .v_flex()
                .min_w_0()
                .flex_1()
                .gap_1()
                .child(div().text_size(ui_text(11.)).child(label))
                .child(
                    Editor::new(editor)
                        .readonly(readonly)
                        .bordered(true)
                        .h(ui_px(height)),
                )
        };
        div()
            .id("draft-reconciliation-body")
            .v_flex()
            .gap_2()
            .max_h(window.viewport_size().height - ui_px(110.))
            .overflow_y_scroll()
            .child(div().text_size(ui_text(11.)).text_ellipsis().child(format!(
                "{} · {}",
                self.comparison.root.display(),
                self.comparison.path.display()
            )))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(pane("편집 전 원본", &self.original, true))
                    .child(pane("내 초안", &self.draft, true))
                    .child(pane("현재 디스크", &self.disk, true)),
            )
            .child(pane("조정 결과 · 직접 편집", &self.result, self.busy))
            .child(div().text_size(ui_text(11.)).children(save_help))
            .when_some(self.error.clone(), |body, error| {
                body.child(
                    div()
                        .text_size(ui_text(11.))
                        .text_color(Palette::new(cx).rgb(RED))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("copy-original-draft")
                            .label("초안 복사")
                            .ghost()
                            .on_click(cx.listener(|panel, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    panel.comparison.draft.clone(),
                                ));
                            })),
                    )
                    .child(
                        Button::new("copy-reconciled-result")
                            .label("결과 복사")
                            .ghost()
                            .on_click(cx.listener(|panel, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    panel.result.read(cx).value().to_string(),
                                ));
                            })),
                    )
                    .child(
                        Button::new("reload-comparison-disk")
                            .label("디스크 다시 읽기")
                            .ghost()
                            .disabled(self.busy)
                            .on_click(cx.listener(|panel, _, w, cx| panel.reload_disk(w, cx))),
                    )
                    .child(
                        Button::new("keep-original-draft")
                            .label("초안 유지")
                            .ghost()
                            .disabled(self.busy)
                            .on_click(|_, w, cx| w.close_dialog(cx)),
                    )
                    .child(
                        Button::new("apply-reconciled-draft")
                            .label(if self.busy {
                                "확인 중…"
                            } else {
                                "조정 결과를 초안에 반영"
                            })
                            .primary()
                            .disabled(self.busy || self.comparison.disk.is_err())
                            .on_click(cx.listener(|panel, _, w, cx| panel.apply(w, cx))),
                    ),
            )
    }
}
