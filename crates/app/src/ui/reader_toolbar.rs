//! One measured command row. Native menus retain commands at every pane width.
use super::*;
use gpui_kit::base::ElementExt;
use gpui_kit::component::native_menu::NativeMenu;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Density {
    #[default]
    Minimal,
    Compact,
    Full,
}
/// Logical width from which the code/unified/split buttons show words.
const VIEW_LABEL_WIDTH: f32 = 900.;

impl Density {
    fn for_width(width: f32) -> Self {
        if width >= 700. {
            Self::Full
        } else if width >= 360. {
            Self::Compact
        } else {
            Self::Minimal
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct Identity {
    root: PathBuf,
    path: PathBuf,
    snapshot: u64,
    file: u64,
    history: Option<(String, Option<String>)>,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) enum Command {
    Diff,
    Split,
    Code,
    Review,
    PreviousChange,
    NextChange,
    Whitespace,
    History,
    Blame,
    BlameMode,
    CloseBlame,
    Analysis,
    Definition,
    References,
    Implementation,
    RenameSymbol,
    QuickFix,
    Complete,
    Diagnostics,
    Symbols,
    Signature,
    Hover,
    LanguageSettings,
    Palette,
    Files,
    Line,
    Wrap,
    Pin,
    PreviousCode,
    Format,
    Edit,
    Save,
    SaveCommit,
    CompareDraft,
    DiscardDraft,
    RecoverDraft(u64),
    CopyPath,
    Worktree,
    CompareBranch,
    CompareHead,
}
#[derive(Action, Clone, Debug, PartialEq, Deserialize)]
#[action(namespace = forkarak_reader, no_json)]
pub(super) struct Run {
    identity: Identity,
    command: Command,
}
impl Identity {
    pub(super) fn item(
        &self,
        menu: NativeMenu,
        label: impl Into<SharedString>,
        command: Command,
        disabled: bool,
        checked: bool,
    ) -> NativeMenu {
        let action = Box::new(Run {
            identity: self.clone(),
            command,
        });
        if disabled {
            menu.menu_with_disabled(label, true, action)
        } else {
            menu.menu_with_check(label, checked, action)
        }
    }
}

pub(super) struct ReaderView<'a> {
    pub path: &'a Path,
    pub code: bool,
    pub history: bool,
    pub deleted: bool,
    pub diff: Option<&'a ReviewDiff>,
    pub target: Option<file_inspector::Target>,
}

impl Pokarak {
    fn reader_identity(&self) -> Option<Identity> {
        let history = self.git_history.active;
        let (path, revision) = if history {
            let (detail, doc) = self.git_history.review_data()?;
            (
                doc?.path.clone(),
                Some((detail.commit.oid.clone(), detail.parent.clone())),
            )
        } else {
            (self.selected()?, None)
        };
        Some(Identity {
            root: self.current.clone()?,
            path,
            snapshot: self.request,
            file: self.file_request,
            history: revision,
        })
    }

    pub(super) fn run_reader_command(
        &mut self,
        action: &Run,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_path.is_some() || self.reader_identity().as_ref() != Some(&action.identity)
        {
            return;
        }
        use Command::*;
        match action.command {
            Diff | Split | Code => {
                let code = action.command == Code;
                if !code {
                    self.set_split_diff(action.command == Split);
                }
                if self.git_history.active {
                    self.select_history_reader_mode(code, window, cx);
                } else {
                    self.select_reader_mode(code, window, cx);
                }
            }
            Review => self.toggle_reviewed_file(window, cx),
            PreviousChange => self.jump_diff_hunk(false, window, cx),
            NextChange => self.jump_diff_hunk(true, window, cx),
            Whitespace => self.toggle_diff_whitespace(cx),
            History | Blame => {
                if let Some(target) = self.reader_inspection_target() {
                    self.inspect_file(
                        target,
                        if action.command == History {
                            file_inspector::Mode::History
                        } else {
                            file_inspector::Mode::Blame
                        },
                        window,
                        cx,
                    );
                }
            }
            BlameMode | CloseBlame => self.reader_blame_action(action.command == CloseBlame, cx),
            Analysis => {
                if self.navigation.is_some() {
                    self.stop_analysis(cx);
                    self.status = "코드 탐색 중지됨".into();
                } else {
                    self.show_explicit_analysis_prompt();
                }
                cx.notify();
            }
            Definition => self.request_navigation(NavigationKind::Definition, None, window, cx),
            References => self.request_navigation(NavigationKind::References, None, window, cx),
            Implementation => {
                self.request_navigation(NavigationKind::Implementation, None, window, cx)
            }
            RenameSymbol => {
                self.request_language(language_tools::Tool::PrepareRename, None, window, cx)
            }
            QuickFix => self.request_language(language_tools::Tool::QuickFix, None, window, cx),
            Complete | Diagnostics | Symbols | Signature | Hover => {
                let tool = match action.command {
                    Complete => language_tools::Tool::Complete,
                    Diagnostics => language_tools::Tool::Diagnostics,
                    Symbols => language_tools::Tool::Symbols,
                    Signature => language_tools::Tool::Signature,
                    _ => language_tools::Tool::Hover,
                };
                self.request_language(tool, None, window, cx);
            }
            LanguageSettings => self.open_settings_section("language", window, cx),
            Palette => self.open_command_palette(window, cx),
            Files => self.open_file_tools(window, cx),
            Line => self.open_go_to_line(window, cx),
            Wrap => self.toggle_word_wrap(window, cx),
            Pin => self.pin_code_tab(window, cx),
            PreviousCode => self.toggle_previous_code(window, cx),
            Format => self.format_code_file(false, false, window, cx),
            Edit => self.start_code_edit(window, cx),
            Save => self.save_code_file(false, window, cx),
            SaveCommit => self.save_code_file(true, window, cx),
            CompareDraft => self.compare_code_draft(window, cx),
            RecoverDraft(revision) => self.recover_code_draft(revision, window, cx),
            DiscardDraft => self.discard_code_draft(window, cx),
            CopyPath => cx.write_to_clipboard(ClipboardItem::new_string(
                action
                    .identity
                    .root
                    .join(&action.identity.path)
                    .display()
                    .to_string(),
            )),
            Worktree => self.open_worktree_picker(window, cx),
            CompareBranch => self.open_branch_review(window, cx),
            CompareHead => self.apply_branch_review(
                action.identity.root.clone(),
                CompareMode::Uncommitted,
                window,
                cx,
            ),
        }
    }

    fn reader_inspection_target(&self) -> Option<file_inspector::Target> {
        let identity = self.reader_identity()?;
        if let Some((revision, parent)) = identity.history {
            let (detail, _) = self.git_history.review_data()?;
            let deleted = detail
                .files
                .iter()
                .any(|f| f.path == identity.path && f.status == FileStatus::Deleted);
            Some(file_inspector::Target {
                path: identity.path,
                revision: Some(revision.clone()),
                blame_revision: if deleted { parent } else { Some(revision) },
            })
        } else {
            let relative = if identity.path.is_absolute() {
                identity.path.strip_prefix(&identity.root).ok()?.to_owned()
            } else {
                identity.path
            };
            let snapshot = self.snapshot();
            let deleted = snapshot.is_some_and(|s| {
                s.changes
                    .iter()
                    .any(|f| f.path == relative && f.status == FileStatus::Deleted)
            });
            Some(file_inspector::Target {
                path: relative,
                revision: None,
                blame_revision: snapshot
                    .filter(|_| deleted)
                    .map(|s| s.worktree.head.clone())
                    .filter(|s| !s.is_empty()),
            })
        }
    }

    pub(super) fn code_support_menu(&self, identity: &Identity, cx: &App) -> NativeMenu {
        let ready = self.code_support_available(cx);
        let navigation = ready && !self.navigating;
        let mut menu = NativeMenu::new();
        for (label, id, command) in [
            ("이름 바꾸기…", "rename_symbol", Command::RenameSymbol),
            ("빠른 수정…", "quick_fix", Command::QuickFix),
        ] {
            menu = identity.item(
                menu,
                keycaps::command_label(label, id, &self.prefs, self.current_project.as_deref()),
                command,
                !ready,
                false,
            );
        }
        menu = menu.separator();
        for (label, command) in [
            ("자동완성  ⌃Space", Command::Complete),
            ("오류 확인", Command::Diagnostics),
            ("파일 구조 검색  ⌘⇧O", Command::Symbols),
            ("함수 도움말  ⌘I", Command::Signature),
            ("타입 · 설명  ⌥F1", Command::Hover),
        ] {
            menu = identity.item(
                menu,
                keycaps::menu_label(label, &self.prefs, self.current_project.as_deref()),
                command,
                !ready,
                false,
            );
        }
        menu = menu.separator();
        for (label, id, command) in [
            ("정의로 이동", "find_definition", Command::Definition),
            ("사용처", "find_references", Command::References),
            ("구현체", "find_implementation", Command::Implementation),
        ] {
            menu = identity.item(
                menu,
                keycaps::command_label(label, id, &self.prefs, self.current_project.as_deref()),
                command,
                !navigation,
                false,
            );
        }
        menu = identity.item(
            menu.separator(),
            if self.navigation.is_some() {
                "코드 탐색 중지"
            } else {
                "코드 탐색 시작…"
            },
            Command::Analysis,
            self.git_history.active,
            self.navigation.is_some(),
        );
        identity.item(
            menu,
            "코드 지원 설정…",
            Command::LanguageSettings,
            false,
            false,
        )
    }

    pub(super) fn show_reader_file_menu(
        &self,
        target: file_inspector::Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reader_inspection_target().as_ref() != Some(&target) {
            return;
        }
        if let Some(identity) = self.reader_identity() {
            let menu = identity.item(
                identity.item(
                    NativeMenu::new(),
                    "History · 파일 커밋 이력…",
                    Command::History,
                    false,
                    false,
                ),
                "Blame · 줄별 작성자…",
                Command::Blame,
                false,
                false,
            );
            self.reader_blame_menu(menu, &identity, cx)
                .show(window.mouse_position(), window, cx);
        }
    }

    pub(super) fn show_code_support_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(identity) = self.reader_identity() {
            self.code_support_menu(&identity, cx)
                .show(window.mouse_position(), window, cx);
        }
    }

    fn reader_menu(&self, view: &ReaderView<'_>, cx: &App) -> Option<NativeMenu> {
        let identity = self.reader_identity()?;
        let busy = self.loading || self.file_loading || self.git_is_busy();
        let binary = view.diff.is_some_and(|d| d.binary);
        let mut modes = NativeMenu::new();
        for (label, command, disabled, selected) in [
            ("코드", Command::Code, binary, view.code),
            (
                "변경 · 통합",
                Command::Diff,
                view.diff.is_none(),
                !view.code && !self.prefs.split_diff,
            ),
            (
                "변경 · 좌우",
                Command::Split,
                view.diff.is_none() || binary,
                !view.code && self.prefs.split_diff,
            ),
        ] {
            modes = identity.item(modes, label, command, busy || disabled, selected);
        }
        modes = identity.item(
            modes.separator(),
            "공백 변경 숨김",
            Command::Whitespace,
            view.code,
            self.hides_diff_whitespace(),
        );
        modes = identity.item(
            modes,
            keycaps::menu_label(
                "이전 변경  ⌥↑",
                &self.prefs,
                self.current_project.as_deref(),
            ),
            Command::PreviousChange,
            view.code || view.diff.is_none(),
            false,
        );
        modes = identity.item(
            modes,
            keycaps::menu_label(
                "다음 변경  ⌥↓",
                &self.prefs,
                self.current_project.as_deref(),
            ),
            Command::NextChange,
            view.code || view.diff.is_none(),
            false,
        );
        let inspection = identity.item(
            identity.item(
                NativeMenu::new(),
                "History · 파일 커밋 이력…",
                Command::History,
                busy || view.target.is_none(),
                false,
            ),
            "Blame · 줄별 작성자…",
            Command::Blame,
            busy || view.target.is_none(),
            false,
        );
        let inspection = self.reader_blame_menu(inspection, &identity, cx);
        let mut menu = NativeMenu::new()
            .submenu("보기", modes)
            .submenu("파일 이력", inspection);
        menu = identity.item(
            menu,
            "검토 완료 표시 전환",
            Command::Review,
            !self.reader_review_available(),
            self.selected_review_status() == "reviewed",
        );
        if !view.history {
            let branch = self
                .session()
                .is_some_and(|s| matches!(s.saved.comparison, CompareMode::Branch(_)));
            let mut workspace = identity.item(
                NativeMenu::new(),
                keycaps::menu_label(
                    "워크트리 선택…  ⌘⇧J",
                    &self.prefs,
                    self.current_project.as_deref(),
                ),
                Command::Worktree,
                busy,
                false,
            );
            workspace = identity.item(
                workspace,
                "브랜치 전체 변경 비교…",
                Command::CompareBranch,
                busy,
                branch,
            );
            workspace = identity.item(
                workspace,
                "HEAD로 돌아가기",
                Command::CompareHead,
                busy || !branch,
                false,
            );
            menu = menu.submenu("워크트리 · 비교", workspace);
            menu = menu.submenu("코드 지원", self.code_support_menu(&identity, cx));
            menu = menu.submenu(
                "편집 · 파일",
                self.reader_editor_menu(&identity, view.path, cx),
            );
        }
        menu = identity.item(
            menu.separator(),
            keycaps::menu_label(
                "명령 팔레트…  ⌘⇧P",
                &self.prefs,
                self.current_project.as_deref(),
            ),
            Command::Palette,
            false,
            false,
        );
        menu = identity.item(menu, "전체 경로 복사", Command::CopyPath, false, false);
        Some(menu)
    }

    pub(super) fn reader_command_bar(
        &self,
        view: ReaderView<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = if view.history {
            "history-reader"
        } else {
            "working-reader"
        };
        let density = self
            .command_bar_density
            .get(key)
            .copied()
            .unwrap_or_default();
        let weak = cx.weak_entity();
        let measured = weak.clone();
        let menu_available = self.reader_identity().is_some();
        // Rebuild against the current identity on click; menu actions also revalidate it.
        let history = view.history;
        let open_more = move |app: &mut Pokarak, window: &mut Window, cx: &mut Context<Pokarak>| {
            app.show_reader_menu(history, window, cx);
        };
        let mut bar = div()
            .id(key)
            .relative()
            .h(ui_px(36.))
            .flex_none()
            .min_w_0()
            .px_1()
            .flex()
            .items_center()
            .gap(ui_px(2.))
            .border_b_1()
            .border_color(Palette::new(cx).rgb(LINE))
            .on_prepaint(move |bounds, _, cx| {
                let width = theme::logical_px(bounds.size.width);
                let density = Density::for_width(width);
                let labels = width >= VIEW_LABEL_WIDTH;
                let _ = measured.update(cx, |app, cx| {
                    if app.command_bar_density.get(key) != Some(&density) {
                        app.command_bar_density.insert(key, density);
                        cx.notify();
                    }
                    if app.command_bar_labels.contains(key) != labels {
                        if labels {
                            app.command_bar_labels.insert(key);
                        } else {
                            app.command_bar_labels.remove(key);
                        }
                        cx.notify();
                    }
                });
            });
        let labelled = density == Density::Full && self.command_bar_labels.contains(key);
        for (id, icon, label, short, command, selected, disabled) in [
            (
                "reader-code",
                IconName::CodeXml,
                "코드",
                "코드",
                Command::Code,
                view.code,
                view.diff.is_some_and(|d| d.binary),
            ),
            (
                "reader-diff",
                IconName::Diff,
                "변경 · 통합",
                "통합",
                Command::Diff,
                !view.code && !self.prefs.split_diff,
                view.diff.is_none(),
            ),
            (
                "reader-split",
                IconName::Columns2,
                "변경 · 좌우",
                "좌우",
                Command::Split,
                !view.code && self.prefs.split_diff,
                view.diff.is_none_or(|d| d.binary),
            ),
        ] {
            if density == Density::Minimal && !selected {
                continue;
            }
            let identity = self.reader_identity();
            let button = if labelled {
                // The most used and least guessable icons get words when there is room.
                Button::new(id)
                    .icon(icon)
                    .label(short)
                    .accessibility_label(label)
                    .ghost()
                    .small()
                    .h(ui_px(26.))
                    .flex_none()
            } else {
                toolbar::toolbar_icon(id, icon, label)
            };
            bar = bar.child(
                button
                    .tooltip(label)
                    .selected(selected)
                    .disabled(disabled || identity.is_none())
                    .on_click(cx.listener(move |app, _, w, cx| {
                        if let Some(identity) = identity.clone() {
                            app.run_reader_command(
                                &Run {
                                    identity,
                                    command: command.clone(),
                                },
                                w,
                                cx,
                            );
                        }
                    })),
            );
        }
        if density != Density::Minimal {
            bar = bar.child(
                self.reviewed_file_button(cx)
                    .label("")
                    .icon(IconName::Check)
                    .w(ui_px(28.))
                    .flex_none(),
            );
        }
        if !view.code && density != Density::Minimal {
            bar = bar.children(
                view.diff
                    .and_then(|d| self.diff_reading_controls(d, view.history, cx)),
            );
        }
        let comparison = !view.history
            && (!view.code
                || self
                    .session()
                    .is_some_and(|s| matches!(s.saved.comparison, CompareMode::Branch(_))));
        // This flexible slot gives controls priority even in a 160px reader.
        // The full applied baseline stays available on hover when truncated.
        bar = bar.child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .when(comparison, |slot| {
                    let label = self.applied_comparison_label();
                    slot.child(
                        div()
                            .id("reader-comparison-label")
                            .min_w_0()
                            .text_size(ui_text(11.))
                            .text_color(Palette::new(cx).rgb(MUTED))
                            .text_ellipsis()
                            .tooltip({
                                let label = label.clone();
                                move |w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(label.clone())
                                        .build(w, cx)
                                }
                            })
                            .child(label),
                    )
                }),
        );
        if view.code
            && !view.history
            && (!view.deleted
                || self
                    .current
                    .as_ref()
                    .is_some_and(|r| self.code_edits.dirty(r, view.path)))
        {
            bar = bar.child(self.code_edit_controls(view.path, cx));
            bar = bar.child(self.language_buttons(cx));
        }
        if density == Density::Full {
            bar = bar.child(self.editor_tool_buttons(view.path, cx));
            if let Some(target) = view.target {
                bar = bar.child(self.file_inspection_buttons(target, cx));
            }
            bar = bar.children(self.inline_blame_controls(cx));
        }
        let mut more_tooltip = if !view.code && self.hides_diff_whitespace() {
            "파일 명령 더 보기 · 공백 변경 숨김 적용 중".to_string()
        } else {
            "파일 명령 더 보기 · 보기, 이력, 코드 지원, 편집, 파일 관리".to_string()
        };
        if comparison {
            more_tooltip.push_str(&format!(" · {}", self.applied_comparison_label()));
        }
        bar.child(
            toolbar::toolbar_icon("reader-more", IconName::Ellipsis, "파일 명령 더 보기")
                .tooltip(more_tooltip)
                .selected(!view.code && self.hides_diff_whitespace())
                .disabled(!menu_available)
                .on_click(cx.listener(move |app, _, w, cx| open_more(app, w, cx))),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, _, w, cx| app.show_reader_menu(history, w, cx)),
        )
        .into_any_element()
    }

    fn show_reader_menu(&self, history: bool, window: &mut Window, cx: &mut Context<Self>) {
        if history != self.git_history.active {
            return;
        }
        let Some(identity) = self.reader_identity() else {
            return;
        };
        let diff = if history {
            self.history_reader_diff()
        } else {
            self.session().and_then(|s| s.diffs.get(&identity.path))
        };
        let code = if history {
            self.history_reader_code()
        } else {
            self.session().is_some_and(|s| s.saved.code)
        };
        let view = ReaderView {
            path: &identity.path,
            code,
            history,
            deleted: false,
            diff,
            target: self.reader_inspection_target(),
        };
        if let Some(menu) = self.reader_menu(&view, cx) {
            menu.show(window.mouse_position(), window, cx);
        }
    }
}
