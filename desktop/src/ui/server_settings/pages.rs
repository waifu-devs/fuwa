//! What the pages the web's server settings have beyond the first ones keep
//! between frames (Overview and Access, Rules & questions, Single sign-on,
//! Usage and Limits, Applications, Transfer ownership, Delete server), and
//! the pieces they're drawn from: the web's `settings/controls.tsx` rows,
//! headings and cards, at the web's sizes.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use super::ServerSettingsView;
use crate::ui::motion;
use crate::ui::settings_controls::shadow_lg;
use crate::ui::theme::{Palette, alpha, radius_3xl, radius_xl};

pub(super) struct Pages {
    pub(super) overview: super::overview::Overview,
    pub(super) join: super::joinform::JoinForm,
    pub(super) sso: super::sso::Sso,
    pub(super) applications: super::applications::Applications,
    pub(super) usage: super::usage::Usage,
    pub(super) people: super::people::PeopleState,
    /// The menu open on the page, by its id.
    pub(super) menu: Option<String>,
    /// Welcome & onboarding's preview: which screen, and at a phone's width.
    pub(super) stage_view: super::stage::View,
    pub(super) stage_phone: bool,
}

impl Pages {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let mut subscriptions = Vec::new();
        let (overview, s) = super::overview::Overview::new(window, cx);
        subscriptions.extend(s);
        let (sso, s) = super::sso::Sso::new(window, cx);
        subscriptions.extend(s);
        let (usage, s) = super::usage::Usage::new(window, cx);
        subscriptions.extend(s);
        let (people, s) = super::people::PeopleState::new(window, cx);
        subscriptions.extend(s);
        let pages = Self {
            overview,
            join: Default::default(),
            sso,
            applications: Default::default(),
            usage,
            people,
            menu: None,
            stage_view: Default::default(),
            stage_phone: false,
        };
        (pages, subscriptions)
    }
}

/// A setting's title (`font-extrabold`) and the line under it (`text-sm text-muted-foreground`).
pub(super) fn heading(title: &str, hint: Option<&str>, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .child(div().font_weight(FontWeight::EXTRA_BOLD).line_height(px(24.0)).child(title.to_owned()))
        .when_some(hint, |el, h| {
            el.child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(h.to_owned()))
        })
}

/// A form label (shadcn's `Label` with `font-extrabold`: `text-sm leading-none`).
pub(super) fn label(text: &str) -> Div {
    div().text_sm().line_height(px(14.0)).font_weight(FontWeight::EXTRA_BOLD).child(text.to_owned())
}

/// One part of a page under a rule (`flex flex-col gap-2 border-b border-border/70 py-5`):
/// the first has no space above, the last no rule below.
pub(super) fn part(first: bool, last: bool, gap: f32, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .pt(px(if first { 0.0 } else { 20.0 }))
        .pb(px(20.0))
        .when(!last, |el| el.border_b_1().border_color(alpha(p.border, 0.7)))
}

