//! The window, drawn with GPUI Kit. It reads the core's store and calls its
//! methods; everything it shows moves the way the web app does.

mod announcement;
pub mod app;
mod arrange;
mod assets;
mod backdrop;
mod call_bar;
pub(crate) mod chat;
mod compose;
mod connect;
mod effects;
mod embeds;
mod emoji;
mod emoji_picker;
mod http;
mod instance_settings;
mod keys;
mod members;
mod mentions;
mod menus;
mod moderate;
mod motion;
mod notify;
pub(crate) mod overlay;
pub mod perf;
mod png;
mod rail;
mod secure;
mod server_settings;
mod settings;
mod settings_account;
mod settings_keys;
mod settings_language;
mod settings_look;
mod settings_privacy;
mod settings_updates;
mod shared_marks;
mod sidebar;
pub mod text;
pub mod theme;
mod update;
mod widgets;

use std::fs::File;

use anyhow::Context as _;
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use crate::core::Core;
use crate::core::config::Paths;

pub fn run() -> anyhow::Result<()> {
    perf::start();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "fuwa_desktop=info,warn".into()),
        )
        .init();
    crate::core::reports::catch_panics();

    let paths = Paths::from_env();
    std::fs::create_dir_all(&paths.config).with_context(|| format!("couldn't make {}", paths.config.display()))?;

    // One copy at a time: two would fight over the same encrypted-message devices.
    let lock = File::create(paths.lock_file())?;
    // Started by "Restart to update": the old app lets go of the lock as it quits.
    let after_update = std::env::var_os(crate::core::updates::AFTER_UPDATE).is_some();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while lock.try_lock().is_err() {
        if !after_update || std::time::Instant::now() > deadline {
            eprintln!("fuwa is already open.");
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // SAFETY: nothing else runs yet; the children this app starts shouldn't wait too.
    unsafe { std::env::remove_var(crate::core::updates::AFTER_UPDATE) };

    let core = Core::start(paths)?;
    perf::mark("core started");

    let trusted: http::Trusted = {
        let core = core.clone();
        std::sync::Arc::new(move || core.instance_urls())
    };
    let pictures = std::sync::Arc::new(http::Client::new(core.handle(), trusted));
    gpui_kit::application().with_assets(assets::Assets).with_http_client(pictures).run(move |cx| {
        perf::mark("app running");
        gpui_kit::init(cx);
        theme::load_fonts(cx);
        perf::mark("fonts loaded");
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
            perf::mark("window open");
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

/// Opens a sign-in page in the system browser, where people's passwords,
/// passkeys and password managers already are.
pub(crate) fn open_in_browser(page: &str) {
    if let Err(err) = open::that_detached(page) {
        tracing::warn!("couldn't open the browser: {err}");
    }
}
