//! The Game overlay page, the desktop's own (`game_overlay.rs`): whether it
//! shows and when, its corner, how solid it is, who's in it, whether messages
//! go to it, and its key. It also says where it can't work: Wayland, macOS
//! until fuwa may hear keys, and games in exclusive full screen.

use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::config::{OverlayCorner, OverlayShow, Prefs};
use crate::core::hotkeys::{Hotkeys, Reach};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::game_overlay;
use crate::ui::settings::SettingsView;
use crate::ui::settings_app::pref;
use crate::ui::settings_controls::{At, Badge, Look, Opt, button, choice, hint, keycaps, toggle};
use crate::ui::theme::{Palette, alpha, radius_xl};
use crate::ui::widgets::icon;

/// Where the page's settings sit for search.
pub(crate) fn overlay_settings() -> Vec<(&'static str, String, &'static str)> {
    vec![
        ("overlay", t("desktop.overlay.on"), "game overlay in-game"),
        ("overlay-show", t("desktop.overlay.show"), "games calls full screen"),
        ("overlay-corner", t("desktop.overlay.corner"), "position place"),
        ("overlay-opacity", t("desktop.overlay.opacity"), "transparent see-through"),
        ("overlay-people", t("desktop.overlay.speakersOnly"), "talking speaking voice"),
        ("overlay-notifications", t("desktop.overlay.notifications"), "messages alerts"),
        ("overlay-key", t("desktop.overlay.key"), "shortcut hotkey keybind"),
    ]
}

fn percent(n: f32) -> String {
    format!("{}%", n.round() as i64)
}

/// A line of note with an icon, as the Keyboard page's.
fn note(glyph: &'static str, text: String, p: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(radius_xl())
        .bg(alpha(p.muted, 0.6))
        .text_sm()
        .text_color(p.muted_foreground)
        .child(icon(glyph).size(px(16.0)).mt(px(2.0)))
        .child(div().flex_1().min_w_0().child(text))
}

