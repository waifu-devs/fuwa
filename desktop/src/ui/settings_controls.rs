//! The pieces the app's settings pages are built from, as in the web's
//! `settings/controls.tsx`, `settings/app/common.tsx` and
//! `settings/account/common.tsx`: a setting with its Default or Changed
//! badge (and the way back), rows of a form under rules, option cards the
//! selection glides between, switches with words, segmented pills, chips,
//! sliders with marks, the shadcn buttons, and the floating save bar.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::base::{Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, BoxShadow, Context, Div, ElementId, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Rgba, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, point, px, relative,
};

use crate::core::config::Prefs;
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::theme::{Palette, alpha, mix, radius_lg, radius_md, radius_xl};
use crate::ui::widgets::icon;

type V = SettingsView;

/// How a setting says whether it follows the default.
pub(crate) enum Badge {
    /// Nothing (a setting with no default, like a form field).
    None,
    /// Follows the default, or was changed and can go back with `reset`.
    Pref { changed: bool, reset: Rc<dyn Fn(&mut Prefs)> },
    /// A setting kept somewhere else (on the instance): Reset runs `reset`.
    Custom { changed: bool, reset: Rc<dyn Fn(&mut V, &mut Context<V>)> },
}

impl Badge {
    /// An app setting: changed when `changed` says so; Reset puts back what `reset` does.
    pub(crate) fn pref(changed: bool, reset: impl Fn(&mut Prefs) + 'static) -> Self {
        Badge::Pref { changed, reset: Rc::new(reset) }
    }
}

/// Where a row sits in its page's stack: the first has no space above, the last no rule below.
#[derive(Clone, Copy)]
pub(crate) struct At {
    pub n: usize,
    pub last: bool,
}

impl At {
    pub(crate) fn of(n: usize, count: usize) -> Self {
        Self { n, last: n + 1 >= count }
    }
}

/// The web's `.shadow-sm`.
pub(crate) fn shadow_sm() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(3.0),
            spread_radius: px(0.0),
            inset: false,
        },
        BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(2.0),
            spread_radius: px(-1.0),
            inset: false,
        },
    ]
}

/// The web's `.shadow-lg`.
pub(crate) fn shadow_lg() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(10.0)),
            blur_radius: px(15.0),
            spread_radius: px(-3.0),
            inset: false,
        },
        BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(4.0)),
            blur_radius: px(6.0),
            spread_radius: px(-4.0),
            inset: false,
        },
    ]
}

/// The web's `.shadow-xl`.
pub(crate) fn shadow_xl() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(20.0)),
            blur_radius: px(25.0),
            spread_radius: px(-5.0),
            inset: false,
        },
        BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(8.0)),
            blur_radius: px(10.0),
            spread_radius: px(-6.0),
            inset: false,
        },
    ]
}

/// A small uppercase heading (`text-[0.7rem] font-bold tracking-wide uppercase`).
pub(crate) fn caps(text: &str, p: &Palette) -> Div {
    div()
        .text_size(px(11.2))
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .child(text.to_uppercase())
}

/// A muted line under a setting (`text-sm text-muted-foreground`).
pub(crate) fn hint(text: impl Into<SharedString>, p: &Palette) -> AnyElement {
    div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(text.into()).into_any_element()
}

/// A line that warns (`font-bold text-destructive`).
pub(crate) fn warn(text: impl Into<SharedString>, p: &Palette) -> AnyElement {
    div()
        .text_sm()
        .line_height(px(20.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.destructive)
        .child(text.into())
        .into_any_element()
}

/// A soft panel under a setting (`rounded-xl bg-muted/50 px-3 py-2 text-sm`).
pub(crate) fn sample(p: &Palette) -> Div {
    div().rounded(radius_xl()).bg(alpha(p.muted, 0.5)).px(px(12.0)).py(px(8.0)).text_sm().line_height(px(20.0))
}

/// A setting found by search glows where it lands.
fn glow<E: IntoElement + gpui_kit::Styled + 'static>(el: E, id: &str, p: &Palette) -> AnyElement {
    let color = p.primary;
    motion::once(el, SharedString::from(format!("found-{id}")), Duration::from_millis(1800), move |el, t| {
        // 0..15% in, held to 60%, out by 100%.
        let k = if t < 0.15 {
            t / 0.15
        } else if t < 0.6 {
            1.0
        } else {
            1.0 - (t - 0.6) / 0.4
        };
        el.bg(alpha(color, 0.12 * k)).rounded(px(12.0))
    })
}

