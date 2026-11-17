mod assets;
mod cli;
mod control;
mod prefs;
pub mod task_runtime;
pub mod extension_host;
#[cfg(target_os = "macos")]
mod terminal;
mod theme;
mod ui;
mod updater;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use std::borrow::Cow;
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--extension-host") {
        std::process::exit(if args.get(1).is_some_and(|arg| arg == "spring") {
            extension_host::run_builtin_spring()
        } else {
            2
        });
    }
    if args.first().is_some_and(|arg| arg == "ctl") {
        let (response, code) = cli::ctl::response(&args[1..]);
        println!("{response}");
        std::process::exit(code);
    }
    if std::env::args_os().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Forkarak — a native macOS Git client\nBrowse commit graphs across worktrees, compare changes, and stage individual lines or hunks.\n\nUsage: forkarak [--window-size WIDTHxHEIGHT] [--session-dir DIR] [--] [PROJECT ...]\n       forkarak ctl help    (headless JSON control / agent instructions)"
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
                    window.set_window_title("Forkarak");
                    cx.new(|cx| ui::Pokarak::new(initial.clone(), window, cx))
                },
            )
            .expect("포카락 창을 열 수 없습니다");
            cx.activate(true);
        });
}
