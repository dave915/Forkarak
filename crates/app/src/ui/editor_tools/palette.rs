use super::*;
use crate::ui::dialogs::DialogWindowExt as _;
#[derive(Clone)]
enum Target {
    Action(fn() -> Box<dyn Action>),
    Worktree(PathBuf),
}
#[derive(Clone)]
struct Entry {
    shortcut: Option<&'static keyboard::Shortcut>,
    title: String,
    keywords: String,
    key: String,
    target: Target,
    enabled: bool,
}
fn entries(app: &Pokarak, window: &Window, cx: &App) -> Vec<Entry> {
    let code = app.code_tools_target(cx).is_some();
    let ready =
        app.current.is_some() && app.opening_path.is_none() && !app.loading && !app.git_is_busy();
    let editable = code
        && app
            .current
            .as_ref()
            .zip(app.selected().as_ref())
            .is_some_and(|(root, path)| app.code_edits.can_edit(root, path));
    let mut list = Vec::new();
    let keyboard = keyboard::Keyboard::new(&app.prefs, app.current_project.as_deref());
    let managed = |s: &'static keyboard::Shortcut, title: &str, enabled: bool| Entry {
        shortcut: Some(s),
        title: title.into(),
        keywords: s.id.replace('_', " "),
        key: keyboard.key(s).into(),
        target: Target::Action(s.action),
        enabled,
    };
    macro_rules! command {
        ($title:literal, $id:literal, $enabled:expr) => {
            list.push(managed(
                keyboard::shortcut($id).expect("registered palette command"),
                $title,
                $enabled,
            ))
        };
        ($title:literal, $id:literal, $key:literal, $action:path, $enabled:expr) => {
            list.push(Entry {
                shortcut: None,
                title: $title.into(),
                keywords: $id.replace('_', " "),
                key: $key.into(),
                target: Target::Action(|| Box::new($action)),
                enabled: $enabled,
            })
        };
    }
    command!(
        "파일: 저장소 열기…",
        "open_repository",
        "cmd-o",
        OpenRepository,
        true
    );
    command!(
        "파일: 이름 변경 · 이동 · 파일/폴더 생성…",
        "manage_files",
        "",
        ManageFiles,
        ready
    );
    command!("파일: 모두 저장", "save_all", ready);
    command!("코드 지원: 이름 바꾸기…", "rename_symbol", editable);
    command!("코드 지원: 빠른 수정…", "quick_fix", editable);
    command!("파일: 현재 파일 저장", "save_file", ready);
    command!(
        "파일: 현재 파일 닫기",
        "close_file",
        "cmd-w",
        CloseFile,
        ready
    );
    command!("편집: 주석 토글", "toggle_comment", editable);
    command!("편집: 줄 위로 이동", "move_line_up", editable);
    command!("편집: 줄 아래로 이동", "move_line_down", editable);
    command!("편집: 줄 위에 복제", "duplicate_line_up", editable);
    command!("편집: 줄 아래에 복제", "duplicate_line_down", editable);
    command!(
        "편집: 같은 단어 다음 항목 추가 선택",
        "select_next_word",
        code
    );
    command!("편집: 위 줄에 커서 추가", "add_cursor_above", ready);
    command!("편집: 아래 줄에 커서 추가", "add_cursor_below", ready);
    command!("편집: 파일 내 바꾸기", "replace_file", editable);
    command!("편집: 파일 포맷", "format_file", editable);
    command!("이동: 줄:열 번호로 이동…", "go_to_line", code);
    command!("이동: 파일 찾기…", "find_file", ready);
    command!("이동: 프로젝트 본문 검색…", "find_text", ready);
    command!("이동: 정의로 이동", "find_definition", ready);
    command!("이동: 사용처 찾기", "find_references", ready);
    command!(
        "코드 지원: 자동완성",
        "language_completion",
        "ctrl-space",
        language_tools::Complete,
        code
    );
    command!(
        "코드 지원: 오류 확인",
        "language_diagnostics",
        "",
        language_tools::Diagnostics,
        code
    );
    command!("코드 지원: 파일 구조 검색", "language_symbols", code);
    command!("코드 지원: 함수 도움말", "language_signature", code);
    command!(
        "코드 지원: 타입 · 설명",
        "language_hover",
        "alt-f1",
        language_tools::Hover,
        code
    );
    command!("이동: 구현으로 이동", "find_implementation", code);
    command!("이동: 뒤로", "back", "cmd-[", Back, ready);
    command!("이동: 앞으로", "forward", "cmd-]", Forward, ready);
    command!("보기: 코드 / 변경사항 전환", "toggle_code", ready);
    command!("보기: 긴 줄 화면 줄바꿈 전환", "word_wrap", code);
    command!("보기: 미리보기 탭 고정", "pin_tab", code);
    command!(
        "보기: 현재 변경 구간의 이전 코드 펼치기",
        "previous_code",
        "",
        PreviousCode,
        code
    );
    command!(
        "보기: 다음 변경 구간",
        "next_diff_hunk",
        "alt-down",
        diff_tools::NextDiffHunk,
        ready
    );
    command!(
        "보기: 이전 변경 구간",
        "previous_diff_hunk",
        "alt-up",
        diff_tools::PreviousDiffHunk,
        ready
    );
    command!("Git: Stage / 커밋…", "open_git_commit", ready);
    command!(
        "Git: 커밋 그래프",
        "git_history",
        "cmd-alt-h",
        OpenGitHistory,
        ready
    );
    command!("Git: 커밋 검색…", "git_search", "", SearchGitGraph, ready);
    command!(
        "Git: Stash / 충돌 해결…",
        "git_workflow",
        "cmd-shift-g",
        OpenGitWorkflow,
        ready
    );
    command!(
        "Git: 브랜치 · 원격 · Git 작업…",
        "git_actions",
        "",
        OpenGitActions,
        ready
    );
    command!("워크트리: 다음 워크트리", "next_worktree", ready);
    command!("워크트리: 이전 워크트리", "previous_worktree", ready);
    command!(
        "프로젝트: 다음 프로젝트",
        "next_project",
        "ctrl-tab",
        NextProject,
        ready
    );
    command!(
        "프로젝트: 이전 프로젝트",
        "previous_project",
        "ctrl-shift-tab",
        PreviousProject,
        ready
    );
    command!("보기: 사이드바 표시 / 숨기기", "toggle_sidebar", ready);
    command!("보기: 파일 목록 표시 / 숨기기", "toggle_files", ready);
    #[cfg(target_os = "macos")]
    command!(
        "터미널: 현재 워크트리 터미널 표시 / 숨기기",
        "terminal",
        "ctrl-`",
        OpenWorktreeTerminal,
        ready
    );
    command!("새로고침: 현재 워크트리", "refresh", ready);
    command!("설정: 환경설정…", "settings", "cmd-,", OpenSettings, true);
    // New feature actions appear when their owning view installs a handler.
    for s in keyboard::KEYBOARD_DEFAULTS {
        if s.id != "command_palette"
            && !list
                .iter()
                .any(|e| e.shortcut.is_some_and(|entry| entry.id == s.id))
            && window.is_action_available((s.action)().as_ref(), cx)
        {
            list.push(managed(s, s.label, true));
        }
    }
    for tree in app.inventory.iter().filter(|t| !t.is_prunable) {
        list.push(Entry {
            shortcut: None,
            title: format!(
                "워크트리 열기: {} · {}",
                tree.branch.as_str(),
                tree.path.display()
            ),
            keywords: "worktree switch 워크트리 전환".into(),
            key: String::new(),
            target: Target::Worktree(tree.path.clone()),
            enabled: ready,
        });
    }
    list
}
struct Palette {
    owner: WeakEntity<Pokarak>,
    input: Entity<InputState>,
    entries: Vec<Entry>,
    query: String,
    cursor: usize,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}