impl SettingsView {
    /// One setting: its title and hint, whether it follows the default (and the way back), and
    /// its controls, under a rule. It rises in `delay` after the page.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn setting(
        &self,
        id: &'static str,
        title: &str,
        hint: Option<AnyElement>,
        badge: Badge,
        at: At,
        body: impl IntoElement,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let badge = self.badge(id, badge, p, cx);
        let el = div()
            .id(SharedString::from(format!("setting-{id}")))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .pt(px(if at.n == 0 { 0.0 } else { 24.0 }))
            .pb(px(24.0))
            .when(!at.last, |el| el.border_b_1().border_color(alpha(p.border, 0.7)))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div().font_weight(FontWeight::EXTRA_BOLD).line_height(px(24.0)).child(title.to_owned()),
                            )
                            .when_some(hint, |el, h| el.child(div().mt(px(2.0)).child(h))),
                    )
                    .child(badge),
            )
            .child(body);
        let el = self.found_mark(id, el, p);
        motion::rise(
            div().child(el),
            SharedString::from(format!("setting-in-{id}")),
            Duration::from_millis((40 * at.n.min(8)) as u64),
            12.0,
        )
        .into_any_element()
    }

    /// Marks an element as a search target: its bounds are kept for scrolling there, and it
    /// glows when it's the one picked.
    pub(crate) fn found_mark<E: IntoElement + gpui_kit::ParentElement + gpui_kit::Styled + 'static>(
        &self,
        id: &'static str,
        el: E,
        p: &Palette,
    ) -> AnyElement {
        use gpui_kit::base::ElementExt as _;
        let places = self.places.clone();
        let el = el.on_prepaint(move |bounds, _, _| {
            places.borrow_mut().insert(id, bounds);
        });
        match &self.glow {
            Some((glowing, n)) if *glowing == id => glow(el, &format!("{id}-{n}"), p),
            _ => el.into_any_element(),
        }
    }

    fn badge(&self, id: &str, badge: Badge, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (changed, reset): (bool, Rc<dyn Fn(&mut V, &mut Context<V>)>) = match badge {
            Badge::None => return div().into_any_element(),
            Badge::Pref { changed, reset } => (
                changed,
                Rc::new(move |this: &mut V, cx: &mut Context<V>| {
                    let reset = reset.clone();
                    this.set(cx, move |pr| reset(pr));
                }),
            ),
            Badge::Custom { changed, reset } => (changed, reset),
        };
        let pill = |text: String, bg: Hsla, fg: Rgba| {
            div()
                .flex_none()
                .rounded_full()
                .bg(bg)
                .px(px(8.0))
                .py(px(2.0))
                .text_size(px(10.4))
                .line_height(px(14.0))
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(text.to_uppercase())
        };
        if changed {
            let fg = p.foreground;
            let hover = p.accent;
            motion::once(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .child(pill(t("settings.controls.changed"), alpha(p.primary, 0.15), p.primary))
                    .child(
                        div()
                            .id(SharedString::from(format!("reset-{id}")))
                            .h(px(28.0))
                            .px(px(8.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(fg)
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| reset(this, cx)))
                            .child(icon("rotate-ccw").size(px(14.0)))
                            .child(t("settings.controls.reset")),
                    ),
                SharedString::from(format!("badge-{id}-changed")),
                Duration::from_millis(260),
                |el, t| el.opacity(t),
            )
        } else {
            motion::once(
                pill(t("settings.controls.default"), p.muted.into(), p.muted_foreground),
                SharedString::from(format!("badge-{id}-default")),
                Duration::from_millis(260),
                |el, t| el.opacity(t),
            )
        }
    }

    /// One field of a form, as a flat row under a rule (account pages' `Row`).
    pub(crate) fn row(
        &self,
        id: &'static str,
        label: &str,
        hint: Option<AnyElement>,
        at: At,
        body: impl IntoElement,
        p: &Palette,
    ) -> AnyElement {
        let el = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pt(px(if at.n == 0 { 0.0 } else { 20.0 }))
            .pb(px(20.0))
            .when(!at.last, |el| el.border_b_1().border_color(alpha(p.border, 0.7)))
            .child(div().text_sm().line_height(px(14.0)).font_weight(FontWeight::EXTRA_BOLD).child(label.to_owned()))
            .child(body)
            .when_some(hint, |el, h| el.child(h));
        self.found_mark(id, el, p)
    }
}