impl SettingsView {
    pub(crate) fn overlay_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = self.column;
        if !game_overlay::offered() {
            return note("monitor-x", t("desktop.overlay.wayland"), p).into_any_element();
        }
        let on = toggle(
            "overlay",
            &t("desktop.overlay.on"),
            Some(&t("desktop.overlay.onHint")),
            prefs.overlay,
            false,
            p,
            window,
            cx,
            |this, on, cx| this.set(cx, |pr| pr.overlay = on),
        );
        let mut on = div().flex().flex_col().gap(px(12.0)).child(on);
        if Hotkeys::reach() == Reach::NotAllowed {
            on = on.child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.0))
                    .child(note("keyboard", t("desktop.overlay.allowKeys"), p))
                    .child(
                        button(
                            "overlay-allow",
                            t("desktop.voice.openPrivacy"),
                            Some("settings"),
                            Look::Outline,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .on_click(|_, _, _| Hotkeys::ask()),
                    ),
            );
        }
        let mut rows: Vec<(&'static str, String, Option<String>, Badge, AnyElement)> =
            vec![("overlay", t("desktop.overlay.title"), None, pref!(prefs, overlay), on.into_any_element())];
        if prefs.overlay {
            let games_hint = if cfg!(target_os = "macos") {
                t("desktop.overlay.showGamesMac")
            } else {
                t("desktop.overlay.showGamesHint")
            };
            let show = choice(
                "overlay-show",
                Some(if prefs.overlay_show == OverlayShow::Games { 0 } else { 1 }),
                vec![
                    Opt::new(t("desktop.overlay.showGames"), games_hint, "gamepad-2"),
                    Opt::new(t("desktop.overlay.showCalls"), t("desktop.overlay.showCallsHint"), "phone"),
                ],
                w,
                p,
                window,
                cx,
                |this, n, cx| {
                    let s = if n == 0 { OverlayShow::Games } else { OverlayShow::Calls };
                    this.set(cx, |pr| pr.overlay_show = s)
                },
            );
            let corner_at = OverlayCorner::ALL.iter().position(|c| *c == prefs.overlay_corner);
            let corner = choice(
                "overlay-corner",
                corner_at,
                vec![
                    Opt::new(t("desktop.overlay.topLeft"), String::new(), "arrow-up-left"),
                    Opt::new(t("desktop.overlay.topRight"), String::new(), "arrow-up-right"),
                    Opt::new(t("desktop.overlay.bottomLeft"), String::new(), "arrow-down-left"),
                    Opt::new(t("desktop.overlay.bottomRight"), String::new(), "arrow-down-right"),
                ],
                w,
                p,
                window,
                cx,
                |this, n, cx| {
                    let c = OverlayCorner::ALL[n];
                    this.set(cx, |pr| pr.overlay_corner = c)
                },
            );
            let opacity = self.slider(
                "overlay-opacity",
                (30.0, 100.0, 5.0),
                f32::from(prefs.overlay_opacity),
                vec![(30.0, percent(30.0)), (100.0, percent(100.0))],
                percent,
                false,
                p,
                window,
                cx,
                |pr, v| pr.overlay_opacity = v.round() as u8,
            );
            let people = toggle(
                "overlay-people",
                &t("desktop.overlay.speakersOnly"),
                Some(&t("desktop.overlay.speakersOnlyHint")),
                prefs.overlay_speakers_only,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.overlay_speakers_only = on),
            );
            let notes = toggle(
                "overlay-notifications",
                &t("desktop.overlay.notifications"),
                Some(&t("desktop.overlay.notificationsHint")),
                prefs.overlay_notifications,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.overlay_notifications = on),
            );
            let combo = crate::core::keybinds::action_by_id("toggleOverlay")
                .and_then(|a| crate::core::keybinds::binding_of(a, &prefs.keybinds));
            let key = div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .text_sm()
                        .child(div().text_color(p.muted_foreground).child(t("appsettings.voice.yourKey")))
                        .child(match &combo {
                            Some(c) => keycaps(c, p),
                            None => div()
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .text_color(p.destructive)
                                .child(t("appsettings.voice.noKey"))
                                .into_any_element(),
                        })
                        .child(
                            button(
                                "overlay-change-key",
                                if combo.is_some() {
                                    t("appsettings.voice.changeKey")
                                } else {
                                    t("appsettings.voice.pickKey")
                                },
                                Some("keyboard"),
                                Look::Outline,
                                false,
                                p,
                            )
                            .rounded(radius_xl())
                            .on_click(cx.listener(|this, _, _, cx| this.show_keyboard(cx))),
                        ),
                )
                .child(hint(
                    match &combo {
                        Some(c) => {
                            t_with("desktop.overlay.keyHint", &[("keys", Arg::Str(&crate::core::keybinds::label(c)))])
                        }
                        None => t("desktop.overlay.usingNoKey"),
                    },
                    p,
                ))
                .child(note("info", t("desktop.overlay.exclusive"), p));
            rows.extend([
                ("overlay-show", t("desktop.overlay.show"), None, pref!(prefs, overlay_show), show),
                ("overlay-corner", t("desktop.overlay.corner"), None, pref!(prefs, overlay_corner), corner),
                ("overlay-opacity", t("desktop.overlay.opacity"), None, pref!(prefs, overlay_opacity), opacity),
                ("overlay-people", t("desktop.overlay.people"), None, pref!(prefs, overlay_speakers_only), people),
                (
                    "overlay-notifications",
                    t("desktop.overlay.messages"),
                    None,
                    pref!(prefs, overlay_notifications),
                    notes,
                ),
                ("overlay-key", t("desktop.overlay.key"), None, Badge::pref(false, |_| {}), key.into_any_element()),
            ]);
        }
        let count = rows.len();
        let mut list = div().flex().flex_col();
        for (n, (id, title, hint_text, badge, body)) in rows.into_iter().enumerate() {
            list = list.child(self.setting(
                id,
                &title,
                hint_text.map(|h| hint(h, p)),
                badge,
                At::of(n, count),
                body,
                p,
                cx,
            ));
        }
        list.into_any_element()
    }
}
