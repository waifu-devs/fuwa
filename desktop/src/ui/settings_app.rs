//! The smaller App settings pages, as the web's `settings/app/`:
//! Accessibility, Chat, Notifications, Streamer mode and Advanced (with the
//! anonymous reports that used to be the desktop's Privacy page).

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::config::{Clock, MotionChoice, NotifyFor, Prefs, RoleColors, SendWith, Sounds};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::motion;
use crate::ui::settings::{Page, SettingsView};
use crate::ui::settings_controls::{At, Badge, IconHover, Look, Opt, button_with, choice, keycaps, sample, toggle};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_lg, radius_xl};
use crate::ui::widgets::icon;

/// The changed badge for settings that are one field of the prefs.
macro_rules! pref {
    ($prefs:expr, $($field:ident).+) => {{
        let d = Prefs::default();
        Badge::pref($prefs.$($field).+ != d.$($field).+, move |pr: &mut Prefs| pr.$($field).+ = Prefs::default().$($field).+)
    }};
}
pub(crate) use pref;

fn percent(n: f32) -> String {
    format!("{}%", n.round() as i64)
}

/// A time of day as the clock setting would write it: the language's own, or 12 or 24 hours.
fn sample_time(clock: Clock) -> String {
    let twelve = match clock {
        Clock::H12 => true,
        Clock::H24 => false,
        Clock::Auto => {
            let (code, _) = crate::core::i18n::current();
            let lang = code.split(['-', '_']).next().unwrap_or("").to_owned();
            matches!(lang.as_str(), "en" | "ko" | "hi") && code != "en-GB"
        }
    };
    if twelve { "3:04 PM".to_owned() } else { "15:04".to_owned() }
}