/// The web's switch (`h-5 w-8`): the primary when on, the input color when off, and the thumb
/// springs across.
pub(crate) fn switch(
    id: impl Into<SharedString>,
    on: bool,
    disabled: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<V>,
    set: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let x = motion::follow(SharedString::from(format!("{id}-x")), if on { 13.0 } else { 1.0 }, window, cx);
    div()
        .id(ElementId::Name(id))
        .flex_none()
        .relative()
        .w(px(32.0))
        .h(px(20.0))
        .rounded_full()
        .bg(if on { p.primary } else { p.border })
        .when(disabled, |el| el.opacity(0.5))
        .when(!disabled, |el| {
            el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                set(this, !on, cx)
            }))
        })
        .child(div().absolute().top(px(2.0)).left(px(x + 1.0)).size(px(16.0)).rounded_full().bg(if p.dark && !on {
            p.foreground
        } else if p.dark {
            p.primary_foreground
        } else {
            p.background
        }))
}

/// A labelled switch: the words flip it too (`Toggle`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn toggle(
    id: &'static str,
    label: &str,
    hint: Option<&str>,
    on: bool,
    disabled: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<V>,
    set: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> AnyElement {
    let set = Rc::new(set);
    let flip = set.clone();
    div()
        .id(SharedString::from(format!("toggle-{id}")))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .when(disabled, |el| el.opacity(0.6))
        .when(!disabled, |el| el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| flip(this, !on, cx))))
        .child(
            div()
                .min_w_0()
                .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(label.to_owned()))
                .when_some(hint, |el, h| {
                    el.child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(h.to_owned()))
                }),
        )
        .child(switch(SharedString::from(format!("switch-{id}")), on, disabled, p, window, cx, move |this, v, cx| {
            set(this, v, cx)
        }))
        .into_any_element()
}

/// One card in a [`choice`].
pub(crate) struct Opt {
    pub label: String,
    pub hint: String,
    pub glyph: &'static str,
    /// Why it can't be picked now, in place of the hint.
    pub disabled: Option<String>,
}

impl Opt {
    pub(crate) fn new(label: impl Into<String>, hint: impl Into<String>, glyph: &'static str) -> Self {
        Self { label: label.into(), hint: hint.into(), glyph, disabled: None }
    }
}

/// How many option cards share a row, as the web's grid has them at this width.
fn columns(n: usize) -> usize {
    match n {
        2 => 2,
        4 => 4,
        _ => 3,
    }
}

