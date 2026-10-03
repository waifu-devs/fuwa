//! The Privacy page: the "Help fix bugs" switch for the anonymous reports
//! (`core::reports`), what they hold in plain words, and a live look at
//! what's waiting to go out and where it would go.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div, px,
};

use crate::core::config::Prefs;
use crate::core::reports::{self, Pending};
use crate::ui::motion;
use crate::ui::settings::{SettingsView, section, toggle_row};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::icon;

/// What a report holds, and what it never does.
const SENT: [(&str, &str); 4] = [
    ("triangle-alert", "Kinds of errors and where in the app they happened"),
    ("timer", "How long things took: starting up, catching up, slow frames and requests"),
    ("mouse-pointer-click", "How often a few features are used"),
    ("info", "The app's version and your OS family (Windows, macOS or Linux)"),
];

impl SettingsView {
    pub(crate) fn privacy_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = prefs.share_reports;
        let pending = if on { reports::pending() } else { Pending::default() };
        self.pending = pending;
        let destination = self.core.report_destination().map(|key| {
            let name = self.core.shared.read(|s| s.instance(&key).map(|i| i.name()));
            // Streamer mode keeps addresses off screen, and a name may be one.
            name.filter(|_| !prefs.streamer_mode).unwrap_or_else(|| "your instance".into())
        });

        let mut what = div().flex().flex_col().gap(px(10.0));
        for (n, (glyph, line)) in SENT.into_iter().enumerate() {
            what = what.child(motion::rise(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .text_sm()
                    .child(
                        div()
                            .flex_none()
                            .size(px(24.0))
                            .rounded(corner(8.0))
                            .bg(alpha(p.primary, 0.12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(glyph).size(px(14.0)).text_color(p.primary)),
                    )
                    .child(div().flex_1().min_w_0().pt(px(2.0)).child(line)),
                SharedString::from(format!("sent-{n}")),
                Duration::from_millis(60 + 40 * n as u64),
                6.0,
            ));
        }
        let what = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .p(px(16.0))
            .rounded(corner(16.0))
            .bg(p.card)
            .border_1()
            .border_color(p.border)
            .child(what)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .p(px(12.0))
                    .rounded(corner(12.0))
                    .bg(p.secondary)
                    .text_sm()
                    .child(icon("eye-off").size(px(16.0)).mt(px(2.0)).text_color(p.muted_foreground))
                    .child(div().flex_1().min_w_0().child(
                        "Never messages, names, file names, ids or addresses. It goes only to your own instance, which adds it to its hourly report to Waifu Devs, and nothing is sent while the instance's own telemetry is off.",
                    )),
            );

        // Dims away when off, and comes back with a spring.
        let lit = motion::follow("privacy-lit", if on { 1.0 } else { 0.0 }, window, cx);
        let chips = div()
            .flex()
            .gap(px(10.0))
            .child(chip("errors", "triangle-alert", pending.errors, "error", "errors", p))
            .child(chip("timings", "timer", pending.timings, "timing", "timings", p))
            .child(chip("usage", "mouse-pointer-click", pending.usage, "feature use", "feature uses", p));
        let (dot, status) = match (&destination, on) {
            (_, false) => (p.muted_foreground, "Off: nothing is counted, and what was waiting is gone.".to_owned()),
            (Some(name), true) => (p.success, format!("Goes to {name} every 10 minutes.")),
            (None, true) => (
                p.primary,
                "No instance you're signed in to takes reports right now (its telemetry is off), so they wait here, up to 64 of each."
                    .to_owned(),
            ),
        };
        let pulse = on && destination.is_some();
        let dot = div().flex_none().mt(px(5.0)).size(px(8.0)).rounded_full().bg(dot);
        let dot = if pulse {
            motion::ambient(dot, "privacy-dot", Duration::from_millis(2400), window, |el, t| {
                let glow = (t * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                el.opacity(0.55 + 0.45 * glow)
            })
        } else {
            dot.into_any_element()
        };
        let preview = div().flex().flex_col().gap(px(12.0)).opacity(0.45 + 0.55 * lit).child(chips).child(
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .child(dot)
                .child(div().flex_1().min_w_0().child(status)),
        );

        div()
            .flex()
            .flex_col()
            .gap(px(28.0))
            .child(toggle_row(
                "share-reports",
                "Help fix bugs",
                "Sends anonymous counts of what went wrong and what was slow to your own instance, so Waifu Devs can find and fix it.",
                on,
                p,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.share_reports = on),
            ))
            .child(section("What's sent", what, p))
            .child(section("Waiting to send", preview, p))
            .into_any_element()
    }
}

/// One count waiting to go out. A new number rises into place, and the chip
/// glows for a moment as it changes.
fn chip(id: &'static str, glyph: &'static str, count: u32, one: &str, many: &str, p: &Palette) -> impl IntoElement {
    let changed = SharedString::from(format!("pending-{id}-{count}"));
    let glow = alpha(p.primary, 0.20);
    div()
        .flex_1()
        .min_w_0()
        .relative()
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(px(12.0))
        .p(px(14.0))
        .rounded(corner(14.0))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .when(count > 0, |el| {
            el.child(div().absolute().inset_0().bg(glow).with_animation(
                SharedString::from(format!("{changed}-glow")),
                Animation::new(Duration::from_millis(900)).with_easing(gpui_kit::ease_out_quint()),
                |el, t| el.opacity(1.0 - t),
            ))
        })
        .child(
            div()
                .flex_none()
                .size(px(32.0))
                .rounded(corner(10.0))
                .bg(alpha(p.primary, if count > 0 { 0.16 } else { 0.08 }))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph).size(px(16.0)).text_color(if count > 0 { p.primary } else { p.muted_foreground })),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().h(px(26.0)).overflow_hidden().child(motion::rise(
                    div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(count.to_string()),
                    changed,
                    Duration::ZERO,
                    14.0,
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(if count == 1 { one.to_owned() } else { many.to_owned() }),
                ),
        )
}
