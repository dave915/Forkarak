use super::*;
#[derive(Clone)]
enum Target {
    Action(fn() -> Box<dyn Action>),
    Worktree(PathBuf),
}
#[derive(Clone)]
struct Entry {
    title: String,
    keywords: String,
    key: String,
    target: Target,
    enabled: bool,
}
fn entries(app: &Pokarak, cx: &App) -> Vec<Entry> {
    let code = app.code_tools_target(cx).is_some();
    let ready =
        app.current.is_some() && app.opening_path.is_none() && !app.loading && !app.git_is_busy();
    let editable = code
        && app
            .current
            .as_ref()
            .zip(app.selected().as_ref())
            .is_some_and(|(root, path)| app.code_edits.can_edit(root, path));
    let mut list = vec![];
    let bindings = app
        .prefs
        .option(app.current_project.as_deref(), "keyboard.bindings")
        .and_then(|v| v.as_object());
    macro_rules! add {
        ($title:literal,$id:literal,$key:literal,$action:expr) => {
            list.push(Entry {
                title: $title.into(),
                keywords: $id.replace('_', " "),
                key: bindings
                    .and_then(|v| v.get($id))
                    .and_then(|v| v.as_str())
                    .unwrap_or($key)
                    .into(),
                target: Target::Action(|| Box::new($action)),
                enabled: match $id {
                    "open_repository" | "settings" => true,
                    "toggle_comment"
                    | "move_line_up"
                    | "move_line_down"
                    | "duplicate_line_up"
                    | "duplicate_line_down"
                    | "replace_file"
                    | "format_file" => editable,
                    "language_completion"
                    | "language_diagnostics"
                    | "language_symbols"
                    | "language_signature"
                    | "language_hover"
                    | "go_to_line"
                    | "word_wrap"
                    | "select_next_word"
                    | "pin_tab"
                    | "previous_code" => code,
                    _ => ready,
                },
            });
        };
    }
    add!(
        "파일: 저장소 열기…",
        "open_repository",
        "cmd-o",
        OpenRepository
    );
    add!(
        "파일: 이름 변경 · 이동 · 파일/폴더 생성…",
        "manage_files",
        "",
        ManageFiles
    );
    add!("파일: 현재 파일 저장", "save_file", "cmd-s", SaveCodeFile);
    add!("파일: 현재 파일 닫기", "close_file", "cmd-w", CloseFile);
    add!("편집: 주석 토글", "toggle_comment", "cmd-/", ToggleComment);
    add!("편집: 줄 위로 이동", "move_line_up", "alt-up", MoveLineUp);
    add!(
        "편집: 줄 아래로 이동",
        "move_line_down",
        "alt-down",
        MoveLineDown
    );
    add!(
        "편집: 줄 위에 복제",
        "duplicate_line_up",
        "alt-shift-up",
        DuplicateLineUp
    );
    add!(
        "편집: 줄 아래에 복제",
        "duplicate_line_down",
        "alt-shift-down",
        DuplicateLineDown
    );
    add!(
        "편집: 같은 단어 다음 항목 추가 선택",
        "select_next_word",
        "cmd-d",
        SelectNextWord
    );
    add!(
        "편집: 위 줄에 커서 추가",
        "add_cursor_above",
        "ctrl-alt-up",
        gpui_kit::base::input::AddCursorAbove
    );
    add!(
        "편집: 아래 줄에 커서 추가",
        "add_cursor_below",
        "ctrl-alt-down",
        gpui_kit::base::input::AddCursorBelow
    );
    add!(
        "편집: 파일 내 바꾸기",
        "replace_file",
        "cmd-alt-f",
        ReplaceFile
    );
    add!("편집: 파일 포맷", "format_file", "alt-shift-f", FormatFile);
    add!("이동: 줄:열 번호로 이동…", "go_to_line", "cmd-l", GoToLine);
    add!("이동: 파일 찾기…", "find_file", "cmd-p", FindFile);
    add!(
        "이동: 프로젝트 본문 검색…",
        "find_text",
        "cmd-shift-f",
        FindText
    );
    add!(
        "이동: 정의로 이동",
        "find_definition",
        "cmd-b",
        FindDefinition
    );
    add!(
        "이동: 사용처 찾기",
        "find_references",
        "alt-f7",
        FindReferences
    );
    add!(
        "언어 분석: 코드 완성",
        "language_completion",
        "ctrl-space",
        language_tools::Complete
    );
    add!(
        "언어 분석: 현재 파일 진단",
        "language_diagnostics",
        "",
        language_tools::Diagnostics
    );
    add!(
        "언어 분석: 문서 심볼 검색",
        "language_symbols",
        "cmd-shift-o",
        language_tools::Symbols
    );
    add!(
        "언어 분석: 함수 시그니처",
        "language_signature",
        "ctrl-shift-space",
        language_tools::Signature
    );
    add!(
        "언어 분석: 타입 · 문서",
        "language_hover",
        "alt-f1",
        language_tools::Hover
    );
    add!("이동: 뒤로", "back", "cmd-[", Back);
    add!("이동: 앞으로", "forward", "cmd-]", Forward);
    add!(
        "보기: 코드 / 변경사항 전환",
        "toggle_code",
        "cmd-alt-d",
        ToggleCode
    );
    add!(
        "보기: 긴 줄 화면 줄바꿈 전환",
        "word_wrap",
        "alt-z",
        ToggleWordWrap
    );
    add!(
        "보기: 미리보기 탭 고정",
        "pin_tab",
        "cmd-shift-enter",
        PinTab
    );
    add!(
        "보기: 현재 변경 구간의 이전 코드 펼치기",
        "previous_code",
        "",
        PreviousCode
    );
    add!(
        "보기: 다음 변경 구간",
        "next_diff_hunk",
        "alt-down",
        diff_tools::NextDiffHunk
    );
    add!(
        "보기: 이전 변경 구간",
        "previous_diff_hunk",
        "alt-up",
        diff_tools::PreviousDiffHunk
    );
    add!(
        "Git: Stage / 커밋…",
        "git_commit",
        "cmd-shift-k",
        OpenGitCommit
    );
    add!(
        "Git: 커밋 그래프",
        "git_history",
        "cmd-alt-h",
        OpenGitHistory
    );
    add!("Git: 커밋 검색…", "git_search", "", SearchGitGraph);
    add!(
        "Git: Stash / 충돌 해결…",
        "git_workflow",
        "cmd-shift-g",
        OpenGitWorkflow
    );
    add!(
        "Git: 브랜치 · 원격 · Git 작업…",
        "git_actions",
        "",
        OpenGitActions
    );
    add!(
        "워크트리: 다음 워크트리",
        "next_worktree",
        "cmd-alt-down",
        NextWorktree
    );
    add!(
        "워크트리: 이전 워크트리",
        "previous_worktree",
        "cmd-alt-up",
        PreviousWorktree
    );
    add!(
        "프로젝트: 다음 프로젝트",
        "next_project",
        "ctrl-tab",
        NextProject
    );
    add!(
        "프로젝트: 이전 프로젝트",
        "previous_project",
        "ctrl-shift-tab",
        PreviousProject
    );
    add!(
        "보기: 사이드바 표시 / 숨기기",
        "toggle_sidebar",
        "cmd-alt-b",
        ToggleSidebar
    );
    add!(
        "보기: 파일 목록 표시 / 숨기기",
        "toggle_files",
        "cmd-shift-b",
        ToggleFiles
    );
    #[cfg(target_os = "macos")]
    {
        add!(
            "터미널: 현재 워크트리 터미널 표시 / 숨기기",
            "terminal",
            "ctrl-`",
            OpenWorktreeTerminal
        );
    }
    add!("새로고침: 현재 워크트리", "refresh", "cmd-r", Refresh);
    add!("설정: 환경설정…", "settings", "cmd-,", OpenSettings);
    for tree in app.inventory.iter().filter(|t| !t.is_prunable) {
        list.push(Entry {
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
            if let Some(editor) = app.active_editor() {
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
                    .text_size(ui_text(11.))
                    .child(format!("{count}개 명령 · ↑↓ 선택 · Return 실행 · Esc 닫기")),
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
                                Some(
                                    Button::new(("palette-command", row))
                                        .ghost()
                                        .w_full()
                                        .selected(row == s.cursor)
                                        .disabled(!entry.enabled)
                                        .label(format!("{}   {}", entry.title, entry.key))
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
        let entries = entries(self, cx);
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
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("명령 팔레트")
                .width(ui_px(720.))
                .keyboard(false)
                .child(palette.clone())
        });
    }
    pub(super) fn open_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("줄 번호로 이동")
                .width(ui_px(380.))
                .child(form.clone())
        });
    }
}