/// A row (or rows) of option cards; the selection glides between them (`Choice`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn choice(
    id: &'static str,
    chosen: Option<usize>,
    options: Vec<Opt>,
    width: f32,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<V>,
    pick: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> AnyElement {
    let gap = 8.0;
    let cols = columns(options.len()).min(options.len().max(1));
    let card_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
    let pick = Rc::new(pick);
    // The cards in a row are as tall as the tallest; the highlight glides between places.
    let at = chosen.map(|i| ((i % cols) as f32 * (card_w + gap), (i / cols) as f32));
    let x = motion::follow(SharedString::from(format!("choice-{id}-x")), at.map_or(0.0, |a| a.0), window, cx);
    let row_of = at.map_or(0.0, |a| a.1);
    let mut rows = div().flex().flex_col().gap(px(gap));
    for (r, chunk) in options.chunks(cols).enumerate() {
        let mut row = div().relative().flex().gap(px(gap));
        if at.is_some() && (row_of - r as f32).abs() < 0.5 {
            row = row.child(
                div()
                    .absolute()
                    .top(px(-1.0))
                    .bottom(px(-1.0))
                    .left(px(x - 1.0))
                    .w(px(card_w + 2.0))
                    .rounded(px(f32::from(radius_xl()) + 1.0))
                    .border_2()
                    .border_color(alpha(p.primary, 0.5))
                    .bg(alpha(p.primary, 0.1)),
            );
        }
        for (c, option) in chunk.iter().enumerate() {
            let i = r * cols + c;
            let active = chosen == Some(i);
            let disabled = option.disabled.is_some();
            let pick = pick.clone();
            let hover = alpha(p.primary, 0.3);
            row = row.child(
                div()
                    .id(SharedString::from(format!("choice-{id}-{i}")))
                    .relative()
                    .w(px(card_w))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(6.0))
                    .p(px(12.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(if active { alpha(p.primary, 0.6) } else { p.border.into() })
                    .when(disabled, |el| el.opacity(0.5))
                    .when(!disabled, |el| {
                        el.cursor_pointer()
                            .when(!active, |el| el.hover(move |s| s.border_color(hover)))
                            .active(|s| s.top(px(1.0)))
                            .on_click(cx.listener(move |this, _, _, cx| pick(this, i, cx)))
                    })
                    .child(
                        div()
                            .size(px(32.0))
                            .rounded(radius_lg())
                            .flex()
                            .items_center()
                            .justify_center()
                            .map(|el| {
                                if active {
                                    el.bg(p.primary).text_color(p.primary_foreground)
                                } else {
                                    el.bg(p.muted).text_color(p.muted_foreground)
                                }
                            })
                            .child(motion::once(
                                div().child(icon(option.glyph).size(px(16.0))),
                                SharedString::from(format!("choice-{id}-{i}-{active}")),
                                Duration::from_millis(360),
                                move |el, t| {
                                    if active {
                                        let k = 1.0 - (1.0 - t).powi(3);
                                        el.opacity(0.4 + 0.6 * k)
                                    } else {
                                        el
                                    }
                                },
                            )),
                    )
                    .child(
                        div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(option.label.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(option.disabled.clone().unwrap_or_else(|| option.hint.clone())),
                    ),
            );
        }
        rows = rows.child(row);
    }
    rows.into_any_element()
}

/// A few options side by side in a pill; the highlight glides to the chosen one (`Segmented`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn segmented(
    id: &'static str,
    options: Vec<(String, Option<&'static str>)>,
    chosen: usize,
    item_w: f32,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<V>,
    pick: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> AnyElement {
    let x = motion::follow(SharedString::from(format!("seg-{id}")), chosen as f32 * item_w, window, cx);
    let pick = Rc::new(pick);
    let mut row = div().relative().flex().p(px(4.0)).rounded(radius_xl()).bg(p.muted).child(
        div()
            .absolute()
            .top(px(4.0))
            .bottom(px(4.0))
            .left(px(4.0 + x))
            .w(px(item_w))
            .rounded(radius_lg())
            .bg(p.background)
            .shadow(shadow_sm()),
    );
    for (n, (label, glyph)) in options.into_iter().enumerate() {
        let on = n == chosen;
        let pick = pick.clone();
        let fg = p.foreground;
        row = row.child(
            div()
                .id(SharedString::from(format!("seg-{id}-{n}")))
                .relative()
                .w(px(item_w))
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .text_color(if on { p.foreground } else { p.muted_foreground })
                .cursor_pointer()
                .hover(move |s| s.text_color(fg))
                .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                .when_some(glyph, |el, g| el.child(icon(g).size(px(14.0))))
                .child(label),
        );
    }
    row.into_any_element()
}

/// Small pill choices; the chosen one fills in with a check (`Chips`).
pub(crate) fn chips(
    id: &'static str,
    options: Vec<String>,
    chosen: usize,
    p: &Palette,
    cx: &mut Context<V>,
    pick: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> AnyElement {
    let pick = Rc::new(pick);
    div()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .children(options.into_iter().enumerate().map(|(n, label)| {
            let on = n == chosen;
            let pick = pick.clone();
            let (hover_border, fg) = (alpha(p.primary, 0.4), p.foreground);
            div()
                .id(SharedString::from(format!("chip-{id}-{n}")))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .px(px(12.0))
                .py(px(4.0))
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .map(|el| {
                    if on {
                        el.border_color(p.primary).bg(p.primary).text_color(p.primary_foreground)
                    } else {
                        el.border_color(p.border)
                            .text_color(p.muted_foreground)
                            .hover(move |s| s.border_color(hover_border).text_color(fg))
                    }
                })
                .active(|s| s.top(px(1.0)))
                .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                .when(on, |el| {
                    el.child(motion::once(
                        div().mr(px(4.0)).child(icon("check").size(px(12.0))),
                        SharedString::from(format!("chip-{id}-{n}-on")),
                        Duration::from_millis(260),
                        |el, t| el.opacity(t),
                    ))
                })
                .child(label)
        }))
        .into_any_element()
}

/// The shadcn button variants the web's settings use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    Primary,
    Outline,
    Ghost,
    Destructive,
    /// An outline in the destructive color (`border-destructive/40 text-destructive`).
    DangerOutline,
}

/// A shadcn button: `small` is `size="sm"` (h-8), otherwise h-9. The `.btn` lift comes with
/// primary ones, as the web's settings add it.
pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: Option<&'static str>,
    look: Look,
    small: bool,
    p: &Palette,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    let (bg, fg, border): (Hsla, Rgba, Option<Hsla>) = match look {
        Look::Primary => (p.primary.into(), p.primary_foreground, None),
        Look::Outline => (p.background.into(), p.foreground, Some(p.border.into())),
        Look::Ghost => (alpha(p.background, 0.0), p.foreground, None),
        Look::Destructive => (p.destructive.into(), gpui_kit::rgb(0xffffff), None),
        Look::DangerOutline => (p.background.into(), p.destructive, Some(alpha(p.destructive, 0.4))),
    };
    let hover = match look {
        Look::Primary => alpha(p.primary, 0.9),
        Look::Destructive => alpha(p.destructive, 0.9),
        Look::DangerOutline => alpha(p.destructive, 0.1),
        _ => p.accent.into(),
    };
    let glow = alpha(p.primary, 1.0);
    div()
        .id(id)
        .flex_none()
        .h(px(if small { 32.0 } else { 36.0 }))
        .px(px(match (small, glyph.is_some()) {
            (true, true) => 10.0,
            (true, false) => 12.0,
            (false, true) => 12.0,
            (false, false) => 16.0,
        }))
        .rounded(radius_md())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(if small { 6.0 } else { 8.0 }))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .bg(bg)
        .text_color(fg)
        .when_some(border, |el, b| el.border_1().border_color(b))
        .when(matches!(look, Look::Outline | Look::DangerOutline), |el| {
            el.shadow(vec![BoxShadow {
                color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.05),
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(2.0),
                spread_radius: px(0.0),
                inset: false,
            }])
        })
        .cursor_pointer()
        .map(|el| {
            if look == Look::Primary {
                el.hover(move |s| {
                    s.bg(hover).top(px(-2.0)).shadow(vec![BoxShadow {
                        color: glow,
                        offset: point(px(0.0), px(8.0)),
                        blur_radius: px(22.0),
                        spread_radius: px(-8.0),
                        inset: false,
                    }])
                })
            } else {
                el.hover(move |s| s.bg(hover))
            }
        })
        .active(|s| s.top(px(1.0)))
        .when_some(glyph, |el, g| el.child(icon(g).size(px(16.0))))
        .when(!label.is_empty(), |el| el.child(label))
}

