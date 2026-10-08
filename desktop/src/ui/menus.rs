//! The bells: how a channel, or a whole server, notifies you. The settings
//! live on the instance, so they follow you to every device. Like the web
//! app's `NotificationBell.tsx`. And your status menu, like `UserPanel.tsx`.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::core::account::NotificationPatch;
use crate::core::dms::now_ms;
use crate::core::i18n::t;
use crate::pb::{self, NotificationLevel as Level};
use crate::ui::app::FuwaApp;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{card, icon, pal};

const LEVELS: [(Level, &str); 3] =
    [(Level::All, "All messages"), (Level::Mentions, "Only @mentions"), (Level::Nothing, "Nothing")];

/// How long a mute can last, like Discord's menu. `None` is until you turn it back on.
pub(crate) const MUTE_FOR: [(&str, Option<i64>); 6] = [
    ("For 15 minutes", Some(15 * 60_000)),
    ("For 1 hour", Some(60 * 60_000)),
    ("For 3 hours", Some(3 * 60 * 60_000)),
    ("For 8 hours", Some(8 * 60 * 60_000)),
    ("For 24 hours", Some(24 * 60 * 60_000)),
    ("Until I turn it back on", None),
];

/// "Muted until 16:30", "Muted until Tue 09:00", or "Muted".
pub(crate) fn muted_label(n: Option<&pb::NotificationSettings>) -> String {
    use chrono::TimeZone as _;
    let Some(until) = n.and_then(|n| n.muted_until.as_ref()) else { return "Muted".into() };
    let Some(at) = chrono::Local.timestamp_millis_opt(until.seconds * 1000).single() else { return "Muted".into() };
    if at.date_naive() == chrono::Local::now().date_naive() {
        format!("Muted until {}", at.format("%H:%M"))
    } else {
        format!("Muted until {}", at.format("%a %H:%M"))
    }
}

