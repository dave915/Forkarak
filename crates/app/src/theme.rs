use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    System,
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy)]
pub struct Palette {
    dark: bool,
}
impl Palette {
    pub fn new(cx: &App) -> Self {
        Self {
            dark: cx.theme().is_dark(),
        }
    }
    pub fn rgb(self, value: u32) -> Rgba {
        let mapped = if self.dark {
            value
        } else {
            match value {
                0x20252c => 0xf8fafc,
                0x1b2026 => 0xeef2f6,
                0x282f38 => 0xe3eaf1,
                0x343e49 => 0xd1dae4,
                0xe5eaf0 => 0x1e2b3a,
                0xa4b0be => 0x566779,
                0x7fb8e2 => 0x255f8d,
                0x83c4a1 => 0x1b6a40,
                0xe5a0a5 => 0xa42f41,
                0x2b3947 | 0x2c3a48 | 0x314353 => 0xd7e6f4,
                0x24382f => 0xe5f3e9,
                0x3c2a30 => 0xfae7e9,
                0x283744 | 0x293848 => 0xe5eef7,
                0xd59ede => 0x88428f,
                0x7ac8ce => 0x226e78,
                0xb0a9ef => 0x6556a2,
                0xa8c483 => 0x527327,
                0xe4bb79 => 0x805512,
                0x3b3326 => 0xfff0cd,
                0xe4c28c => 0x7b4f0c,
                other => other,
            }
        };
        gpui_kit::rgb(mapped)
    }
}

pub fn code_row_height(font_size: Pixels) -> Pixels {
    px((f32::from(font_size) * 1.55).ceil())
}

pub fn apply(choice: ThemeChoice, font_size: f32, window: &mut Window, cx: &mut App) {
    let mode = match choice {
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::System => window.appearance().into(),
    };
    Theme::change(mode, Some(window), cx);
    let palette = Palette::new(cx);
    Theme::update(cx, |theme| {
        theme.font_family = "IBM Plex Sans".into();
        theme.font_size = px(13.);
        theme.mono_font_family = "JetBrains Mono".into();
        theme.mono_font_size = px(font_size);
        theme.colors.background = palette.rgb(0x20252c).into();
        theme.colors.foreground = palette.rgb(0xe5eaf0).into();
        theme.colors.border = palette.rgb(0x343e49).into();
        theme.colors.accent = palette.rgb(0x314353).into();
        theme.colors.accent_foreground = palette.rgb(0xe5eaf0).into();
    });
}