/// A text field as the web draws its inputs: bordered, `h-11 rounded-xl` unless told otherwise.
pub(crate) fn field(input: impl IntoElement, p: &Palette) -> Div {
    div()
        .h(px(44.0))
        .w_full()
        .rounded(radius_xl())
        .border_1()
        .border_color(p.border)
        .bg(p.background)
        .px(px(12.0))
        .flex()
        .items_center()
        .text_sm()
        .shadow(vec![BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.05),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(2.0),
            spread_radius: px(0.0),
            inset: false,
        }])
        .child(div().flex_1().min_w_0().child(input))
}

/// A form with a live preview beside it, like the web's `WithPreview`: on the right at the
/// web's `xl` width (with a "Preview" label), above the form below it.
pub(crate) fn with_preview(form: impl IntoElement, preview: impl IntoElement, wide: bool, p: &Palette) -> AnyElement {
    let aside = motion::rise(
        div().flex().flex_col().child(caps(&t("settings.controls.preview"), p).mb(px(8.0))).child(preview),
        "with-preview",
        Duration::from_millis(80),
        12.0,
    );
    if wide {
        div()
            .flex()
            .items_start()
            .gap(px(40.0))
            .child(div().flex_1().min_w_0().child(form))
            .child(div().w(px(288.0)).flex_none().child(aside))
            .into_any_element()
    } else {
        div().flex().flex_col().gap(px(24.0)).child(aside).child(form).into_any_element()
    }
}