impl FuwaApp {
    /// The menu under a server's bell, in the sidebar.
    pub(crate) fn server_bell_menu(&mut self, key: &str, server: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (own, default, name) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, Level::Unspecified, String::new()) };
            (
                i.notification_settings(server, "").cloned(),
                i.server(server).map(|s| s.default_notifications()).unwrap_or(Level::Unspecified),
                i.server(server).map(|s| s.name.clone()).unwrap_or_default(),
            )
        });
        let level = own.as_ref().map(|n| n.level()).unwrap_or(Level::Unspecified);
        let muted = crate::core::notifications::is_muted(own.as_ref(), now_ms());
        let suppress = own.as_ref().is_some_and(|n| n.suppress_everyone);
        let default_label = match default {
            Level::All => "Server default (all messages)",
            Level::Mentions => "Server default (@mentions)",
            Level::Nothing => "Server default (nothing)",
            _ => "This computer's setting",
        };
        let mut body = menu_title(&format!("Notifications for {name}"), &p);
        body = body.child(section("Notify me about", &p));
        body = body.child(self.level_item(key, server, "", Level::Unspecified, default_label, level, &p, cx));
        for (l, label) in LEVELS {
            body = body.child(self.level_item(key, server, "", l, label, level, &p, cx));
        }
        body = body.child(div().h(px(1.0)).my(px(6.0)).bg(p.border));
        body = body.child(self.item(
            "suppress",
            if suppress { "check" } else { "at-sign" },
            "Ignore @everyone and @here",
            suppress,
            key,
            server,
            "",
            NotificationPatch { suppress_everyone: Some(!suppress), ..NotificationPatch::default() },
            &p,
            cx,
        ));
        body = body.child(div().h(px(1.0)).my(px(6.0)).bg(p.border));
        body = self.mute_items(body, key, server, "", muted, own.as_ref(), &p, cx);
        float_left(body, "menu-server", &p, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn level_item(
        &self,
        key: &str,
        server: &str,
        channel: &str,
        value: Level,
        label: &str,
        current: Level,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let on = value == current;
        self.item(
            &format!("level-{}", value as i32),
            if on { "circle-check" } else { "circle" },
            label,
            on,
            key,
            server,
            channel,
            NotificationPatch { level: Some(value), ..NotificationPatch::default() },
            p,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn mute_items(
        &self,
        mut body: gpui_kit::Div,
        key: &str,
        server: &str,
        channel: &str,
        muted: bool,
        own: Option<&pb::NotificationSettings>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        if muted {
            body = body.child(note(&muted_label(own), p)).child(self.item(
                "unmute",
                "bell-ring",
                "Unmute",
                false,
                key,
                server,
                channel,
                NotificationPatch { unmute: true, ..NotificationPatch::default() },
                p,
                cx,
            ));
            return body;
        }
        body = body.child(section(if channel.is_empty() { "Mute the server" } else { "Mute the channel" }, p));
        for (n, (label, ms)) in MUTE_FOR.into_iter().enumerate() {
            body = body.child(self.item(
                &format!("mute-{n}"),
                "bell-off",
                label,
                false,
                key,
                server,
                channel,
                NotificationPatch { mute_until: Some(ms.map(|ms| now_ms() + ms)), ..NotificationPatch::default() },
                p,
                cx,
            ));
        }
        body
    }

    /// One choice: it saves at once, and the menu closes.
    #[allow(clippy::too_many_arguments)]
    fn item(
        &self,
        id: &str,
        glyph: &str,
        label: &str,
        on: bool,
        key: &str,
        server: &str,
        channel: &str,
        patch: NotificationPatch,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let hover = alpha(p.primary, 0.1);
        let (key, server, channel) = (key.to_owned(), server.to_owned(), channel.to_owned());
        div()
            .id(SharedString::from(format!("menu-{id}")))
            .h(px(34.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .rounded(corner(10.0))
            .cursor_pointer()
            .text_sm()
            .when(on, |el| el.text_color(p.primary).font_weight(FontWeight::BOLD))
            .hover(move |s| s.bg(hover))
            .active(|s| s.top(px(1.0)))
            .on_click(
                cx.listener(move |this, _, _, cx| this.save_notifications(&key, &server, &channel, patch.clone(), cx)),
            )
            .child(icon(glyph).size(px(16.0)))
            .child(label.to_owned())
    }
}

impl FuwaApp {
    /// The bell in a channel's header (`NotificationBell`): amber while the
    /// channel or its server is muted, with its menu dropping down under it.
    pub(crate) fn bell_button(&mut self, key: &str, server: &str, channel: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let now = now_ms();
        let (channel_muted, server_muted) = self.core.shared.read(|s| {
            s.instance(key).map_or((false, false), |i| {
                (
                    crate::core::notifications::is_muted(i.notification_settings(server, channel), now),
                    crate::core::notifications::is_muted(i.notification_settings(server, ""), now),
                )
            })
        });
        let muted = channel_muted || server_muted;
        let menu = crate::ui::app::Menu::Channel {
            key: key.to_owned(),
            server: server.to_owned(),
            channel: channel.to_owned(),
        };
        let open = self.menu.as_ref() == Some(&menu);
        let hover = p.muted;
        let amber = gpui_kit::rgb(0xf59e0b);
        let toggle = menu.clone();
        let button = div()
            .id("bell")
            .size(px(36.0))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(if muted { amber } else { p.muted_foreground })
            .when(open, |el| el.bg(p.muted))
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.menu = if this.menu.as_ref() == Some(&toggle) { None } else { Some(toggle.clone()) };
                this.msg_ui.bell_sub = false;
                cx.notify();
            }))
            .child(motion::once(
                icon(if muted { "bell-off" } else { "bell" }).size(px(20.0)),
                SharedString::from(format!("bell-ring|{muted}")),
                Duration::from_millis(600),
                |el, t| {
                    // Rings when the mute flips: a swing that settles.
                    let k = [0.0, -22.0, 18.0, -12.0, 8.0, -4.0, 0.0];
                    let x = t * 6.0;
                    let n = (x.floor() as usize).min(5);
                    let deg = k[n] + (k[n + 1] - k[n]) * (x - n as f32);
                    el.rotate(gpui_kit::radians(deg.to_radians()))
                },
            ));
        let card = open.then(|| self.bell_card(key, server, channel, channel_muted, server_muted, &p, cx));
        div()
            .relative()
            .flex_none()
            .child(button)
            .when_some(card, |el, card| {
                let away = div()
                    .id("bell-away")
                    .absolute()
                    .top(px(-4000.0))
                    .left(px(-4000.0))
                    .size(px(12000.0))
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }));
                el.child(
                    gpui_kit::deferred(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .child(away)
                            .child(div().absolute().top(px(41.0)).right_0().child(card)),
                    )
                    .with_priority(1),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn bell_card(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        channel_muted: bool,
        server_muted: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = now_ms();
        let (own, server_own, server_default, name) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, None, Level::Unspecified, String::new()) };
            (
                i.notification_settings(server, channel).cloned(),
                i.notification_settings(server, "").cloned(),
                i.server(server).map(|s| s.default_notifications()).unwrap_or(Level::Unspecified),
                i.channel(server, channel).map(|c| c.name.clone()).unwrap_or_default(),
            )
        });
        let level = own.as_ref().map(|n| n.level()).unwrap_or(Level::Unspecified);
        // What "Use the server's" means here: the server's own setting, its default, or this device.
        let server_level = match server_own.as_ref().map(|n| n.level()) {
            Some(Level::All) => Some(t("common.notify.all")),
            Some(Level::Mentions) => Some(t("common.notify.mentions")),
            Some(Level::Nothing) => Some(t("common.notify.nothing")),
            _ if server_default == Level::Mentions => Some(t("chat.bell.mentionsDefault")),
            _ => None,
        };
        let save = |patch: NotificationPatch| {
            let (key, server, channel) = (key.to_owned(), server.to_owned(), channel.to_owned());
            cx.listener(move |this: &mut FuwaApp, _: &gpui_kit::ClickEvent, _, cx| {
                this.msg_ui.bell_sub = false;
                this.save_notifications(&key, &server, &channel, patch.clone(), cx)
            })
        };
        let close_sub = cx.listener(|this: &mut FuwaApp, on: &bool, _, cx| {
            if *on && this.msg_ui.bell_sub {
                this.msg_ui.bell_sub = false;
                cx.notify();
            }
        });
        let close_sub = std::rc::Rc::new(close_sub);
        let mut body = div().flex().flex_col().child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.muted_foreground)
                .truncate()
                .child(format!("#{name}")),
        );
        if channel_muted {
            let until = mute_until(own.as_ref());
            let hint = if until.is_empty() {
                String::new()
            } else {
                crate::core::i18n::t_with("common.notify.until", &[("time", crate::core::i18n::Arg::Str(&until))])
            };
            let c = close_sub.clone();
            body = body.child(
                drop_item("bell-unmute", Some("bell"), t("chat.bell.unmute"), p)
                    .on_hover(move |on, w, cx| c(on, w, cx))
                    .on_click(save(NotificationPatch { unmute: true, ..NotificationPatch::default() }))
                    .when(!hint.is_empty(), |el| {
                        el.child(
                            div().ml_auto().pl(px(8.0)).truncate().text_xs().text_color(p.muted_foreground).child(hint),
                        )
                    }),
            );
        } else {
            let sub_open = self.msg_ui.bell_sub;
            let mut trigger = drop_item("bell-mute", Some("bell-off"), t("chat.bell.mute"), p)
                .on_hover(cx.listener(|this, on: &bool, _, cx| {
                    if *on && !this.msg_ui.bell_sub {
                        this.msg_ui.bell_sub = true;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.msg_ui.bell_sub = !this.msg_ui.bell_sub;
                    cx.notify();
                }))
                .child(
                    div().ml_auto().child(
                        icon("chevron-right")
                            .size(px(16.0))
                            .text_color(p.muted_foreground)
                            .when(sub_open, |el| el.rotate(gpui_kit::radians(std::f32::consts::FRAC_PI_2))),
                    ),
                );
            if sub_open {
                trigger = trigger.bg(p.accent);
                let mut sub = div().flex().flex_col();
                for (n, (_, ms)) in MUTE_FOR.into_iter().enumerate() {
                    let label = match ms {
                        None => t("common.notify.forever"),
                        Some(ms) if ms < 60 * 60_000 => crate::core::i18n::t_with(
                            "common.notify.forMinutes",
                            &[("count", crate::core::i18n::Arg::Num(ms / 60_000))],
                        ),
                        Some(ms) => crate::core::i18n::t_with(
                            "common.notify.forHours",
                            &[("count", crate::core::i18n::Arg::Num(ms / 3_600_000))],
                        ),
                    };
                    sub = sub.child(drop_item(SharedString::from(format!("bell-mute-{n}")), None, label, p).on_click(
                        save(NotificationPatch {
                            mute_until: Some(ms.map(|ms| now + ms)),
                            ..NotificationPatch::default()
                        }),
                    ));
                }
                trigger = trigger.child(div().absolute().top(px(0.0)).left(gpui_kit::relative(1.0)).ml(px(1.0)).child(
                    motion::rise(
                        menu_frame(208.0, p).shadow(crate::ui::chat_rows::shadow_lg()).child(sub),
                        "bell-sub-in",
                        Duration::ZERO,
                        -4.0,
                    ),
                ));
            }
            body = body.child(trigger.relative());
        }
        if server_muted {
            let until = mute_until(server_own.as_ref());
            body = body.child(
                div()
                    .px(px(8.0))
                    .pb(px(4.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(gpui_kit::rgb(0xf59e0b))
                    .child(if until.is_empty() {
                        t("common.notify.wholeServerMuted")
                    } else {
                        crate::core::i18n::t_with(
                            "common.notify.wholeServerMutedUntil",
                            &[("time", crate::core::i18n::Arg::Str(&until))],
                        )
                    }),
            );
        }
        body = body.child(separator(p));
        let levels = [
            (Level::Unspecified, t("chat.bell.useServer")),
            (Level::All, t("common.notify.all")),
            (Level::Mentions, t("common.notify.mentions")),
            (Level::Nothing, t("common.notify.nothing")),
        ];
        for (value, label) in levels {
            let on = value == level;
            let c = close_sub.clone();
            let item = drop_item(SharedString::from(format!("bell-level-{}", value as i32)), None, String::new(), p)
                .pl(px(32.0))
                .on_hover(move |on, w, cx| c(on, w, cx))
                .on_click(save(NotificationPatch { level: Some(value), ..NotificationPatch::default() }))
                .when(on, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(px(8.0))
                            .size(px(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(px(8.0)).rounded_full().bg(p.foreground)),
                    )
                })
                .child(if value == Level::Unspecified {
                    div()
                        .min_w_0()
                        .child(div().child(label))
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(server_level.clone().unwrap_or_else(|| t("chat.bell.deviceDecides"))),
                        )
                        .into_any_element()
                } else {
                    div().child(label).into_any_element()
                });
            body = body.child(item.relative());
        }
        body = body.child(separator(p));
        let c = close_sub.clone();
        body = body.child(
            drop_item("bell-settings", Some("settings"), t("chat.bell.settings"), p)
                .on_hover(move |on, w, cx| c(on, w, cx))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.menu = None;
                    this.open_settings(window, cx);
                    if let Some(view) = &this.settings {
                        view.update(cx, |view, cx| {
                            view.page = crate::ui::settings::Page::Notifications;
                            cx.notify();
                        });
                    }
                    cx.notify();
                })),
        );
        motion::rise(
            menu_frame(256.0, p).id("bell-menu").occlude().on_click(|_, _, cx| cx.stop_propagation()).child(body),
            "bell-menu-in",
            Duration::ZERO,
            -6.0,
        )
        .into_any_element()
    }
}