impl Palette {
    fn filtered(&self) -> Vec<usize> {
        let terms: Vec<_> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let text = format!("{} {} {}", e.title, e.key, e.keywords).to_lowercase();
                terms.iter().all(|q| text.contains(q))
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn run(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(index).cloned() else {
            return;
        };
        if !entry.enabled {
            return;
        }
        window.close_dialog(cx);
        let _ = self.owner.update(cx, |app, cx| {
            if entry.shortcut.is_some_and(|s| s.id == "open_to_side") {
                app.search_input.focus_handle(cx).focus(window, cx);
            } else if let Some(editor) = app.active_editor() {
                editor.read(cx).focus_handle(cx).focus(window, cx);
            } else {
                app.focus.focus(window, cx);
            }
            match entry.target {
                Target::Action(action) => window.dispatch_action(action(), cx),
                Target::Worktree(root) => app.open_repository(root, window, cx),
            }
        });
    }
}
impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(owner) = self.owner.upgrade() {
            let app = owner.read(cx);
            let keyboard = keyboard::Keyboard::new(&app.prefs, app.current_project.as_deref());
            for entry in &mut self.entries {
                if let Some(shortcut) = entry.shortcut {
                    entry.key = keyboard.key(shortcut).into();
                }
            }
        }
        let indices = self.filtered();
        let list = indices.clone();
        let count = indices.len();
        let entity = cx.entity().clone();
        div()
            .v_flex()
            .gap_2()
            .capture_key_down(cx.listener(move |s, event: &KeyDownEvent, w, cx| {
                match event.keystroke.key.as_str() {
                    "down" => s.cursor = (s.cursor + 1).min(count.saturating_sub(1)),
                    "up" => s.cursor = s.cursor.saturating_sub(1),
                    "enter" => {
                        if let Some(index) = list.get(s.cursor) {
                            s.run(*index, w, cx);
                        }
                    }
                    "escape" => w.close_dialog(cx),
                    _ => return,
                }
                s.scroll.scroll_to_item(s.cursor, ScrollStrategy::Nearest);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(Input::new(&self.input))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(ui_text(11.))
                    .child(format!("{count}개 명령 ·"))
                    .child(keycaps::hint(
                        "선택",
                        "up down",
                        crate::theme::Palette::new(cx),
                    ))
                    .child(keycaps::hint(
                        "실행",
                        "enter",
                        crate::theme::Palette::new(cx),
                    ))
                    .child(keycaps::hint(
                        "닫기",
                        "escape",
                        crate::theme::Palette::new(cx),
                    )),
            )
            .when(count == 0, |body| body.child("일치하는 명령이 없습니다."))
            .child(
                uniform_list("command-palette-list", count, move |range, _, cx| {
                    entity.update(cx, |s, cx| {
                        let filtered = s.filtered();
                        range
                            .filter_map(|row| {
                                let index = *filtered.get(row)?;
                                let entry = &s.entries[index];
                                let label = format!("{}   {}", entry.title, entry.key);
                                Some(
                                    Button::new(("palette-command", row))
                                        .ghost()
                                        .w_full()
                                        .selected(row == s.cursor)
                                        .disabled(!entry.enabled)
                                        .accessibility_label(label.clone())
                                        .child(
                                            dialogs::choice_body()
                                                .flex()
                                                .items_center()
                                                .gap_3()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .text_ellipsis()
                                                        .child(entry.title.clone()),
                                                )
                                                .child(keycaps::binding(
                                                    &entry.key,
                                                    crate::theme::Palette::new(cx),
                                                )),
                                        )
                                        .on_click(
                                            cx.listener(move |s, _, w, cx| s.run(index, w, cx)),
                                        )
                                        .into_any_element(),
                                )
                            })
                            .collect()
                    })
                })
                .h(ui_px(360.))
                .track_scroll(&self.scroll),
            )
    }
}
struct LineForm {
    owner: WeakEntity<Pokarak>,
    root: PathBuf,
    path: PathBuf,
    id: EntityId,
    input: Entity<InputState>,
    error: Option<String>,
    _subscription: Subscription,
}
impl LineForm {
    fn jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value();
        let (line, column) = value.split_once(':').unwrap_or((&value, "1"));
        let parsed = line
            .trim()
            .parse::<u32>()
            .ok()
            .zip(column.trim().parse::<u32>().ok())
            .filter(|(l, c)| *l > 0 && *c > 0);
        let Some((line, column)) = parsed else {
            self.error = Some("1부터 시작하는 줄 번호 또는 줄:열을 입력하세요.".into());
            cx.notify();
            return;
        };
        let result = self.owner.update(cx, |app, cx| {
            let Some((root, path, editor)) = app.code_tools_target(cx) else {
                return Err("코드 파일이 닫혔습니다.");
            };
            if root != self.root || path != self.path || editor.entity_id() != self.id {
                return Err("파일이 변경되었습니다. 다시 열어 주세요.");
            }
            let lines = editor.read(cx).text().lines_len();
            if line as usize > lines {
                return Err("파일의 마지막 줄을 넘었습니다.");
            }
            window.close_dialog(cx);
            editor.update(cx, |e, cx| {
                e.set_cursor_position(Position::new(line - 1, column - 1), window, cx)
            });
            editor.read(cx).focus_handle(cx).focus(window, cx);
            Ok(())
        });
        if let Ok(Err(error)) = result {
            self.error = Some(error.into());
            cx.notify();
        }
    }
}
impl Render for LineForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .child("줄 번호 또는 줄:열 (1부터 시작)")
            .child(Input::new(&self.input))
            .children(
                self.error
                    .as_ref()
                    .map(|e| div().text_color(rgb(RED)).child(e.clone())),
            )
            .child(
                Button::new("go-to-line-submit")
                    .label("이동")
                    .on_click(cx.listener(|s, _, w, cx| s.jump(w, cx))),
            )
    }
}
impl Pokarak {
    pub(in crate::ui) fn open_command_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let entries = entries(self, window, cx);
        let owner = cx.entity().downgrade();
        let palette = cx.new(|cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("명령 검색 · Git · 편집 · 워크트리")
            });
            let subscription =
                cx.subscribe(&input, |s: &mut Palette, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        s.query = input.read(cx).value().to_string();
                        s.cursor = 0;
                        cx.notify();
                    }
                });
            input.read(cx).focus_handle(cx).focus(window, cx);
            Palette {
                owner,
                input,
                entries,
                query: String::new(),
                cursor: 0,
                scroll: UniformListScrollHandle::new(),
                _subscriptions: vec![subscription],
            }
        });
        window.open_app_dialog(cx, move |dialog, _, _| {
            dialog
                .title("명령 팔레트")
                .width(ui_px(720.))
                .keyboard(false)
                .child(palette.clone())
        });
    }
    pub(in crate::ui) fn open_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let Some((root, path, editor)) = self.code_tools_target(cx) else {
            return;
        };
        let id = editor.entity_id();
        let line = editor.read(cx).cursor_position().line + 1;
        let owner = cx.entity().downgrade();
        let form = cx.new(|cx| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(line.to_string()));
            input.update(cx, |input, cx| input.select_all(window, cx));
            input.read(cx).focus_handle(cx).focus(window, cx);
            let subscription = cx.subscribe_in(
                &input,
                window,
                |s: &mut LineForm, _, event: &InputEvent, w, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        s.jump(w, cx);
                    }
                },
            );
            LineForm {
                owner,
                root,
                path,
                id,
                input,
                error: None,
                _subscription: subscription,
            }
        });
        window.open_app_dialog(cx, move |dialog, _, _| {
            dialog
                .title("줄 번호로 이동")
                .width(ui_px(380.))
                .child(form.clone())
        });
    }
}