/// The bar that slides up while there are unsaved changes (`SaveBar`). `alarm` while someone
/// tried to leave: it shakes and turns red.
#[allow(clippy::too_many_arguments)]
pub(crate) fn save_bar(
    id: &'static str,
    count: usize,
    saving: bool,
    error: Option<&str>,
    alarm: Option<u32>,
    p: &Palette,
    cx: &mut Context<V>,
    save: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    discard: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    if count == 0 {
        return div().into_any_element();
    }
    let alarmed = alarm.is_some();
    let line: AnyElement = if let Some(e) = error {
        div().text_color(p.destructive).child(e.to_owned()).into_any_element()
    } else if alarmed {
        div()
            .font_weight(FontWeight::BOLD)
            .text_color(p.destructive)
            .child(t("settings.controls.careful"))
            .into_any_element()
    } else {
        div()
            .flex()
            .gap(px(4.0))
            .child(div().font_weight(FontWeight::BOLD).child(t("settings.controls.unsaved")))
            .child(
                div()
                    .text_color(p.muted_foreground)
                    .child(t_with("settings.controls.unsavedCount", &[("count", Arg::Num(count as i64))])),
            )
            .into_any_element()
    };
    let bar = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(12.0))
        .rounded(crate::ui::theme::radius_2xl())
        .border_1()
        .border_color(if alarmed { alpha(p.destructive, 0.7) } else { p.border.into() })
        .bg(if alarmed { mix(p.card, p.destructive, 0.1) } else { alpha(p.card, 0.95) })
        .p(px(12.0))
        .pl(px(16.0))
        .shadow(shadow_xl())
        .child(div().flex_1().min_w_0().text_sm().line_height(px(20.0)).child(line))
        .child(
            button(
                SharedString::from(format!("{id}-discard")),
                t("settings.controls.discard"),
                None,
                Look::Ghost,
                true,
                p,
            )
            .rounded(radius_xl())
            .when(!saving, |el| el.on_click(cx.listener(move |this, _, w, cx| discard(this, w, cx)))),
        )
        .child(
            button(
                SharedString::from(format!("{id}-save")),
                if saving { t("settings.controls.saving") } else { t("settings.controls.save") },
                None,
                Look::Primary,
                true,
                p,
            )
            .rounded(radius_xl())
            .px(px(16.0))
            .font_weight(FontWeight::BOLD)
            .when(!saving, |el| el.on_click(cx.listener(move |this, _, w, cx| save(this, w, cx)))),
        );
    let bar: AnyElement = match alarm {
        Some(n) => {
            motion::once(bar, SharedString::from(format!("{id}-shake-{n}")), Duration::from_millis(500), |el, t| {
                let x = [0.0, -10.0, 10.0, -8.0, 8.0, -4.0, 4.0, 0.0];
                let at = t * 7.0;
                let i = (at.floor() as usize).min(6);
                let f = at - i as f32;
                el.relative().left(px(x[i] + (x[i + 1] - x[i]) * f))
            })
        }
        None => bar.into_any_element(),
    };
    motion::rise(
        div().mt(px(24.0)).pb(px(8.0)).child(bar),
        SharedString::from(format!("{id}-bar")),
        Duration::ZERO,
        80.0,
    )
    .into_any_element()
}

/// Keys on a keyboard (`.keycap`), for a shortcut.
pub(crate) fn keycaps(combo: &str, p: &Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .children(crate::core::keybinds::keycaps(combo).into_iter().map(|cap| {
            div()
                .min_w(px(25.6))
                .h(px(25.6))
                .px(px(6.4))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.2))
                .border_1()
                .border_b(px(3.0))
                .border_color(p.border)
                .bg(p.card)
                .text_color(p.foreground)
                .text_xs()
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(cap)
        }))
        .into_any_element()
}

/// The sliders the pages show, kept between frames: each one's state and what it sets.
#[derive(Default)]
pub(crate) struct Sliders {
    states: HashMap<&'static str, Entity<SliderState>>,
    held: HashMap<&'static str, bool>,
    _subscriptions: Vec<Subscription>,
}

/// A mark under a slider's track: a value and its label.
pub(crate) type Mark = (f32, String);