/// When a timed mute runs out: "4:30 PM", or "Tue 9:00 AM" on another day; "" when it lasts until turned off.
fn mute_until(n: Option<&pb::NotificationSettings>) -> String {
    use chrono::TimeZone as _;
    let Some(until) = n.and_then(|n| n.muted_until.as_ref()) else { return String::new() };
    let ms = until.seconds * 1000;
    let Some(at) = chrono::Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let time = crate::ui::text::clock(ms);
    if at.date_naive() == chrono::Local::now().date_naive() { time } else { format!("{} {time}", at.format("%a")) }
}

/// A dropdown's card (the web's `DropdownMenuContent`): popover colour, a border, 4px in.
fn menu_frame(width: f32, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(SharedString::from(format!("menu-frame-{width}")))
        .w(px(width))
        .p(px(4.0))
        .rounded(crate::ui::theme::radius_md())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .text_color(p.foreground)
        .shadow(crate::ui::chat_rows::shadow_md())
}

/// One line of a dropdown (`DropdownMenuItem`): a 16px icon, words, the accent under the pointer.
fn drop_item(
    id: impl Into<SharedString>,
    glyph: Option<&str>,
    label: String,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = p.accent;
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(crate::ui::theme::radius_sm())
        .text_sm()
        .line_height(px(20.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when_some(glyph, |el, g| el.child(icon(g).size(px(16.0)).text_color(p.muted_foreground)))
        .when(!label.is_empty(), |el| el.child(label))
}

fn separator(p: &Palette) -> impl IntoElement {
    div().mx(px(-4.0)).my(px(4.0)).h(px(1.0)).bg(p.border)
}

// Kept for code that names a status in English; the menus use `user_menu::status_name`.
#[allow(dead_code)]
pub fn status_label(status: pb::PresenceStatus) -> &'static str {
    match status {
        pb::PresenceStatus::Idle => "Idle",
        pb::PresenceStatus::DoNotDisturb => "Do not disturb",
        pb::PresenceStatus::Invisible => "Invisible",
        pb::PresenceStatus::Offline => "Offline",
        _ => "Online",
    }
}

/// The dot for a status: green, amber, red with a bar, or grey with a hole.
/// `ring` cuts it out of what it sits on (your picture); `under` is the color behind it.
#[allow(dead_code)]
pub fn status_dot(
    status: pb::PresenceStatus,
    size: f32,
    ring: bool,
    under: gpui_kit::Rgba,
    p: &Palette,
) -> gpui_kit::Div {
    let inner = if ring { size - 4.0 } else { size };
    let dot = div().size(px(size)).rounded_full().flex().items_center().justify_center();
    let dot = if ring { dot.border_2().border_color(under) } else { dot };
    match status {
        pb::PresenceStatus::Idle => dot.bg(gpui_kit::hsla(0.12, 0.9, 0.55, 1.0)),
        pb::PresenceStatus::DoNotDisturb => {
            dot.bg(p.destructive).child(div().w(px(inner * 0.6)).h(px(inner * 0.2)).rounded_full().bg(under))
        }
        pb::PresenceStatus::Invisible | pb::PresenceStatus::Offline => {
            dot.bg(p.muted_foreground).child(div().size(px(inner * 0.45)).rounded_full().bg(under))
        }
        _ => dot.bg(p.success),
    }
}

fn menu_title(text: &str, p: &Palette) -> gpui_kit::Div {
    div().flex().flex_col().p(px(6.0)).w(px(280.0)).child(
        div()
            .px(px(10.0))
            .pt(px(6.0))
            .pb(px(2.0))
            .font_weight(FontWeight::EXTRA_BOLD)
            .whitespace_nowrap()
            .text_ellipsis()
            .overflow_hidden()
            .child(text.to_owned())
            .text_color(p.foreground),
    )
}

fn section(text: &str, p: &Palette) -> impl IntoElement {
    div()
        .px(px(10.0))
        .pt(px(8.0))
        .pb(px(4.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(text.to_uppercase())
}

fn note(text: &str, p: &Palette) -> impl IntoElement {
    div()
        .mx(px(6.0))
        .my(px(4.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(corner(10.0))
        .bg(alpha(p.primary, 0.1))
        .text_xs()
        .text_color(p.primary)
        .font_weight(FontWeight::BOLD)
        .child(text.to_owned())
}

/// The same, under the sidebar's header on the left.
fn float_left(body: gpui_kit::Div, id: &'static str, p: &Palette, cx: &mut Context<FuwaApp>) -> AnyElement {
    div()
        .id(SharedString::from(format!("{id}-away")))
        .absolute()
        .inset_0()
        .occlude()
        .on_click(cx.listener(|this, _, _, cx| {
            this.menu = None;
            cx.notify();
        }))
        .child(
            div()
                .id(id)
                .absolute()
                .top(px(52.0))
                .left(px(crate::ui::rail::RAIL + 8.0))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(motion::rise(card(p).rounded(corner(16.0)).child(body), id, Duration::ZERO, -8.0)),
        )
        .into_any_element()
}
