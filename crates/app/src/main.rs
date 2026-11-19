mod app_icon;
mod assets;
mod cli;
mod control;
pub mod extension_host;
mod prefs;
pub mod task_runtime;
#[cfg(target_os = "macos")]
mod terminal;
mod test_report;
mod test_target;
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
    if args.first().is_some_and(|arg| arg == "install-cli") {
        let result = cli::launch::install_cli(&args[1..]);
        match result {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(2);
            }
        }
    }
    if args.first().is_some_and(|arg| arg == "ctl") {
        let (response, code) = cli::ctl::response(&args[1..]);
        println!("{response}");
        std::process::exit(code);
    }
    if args.first().is_some_and(|arg| arg == "--cli-open") {
        let result = args
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("Missing installed app path"))
            .and_then(|app| cli::launch(std::path::Path::new(app), &args[2..]));
        if let Err(error) = result {
            eprintln!("{error:#}");
            std::process::exit(2);
        }
        return;
    }
    if let Some(version) = cli::informational(&args) {
        println!(
            "{}",
            if version {
                env!("CARGO_PKG_VERSION")
            } else {
                cli::HELP
            }
        );
        return;
    }
    // LaunchServices receives prevalidated absolute targets as data, never shell text.
    let mut public_args = args.clone();
    let handoff = public_args
        .first()
        .is_some_and(|arg| arg == "--cli-targets")
        .then_some(0);
    let targets = handoff
        .map(|index| {
            let value = public_args
                .get(index + 1)
                .ok_or_else(|| anyhow::anyhow!("Missing CLI targets"))?;
            let targets: Vec<cli::OpenTarget> = serde_json::from_str(
                value
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid CLI targets"))?,
            )?;
            for target in &targets {
                cli::launch::validate_target(target)?;
            }
            public_args.drain(index..=index + 1);
            anyhow::Ok(targets)
        })
        .transpose()
        .unwrap_or_else(|error| {
            eprintln!("{error:#}");
            std::process::exit(2);
        });
    let options = cli::parse(public_args).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let initial = targets.unwrap_or_else(|| {
        cli::resolve(&options).unwrap_or_else(|error| {
            eprintln!("{error:#}");
            std::process::exit(2);
        })
    });
    if handoff.is_none()
        && cli::launch::reuse(&options, &initial).unwrap_or_else(|error| {
            eprintln!("{error:#}");
            std::process::exit(2);
        })
    {
        return;
    }
    if let Some(path) = options.session_dir {
        prefs::set_session_dir(cli::launch::absolute(&path).unwrap_or(path));
    }
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
                    cx.new(|cx| {
                        let roots = initial
                            .first()
                            .map(|target| vec![target.repo.clone()])
                            .unwrap_or_default();
                        let mut app = ui::Pokarak::new(roots, window, cx);
                        app.queue_cli_targets(initial.clone());
                        app
                    })
                },
            )
            .expect("포카락 창을 열 수 없습니다");
            cx.activate(true);
        });
}