impl SettingsView {
    /// A slider on the web's look: a muted track filling with the primary, a ringed thumb, the
    /// value in a bubble while it's held, and marks under it that jump there. `set` runs as it
    /// moves (or, with `on_release`, once it's let go).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn slider(
        &mut self,
        id: &'static str,
        (min, max, step): (f32, f32, f32),
        value: f32,
        marks: Vec<Mark>,
        format: fn(f32) -> String,
        on_release: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
        set: fn(&mut Prefs, f32),
    ) -> AnyElement {
        let state = match self.sliders.states.get(id) {
            Some(state) => state.clone(),
            None => {
                let state = cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(value));
                let sub = cx.subscribe_in(&state, window, move |this, _, event: &SliderEvent, _, cx| match event {
                    SliderEvent::Change(v) => {
                        this.sliders.held.insert(id, true);
                        if !on_release {
                            let v = v.end();
                            this.set(cx, move |pr| set(pr, v));
                        } else {
                            cx.notify();
                        }
                    }
                    SliderEvent::Release(v) => {
                        this.sliders.held.insert(id, false);
                        let v = v.end();
                        this.set(cx, move |pr| set(pr, v));
                    }
                });
                self.sliders._subscriptions.push(sub);
                self.sliders.states.insert(id, state.clone());
                state
            }
        };
        let held = self.sliders.held.get(id).copied().unwrap_or(false);
        let shown = state.read(cx).value().end();
        // Something else changed the setting (a reset): the thumb follows, unless it's held.
        if !held && (shown - value).abs() > step / 2.0 {
            state.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        let now = if held { shown } else { value };
        let frac = ((now - min) / (max - min)).clamp(0.0, 1.0);
        let grow =
            motion::follow(SharedString::from(format!("slider-{id}-held")), if held { 1.0 } else { 0.0 }, window, cx);
        let thumb = SliderThumb::new(&state)
            .absolute()
            .top(px(-6.0 - 2.5 * grow))
            .left(relative(frac))
            .ml(px(-10.0 - 2.5 * grow))
            .size(px(20.0 + 5.0 * grow))
            .rounded_full()
            .border(px(3.0))
            .border_color(p.primary)
            .bg(p.background)
            .shadow(vec![
                BoxShadow {
                    color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
                    offset: point(px(0.0), px(4.0)),
                    blur_radius: px(6.0),
                    spread_radius: px(-1.0),
                    inset: false,
                },
                BoxShadow {
                    color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
                    offset: point(px(0.0), px(2.0)),
                    blur_radius: px(4.0),
                    spread_radius: px(-2.0),
                    inset: false,
                },
            ])
            .cursor_pointer();
        let bubble = held.then(|| {
            div().absolute().bottom(px(20.0)).left(relative(frac)).child(
                div().relative().left(px(-40.0)).w(px(80.0)).flex().justify_center().child(
                    div()
                        .rounded(radius_lg())
                        .bg(p.primary)
                        .px(px(8.0))
                        .py(px(2.0))
                        .text_xs()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(p.primary_foreground)
                        .shadow(shadow_sm())
                        .child(format(now)),
                ),
            )
        });
        let track =
            SliderTrack::new(&state).relative().w_full().h(px(20.0)).flex().items_center().cursor_pointer().child(
                SliderIndicator::new(&state)
                    .relative()
                    .w_full()
                    .h(px(8.0))
                    .rounded_full()
                    .bg(p.muted)
                    .child(div().absolute().top_0().bottom_0().left_0().w(relative(frac)).rounded_full().bg(p.primary))
                    .child(thumb),
            );
        let marks_row = (!marks.is_empty()).then(|| {
            let mut row = div().relative().mt(px(6.0)).h(px(16.0)).text_size(px(11.2)).text_color(p.muted_foreground);
            for (n, (at, label)) in marks.into_iter().enumerate() {
                let f = ((at - min) / (max - min)).clamp(0.0, 1.0);
                let on = (at - now).abs() < step / 2.0;
                let fg = p.foreground;
                row = row.child(
                    div().absolute().top_0().left(relative(f)).child(
                        div()
                            .id(SharedString::from(format!("mark-{id}-{n}")))
                            .relative()
                            .left(px(-30.0))
                            .w(px(60.0))
                            .flex()
                            .justify_center()
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .when(on, |el| el.font_weight(FontWeight::BOLD).text_color(p.primary))
                            .when(!on, |el| el.hover(move |s| s.text_color(fg)))
                            .on_click(cx.listener(move |this, _, _, cx| this.set(cx, move |pr| set(pr, at))))
                            .child(label),
                    ),
                );
            }
            row
        });
        div()
            .pt(px(28.0))
            .pb(px(4.0))
            .child(BaseSlider::new(&state).relative().w_full().child(track).children(bubble))
            .children(marks_row)
            .into_any_element()
    }
}
