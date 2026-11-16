//! Dedicated immutable history view. Its editors never enter working-tree
//! documents/navigation, and writes use the existing Git job lifecycle.
use super::*;
use pokarak_core::history::{self, Commit, CommitDetail, GraphRow, HistoryCursor, HistoryEntry};
use pokarak_core::operations::{
    GitOperations,
    history_actions::{HistoryAction, HistoryTarget},
};

#[derive(Default)]
pub(super) struct HistoryUi {
    active: bool,
    root: Option<PathBuf>,
    request: u64,
    loading: bool,
    entries: Vec<HistoryEntry>,
    cursor: Option<HistoryCursor>,
    limit_reached: bool,
    shallow: bool,
    detail: Option<CommitDetail>,
    selected: Option<PathBuf>,
    document: Option<DiffDocument>,
    editor: Option<Entity<EditorState>>,
    code: bool,
    scroll: UniformListScrollHandle,
    error: Option<String>,
}
impl Pokarak {
    fn history_read<T: Send + 'static>(
        &mut self,
        job: impl FnOnce(&Path) -> anyhow::Result<T> + Send + 'static,
        ready: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.git_history.root.clone() else {
            return;
        };
        if self.git_is_busy() || self.current.as_ref() != Some(&root) || self.loading {
            return;
        }
        self.git_history.request += 1;
        let token = self.git_history.request;
        let navigation_generation = self.request;
        self.git_history.loading = true;
        self.git_history.error = None;
        let worktree = root.clone();
        let task = cx.background_spawn(async move { job(&worktree) });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |app, window, cx| {
                if app.git_history.request != token {
                    return;
                }
                app.git_history.loading = false;
                if !app.git_history.active
                    || !window.has_active_dialog(cx)
                    || app.current.as_ref() != Some(&root)
                    || app.request != navigation_generation
                    || app.git_is_busy()
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(data) => ready(app, data, window, cx),
                    Err(error) => app.git_history.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn open_git_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_is_busy() || self.loading {
            return;
        }
        let Some(root) = self.current.clone() else {
            return;
        };
        self.navigation_request += 1;
        self.navigating = false;
        self.git_history.request += 1;
        self.git_history.root = Some(root);
        self.git_history.entries.clear();
        self.git_history.cursor = None;
        self.git_history.detail = None;
        self.git_history.selected = None;
        self.git_history.document = None;
        self.git_history.editor = None;
        self.git_history.scroll = UniformListScrollHandle::new();
        self.git_history.error = None;
        self.git_history.limit_reached = false;
        self.git_history.shallow = false;
        self.show_history_list(window, cx);
        self.load_history_page(true, window, cx);
    }
    fn load_history_page(&mut self, first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.git_history.cursor.clone();
        self.history_read(
            move |root| {
                if first {
                    history::first_page(root, 100)
                } else {
                    history::next_page(
                        root,
                        cursor
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("No further page"))?,
                        100,
                    )
                }
            },
            |app, page, _, _| {
                app.git_history.entries.extend(page.entries);
                app.git_history.cursor = page.next;
                app.git_history.limit_reached = page.limit_reached;
                app.git_history.shallow = page.shallow;
            },
            window,
            cx,
        );
    }
    fn show_history_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_all_dialogs(cx);
        self.git_history.active = true;
        let weak = cx.entity().downgrade();
        let closing = weak.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let Some(view) = weak.upgrade() else { return dialog; };
            let app = view.read(cx);
            let ui = &app.git_history;
            let palette = Palette::new(cx);
            let busy = history_disabled(app);
            let graph_width = (ui.entries.iter().map(|e| e.graph.columns).max().unwrap_or(1) * 14 + 12) as f32;
            let entries = ui.entries.clone();
            let target = weak.clone();
            let list = uniform_list("history-commits", entries.len(), move |range, _, _| {
                range.map(|index| {
                    let entry = &entries[index];
                    let commit = entry.commit.clone();
                    let oid = commit.oid.clone();
                    let row_target = target.clone();
                    let label = format!("{} {} {} {} {}", short(&commit.oid), commit.subject, commit.author, commit.date, commit.refs.join(", "));
                    let refs = commit.refs.iter().map(|r| r.strip_prefix("refs/").unwrap_or(r)).collect::<Vec<_>>().join(", ");
                    div().h(px(48.)).w_full().flex().items_center()
                        .child(graph_view(entry.graph.clone(), graph_width, palette))
                        .child(Button::new(("history-row", index)).ghost().small().w_full().h(px(48.)).min_w_0().justify_start()
                            .tooltip(label.clone()).accessibility_label(label).disabled(busy)
                            .child(div().v_flex().w_full().min_w_0().items_start()
                                .child(div().w_full().text_ellipsis().child(format!("{}  {}", short(&commit.oid), visible(&commit.subject))))
                                .child(div().w_full().text_size(px(11.)).text_color(palette.rgb(MUTED)).text_ellipsis().child(format!("{}  {}  {}", visible(&commit.author), commit.date, refs))))
                            .on_click(move |_, w, cx| { let _ = row_target.update(cx, |app, cx| app.load_history_detail(oid.clone(), 0, w, cx)); }))
                }).collect::<Vec<_>>()
            }).track_scroll(&ui.scroll).h(px(264.)).w_full();
            let status = if ui.loading { "커밋 읽는 중…".into() } else if ui.limit_reached { format!("{}개 표시 · 안전 한도 {}개 도달", ui.entries.len(), history::HISTORY_LIMIT) } else { format!("{}개 표시 · {}", ui.entries.len(), if ui.cursor.is_some() { "다음 페이지 있음" } else { "끝" }) };
            dialog.title("커밋 그래프 / 기록").width(px(860.)).margin_top(px(12.)).overlay_closable(false).on_ok(|_,_,_| false)
                .on_close({ let closing = closing.clone(); move |_, _, cx| { let _ = closing.update(cx, |app, cx| { app.git_history.active = false; app.git_history.request += 1; app.git_history.loading = false; cx.notify(); }); } })
                .child(div().v_flex().gap_2().min_w_0()
                    .child(history_scope(ui))
                    .child(div().text_size(px(11.)).text_color(palette.rgb(MUTED)).child("브랜치·원격 추적·태그의 스냅샷 · 커밋 선택으로 파일과 부모 대비 diff 보기"))
                    .child(div().id("history-graph-scroll").overflow_x_scroll().child(div().min_w(px(graph_width + 600.)).child(list)))
                    .child(div().text_size(px(12.)).child(status))
                    .when(ui.shallow, |d| d.child(div().text_size(px(11.)).child("Shallow 저장소: 로컬에 있는 기록만 표시합니다. 필요 시 명시적으로 fetch 하세요.")))
                    .when_some(ui.error.clone(), |d,e| d.child(history_error(e))))
                .footer(div().flex().gap_2().justify_end()
                    .child(history_button(&weak, "history-reload", "기록 새로고침", busy, |a,w,cx| a.open_git_history(w,cx)))
                    .child(history_button(&weak, "history-more", "다음 100개", busy || ui.cursor.is_none(), |a,w,cx| a.load_history_page(false,w,cx)))
                    .child(history_close(&weak)))
        });
    }
    fn load_history_detail(
        &mut self,
        oid: String,
        parent: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.history_read(
            move |root| history::commit_detail(root, &oid, parent),
            |app, detail, w, cx| {
                app.git_history.detail = Some(detail);
                app.git_history.selected = None;
                app.git_history.document = None;
                app.git_history.editor = None;
                app.git_history.code = false;
                app.show_history_detail(w, cx);
                if let Some(file) = app
                    .git_history
                    .detail
                    .as_ref()
                    .and_then(|d| d.files.first())
                    .cloned()
                {
                    app.load_history_file(file.path, w, cx);
                }
            },
            window,
            cx,
        );
    }
    fn load_history_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.git_history.detail.clone() else {
            return;
        };
        self.git_history.selected = Some(path.clone());
        self.git_history.document = None;
        self.git_history.editor = None;
        self.history_read(
            move |root| history::historical_diff(root, &detail, &path),
            |app, doc, w, cx| {
                app.git_history.document = Some(doc);
                app.update_history_editor(w, cx);
            },
            window,
            cx,
        );
    }
    fn update_history_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = &self.git_history.document else {
            return;
        };
        let text = if self.git_history.code && !doc.binary {
            doc.new_text.clone()
        } else {
            doc.patch.clone()
        };
        let lang = if self.git_history.code {
            language(&doc.path)
        } else {
            "diff"
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(lang)
                .line_number(true)
                .soft_wrap(false)
                .default_value(text)
        });
        editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
        self.git_history.editor = Some(editor);
        cx.notify();
    }
    fn show_history_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_all_dialogs(cx);
        self.git_history.active = true;
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let Some(view) = weak.upgrade() else { return dialog; };
            let app = view.read(cx); let ui = &app.git_history;
            let Some(detail) = &ui.detail else { return dialog; };
            let busy = history_disabled(app); let palette = Palette::new(cx);
            let mut parents = div().flex().gap_2().items_center();
            if detail.commit.parents.is_empty() { parents = parents.child("Root commit → 빈 트리 대비"); }
            for (index, oid) in detail.commit.parents.iter().enumerate() {
                let commit = detail.commit.oid.clone();
                parents = parents.child(history_button(&weak, ("history-parent",index), format!("부모 {} {}",index+1,short(oid)), busy,
                    move |a,w,cx| a.load_history_detail(commit.clone(),index,w,cx)).selected(detail.parent.as_ref() == Some(oid)));
            }
            let changed_files = detail.files.clone();
            let selected_file = ui.selected.clone();
            let files_target = weak.clone();
            let files = uniform_list("history-files", changed_files.len(), move |range, _, _| {
                range.map(|index| {
                    let file = &changed_files[index];
                    let path = file.path.clone();
                    let label = format!("{} {}", match file.status { FileStatus::Added => "A", FileStatus::Deleted => "D", _ => "M" }, visible(&path.to_string_lossy()));
                    div().h(px(28.)).w_full().child(history_button(&files_target, ("history-file",index), label.clone(), busy,
                        move |a,w,cx| a.load_history_file(path.clone(),w,cx)).ghost().w_full().justify_start().tooltip(label).selected(selected_file.as_ref() == Some(&file.path)))
                }).collect::<Vec<_>>()
            }).w(px(205.)).flex_none().h(px(232.));
            let preview = if let Some(editor) = &ui.editor {
                Editor::new(editor).readonly(true).bordered(true).h(px(232.)).w_full().font_family("JetBrains Mono").text_size(px(12.)).into_any_element()
            } else { div().h(px(232.)).p_3().child(if ui.loading { "파일 읽는 중…" } else if detail.files.is_empty() { "이 부모와 비교한 파일 변경이 없습니다." } else { "파일을 선택하세요." }).into_any_element() };
            let truncated = ui.document.as_ref().is_some_and(|d| d.truncated);
            let binary = ui.document.as_ref().is_some_and(|d| d.binary);
            let merge = detail.commit.parents.len() > 1;
            dialog.title(div().text_ellipsis().child(format!("{}  {}",short(&detail.commit.oid),visible(&detail.commit.subject)))).width(px(860.)).margin_top(px(12.)).overlay_closable(false).on_ok(|_,_,_| false)
                .on_close({ let weak = weak.clone(); move |_,_,cx| { let _=weak.update(cx,|a,cx| {a.git_history.active=false;a.git_history.request+=1;a.git_history.loading=false;cx.notify();}); } })
                .child(div().v_flex().gap_2().min_w_0()
                    .child(history_scope(ui))
                    .child(div().text_size(px(11.)).text_ellipsis().child(format!("{}  {}  {}",visible(&detail.commit.author),detail.commit.date,detail.commit.oid)))
                    .child(div().id("history-parent-scroll").overflow_x_scroll().text_size(px(12.)).child(parents))
                    .child(div().flex().gap_2().items_center()
                        .child(history_button(&weak,"history-diff","Diff",busy,|a,w,cx|{a.git_history.code=false;a.update_history_editor(w,cx)}).selected(!ui.code))
                        .child(history_button(&weak,"history-code","커밋의 파일",busy||binary,|a,w,cx|{a.git_history.code=true;a.update_history_editor(w,cx)}).selected(ui.code))
                        .child(div().min_w_0().text_size(px(11.)).text_ellipsis().child(ui.selected.as_ref().map(|p|visible(&p.to_string_lossy())).unwrap_or_default())))
                    .child(div().flex().gap_2().min_w_0().child(files).child(div().flex_1().min_w_0().child(preview)))
                    .child(div().text_size(px(11.)).text_color(palette.rgb(MUTED)).child(if truncated { "미리보기 제한: 2 MiB / 50,000줄. 역사 파일은 읽기 전용이며 현재 코드 탐색을 사용하지 않습니다." } else if merge { "Merge 커밋: 부모를 선택해 비교합니다. mainline 지정이 필요한 cherry-pick/revert는 외부 Git을 사용하세요." } else { "읽기 전용 역사 파일 · 현재 코드의 정의/사용처 탐색 없음 · 이름 변경은 삭제/추가로 표시" }))
                    .when_some(ui.error.clone(), |d,e|d.child(history_error(e))))
                .footer(div().flex().gap_2().justify_end()
                    .child(history_button(&weak,"history-back","그래프로",busy,|a,w,cx|a.show_history_list(w,cx)))
                    .child(history_button(&weak,"history-pick","Cherry-pick…",busy||merge,|a,w,cx|a.prepare_history_action(HistoryAction::CherryPick,w,cx)))
                    .child(history_button(&weak,"history-revert","Revert…",busy||merge,|a,w,cx|a.prepare_history_action(HistoryAction::Revert,w,cx)))
                    .child(history_close(&weak)))
        });
    }
    fn prepare_history_action(
        &mut self,
        action: HistoryAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.git_history.detail.as_ref().map(|d| d.commit.clone()) else {
            return;
        };
        self.history_read(
            |root| GitOperations::open(root)?.history_target(),
            move |app, target, w, cx| app.confirm_history_action(action, commit, target, w, cx),
            window,
            cx,
        );
    }
    fn confirm_history_action(
        &mut self,
        action: HistoryAction,
        commit: Commit,
        target: HistoryTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_all_dialogs(cx);
        self.git_history.active = true;
        let weak = cx.entity().downgrade();
        window.open_dialog(cx,move |dialog,_,cx| {
            let busy=weak.upgrade().is_none_or(|v|history_disabled(v.read(cx)));
            let destination=target.clone(); let source=commit.oid.clone();
            dialog.title(format!("{} 확인",action.label())).width(px(650.)).margin_top(px(20.)).overlay_closable(false).on_ok(|_,_,_|false)
                .on_close({let weak=weak.clone();move |_,_,cx|{let _=weak.update(cx,|a,cx|{a.git_history.active=false;a.git_history.request+=1;a.git_history.loading=false;cx.notify();});}})
                .child(div().v_flex().gap_3().text_size(px(13.))
                    .child(format!("원본 커밋: {}\n{}",commit.oid,visible(&commit.subject)))
                    .child(format!("대상 브랜치: {}\n대상 HEAD: {}",target.branch,target.head))
                    .child(format!("대상 워크트리: {}",target.root.display()))
                    .child(if action==HistoryAction::CherryPick {"이 커밋 하나의 변경을 대상 브랜치에 새 커밋으로 적용합니다."}else{"이 커밋 하나의 변경을 되돌리는 새 커밋을 대상 브랜치에 만듭니다."})
                    .child("작업 트리와 index가 깨끗해야 합니다. 충돌 발생 시 상태를 보존하며, Stash / 충돌 해결에서 계속 또는 중단할 수 있습니다. 저장소에 설정된 Git hook이 실행될 수 있습니다."))
                .footer(div().flex().gap_2().justify_end()
                    .child(history_button(&weak,"history-confirm-back","취소",busy,|a,w,cx|a.show_history_detail(w,cx)))
                    .child(history_button(&weak,"history-confirm",format!("{} 실행",action.label()),busy,move |a,w,cx| {
                        a.git_history.request+=1;
                        a.git_history.active=false;
                        w.close_all_dialogs(cx);
                        let source=source.clone(); let target=destination.clone();
                        a.run_git_job(target.root.clone(),action.label().into(),move |ops|ops.history_action(action,&source,&target),w,cx);
                    }).primary()))
        });
    }
}
fn history_disabled(app: &Pokarak) -> bool {
    app.git_is_busy()
        || app.loading
        || app.git_history.loading
        || app.current != app.git_history.root
}
fn history_button(
    weak: &WeakEntity<Pokarak>,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
    action: impl Fn(&mut Pokarak, &mut Window, &mut Context<Pokarak>) + 'static,
) -> Button {
    let weak = weak.clone();
    Button::new(id)
        .label(label)
        .small()
        .disabled(disabled)
        .on_click(move |_, w, cx| {
            let _ = weak.update(cx, |a, cx| action(a, w, cx));
        })
}
fn history_close(weak: &WeakEntity<Pokarak>) -> Button {
    history_button(weak, "history-close", "닫기", false, |app, w, cx| {
        app.git_history.active = false;
        app.git_history.request += 1;
        app.git_history.loading = false;
        w.close_dialog(cx);
        cx.notify();
    })
}
fn history_scope(ui: &HistoryUi) -> Div {
    div().text_size(px(12.)).text_ellipsis().child(
        ui.root
            .as_ref()
            .map(|p| format!("워크트리: {}", p.display()))
            .unwrap_or_default(),
    )
}
fn history_error(error: String) -> impl IntoElement {
    div()
        .id("history-error")
        .max_h(px(54.))
        .overflow_y_scroll()
        .text_size(px(12.))
        .child(error)
}
fn short(oid: &str) -> &str {
    oid.get(..12).unwrap_or(oid)
}
fn visible(text: &str) -> String {
    text.replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
fn graph_view(row: GraphRow, width: f32, palette: Palette) -> impl IntoElement {
    canvas(
        move |_, _, _| row,
        move |bounds, row, window, _| {
            let x = |lane: usize| bounds.left() + px(10. + lane as f32 * 14.);
            let top = bounds.top();
            let middle = top + px(24.);
            let bottom = bounds.bottom();
            let mut path = PathBuilder::stroke(px(1.5));
            for lane in 0..row.incoming {
                path.move_to(point(x(lane), top));
                path.line_to(point(x(lane), middle));
            }
            for (from, to) in row.edges {
                path.move_to(point(x(from), middle));
                path.line_to(point(x(to), bottom));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, palette.rgb(BLUE));
            }
            window.paint_quad(fill(
                Bounds::new(
                    point(x(row.lane) - px(3.), middle - px(3.)),
                    size(px(6.), px(6.)),
                ),
                palette.rgb(TEXT),
            ));
        },
    )
    .w(px(width))
    .h(px(48.))
    .flex_none()
}
