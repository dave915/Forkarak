use super::*;
use crate::theme::DisplaySettings;

#[derive(Clone, Copy)]
enum Setting {
    InterfaceFont,
    CodeFont,
    Zoom,
}

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Self::InterfaceFont => "인터페이스 글자 크기",
            Self::CodeFont => "코드·Diff 글자 크기",
            Self::Zoom => "화면 배율",
        }
    }

    fn value(self, display: DisplaySettings) -> String {
        match self {
            Self::InterfaceFont => format!("{} pt", display.ui_font_size),
            Self::CodeFont => format!("{} pt", display.code_font_size),
            Self::Zoom => format!("{}%", display.ui_zoom_percent),
        }
    }

    fn button_ids(self) -> [&'static str; 2] {
        match self {
            Self::InterfaceFont => ["ui-font-smaller", "ui-font-larger"],
            Self::CodeFont => ["font-smaller", "font-larger"],
            Self::Zoom => ["ui-zoom-out", "ui-zoom-in"],
        }
    }

    fn step(self, mut display: DisplaySettings, direction: i16) -> DisplaySettings {
        match self {
            Self::InterfaceFont => {
                display.ui_font_size = (display.ui_font_size + f32::from(direction)).clamp(
                    *DisplaySettings::UI_RANGE.start(),
                    *DisplaySettings::UI_RANGE.end(),
                )
            }
            Self::CodeFont => {
                display.code_font_size = (display.code_font_size + f32::from(direction)).clamp(
                    *DisplaySettings::CODE_RANGE.start(),
                    *DisplaySettings::CODE_RANGE.end(),
                )
            }
            Self::Zoom => {
                display.ui_zoom_percent = display
                    .ui_zoom_percent
                    .saturating_add_signed(direction * 10)
                    .clamp(
                        *DisplaySettings::ZOOM_RANGE.start(),
                        *DisplaySettings::ZOOM_RANGE.end(),
                    )
            }
        }
        display
    }

    fn row(self, display: DisplaySettings, view: &WeakEntity<Pokarak>) -> Div {
        let button = |index: usize, direction: i16, icon, action: &'static str| {
            let view = view.clone();
            Button::new(self.button_ids()[index])
                .icon(icon)
                .small()
                .accessibility_label(format!("{} {action}", self.label()))
                .tooltip(format!("{} {action}", self.label()))
                .disabled(self.step(display, direction) == display)
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |app, cx| {
                        app.set_display(self.step(app.prefs.display, direction), cx);
                    });
                })
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.label()),
            )
            .child(button(0, -1, IconName::Minus, "줄이기"))
            .child(
                div()
                    .min_w(ui_text(48.))
                    .text_center()
                    .child(self.value(display)),
            )
            .child(button(1, 1, IconName::Plus, "늘리기"))
    }
}

impl Pokarak {
    fn set_display(&mut self, display: DisplaySettings, cx: &mut Context<Self>) {
        self.prefs.display = display;
        self.prefs.save();
        theme::apply_display(display, cx);
        cx.notify();
    }

    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigation_request += 1;
        self.navigating = false;
        let weak = cx.weak_entity();
        let sample = [
            ("− ", patch::DELETE_BG, RED, "val count = 1"),
            ("+ ", patch::ADD_BG, GREEN, "val count = 2"),
        ]
        .map(|(sign, background, color, text)| {
            let syntax = syntax::CodeSyntax::new(Path::new("Preview.kt"), text);
            (sign, background, color, text, syntax)
        });
        window.open_dialog(cx, move |dialog, window, cx| {
            let (choice, display) = weak
                .read_with(cx, |app, _| (app.prefs.theme, app.prefs.display))
                .unwrap_or_default();
            let mut choices = div().flex().gap_2();
            for (index, (value, label, icon)) in [
                (ThemeChoice::System, "시스템", IconName::Monitor),
                (ThemeChoice::Light, "라이트", IconName::Sun),
                (ThemeChoice::Dark, "다크", IconName::Moon),
            ]
            .into_iter()
            .enumerate()
            {
                let target = weak.clone();
                choices = choices.child(
                    Button::new(("theme", index))
                        .label(label)
                        .icon(icon)
                        .selected(choice == value)
                        .on_click(move |_, window, cx| {
                            let _ = target.update(cx, |app, cx| app.set_theme(value, window, cx));
                        }),
                );
            }
            let palette = Palette::new(cx);
            let restore = weak.clone();
            let font = ui_px(display.code_font_size);
            let preview = sample.iter().map(|(sign, background, color, text, syntax)| {
                let gutter = div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(palette.rgb(*color))
                    .child(*sign);
                div()
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(palette.rgb(*background))
                    .child(gutter)
                    .child(syntax.styled_line(0, text, false, cx))
            });
            dialog
                .title("설정")
                .width(ui_px(630.).min(window.viewport_size().width - ui_px(24.)))
                .child(
                    div()
                        .id("settings-scroll")
                        .max_h(window.viewport_size().height - ui_px(200.))
                        .overflow_y_scroll()
                        .v_flex()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child("테마"))
                                .child(choices),
                        )
                        .child(Setting::Zoom.row(display, &weak))
                        .child(Setting::InterfaceFont.row(display, &weak))
                        .child(Setting::CodeFont.row(display, &weak))
                        .child(
                            div()
                                .text_size(ui_text(12.))
                                .text_color(palette.rgb(MUTED))
                                .child(
                                    "코드 글자 크기는 파일 본문, 변경 비교, Blame, 터미널에 적용됩니다.",
                                ),
                        )
                        .child(
                            div()
                                .id("code-font-preview")
                                .v_flex()
                                .rounded_md()
                                .overflow_hidden()
                                .font_family("JetBrains Mono")
                                .text_size(font)
                                .line_height(theme::code_row_height(font))
                                .children(preview),
                        )
                        .children(weak.read_with(cx, |app, cx| app.update_settings(weak.clone(), cx)).ok())
                        .children(weak.read_with(cx, |app, cx| app.ai_cli_settings(cx)).ok())
                )
                .footer(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    Button::new("display-reset")
                                        .label("표시 기본값 복원")
                                        .ghost()
                                        .disabled(display == DisplaySettings::DEFAULT)
                                        .on_click(move |_, _, cx| {
                                            let _ = restore.update(cx, |app, cx| {
                                                app.set_display(DisplaySettings::DEFAULT, cx)
                                            });
                                        }),
                                )
                                .child(
                                    Button::new("settings-close")
                                        .label("닫기")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                ),
                )
        });
    }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn agent_prompt(repo: &Path, agent: &str) -> String {
    format!(
        "Use Pokarak CLI for this worktree: {}. First run pokarak ctl instructions --agent {agent}, then pokarak ctl status --repo {}. Read the JSON instructions and follow them. All ctl responses are JSON; verify ok and the canonical repo. Use read-only inspection first. File edits, staging, commits and merges require my explicit request and --yes. Save only with exact expected content. Do not force, discard, reset, stash, checkout, start builds or language servers implicitly.",
        repo.display(),
        shell_quote(&repo.to_string_lossy())
    )
}

