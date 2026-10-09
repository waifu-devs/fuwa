//! The bars across the top of the app besides the instance's announcement
//! (`announcement.rs`): streamer mode being on (the web's `StreamerBanner`
//! in `Shortcuts.tsx`), and a way to sign in added to your account lately
//! (`SignInNotice.tsx`), so one you didn't add gets seen and undone.

use std::time::Duration;

use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, linear_color_stop, linear_gradient, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::motion;
use crate::ui::theme::{alpha, mix};
use crate::ui::widgets::{icon, pal};

/// A provider's mark (google, x, twitch), or a key for one this app doesn't know.
fn provider_mark(kind: &str, size: f32) -> AnyElement {
    match kind {
        "google" | "x" | "twitch" => gpui_kit::svg()
            .path(SharedString::from(format!("providers/{kind}.svg")))
            .size(px(size))
            .flex_none()
            .text_color(crate::ui::announcement::ink())
            .into_any_element(),
        _ => icon("key-round").size(px(size)).text_color(crate::ui::announcement::ink()).into_any_element(),
    }
}

impl FuwaApp {
    /// "Streamer mode is on", over everything, with Hide and Turn off.
    pub(crate) fn render_streamer_banner(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.prefs.streamer_mode || self.streamer_banner_hidden {
            return None;
        }
        let p = pal(cx);
        let from = mix(p.primary, gpui_kit::rgb(0xa78bfa), 0.15);
        let white = |a: f32| gpui_kit::hsla(0.0, 0.0, 1.0, a);
        let bar = div()
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(6.0))
            .bg(linear_gradient(90.0, linear_color_stop(from, 0.0), linear_color_stop(p.primary, 1.0)))
            .text_color(p.primary_foreground)
            .text_size(px(14.0))
            .line_height(px(20.0))
            .font_weight(FontWeight::BOLD)
            // A light sweeps across it now and then.
            .child(motion::ambient(
                div().absolute().top_0().bottom_0().w(gpui_kit::relative(0.4)).bg(linear_gradient(
                    100.0,
                    linear_color_stop(white(0.0), 0.0),
                    linear_color_stop(white(0.35), 1.0),
                )),
                "streamer-sweep",
                Duration::from_millis(3700),
                window,
                |el, t| el.left(gpui_kit::relative(-0.4 + 1.6 * t)).opacity((1.0 - (2.0 * t - 1.0).abs()).min(1.0)),
            ))
            // The screen pops in a moment after the bar (`scale: 0, rotate: -30`).
            .child(motion::pop(
                div().child(icon("tv-minimal-play").size(px(16.0))),
                "streamer-icon",
                0.0,
                -30.0,
                Duration::from_millis(100),
            ))
            .child(div().truncate().child(t("chattools.streamer.on")))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id("streamer-hide")
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .px(px(10.0))
                            .py(px(2.0))
                            .rounded_full()
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(white(0.2)))
                            .active(|s| s.scale(0.95))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.streamer_banner_hidden = true;
                                cx.notify();
                            }))
                            .child(icon("eye-off").size(px(14.0)))
                            .child(t("chattools.streamer.hide")),
                    )
                    .child(
                        div()
                            .id("streamer-off")
                            .px(px(10.0))
                            .py(px(2.0))
                            .rounded_full()
                            .bg(white(0.25))
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(white(0.35)))
                            .active(|s| s.scale(0.95))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.core.set_prefs(|p| p.streamer_mode = false);
                                this.prefs = this.core.prefs();
                                cx.notify();
                            }))
                            .child(t("chattools.streamer.turnOff")),
                    ),
            );
        Some(motion::rise(div().flex_none().child(bar), "streamer-banner", Duration::ZERO, -8.0).into_any_element())
    }

    /// The newest way to sign in added to your account in the last week, on
    /// the instance you're on, until closed here.
    pub(crate) fn render_sign_in_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = match &self.nav {
            Nav::Instance { key }
            | Nav::Server { key, .. }
            | Nav::Friends { key }
            | Nav::Home { dm: Some((key, _)) } => key.clone(),
            Nav::Home { dm: None } => return None,
        };
        let latest = self.core.shared.read(|s| s.instance(&key).and_then(|i| i.recent_sign_ins.first().cloned()))?;
        let linked = latest.linked_at.as_ref()?;
        let id = format!("{}:{}", latest.kind, linked.seconds);
        if self.prefs.sign_in_notice_closed.get(&key) == Some(&id) {
            return None;
        }
        let p = pal(cx);
        // Black over a light theme, white over a dark one, as the web's `bg-black/15 dark:bg-white/15`.
        let dark = p.dark;
        let shade = move |a: f32| alpha(if dark { gpui_kit::rgb(0xffffff) } else { gpui_kit::rgb(0x000000) }, a);
        let date = {
            use chrono::TimeZone as _;
            chrono::Local
                .timestamp_opt(linked.seconds, 0)
                .single()
                .map(|d| d.format("%b %-d").to_string())
                .unwrap_or_default()
        };
        // The provider's mark pops in after the bar (`scale: 0, rotate: -40`).
        let badge = motion::pop(
            div()
                .size(px(28.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(crate::ui::announcement::amber())
                .child(provider_mark(&latest.kind, 14.0)),
            SharedString::from(format!("sign-in-badge|{id}")),
            0.0,
            -40.0,
            Duration::from_millis(120),
        );
        let review = div()
            .id("sign-in-review")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .px(px(10.0))
            .py(px(2.0))
            .rounded_full()
            .bg(if p.dark { alpha(gpui_kit::rgb(0xffffff), 0.1) } else { alpha(gpui_kit::rgb(0x000000), 0.1) })
            .text_size(px(12.0))
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .hover(move |s| s.bg(shade(0.15)))
            .on_click({
                let key = key.clone();
                cx.listener(move |this, _, window, cx| this.open_security_settings(&key, window, cx))
            })
            .child(icon("shield-alert").size(px(14.0)))
            .child(t("shell.signInNotice.review"));
        let close = {
            let (key, id) = (key.clone(), id.clone());
            div()
                .id("sign-in-notice-close")
                .group("sign-in-notice-close")
                .size(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(if dark { shade(0.15) } else { shade(0.1) }))
                .active(|s| s.scale(0.9))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.core.set_prefs(|p| {
                        p.sign_in_notice_closed.insert(key.clone(), id.clone());
                    });
                    this.prefs = this.core.prefs();
                    cx.notify();
                }))
                // Its cross turns a quarter while hovered.
                .child(
                    div()
                        .id("sign-in-notice-x")
                        .group_hover("sign-in-notice-close", |s| s.rotate(gpui_kit::radians(90f32.to_radians())))
                        .child(icon("x").size(px(16.0))),
                )
        };
        let bar = div()
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .px(px(48.0))
            .py(px(8.0))
            .bg(mix(p.background, gpui_kit::rgb(0xf59e0b), 0.18))
            .text_color(p.foreground)
            .text_size(px(14.0))
            .line_height(px(20.0))
            .child(badge)
            .child(div().min_w_0().font_weight(FontWeight::BOLD).child(t_with(
                "shell.signInNotice.added",
                &[("name", Arg::Str(&latest.name)), ("date", Arg::Str(&date))],
            )))
            .child(review)
            .child(crate::ui::announcement::hazard("sign-in-notice", window))
            .child(div().absolute().right(px(8.0)).top_0().bottom_0().flex().items_center().child(close));
        Some(
            motion::rise(
                div().flex_none().child(bar),
                SharedString::from(format!("sign-in-notice|{id}")),
                Duration::ZERO,
                -12.0,
            )
            .into_any_element(),
        )
    }

    /// Settings, on Security for one instance (where ways to sign in are reviewed).
    pub(crate) fn open_security_settings(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(window, cx);
        if let Some(view) = &self.settings {
            let key = key.to_owned();
            view.update(cx, |view, cx| {
                view.page = crate::ui::settings::Page::Security;
                view.account.key = Some(key);
                cx.notify();
            });
        }
    }
}
