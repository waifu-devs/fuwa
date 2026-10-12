//! The shortcuts every app on the system answers to, whatever fuwa's own
//! keyboard settings say: Cmd+Q, Cmd+W, Cmd+M, Cmd+H and the rest on macOS,
//! with the menu bar that names them; Alt+F4 on Windows and Linux, and Ctrl+Q
//! on Linux. They're key bindings, which GPUI runs before the window's own key
//! handler (`keys.rs`), so nothing on the Keyboard page can take them, and
//! `core::keybinds::SYSTEM` keeps them from being offered there.

use gpui_kit::{App, KeyBinding, WindowId, actions};

actions!(fuwa_system, [Quit, CloseWindow, Minimize, ToggleFullScreen, Hide, HideOthers, ShowAll]);

/// Binds the keys and, on macOS, puts up the menu bar. `main` is the app's
/// window: closing it quits, so Cmd+W hides fuwa instead, as chat apps do.
pub fn init(main: Option<WindowId>, cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
    ]);
    #[cfg(target_os = "linux")]
    cx.bind_keys([KeyBinding::new("ctrl-q", Quit, None), KeyBinding::new("alt-f4", CloseWindow, None)]);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    cx.bind_keys([KeyBinding::new("alt-f4", CloseWindow, None)]);

    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(move |_: &CloseWindow, cx| {
        let Some(window) = cx.active_window() else { return };
        if cfg!(target_os = "macos") && Some(window.window_id()) == main {
            cx.hide();
            return;
        }
        let _ = window.update(cx, |_, window, _| window.remove_window());
    });
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &ToggleFullScreen, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.toggle_fullscreen());
        }
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());

    #[cfg(target_os = "macos")]
    set_menus(cx);
}

/// The menu bar: the app menu and the Window menu, the way every Mac app has them.
#[cfg(target_os = "macos")]
fn set_menus(cx: &mut App) {
    use gpui_kit::{Menu, MenuItem, SystemMenuType};

    use crate::core::i18n::t;

    cx.set_menus([
        Menu::new("fuwa").items([
            MenuItem::action(t("desktop.menu.settings"), crate::ui::app::OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t("desktop.menu.services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t("desktop.menu.hide"), Hide),
            MenuItem::action(t("desktop.menu.hideOthers"), HideOthers),
            MenuItem::action(t("desktop.menu.showAll"), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t("desktop.menu.quit"), Quit),
        ]),
        Menu::new(t("desktop.menu.window")).items([
            MenuItem::action(t("desktop.menu.minimize"), Minimize),
            MenuItem::action(t("desktop.menu.fullScreen"), ToggleFullScreen),
            MenuItem::separator(),
            MenuItem::action(t("desktop.menu.close"), CloseWindow),
        ]),
    ]);
}
