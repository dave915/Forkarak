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

/// Stored in logical units. Existing sessions already use the code_font_size key.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    pub code_font_size: f32,
    pub ui_font_size: f32,
    pub ui_zoom_percent: u16,
}

impl DisplaySettings {
    pub const DEFAULT: Self = Self {
        code_font_size: 13.,
        ui_font_size: 13.,
        ui_zoom_percent: 100,
    };
    pub const CODE_RANGE: std::ops::RangeInclusive<f32> = 10. ..=24.;
    pub const UI_RANGE: std::ops::RangeInclusive<f32> = 11. ..=18.;
    pub const ZOOM_RANGE: std::ops::RangeInclusive<u16> = 80..=140;

    pub fn validated(mut self) -> Self {
        if !Self::CODE_RANGE.contains(&self.code_font_size) {
            self.code_font_size = Self::DEFAULT.code_font_size;
        }
        if !Self::UI_RANGE.contains(&self.ui_font_size) {
            self.ui_font_size = Self::DEFAULT.ui_font_size;
        }
        if !Self::ZOOM_RANGE.contains(&self.ui_zoom_percent) {
            self.ui_zoom_percent = Self::DEFAULT.ui_zoom_percent;
        }
        self
    }

    fn scale(self) -> f32 {
        f32::from(self.ui_zoom_percent) / 100.
    }
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// Window layout and drawing run on GPUI's main thread. These app-wide units also
// apply to menus, dialogs, and virtual rows built outside the root view's render.
thread_local! {
    static DISPLAY: std::cell::Cell<DisplaySettings> = const { std::cell::Cell::new(DisplaySettings::DEFAULT) };
}

/// Convert a design dimension to actual GPUI layout pixels. Measured bounds and
/// pointer coordinates are already in those pixels.
pub fn ui_px(value: f32) -> Pixels {
    px(value * DISPLAY.get().scale())
}

pub fn logical_px(value: Pixels) -> f32 {
    f32::from(value) / DISPLAY.get().scale()
}

pub fn ui_text(value: f32) -> Pixels {
    let display = DISPLAY.get();
    px(value * display.ui_font_size / 13. * display.scale())
}

pub fn apply_display(settings: DisplaySettings, cx: &mut App) {
    DISPLAY.set(settings);
    Theme::update(cx, |theme| {
        // GPUI Kit sets the root rem from font_size, including its overlay layer.
        theme.font_size = ui_text(13.);
        theme.mono_font_size = ui_px(settings.code_font_size);
    });
    cx.refresh_windows();
}

pub fn code_row_height(font_size: Pixels) -> Pixels {
    px((f32::from(font_size) * 1.55).ceil())
}

pub fn apply(choice: ThemeChoice, display: DisplaySettings, window: &mut Window, cx: &mut App) {
    let mode = match choice {
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::System => window.appearance().into(),
    };
    Theme::change(mode, Some(window), cx);
    let palette = Palette::new(cx);
    Theme::update(cx, |theme| {
        theme.font_family = "IBM Plex Sans".into();
        theme.mono_font_family = "JetBrains Mono".into();
        theme.colors.background = palette.rgb(0x20252c).into();
        theme.colors.foreground = palette.rgb(0xe5eaf0).into();
        theme.colors.border = palette.rgb(0x343e49).into();
        theme.colors.accent = palette.rgb(0x314353).into();
        theme.colors.accent_foreground = palette.rgb(0xe5eaf0).into();
    });
    apply_display(display, cx);
}
