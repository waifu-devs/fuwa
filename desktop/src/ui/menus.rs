//! The bells: how a channel, or a whole server, notifies you. The settings
//! live on the instance, so they follow you to every device. Like the web
//! app's `NotificationBell.tsx`.

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
const MUTE_FOR: [(&str, Option<i64>); 5] = [
    ("For 15 minutes", Some(15 * 60_000)),
    ("For 1 hour", Some(60 * 60_000)),
    ("For 8 hours", Some(8 * 60 * 60_000)),
    ("For 24 hours", Some(24 * 60 * 60_000)),
    ("Until I turn it back on", None),
];

/// "Muted until 16:30", "Muted until Tue 09:00", or "Muted".
fn muted_label(n: Option<&pb::NotificationSettings>) -> String {
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
            .on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                let core = this.core.clone();
                let (key, server, channel, patch) = (key.clone(), server.clone(), channel.clone(), patch.clone());
                this.run(
                    cx,
                    async move { core.update_notifications(&key, &server, &channel, patch).await },
                    |this, result, cx| {
                        if let Err(err) = result {
                            this.toast("circle-alert", "Couldn't change that".into(), err.message, None, None, cx);
                        }
                        cx.notify();
                    },
                );
                cx.notify();
            }))
            .child(icon(glyph).size(px(16.0)))
            .child(label.to_owned())
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
