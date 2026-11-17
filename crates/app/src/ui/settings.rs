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
            dialog
                .title("설정")
                .width(ui_px(445.).min(window.viewport_size().width - ui_px(24.)))
                .child(
                    div()
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
                                    "코드 글자 크기는 파일 본문, 변경 비교, Blame에 적용됩니다.",
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
                                .child(
                                    div()
                                        .px_2()
                                        .bg(palette.rgb(patch::DELETE_BG))
                                        .text_color(palette.rgb(RED))
                                        .child("− val count = 1"),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .bg(palette.rgb(patch::ADD_BG))
                                        .text_color(palette.rgb(GREEN))
                                        .child("+ val count = 2"),
                                ),
                        )
                        .child(
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
                        ),
                )
        });
    }
}
