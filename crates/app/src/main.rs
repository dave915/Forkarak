mod assets;
mod cli;
mod prefs;
#[cfg(target_os = "macos")]
mod terminal;
mod theme;
mod ui;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use std::borrow::Cow;
fn main() {
    #[cfg(target_os = "macos")]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new(terminal::HELPER_ARG)) {
        let result = std::env::args_os()
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("터미널 세션이 없습니다"))
            .and_then(|socket| terminal::exec_shell(&socket));
        if let Err(error) = result {
            eprintln!("포카락: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args_os().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Pokarak — native Git worktree review\nUsage: pokarak [PROJECT ...] [--window-size WIDTHxHEIGHT]"
        );
        return;
    }
    let options = cli::parse(std::env::args_os().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    if let Some(path) = options.session_dir {
        prefs::set_session_dir(path);
    }
    let initial = options.projects;
    gpui_kit::application()
        .with_assets(assets::AppAssets)
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
                    app_owns_titlebar_drag: true,
                    titlebar: Some(TitlebarOptions {
                        title: None,
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(15.), px(11.))),
                    }),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(options.width), px(options.height)),
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
