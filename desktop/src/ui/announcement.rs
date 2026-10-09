//! The instance's announcement, across the top of the app while you're on
//! that instance: news in the theme's color, heads-ups in amber, and urgent
//! ones in red, which can't be closed. A closed banner stays closed on this
//! computer until the admins put up a new one. Like the web app's
//! `AnnouncementBanner.tsx`, whose look per tone is in its `app.css`.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, FontWeight, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, hsla,
    linear_color_stop, linear_gradient, px,
};

use crate::core::instance_manage::{ends_label, ends_ms, stamp_label, tone_of};
use crate::pb::{self, AnnouncementTone as Tone};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, mix};
use crate::ui::widgets::icon;

/// The heads-up amber, and the near-black on it.
pub fn amber() -> Hsla {
    gpui_kit::rgb(0xf59e0b).into()
}

pub(crate) fn ink() -> Hsla {
    gpui_kit::rgb(0x1c1917).into()
}

pub fn glyph(tone: Tone) -> &'static str {
    match tone {
        Tone::Warning => "triangle-alert",
        Tone::Critical => "siren",
        _ => "megaphone",
    }
}

/// The banner itself, also the live preview on the Announcement page (whose
/// rounded box clips it). `close` is its close button, when it can be closed;
/// `id` keeps its motion apart from another banner's.
pub fn banner(
    id: &str,
    a: &pb::Announcement,
    now: i64,
    close: Option<AnyElement>,
    p: &Palette,
    window: &Window,
) -> AnyElement {
    let tone = tone_of(a);
    let (bg, fg, chip, chip_fg) = match tone {
        Tone::Warning => (mix(p.background, gpui_kit::rgb(0xf59e0b), 0.18), p.foreground.into(), amber(), ink()),
        Tone::Critical => {
            (hsla(0.0, 0.72, 0.51, 1.0), gpui_kit::white(), alpha(gpui_kit::rgb(0xffffff), 0.22), gpui_kit::white())
        }
        _ => (mix(p.background, p.primary, 0.14), p.foreground.into(), p.primary.into(), p.primary_foreground.into()),
    };
    let period = match tone {
        Tone::Warning => 2200,
        Tone::Critical => 1100,
        _ => 4500,
    };
    // Each tone's icon moves its own way: news waves now and then, heads-ups
    // bob, urgent ones swing like a siren.
    let mark = motion::ambient(
        div().child(icon(glyph(tone)).size(px(16.0)).text_color(chip_fg)),
        SharedString::from(format!("{id}-glyph-{}", tone as i32)),
        Duration::from_millis(period),
        window,
        move |el, t| match tone {
            Tone::Warning => el.relative().top(px(-1.5 * (t * std::f32::consts::TAU).sin().abs())),
            Tone::Critical => el.relative().left(px(1.5 * (t * std::f32::consts::TAU).sin())),
            _ => {
                let k = ((t - 0.7) / 0.18).clamp(0.0, 1.0);
                let wave = if k > 0.0 && k < 1.0 { (k * 3.0 * std::f32::consts::PI).sin() * (1.0 - k) } else { 0.0 };
                el.relative().top(px(-3.0 * wave.abs())).left(px(-2.0 * wave))
            }
        },
    );
    // Pops in turning upright, a moment after the banner (the web's `scale: 0, rotate: -40`).
    let badge = motion::pop(
        div()
            .relative()
            .flex_none()
            .size(px(28.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(chip)
            .when(tone == Tone::Critical, |el| {
                el.child(motion::ambient(
                    div().absolute().inset_0().rounded_full().bg(alpha(gpui_kit::rgb(0xffffff), 0.4)),
                    SharedString::from(format!("{id}-ping")),
                    Duration::from_millis(1000),
                    window,
                    |el, t| el.opacity(0.6 * (1.0 - t)).size(px(28.0 + 12.0 * t)).top(px(-6.0 * t)).left(px(-6.0 * t)),
                ))
            })
            .child(mark),
        SharedString::from(format!("{id}-badge-{}", tone as i32)),
        0.0,
        -40.0,
        Duration::from_millis(120),
    );
    let ends = ends_ms(a).map(|at| (ends_label(at, now), stamp_label(at, now)));
    let words = motion::rise(
        div().min_w_0().flex_shrink(1.0).text_sm().font_weight(FontWeight::BOLD).text_color(fg).line_clamp(2).child(
            crate::ui::text::markdown(
                SharedString::from(format!("{id}-text-{}", a.id)),
                crate::ui::text::images_as_links(&a.text),
            ),
        ),
        SharedString::from(format!("{id}-words-{}|{}", a.id, a.text.len())),
        Duration::from_millis(80),
        -8.0,
    );
    let mut row = div()
        .relative()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .px(px(48.0))
        .py(px(8.0))
        .bg(bg)
        .text_color(fg)
        .when(tone == Tone::Info, |el| {
            // The web's three-stop sweep: 18% of the primary at the edges, 9% at 60%.
            let (edge, middle) = (mix(p.background, p.primary, 0.18), mix(p.background, p.primary, 0.09));
            el.border_b_1()
                .border_color(alpha(p.primary, 0.3))
                .child(div().absolute().top_0().bottom_0().left_0().w(gpui_kit::relative(0.6)).bg(linear_gradient(
                    90.0,
                    linear_color_stop(edge, 0.0),
                    linear_color_stop(middle, 1.0),
                )))
                .child(div().absolute().top_0().bottom_0().right_0().w(gpui_kit::relative(0.4)).bg(linear_gradient(
                    90.0,
                    linear_color_stop(middle, 0.0),
                    linear_color_stop(edge, 1.0),
                )))
        })
        .child(badge)
        .child(words)
        .when_some(ends, |el, (short, _)| {
            el.child(
                div()
                    .flex_none()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded_full()
                    .bg(if p.dark || tone == Tone::Critical {
                        alpha(gpui_kit::rgb(0xffffff), 0.1)
                    } else {
                        alpha(gpui_kit::rgb(0x000000), 0.1)
                    })
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(crate::core::i18n::t_with(
                        "shell.announcement.until",
                        &[("time", crate::core::i18n::Arg::Str(&short))],
                    )),
            )
        });
    if tone == Tone::Critical {
        // A light sweeps across it.
        row = row.child(motion::ambient(
            div().absolute().top_0().bottom_0().w(px(260.0)).bg(linear_gradient(
                100.0,
                linear_color_stop(alpha(gpui_kit::rgb(0xffffff), 0.0), 0.0),
                linear_color_stop(alpha(gpui_kit::rgb(0xffffff), 0.22), 1.0),
            )),
            SharedString::from(format!("{id}-sweep")),
            Duration::from_millis(2600),
            window,
            |el, t| el.left(gpui_kit::relative(-0.3 + 1.6 * t)).opacity((1.0 - (2.0 * t - 1.0).abs()) * 0.9),
        ));
    }
    if tone == Tone::Warning {
        row = row.child(hazard(id, window));
    }
    if let Some(close) = close {
        row = row.child(div().absolute().right(px(8.0)).top_0().bottom_0().flex().items_center().child(close));
    }
    row.into_any_element()
}

/// The striped edge under a heads-up, moving along.
pub(crate) fn hazard(id: &str, window: &Window) -> AnyElement {
    let mut stripes = div().absolute().top_0().bottom_0().left(px(-32.0)).flex();
    for n in 0..240 {
        stripes = stripes.child(div().w(px(8.0)).h_full().bg(if n % 2 == 0 { amber() } else { ink() }));
    }
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(3.0))
        .overflow_hidden()
        .opacity(0.85)
        .child(motion::ambient(
            stripes,
            SharedString::from(format!("{id}-hazard")),
            Duration::from_millis(1200),
            window,
            |el, t| el.left(px(-32.0 + 16.0 * t)),
        ))
        .into_any_element()
}

impl crate::ui::app::FuwaApp {
    /// The banner for the instance you're on, unless it's down, over or
    /// closed here. Once it goes, it steps out of the way at once and fades
    /// as it lifts (the web's `SLIDE_IN` under `mode="popLayout"`).
    pub(crate) fn render_announcement(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Option<AnyElement> {
        let now = crate::core::dms::now_ms();
        let shown = self.announcement_shown(now);
        let leaving = motion::kept("announcement", shown.as_ref(), window, cx);
        if let Some((key, a)) = shown {
            return Some(self.announcement_banner(&key, &a, now, window, cx));
        }
        let ((key, a), t) = leaving?;
        let gone = gpui_kit::ease_out_quint()(t);
        let banner = self.announcement_banner(&key, &a, now, window, cx);
        Some(
            div()
                .relative()
                .h(px(0.0))
                .flex_none()
                .child(
                    gpui_kit::deferred(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .opacity(1.0 - gone)
                            .translate_y(px(-6.0 * gone))
                            .child(banner),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }

    /// The instance you're on and its announcement, while it should show.
    fn announcement_shown(&self, now: i64) -> Option<(String, pb::Announcement)> {
        use crate::ui::app::Nav;
        let key = match &self.nav {
            Nav::Instance { key } | Nav::Server { key, .. } => key.clone(),
            Nav::Home { dm: Some((key, _)) } | Nav::Friends { key } => key.clone(),
            Nav::Home { dm: None } => return None,
        };
        let a = self
            .core
            .shared
            .read(|s| s.instance(&key).and_then(|i| i.node.as_ref()).and_then(|n| n.announcement.clone()))?;
        if !crate::core::instance_manage::is_live(Some(&a), now) {
            return None;
        }
        let critical = tone_of(&a) == Tone::Critical;
        if !critical && self.prefs.closed_announcements.get(&key) == Some(&a.id) {
            return None;
        }
        Some((key, a))
    }

    fn announcement_banner(
        &mut self,
        key: &str,
        a: &pb::Announcement,
        now: i64,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> AnyElement {
        use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _};
        let critical = tone_of(a) == Tone::Critical;
        let p = crate::ui::widgets::pal(cx);
        let close = (!critical).then(|| {
            let (key, id) = (key.to_owned(), a.id.clone());
            let button = div()
                .id("announcement-close")
                .size(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                // The web's `hover:bg-black/10 dark:hover:bg-white/15`.
                .hover({
                    let tint =
                        if p.dark { alpha(gpui_kit::rgb(0xffffff), 0.15) } else { alpha(gpui_kit::rgb(0x000000), 0.1) };
                    move |s| s.bg(tint)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.core.set_prefs(|p| {
                        p.closed_announcements.insert(key.clone(), id.clone());
                    });
                    this.prefs = this.core.prefs();
                    cx.notify();
                }))
                .child(icon("x").size(px(16.0)));
            // The web's `group-hover:rotate-90` and `active:scale-90`.
            let (hover, press) = (motion::Pose::turn(90.0), motion::Pose { scale: 0.9, ..motion::Pose::turn(90.0) });
            motion::answer(button, "announcement-close", hover, press, window, cx).into_any_element()
        });
        // It drops in from just above (the web's `SLIDE_IN`, on `stiffness: 420, damping: 40`).
        motion::spring_in(
            div().flex_none().child(banner("announcement", a, now, close, &p, window)),
            SharedString::from(format!("announcement-in-{key}-{}", a.id)),
            (420.0, 40.0),
            Duration::ZERO,
            |el, t| el.opacity(t.clamp(0.0, 1.0)).translate_y(px(-6.0 * (1.0 - t))),
        )
    }
}
