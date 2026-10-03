//! The window, drawn with GPUI Kit. It reads the core's store and calls its
//! methods; everything it shows moves the way the web app does.

pub mod app;
mod assets;
mod chat;
mod compose;
mod connect;
mod embeds;
mod emoji;
mod emoji_picker;
mod http;
mod mentions;
mod menus;
mod moderate;
mod motion;
mod notify;
mod overlay;
mod rail;
mod server_settings;
mod settings;
mod settings_account;
mod sidebar;
pub mod text;
pub mod theme;
mod widgets;

use std::fs::File;

use anyhow::Context as _;
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use crate::core::Core;
use crate::core::config::Paths;

pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "fuwa_desktop=info,warn".into()),
        )
        .init();

    let paths = Paths::from_env();
    std::fs::create_dir_all(&paths.config).with_context(|| format!("couldn't make {}", paths.config.display()))?;

    // One copy at a time: two would fight over the same encrypted-message devices.
    let lock = File::create(paths.lock_file())?;
    if lock.try_lock().is_err() {
        eprintln!("fuwa is already open.");
        return Ok(());
    }

    let core = Core::start(paths)?;

    let trusted: http::Trusted = {
        let core = core.clone();
        std::sync::Arc::new(move || core.instance_urls())
    };
    let pictures = std::sync::Arc::new(http::Client::new(core.handle(), trusted));
    gpui_kit::application().with_assets(assets::Assets).with_http_client(pictures).run(move |cx| {
        gpui_kit::init(cx);
        theme::load_fonts(cx);
        app::bind_keys(cx);

        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(860.0), px(560.0))),
            titlebar: Some(TitlebarOptions { title: Some("fuwa".into()), ..Default::default() }),
            app_id: Some("fuwa".into()),
            ..Default::default()
        };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            theme::apply(&core.prefs(), window.appearance(), cx);
            cx.new(|cx| app::FuwaApp::new(core.clone(), window, cx))
        });
        match opened {
            Ok((window, _)) => {
                let _ = window.update(cx, |_, window, _| window.activate_window());
            }
            Err(err) => {
                eprintln!("couldn't open the window: {err}");
                cx.quit();
            }
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
    drop(lock);
    Ok(())
}
