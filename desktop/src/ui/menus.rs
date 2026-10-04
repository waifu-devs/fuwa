//! The bells: how a channel, or a whole server, notifies you. The settings
//! live on the instance, so they follow you to every device. Like the web
//! app's `NotificationBell.tsx`. And your status menu, like `UserPanel.tsx`.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::account::NotificationPatch;
use crate::core::dms::now_ms;
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
    /// The menu under a channel's bell.
    pub(crate) fn bell_menu(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let (own, inherited, muted_by_server, name) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, Level::Unspecified, false, String::new()) };
            let own = i.notification_settings(server, channel).cloned();
            let above = i.notification_settings(server, "").map(|n| n.level()).filter(|l| *l != Level::Unspecified);
            let inherited = above
                .or_else(|| i.server(server).map(|s| s.default_notifications()).filter(|l| *l != Level::Unspecified))
                .unwrap_or(Level::Unspecified);
            let by_server = crate::core::notifications::is_muted(i.notification_settings(server, ""), now_ms());
            (own, inherited, by_server, i.channel(server, channel).map(|c| format!("#{}", c.name)).unwrap_or_default())
        });
        let level = own.as_ref().map(|n| n.level()).unwrap_or(Level::Unspecified);
        let muted = crate::core::notifications::is_muted(own.as_ref(), now_ms());
        let default_label = match inherited {
            Level::All => "Like the server (all messages)",
            Level::Mentions => "Like the server (@mentions)",
            Level::Nothing => "Like the server (nothing)",
            _ => "Like the server",
        };
        let mut body = menu_title(&format!("Notifications for {name}"), &p);
        body = body.child(section("Notify me about", &p));
        body = body.child(self.level_item(key, server, channel, Level::Unspecified, default_label, level, &p, cx));
        for (l, label) in LEVELS {
            body = body.child(self.level_item(key, server, channel, l, label, level, &p, cx));
        }
        body = body.child(div().h(px(1.0)).my(px(6.0)).bg(p.border));
        body = self.mute_items(body, key, server, channel, muted, own.as_ref(), &p, cx);
        if muted_by_server && !muted {
            body = body.child(note("The whole server is muted.", &p));
        }
        float(body, "menu-channel", 60.0, &p, cx)
    }

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

/// The statuses you can pick, like the web app's menu, and what each means.
const STATUSES: [(pb::PresenceStatus, &str); 4] = [
    (pb::PresenceStatus::Online, ""),
    (pb::PresenceStatus::Idle, "Shown as away"),
    (pb::PresenceStatus::DoNotDisturb, "No sounds or notifications"),
    (pb::PresenceStatus::Invisible, "Look offline, still use everything"),
];

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

impl FuwaApp {
    /// Your status on an instance: it saves at once and follows the account,
    /// so your other apps there show the same.
    pub(crate) fn status_menu(&mut self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (current, instance) = self
            .core
            .shared
            .read(|s| s.instance(key).map(|i| (i.status(), i.name())))
            .unwrap_or((pb::PresenceStatus::Online, String::new()));
        let mut body = menu_title(&format!("Your status on {instance}"), &p);
        for (n, (status, hint)) in STATUSES.into_iter().enumerate() {
            let on = status == current;
            let hover = alpha(p.primary, 0.1);
            let key = key.to_owned();
            body = body.child(motion::rise(
                div()
                    .id(SharedString::from(format!("status-{}", status as i32)))
                    .min_h(px(38.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(corner(10.0))
                    .cursor_pointer()
                    .text_sm()
                    .hover(move |s| s.bg(hover))
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        let core = this.core.clone();
                        let key = key.clone();
                        this.run(cx, async move { core.set_status(&key, status).await }, |this, result, cx| {
                            if let Err(err) = result {
                                this.toast(
                                    "circle-alert",
                                    "Couldn't change your status".into(),
                                    err.message,
                                    None,
                                    None,
                                    cx,
                                );
                            }
                            cx.notify();
                        });
                        cx.notify();
                    }))
                    .child(status_dot(status, 12.0, false, p.card, &p).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::BOLD).child(status_label(status)))
                            .when(!hint.is_empty(), |el| {
                                el.child(div().text_xs().text_color(p.muted_foreground).child(hint))
                            }),
                    )
                    .when(on, |el| el.child(div().flex_none().size(px(6.0)).rounded_full().bg(p.primary))),
                SharedString::from(format!("status-in-{n}")),
                Duration::from_millis(30 * n as u64),
                4.0,
            ));
        }
        div()
            .id("menu-status-away")
            .absolute()
            .inset_0()
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .id("menu-status")
                    .absolute()
                    .bottom(px(68.0))
                    .left(px(crate::ui::rail::RAIL + 8.0))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(motion::rise(
                        card(&p).rounded(corner(16.0)).child(body),
                        "menu-status",
                        Duration::ZERO,
                        8.0,
                    )),
            )
            .into_any_element()
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

/// A clear layer that closes the menu when clicked, with the menu dropping in
/// under the header on the right.
fn float(body: gpui_kit::Div, id: &'static str, right: f32, p: &Palette, cx: &mut Context<FuwaApp>) -> AnyElement {
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
                .right(px(right))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(motion::rise(card(p).rounded(corner(16.0)).child(body), id, Duration::ZERO, -8.0)),
        )
        .into_any_element()
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