/// The web's `Setting`: a title and hint over its controls, `py-6` under a rule.
pub(super) fn setting(title: &str, hint: Option<&str>, first: bool, last: bool, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .pt(px(if first { 0.0 } else { 24.0 }))
        .pb(px(if last { 0.0 } else { 24.0 }))
        .when(!last, |el| el.border_b_1().border_color(alpha(p.border, 0.7)))
        .child(
            div()
                .child(div().font_weight(FontWeight::EXTRA_BOLD).line_height(px(24.0)).child(title.to_owned()))
                .when_some(hint, |el, h| {
                    el.child(
                        div()
                            .mt(px(2.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(h.to_owned()),
                    )
                }),
        )
}

/// A text box's frame as the web draws inputs: bordered, `rounded-xl`, `h` tall, a faint shadow.
pub(super) fn boxed(input: impl IntoElement, h: f32, focused: bool, p: &Palette) -> Div {
    let el = div()
        .h(px(h))
        .w_full()
        .rounded(radius_xl())
        .border_1()
        .border_color(p.border)
        .bg(alpha(p.background, 0.0))
        .px(px(1.0))
        .flex()
        .items_center()
        .text_sm()
        // No `shadow-xs`: GPUI fills under a shadow, which greys a see-through field.
        .child(div().flex_1().min_w_0().child(input));
    crate::ui::instance_home::focus_ring(el, focused, p)
}

/// Whether a text box has the keyboard.
pub(super) fn focused<T: gpui_kit::Focusable>(
    state: &gpui_kit::Entity<T>,
    window: &Window,
    cx: &gpui_kit::App,
) -> bool {
    crate::ui::instance_home::has_focus(state, window, cx)
}

/// A preview card (`rounded-3xl border bg-card shadow-lg`).
pub(super) fn preview_card(p: &Palette) -> Div {
    div().rounded(radius_3xl()).border_1().border_color(p.border).bg(p.card).shadow(shadow_lg())
}

/// A muted line saying why a page couldn't load (`text-sm text-muted-foreground first-letter:uppercase`).
pub(super) fn problem(text: &str, p: &Palette) -> AnyElement {
    div()
        .text_sm()
        .line_height(px(20.0))
        .text_color(p.muted_foreground)
        .child(crate::ui::instance_home::capitalized(text))
        .into_any_element()
}

/// Shimmering blocks while a page loads (`shimmer h-* rounded-*`).
pub(super) fn shimmers(n: usize, h: f32, radius: gpui_kit::Pixels, p: &Palette, window: &Window) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .children((0..n).map(|k| crate::ui::instance_home::shimmer(h, radius, p, window, k)))
        .into_any_element()
}

/// Something sliding in from above as it appears (the web's `SLIDE_IN`).
pub(super) fn slide_in(el: impl IntoElement + gpui_kit::Styled + 'static, id: impl Into<SharedString>) -> AnyElement {
    motion::rise(el, gpui_kit::ElementId::Name(id.into()), Duration::ZERO, -8.0).into_any_element()
}

/// A length of time in its largest whole unit, the web's `formatDuration`.
pub(super) fn duration(seconds: i64) -> String {
    crate::ui::composer::format_duration(seconds)
}

/// Time left, rounded as the web's `timeLeft` does: "5 minutes", "2 hours", "3 days".
pub(super) fn time_left(ms: i64) -> String {
    let minutes = ((ms as f64) / 60_000.0).round().max(1.0) as i64;
    let seconds = if minutes >= 1440 {
        ((minutes as f64) / 1440.0).round() as i64 * 86_400
    } else if minutes >= 60 {
        ((minutes as f64) / 60.0).round() as i64 * 3600
    } else {
        minutes * 60
    };
    duration(seconds)
}

/// How long something has been around, the web's `roughly`: "5 minutes", "3 days", "2 months".
pub(super) fn roughly(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    let (n, unit) = match minutes {
        0 => return crate::core::i18n::t("common.time.lessThanMinute"),
        m if m < 60 => (m, "minute"),
        m if m < 1440 => (m / 60, "hour"),
        m if m < 43_200 => (m / 1440, "day"),
        m if m < 525_600 => (m / 43_200, "month"),
        m => (m / 525_600, "year"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// One field of a form (the web's account `Row`): its label, the control, a hint, under a rule.
pub(super) fn form_row(label: &str, hint: Option<String>, body: impl IntoElement, last: bool, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .pt(px(20.0))
        .pb(px(20.0))
        .when(!last, |el| el.border_b_1().border_color(alpha(p.border, 0.7)))
        .child(div().text_sm().line_height(px(14.0)).font_weight(FontWeight::EXTRA_BOLD).child(label.to_owned()))
        .child(body)
        .when_some(hint, |el, h| {
            el.child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(h))
        })
}

impl ServerSettingsView {
    /// The web's `Segmented` with each option as wide as its words; the highlight glides to the chosen one.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn seg_tabs(
        &mut self,
        id: &str,
        options: Vec<String>,
        chosen: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let widths: Vec<f32> = options.iter().map(|l| 24.0 + l.chars().count() as f32 * 7.6).collect();
        let chosen = chosen.min(options.len().saturating_sub(1));
        let x = motion::follow(SharedString::from(format!("{id}-x")), widths[..chosen].iter().sum::<f32>(), window, cx);
        let w = motion::follow(
            SharedString::from(format!("{id}-w")),
            widths.get(chosen).copied().unwrap_or(0.0),
            window,
            cx,
        );
        let pick = std::rc::Rc::new(pick);
        let mut row = div().relative().flex().p(px(4.0)).rounded(crate::ui::theme::radius_xl()).bg(p.muted).child(
            div()
                .absolute()
                .top(px(4.0))
                .bottom(px(4.0))
                .left(px(4.0 + x))
                .w(px(w))
                .rounded(crate::ui::theme::radius_lg())
                .bg(p.background)
                .shadow(crate::ui::settings_controls::shadow_sm()),
        );
        for (n, label) in options.into_iter().enumerate() {
            let on = n == chosen;
            let fg = p.foreground;
            let pick = pick.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("{id}-{n}")))
                    .relative()
                    .w(px(widths[n]))
                    .py(px(6.0))
                    .flex()
                    .justify_center()
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .whitespace_nowrap()
                    .text_color(if on { p.foreground } else { p.muted_foreground })
                    .cursor_pointer()
                    .hover(move |s| s.text_color(fg))
                    .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                    .child(label),
            );
        }
        row.into_any_element()
    }
}

/// The web's `<Button className="rounded-xl font-bold">` with something before its words (an
/// icon, or a spinner while it works).
pub(super) fn act(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    lead: impl IntoElement,
    look: crate::ui::settings_controls::Look,
    p: &Palette,
) -> gpui_kit::Stateful<Div> {
    crate::ui::settings_controls::button(id, "", None, look, false, p)
        .px(px(12.0))
        .rounded(radius_xl())
        .font_weight(FontWeight::BOLD)
        .child(lead)
        .child(label.into())
}

/// `settings_controls::button` with its own hover in place of the look's (GPUI takes one hover
/// per element): the web's ghost and outline buttons that turn red under the pointer.
pub(super) fn hover_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    glyph: Option<&'static str>,
    outline: bool,
    p: &Palette,
    hover: impl Fn(gpui_kit::StyleRefinement) -> gpui_kit::StyleRefinement + 'static,
) -> gpui_kit::Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex_none()
        .h(px(36.0))
        .px(px(if glyph.is_some() { 12.0 } else { 16.0 }))
        .rounded(radius_xl())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .whitespace_nowrap()
        .text_color(p.foreground)
        .when(outline, |el| el.border_1().border_color(p.border).bg(p.background))
        .cursor_pointer()
        .hover(hover)
        .group("settings-button")
        .when_some(glyph, |el, g| el.child(crate::ui::settings_controls::glyph_in_button(g)))
        .when(!label.is_empty(), |el| el.child(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_like_the_web() {
        assert_eq!(time_left(90 * 60_000), "2 hours");
        assert_eq!(time_left(30_000), "1 minute");
        assert_eq!(time_left(3 * 86_400_000), "3 days");
        assert_eq!(roughly(2 * 3_600_000), "2 hours");
        assert_eq!(roughly(45 * 86_400_000), "1 month");
    }
}