fn copy_command(id: &'static str, label: &'static str, command: String, palette: &Palette) -> Div {
    let copy = command.clone();
    div()
        .v_flex()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(label))
                .child(
                    Button::new(id)
                        .label("복사")
                        .small()
                        .ghost()
                        .accessibility_label(format!("{label} 복사"))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }),
                ),
        )
        .child(
            div()
                .text_size(ui_text(11.))
                .text_color(palette.rgb(MUTED))
                .font_family("JetBrains Mono")
                .child(command),
        )
}

impl Pokarak {
    fn ai_cli_settings(&self, cx: &App) -> Div {
        let palette = Palette::new(cx);
        let repo = self
            .current
            .as_deref()
            .unwrap_or_else(|| Path::new("/absolute/worktree"));
        let quoted = shell_quote(&repo.to_string_lossy());
        let claude = agent_prompt(repo, "claude");
        let codex = agent_prompt(repo, "codex");
        let mut section = div().v_flex().gap_2().pt_3().border_t_1().border_color(palette.rgb(LINE))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Claude · Codex CLI"))
            .child(div().text_size(ui_text(12.)).text_color(palette.rgb(MUTED)).child("터미널에서 실행하세요. 각 명령은 선택한 워크트리를 명시하며 ctl 응답은 JSON입니다. 프롬프트는 AI 대화에 붙여 넣을 수 있습니다."))
            .child(copy_command("ai-cli-claude","Claude 시작",format!("claude {}",shell_quote(&claude)),&palette))
            .child(copy_command("ai-cli-codex","Codex 시작",format!("codex {}",shell_quote(&codex)),&palette))
            .child(copy_command("ai-cli-prompt","AI에게 보낼 프롬프트",agent_prompt(repo,"codex"),&palette))
            .child(copy_command("ai-cli-status","상태 확인",format!("pokarak ctl status --repo {quoted}"),&palette))
            .child(copy_command("ai-cli-diff","변경 파일 확인",format!("pokarak ctl files --repo {quoted} --changed"),&palette))
            .child(copy_command("ai-cli-guide","전체 명령 · 사용 지침","pokarak ctl instructions".into(),&palette));
        if let Some(server) = &self.cli_control {
            section = section.child(copy_command(
                "ai-cli-gui",
                "현재 창 제어",
                format!(
                    "pokarak ctl gui status --session {}",
                    shell_quote(&server.socket.to_string_lossy())
                ),
                &palette,
            ));
        }
        section
    }
}

#[cfg(test)]
mod cli_tests {
    use super::{agent_prompt, shell_quote};
    use std::path::Path;
    #[::core::prelude::v1::test]
    fn launcher_quotes_spaces_apostrophes_and_substitutions() {
        assert_eq!(shell_quote("a'b $c `d`"), "'a'\\''b $c `d`'");
        let prompt = agent_prompt(Path::new("/tmp/a'b $(whoami)"), "claude");
        assert!(prompt.contains("pokarak ctl instructions --agent claude"));
        assert!(prompt.contains("--yes"));
        // Verify the launch argument round-trips through a real shell without evaluation.
        let command = format!("printf '%s' {}", shell_quote(&prompt));
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), prompt);
    }
}