impl SettingsView {
    pub(crate) fn accessibility_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = self.column;
        let calm = cx.reduce_motion();
        let motion_hint = match prefs.motion {
            MotionChoice::System if calm => t("appsettings.accessibility.motionSystemCalm"),
            MotionChoice::System => t("appsettings.accessibility.motionSystemMoves"),
            MotionChoice::Reduced => t("appsettings.accessibility.motionOff"),
            MotionChoice::Full => t("appsettings.accessibility.motionOn"),
        };
        let motion_at = match prefs.motion {
            MotionChoice::System => 0,
            MotionChoice::Reduced => 1,
            MotionChoice::Full => 2,
        };
        let motion = choice(
            "motion",
            Some(motion_at),
            vec![
                Opt::new(
                    t("appsettings.accessibility.motionSystem"),
                    t("appsettings.accessibility.motionSystemHint"),
                    "monitor",
                ),
                Opt::new(
                    t("appsettings.accessibility.motionReduce"),
                    t("appsettings.accessibility.motionReduceHint"),
                    "snail",
                ),
                Opt::new(
                    t("appsettings.accessibility.motionFull"),
                    t("appsettings.accessibility.motionFullHint"),
                    "sparkles",
                ),
            ],
            w,
            p,
            window,
            cx,
            |this, n, cx| {
                let m = [MotionChoice::System, MotionChoice::Reduced, MotionChoice::Full][n];
                this.set(cx, |pr| pr.motion = m)
            },
        );
        let saturation = self.slider(
            "saturation",
            (0.0, 100.0, 5.0),
            f32::from(prefs.saturation),
            vec![(0.0, percent(0.0)), (50.0, percent(50.0)), (100.0, percent(100.0))],
            percent,
            false,
            p,
            window,
            cx,
            |pr, v| pr.saturation = v.round() as u8,
        );
        let roles_at = match prefs.role_colors {
            RoleColors::Names => 0,
            RoleColors::Beside => 1,
            RoleColors::Off => 2,
        };
        let roles = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(choice(
                "role-colors",
                Some(roles_at),
                vec![
                    Opt::new(
                        t("appsettings.accessibility.roleNames"),
                        t("appsettings.accessibility.roleNamesHint"),
                        "palette",
                    ),
                    Opt::new(
                        t("appsettings.accessibility.roleBeside"),
                        t("appsettings.accessibility.roleBesideHint"),
                        "circle",
                    ),
                    Opt::new(
                        t("appsettings.accessibility.roleOff"),
                        t("appsettings.accessibility.roleOffHint"),
                        "circle-off",
                    ),
                ],
                w,
                p,
                window,
                cx,
                |this, n, cx| {
                    let r = [RoleColors::Names, RoleColors::Beside, RoleColors::Off][n];
                    this.set(cx, |pr| pr.role_colors = r)
                },
            ))
            .child(
                sample(p).flex().flex_wrap().gap_x(px(20.0)).gap_y(px(4.0)).children(
                    [
                        ("preview-sakura", "Sakura", 0xf472b6),
                        ("preview-ren", "Ren", 0x60a5fa),
                        ("preview-mio", "Mio", 0x34d399),
                    ]
                    .into_iter()
                    .map(|(id, name, color)| role_name(id, name, color, prefs.role_colors, p)),
                ),
            );
        let effects = toggle(
            "others-effects",
            &t("appsettings.accessibility.effectsToggle"),
            Some(&t("appsettings.accessibility.effectsToggleHint")),
            prefs.others_effects,
            false,
            p,
            window,
            cx,
            |this, on, cx| this.set(cx, |pr| pr.others_effects = on),
        );
        let link_text = t("appsettings.accessibility.linksSampleLink");
        let line = t_with("appsettings.accessibility.linksSample", &[("link", Arg::Str("\u{0}"))]);
        let (before, after) = line.split_once('\u{0}').unwrap_or((line.as_str(), ""));
        let links = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle(
                "underline-links",
                &t("appsettings.accessibility.linksToggle"),
                Some(&t("appsettings.accessibility.linksToggleHint")),
                prefs.underline_links,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.underline_links = on),
            ))
            .child(
                sample(p).child(
                    div()
                        .flex()
                        .flex_wrap()
                        .child(before.to_owned())
                        .child(
                            div()
                                .id("links-sample")
                                .text_color(p.primary)
                                .when(prefs.underline_links, |el| el.underline())
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(|_, _, cx| cx.open_url("https://github.com/waifu-devs/fuwa"))
                                .child(link_text),
                        )
                        .child(after.to_owned()),
                ),
            );
        let rows = [
            ("reduce-motion", t("appsettings.accessibility.motion"), Some(motion_hint), pref!(prefs, motion), motion),
            (
                "saturation",
                t("appsettings.accessibility.saturation"),
                Some(t("appsettings.accessibility.saturationHint")),
                pref!(prefs, saturation),
                saturation,
            ),
            (
                "role-colors",
                t("appsettings.accessibility.roleColors"),
                Some(t("appsettings.accessibility.roleColorsHint")),
                pref!(prefs, role_colors),
                roles.into_any_element(),
            ),
            (
                "others-effects",
                t("appsettings.accessibility.effects"),
                Some(if calm {
                    t("appsettings.accessibility.effectsStill")
                } else {
                    t("appsettings.accessibility.effectsHint")
                }),
                pref!(prefs, others_effects),
                effects,
            ),
            (
                "underline-links",
                t("appsettings.accessibility.links"),
                None,
                pref!(prefs, underline_links),
                links.into_any_element(),
            ),
        ];
        self.stack(rows, p, cx)
    }

    /// Settings one under another, each under a rule.
    pub(crate) fn stack<const N: usize>(
        &self,
        rows: [(&'static str, String, Option<String>, Badge, AnyElement); N],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_col();
        for (n, (id, title, hint, badge, body)) in rows.into_iter().enumerate() {
            list = list.child(self.setting(
                id,
                &title,
                hint.map(|h| crate::ui::settings_controls::hint(h, p)),
                badge,
                At::of(n, N),
                body,
                p,
                cx,
            ));
        }
        list.into_any_element()
    }

    pub(crate) fn chat_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = self.column;
        let clock_at = match prefs.clock {
            Clock::Auto => 0,
            Clock::H12 => 1,
            Clock::H24 => 2,
        };
        let clock = choice(
            "clock",
            Some(clock_at),
            vec![
                Opt::new(t("appsettings.chat.clockAuto"), sample_time(Clock::Auto), "monitor"),
                Opt::new(t("appsettings.chat.clock12"), sample_time(Clock::H12), "clock"),
                Opt::new(t("appsettings.chat.clock24"), sample_time(Clock::H24), "clock"),
            ],
            w,
            p,
            window,
            cx,
            |this, n, cx| {
                let c = [Clock::Auto, Clock::H12, Clock::H24][n];
                this.set(cx, |pr| pr.clock = c)
            },
        );
        let label = crate::core::keybinds::label;
        let send = choice(
            "send-with",
            Some(if prefs.send_with == SendWith::Enter { 0 } else { 1 }),
            vec![
                Opt::new(
                    label("Enter"),
                    t_with("appsettings.chat.newLine", &[("keys", Arg::Str(&label("Shift+Enter")))]),
                    "send-horizontal",
                ),
                Opt::new(
                    label("Mod+Enter"),
                    t_with("appsettings.chat.newLineMarkdown", &[("keys", Arg::Str(&label("Enter")))]),
                    "corner-down-left",
                ),
            ],
            w,
            p,
            window,
            cx,
            |this, n, cx| {
                let s = if n == 0 { SendWith::Enter } else { SendWith::ModEnter };
                this.set(cx, |pr| pr.send_with = s)
            },
        );
        self.stack(
            [
                (
                    "clock",
                    t("appsettings.chat.clock"),
                    Some(t("appsettings.chat.clockHint")),
                    pref!(prefs, clock),
                    clock,
                ),
                (
                    "send-with",
                    t("appsettings.chat.sendWith"),
                    Some(t("appsettings.chat.sendWithHint")),
                    pref!(prefs, send_with),
                    send,
                ),
            ],
            p,
            cx,
        )
    }

    pub(crate) fn notifications_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = self.column;
        let quiet = prefs.streamer_mode && (prefs.streamer_mute_notifications || prefs.streamer_mute_sounds);
        let banner = quiet.then(|| {
            let line = if prefs.streamer_mute_notifications && prefs.streamer_mute_sounds {
                t("appsettings.notifications.streamerQuietBoth")
            } else if prefs.streamer_mute_sounds {
                t("appsettings.notifications.streamerQuietSounds")
            } else {
                t("appsettings.notifications.streamerQuietNotifications")
            };
            motion::rise(
                div()
                    .id("streamer-quiet")
                    .mb(px(40.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.primary, 0.1))
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_sm()
                    .text_color(p.primary)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.choose(Page::Streamer, None, cx)))
                    .child(icon("tv-minimal-play").size(px(16.0)))
                    .child(line),
                "streamer-quiet-in",
                Duration::ZERO,
                -8.0,
            )
        });
        let desktop = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle(
                "desktop-notifications",
                &t("appsettings.notifications.desktopToggle"),
                Some(&t("desktop.settings.notificationsHint")),
                prefs.notifications,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.notifications = on),
            ))
            .when(prefs.notifications, |el| {
                el.child(motion::rise(
                    div().flex().child(
                        button_with(
                            "notify-test",
                            t("appsettings.notifications.test"),
                            Some("bell-ring"),
                            IconHover::Turn(12.0),
                            Look::Outline,
                            true,
                            p,
                        )
                        .rounded(radius_xl())
                        .on_click(|_, _, _| {
                            crate::ui::notify::show(
                                "fuwa".into(),
                                t("workspace.notify.test"),
                                crate::ui::notify::Clicked {
                                    instance: String::new(),
                                    server: None,
                                    channel: String::new(),
                                    thread: None,
                                },
                            )
                        }),
                    ),
                    "notify-test-in",
                    Duration::ZERO,
                    -8.0,
                ))
            });
        let notify_for = choice(
            "notify-for",
            Some(if prefs.notify_for == NotifyFor::Mentions { 0 } else { 1 }),
            vec![
                Opt::new(
                    t("appsettings.notifications.mentions"),
                    t("appsettings.notifications.mentionsHint"),
                    "at-sign",
                ),
                Opt::new(t("appsettings.notifications.all"), t("appsettings.notifications.allHint"), "messages-square"),
            ],
            w,
            p,
            window,
            cx,
            |this, n, cx| {
                let v = if n == 0 { NotifyFor::Mentions } else { NotifyFor::All };
                this.set(cx, |pr| pr.notify_for = v)
            },
        );
        let badge = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle(
                "unread-badge",
                &t("appsettings.notifications.unreadBadgeToggle"),
                Some(&t("appsettings.notifications.unreadBadgeToggleHint")),
                prefs.unread_badge,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.unread_badge = on),
            ))
            .child(tab_preview(prefs.unread_badge, p));
        use crate::core::sounds::Sound;
        type Put = fn(&mut Sounds, bool);
        let kinds: [(Sound, &str, &str, bool, Put); 4] = [
            (
                Sound::Message,
                "appsettings.notifications.soundMessage",
                "appsettings.notifications.soundMessageHint",
                prefs.sounds.message,
                |s, v| s.message = v,
            ),
            (
                Sound::Mention,
                "appsettings.notifications.soundMention",
                "appsettings.notifications.soundMentionHint",
                prefs.sounds.mention,
                |s, v| s.mention = v,
            ),
            (Sound::Dm, "desktop.sounds.dm", "desktop.sounds.dmHint", prefs.sounds.dm, |s, v| s.dm = v),
            (
                Sound::Join,
                "appsettings.notifications.soundJoin",
                "appsettings.notifications.soundJoinHint",
                prefs.sounds.join,
                |s, v| s.join = v,
            ),
        ];
        let mut sounds = div().flex().flex_col().gap(px(12.0));
        for (sound, label, hint, on, put) in kinds {
            sounds = sounds.child(self.sound_row(
                sound,
                t(label),
                Some(t(hint)),
                Some((on, put)),
                false,
                prefs,
                p,
                window,
                cx,
            ));
        }
        sounds = sounds.child(crate::ui::settings_controls::hint(t("desktop.sounds.filesHint"), p));
        let volume = self.slider(
            "volume",
            (0.0, 100.0, 5.0),
            f32::from(prefs.volume),
            Vec::new(),
            percent,
            false,
            p,
            window,
            cx,
            |pr, v| pr.volume = v.round() as u8,
        );
        sounds = sounds.child(
            div()
                .pt(px(4.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(div().text_color(p.muted_foreground).child(icon("volume-2").size(px(20.0))))
                .child(div().flex_1().child(volume)),
        );
        const MESSAGE_SOUNDS: [Sound; 4] = [Sound::Message, Sound::Mention, Sound::Dm, Sound::Join];
        let d = Sounds::default();
        let sounds_changed = (prefs.sounds.message, prefs.sounds.mention, prefs.sounds.dm, prefs.sounds.join)
            != (d.message, d.mention, d.dm, d.join)
            || prefs.volume != Prefs::default().volume
            || crate::ui::settings_sounds::picks_changed(prefs, &MESSAGE_SOUNDS);
        let list = self.stack(
            [
                (
                    "desktop-notifications",
                    t("appsettings.notifications.desktop"),
                    None,
                    pref!(prefs, notifications),
                    desktop.into_any_element(),
                ),
                ("notify-for", t("appsettings.notifications.notifyFor"), None, pref!(prefs, notify_for), notify_for),
                (
                    "unread-badge",
                    t("appsettings.notifications.unreadBadge"),
                    None,
                    pref!(prefs, unread_badge),
                    badge.into_any_element(),
                ),
                (
                    "sounds",
                    t("appsettings.notifications.sounds"),
                    None,
                    Badge::pref(sounds_changed, |pr| {
                        let d = Sounds::default();
                        (pr.sounds.message, pr.sounds.mention, pr.sounds.dm, pr.sounds.join) =
                            (d.message, d.mention, d.dm, d.join);
                        pr.volume = Prefs::default().volume;
                        crate::ui::settings_sounds::reset_picks(pr, &MESSAGE_SOUNDS);
                    }),
                    sounds.into_any_element(),
                ),
            ],
            p,
            cx,
        );
        div().flex().flex_col().children(banner).child(list).into_any_element()
    }

    pub(crate) fn streamer_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = prefs.streamer_mode;
        let combo = crate::core::keybinds::action_by_id("toggleStreamer")
            .and_then(|a| crate::core::keybinds::binding_of(a, &prefs.keybinds));
        let card_hover = alpha(p.primary, 0.3);
        let card = div()
            .id("streamer-card")
            .flex()
            .items_center()
            .gap(px(16.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(if on { alpha(p.primary, 0.6) } else { p.border.into() })
            .when(on, |el| el.bg(alpha(p.primary, 0.05)))
            .when(!on, |el| el.hover(move |s| s.border_color(card_hover)))
            .p(px(16.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.set(cx, |pr| pr.streamer_mode = !on)))
            .child(motion::once(
                div()
                    .size(px(44.0))
                    .flex_none()
                    .rounded(radius_xl())
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|el| {
                        if on {
                            el.bg(p.primary).text_color(p.primary_foreground)
                        } else {
                            el.bg(p.muted).text_color(p.muted_foreground)
                        }
                    })
                    .child(icon("tv-minimal-play").size(px(20.0))),
                SharedString::from(format!("streamer-icon-{on}")),
                Duration::from_millis(500),
                move |el, t| {
                    if on {
                        let lift = (t * std::f32::consts::PI).sin();
                        el.relative().top(px(-3.0 * lift))
                    } else {
                        el
                    }
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::BOLD).child(if on {
                        t("appsettings.streamer.on")
                    } else {
                        t("appsettings.streamer.off")
                    }))
                    .child(div().text_sm().text_color(p.muted_foreground).child(t("appsettings.streamer.hint"))),
            )
            .child(crate::ui::settings_controls::switch("streamer-switch", on, false, p, window, cx, |this, v, cx| {
                this.set(cx, |pr| pr.streamer_mode = v)
            }));
        let shortcut = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .text_sm()
            .text_color(p.muted_foreground)
            .child(icon("keyboard").size(px(16.0)))
            .map(|el| match &combo {
                Some(combo) => {
                    let line = t_with("appsettings.streamer.shortcut", &[("keys", Arg::Str("\u{0}"))]);
                    let (a, b) = line
                        .split_once('\u{0}')
                        .map(|(a, b)| (a.to_owned(), b.to_owned()))
                        .unwrap_or((line, String::new()));
                    el.child(a).child(keycaps(combo, p)).child(b)
                }
                None => el.child(t("appsettings.streamer.noShortcut")).child(
                    div()
                        .id("streamer-add-shortcut")
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(|this, _, _, cx| this.choose(Page::Keybinds, None, cx)))
                        .child(t("appsettings.streamer.addShortcut")),
                ),
            });
        let mode = div().flex().flex_col().gap(px(12.0)).child(card).child(shortcut);
        let hide = toggle(
            "streamer-hide-personal",
            &t("appsettings.streamer.hidePersonalToggle"),
            Some(&t("desktop.settings.streamerHidePersonalHint")),
            prefs.streamer_hide_personal,
            false,
            p,
            window,
            cx,
            |this, on, cx| this.set(cx, |pr| pr.streamer_hide_personal = on),
        );
        let sounds = toggle(
            "streamer-sounds",
            &t("appsettings.streamer.muteSounds"),
            Some(&t("appsettings.streamer.muteSoundsHint")),
            prefs.streamer_mute_sounds,
            false,
            p,
            window,
            cx,
            |this, on, cx| this.set(cx, |pr| pr.streamer_mute_sounds = on),
        );
        let notes = toggle(
            "streamer-notifications",
            &t("appsettings.streamer.muteNotifications"),
            Some(&t("appsettings.streamer.muteNotificationsHint")),
            prefs.streamer_mute_notifications,
            false,
            p,
            window,
            cx,
            |this, on, cx| this.set(cx, |pr| pr.streamer_mute_notifications = on),
        );
        let list = self.stack(
            [
                (
                    "streamer",
                    t("appsettings.streamer.title"),
                    None,
                    pref!(prefs, streamer_mode),
                    mode.into_any_element(),
                ),
                (
                    "streamer-hide-personal",
                    t("appsettings.streamer.hidePersonal"),
                    None,
                    pref!(prefs, streamer_hide_personal),
                    hide,
                ),
                (
                    "streamer-sounds",
                    t("appsettings.notifications.sounds"),
                    None,
                    pref!(prefs, streamer_mute_sounds),
                    sounds,
                ),
                (
                    "streamer-notifications",
                    t("settings.nav.notifications"),
                    None,
                    pref!(prefs, streamer_mute_notifications),
                    notes,
                ),
            ],
            p,
            cx,
        );
        let preview = self.stream_preview(on, p);
        crate::ui::settings_controls::with_preview(list, preview, self.wide, p)
    }

    /// What a stream would see: the address, your name and an instance's tooltip, live.
    fn stream_preview(&mut self, on: bool, p: &Palette) -> AnyElement {
        let me = self.account_me();
        let hidden = on && self.core.prefs().streamer_hide_personal;
        let dot = |c: u32, a: f32| div().size(px(10.0)).rounded_full().bg(alpha(rgb(c), a));
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .rounded(crate::ui::theme::radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(16.0))
            .shadow(crate::ui::settings_controls::shadow_lg())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(dot(0xef4444, 0.7))
                    .child(dot(0xfbbf24, 0.7))
                    .child(dot(0x34d399, 0.7))
                    .when(on, |el| {
                        el.child(
                            div().ml_auto().child(motion::once(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .rounded_full()
                                    .bg(p.destructive)
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .text_size(px(9.6))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .text_color(rgb(0xffffff))
                                    .child(div().size(px(6.0)).rounded_full().bg(rgb(0xffffff)))
                                    .child(t("appsettings.streamer.live").to_uppercase()),
                                "stream-live",
                                Duration::from_millis(260),
                                |el, t| el.opacity(t),
                            )),
                        )
                    }),
            );
        if let Some((key, me)) = me {
            let place = self.place(&key);
            card = card
                .child(
                    div()
                        .rounded(radius_lg())
                        .bg(p.muted)
                        .px(px(10.0))
                        .py(px(6.0))
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .truncate()
                        .child(if hidden { "fuwa://…".to_owned() } else { format!("{key}/…") }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_xl())
                        .bg(alpha(p.muted, 0.5))
                        .p(px(8.0))
                        .child(crate::ui::widgets::avatar(Some(&me), 32.0, p))
                        .child(
                            div()
                                .min_w_0()
                                .text_sm()
                                .child(
                                    div()
                                        .font_weight(FontWeight::BOLD)
                                        .truncate()
                                        .child(crate::core::store::user_name(&me)),
                                )
                                .child(div().text_xs().text_color(p.muted_foreground).truncate().child(if hidden {
                                    "@••••••".to_owned()
                                } else {
                                    format!("@{}", me.username)
                                })),
                        ),
                )
                .child(
                    div().flex().child(
                        div()
                            .rounded(radius_lg())
                            .bg(p.card)
                            .px(px(10.0))
                            .py(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .shadow(crate::ui::settings_controls::shadow_sm())
                            .child(format!("{place} · {}", if hidden { "••••••".to_owned() } else { key.clone() })),
                    ),
                );
        }
        card.into_any_element()
    }

    pub(crate) fn advanced_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = prefs.developer_mode;
        let reports = self.reports_section(prefs, p, window, cx);
        let developer = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle(
                "developer-mode",
                &t("appsettings.advanced.developerToggle"),
                Some(&t("appsettings.advanced.developerToggleHint")),
                on,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.developer_mode = on),
            ))
            .child(
                sample(p)
                    .py(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("#{}", t("appsettings.preview.general"))),
                            )
                            .child(
                                div()
                                    .text_color(p.muted_foreground)
                                    .truncate()
                                    .child(format!("· {}", t("appsettings.advanced.developerSample"))),
                            ),
                    )
                    .when(on, |el| {
                        el.child(motion::slide_in(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(radius_lg())
                                .bg(p.background)
                                .shadow(crate::ui::settings_controls::shadow_sm())
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .child(icon("fingerprint-pattern").size(px(14.0)).text_color(p.primary))
                                .child(t("appsettings.advanced.copyChannelId")),
                            "dev-preview",
                            8.0,
                        ))
                    }),
            );
        self.stack(
            [
                ("share-reports", t("appsettings.advanced.reports"), None, pref!(prefs, share_reports), reports),
                (
                    "developer-mode",
                    t("appsettings.advanced.developer"),
                    None,
                    pref!(prefs, developer_mode),
                    developer.into_any_element(),
                ),
            ],
            p,
            cx,
        )
    }
}

