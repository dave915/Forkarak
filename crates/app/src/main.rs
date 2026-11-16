mod prefs;
mod ui;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use std::borrow::Cow;
fn main() {
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            let _ = cx.text_system().add_fonts(vec![
                Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/IBMPlexSans-Regular.ttf"
                )),
                Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/IBMPlexSans-Medium.ttf"
                )),
                Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/IBMPlexSans-SemiBold.ttf"
                )),
                Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/JetBrainsMono-Regular.ttf"
                )),
            ]);
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            Theme::update(cx, |theme| {
                theme.font_family = "IBM Plex Sans".into();
                theme.font_size = px(13.);
                theme.mono_font_family = "JetBrains Mono".into();
                theme.mono_font_size = px(13.);
                theme.colors.background = rgb(0x20252c).into();
                theme.colors.foreground = rgb(0xe5eaf0).into();
                theme.colors.border = rgb(0x343e49).into();
                theme.colors.accent = rgb(0x314353).into();
                theme.colors.accent_foreground = rgb(0xe5eaf0).into();
            });
            ui::bind_keys(cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1440.), px(900.)),
                        cx,
                    ))),
                    window_min_size: Some(size(px(940.), px(580.))),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    window.set_window_title("포카락");
                    cx.new(|cx| ui::Pokarak::new(initial.clone(), window, cx))
                },
            )
            .expect("포카락 창을 열 수 없습니다");
            cx.activate(true);
        });
}