/// A role-colored name, as the Role colors setting shows it (the web's `RoleName`).
fn role_name(id: &str, name: &str, color: u32, mode: RoleColors, p: &Palette) -> AnyElement {
    let tint = crate::ui::widgets::name_tint(id, p);
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(if mode == RoleColors::Names { rgb(color).into() } else { tint })
                .child(name.to_owned()),
        )
        .when(mode == RoleColors::Beside, |el| {
            el.child(div().size(px(8.0)).rounded_full().border_2().border_color(p.background).bg(rgb(color)))
        })
        .into_any_element()
}

/// A browser tab with fuwa in it, showing where the count goes (the web's `TabPreview`).
fn tab_preview(on: bool, p: &Palette) -> AnyElement {
    div()
        .flex()
        .items_end()
        .gap(px(4.0))
        .rounded(radius_xl())
        .bg(alpha(p.muted, 0.6))
        .px(px(12.0))
        .pt(px(12.0))
        .child(
            div()
                .w(px(224.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded_t(radius_lg())
                .bg(p.background)
                .px(px(12.0))
                .py(px(8.0))
                .text_xs()
                .shadow(crate::ui::settings_controls::shadow_sm())
                .child(div().relative().flex_none().child(crate::ui::widgets::fuwa_mark(16.0, p)).when(on, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(-6.0))
                            .right(px(-6.0))
                            .size(px(12.0))
                            .rounded_full()
                            .bg(p.destructive)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(8.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(rgb(0xffffff))
                            .child("3"),
                    )
                }))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .truncate()
                        .when(on, |el| el.child(div().font_weight(FontWeight::BOLD).child("(3)\u{a0}")))
                        .child(format!("#{} · Waifu Devs", t("appsettings.preview.general"))),
                ),
        )
        .child(
            div()
                .w(px(96.0))
                .rounded_t(radius_lg())
                .bg(alpha(p.background, 0.4))
                .px(px(12.0))
                .py(px(8.0))
                .text_xs()
                .text_color(p.muted_foreground)
                .child(t("appsettings.notifications.otherTab")),
        )
        .into_any_element()
}
