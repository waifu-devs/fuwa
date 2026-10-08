//! The pieces instance settings pages are built from, as in the web's
//! `settings/controls.tsx` (and drawn like the app settings' copies in
//! `settings_controls.rs`): a setting with its Default or Changed badge and
//! Reset, option cards the selection glides between, switches with words,
//! caps that can be off, heads-ups that slide in, text fields, and the save
//! bar.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, BoxShadow, Context, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
};

use super::InstanceSettingsView;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{self as admin, UNITS};
use crate::ui::motion;
use crate::ui::settings_controls::{Look, button, shadow_sm, shadow_xl};
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_3xl, radius_lg, radius_md, radius_xl};
use crate::ui::widgets::icon;

type View = InstanceSettingsView;

/// One card in a [`InstanceSettingsView::choice`].
pub(super) struct Opt {
    pub value: i32,
    pub label: String,
    pub hint: String,
    pub glyph: &'static str,
    /// Why it can't be picked now, in place of the hint.
    pub disabled: Option<String>,
}

impl Opt {
    pub fn new(value: i32, label: impl Into<String>, hint: impl Into<String>, glyph: &'static str) -> Self {
        Self { value, label: label.into(), hint: hint.into(), glyph, disabled: None }
    }

    pub fn unless(mut self, blocked: bool, why: impl Into<String>) -> Self {
        if blocked {
            self.disabled = Some(why.into());
        }
        self
    }
}

/// A text field as the instance pages draw theirs (`h-10 rounded-xl`), with an icon in front when
/// given one (`pl-9`).
pub(super) fn input_box(input: impl IntoElement, glyph: Option<&'static str>, p: &Palette) -> Div {
    div()
        .relative()
        .h(px(40.0))
        .w_full()
        .rounded(radius_xl())
        .border_1()
        .border_color(p.border)
        .flex()
        .items_center()
        .pl(px(if glyph.is_some() { 35.0 } else { 11.0 }))
        .pr(px(11.0))
        .text_sm()
        .when_some(glyph, |el, g| {
            el.child(
                div()
                    .absolute()
                    .left(px(11.0))
                    .top(px(11.0))
                    .child(icon(g).size(px(16.0)).text_color(p.muted_foreground)),
            )
        })
        .child(div().flex_1().min_w_0().child(input))
}

/// A box of several lines (`Textarea`, `rounded-xl`, `px-3 py-2`), with an icon at its top left
/// when given one (`pl-9`).
pub(super) fn area_box(input: impl IntoElement, glyph: Option<&'static str>, p: &Palette) -> Div {
    div()
        .relative()
        .w_full()
        .rounded(radius_xl())
        .border_1()
        .border_color(p.border)
        // The editor keeps its own 12px across and 8px down (the web's `px-3 py-2`).
        .pl(px(if glyph.is_some() { 23.0 } else { 0.0 }))
        .text_sm()
        .when_some(glyph, |el, g| {
            el.child(
                div()
                    .absolute()
                    .left(px(11.0))
                    .top(px(11.0))
                    .child(icon(g).size(px(16.0)).text_color(p.muted_foreground)),
            )
        })
        .child(input)
}

/// A muted block standing in while a page loads (`.shimmer`).
pub(super) fn shimmer(n: usize, height: f32, p: &Palette) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let (muted, card) = (p.muted, p.card);
    div()
        .h(px(height))
        .rounded(radius_2xl())
        .bg(muted)
        .with_animation(
            SharedString::from(format!("ishimmer-{n}")),
            Animation::new(Duration::from_millis(1400)).repeat(),
            move |el, t| {
                // The light band sweeps across: here, as the block's color easing toward it and back.
                let k = (t * std::f32::consts::PI).sin();
                el.bg(mix(muted, card, 0.5 * k))
            },
        )
        .into_any_element()
}

/// The web's switch (`h-5 w-8`): the primary when on, the input color when off, and the thumb
/// springs across.
pub(super) fn switch(
    id: impl Into<SharedString>,
    on: bool,
    disabled: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<View>,
    set: impl Fn(&mut View, bool, &mut Window, &mut Context<View>) + 'static,
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
            el.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                set(this, !on, window, cx)
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

/// A switch whose change also gets the window, for boxes it fills.
pub(super) fn switch_in(
    id: SharedString,
    on: bool,
    disabled: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<View>,
    set: impl Fn(&mut View, bool, &mut Window, &mut Context<View>) + 'static,
) -> Stateful<Div> {
    switch(id, on, disabled, p, window, cx, set)
}

/// A heads-up under a setting (`rounded-xl bg-amber-500/10 px-3 py-2 text-xs`), sliding in.
pub(super) fn notice_el(id: &str, text: &str, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .px(px(12.0))
            .py(px(8.0))
            .rounded(radius_xl())
            .bg(gpui_kit::hsla(38.0 / 360.0, 0.92, 0.5, 0.1))
            .text_xs()
            .line_height(px(16.0))
            .text_color(amber_text(p))
            .child(text.to_owned()),
        SharedString::from(format!("inotice-{id}")),
        Duration::ZERO,
        -6.0,
    )
    .into_any_element()
}

/// The web's `text-amber-700 dark:text-amber-300`.
pub(super) fn amber_text(p: &Palette) -> gpui_kit::Hsla {
    if p.dark { gpui_kit::rgb(0xfcd34d).into() } else { gpui_kit::rgb(0xb45309).into() }
}

/// The web's `text-amber-600 dark:text-amber-400`.
pub(super) fn amber_soft(p: &Palette) -> gpui_kit::Hsla {
    if p.dark { gpui_kit::rgb(0xfbbf24).into() } else { gpui_kit::rgb(0xd97706).into() }
}

/// The bar that slides up while there are unsaved changes (`SaveBar`). `alarm` while someone
/// tried to leave: it shakes and turns red.
#[allow(clippy::too_many_arguments)]
pub(super) fn save_bar(
    id: &'static str,
    count: usize,
    saving: bool,
    error: Option<&str>,
    alarm: Option<u32>,
    p: &Palette,
    cx: &mut Context<View>,
    save: impl Fn(&mut View, &mut Window, &mut Context<View>) + 'static,
    discard: impl Fn(&mut View, &mut Window, &mut Context<View>) + 'static,
) -> AnyElement {
    let alarmed = alarm.is_some();
    let line: AnyElement = if let Some(e) = error {
        let mut text = e.to_owned();
        if let Some(first) = text.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        div().text_color(p.destructive).child(text).into_any_element()
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
        .rounded(radius_2xl())
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
            .when(saving, |el| el.opacity(0.5))
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
            .when(saving, |el| el.opacity(0.5))
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
    motion::rise(div().child(bar), SharedString::from(format!("{id}-bar")), Duration::ZERO, 80.0).into_any_element()
}

/// How many option cards share a row, as the web's grid has them at this width.
fn columns(n: usize) -> usize {
    match n {
        2 => 2,
        4 => 4,
        _ => 3,
    }
}

impl InstanceSettingsView {
    /// One setting: its title and hint, whether it follows the default (with Reset when it
    /// doesn't), and its controls. Settings stack as flat rows with a rule between them; the
    /// first (`n` 0) has no space above. It rises in `n` beats after the page.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn setting(
        &self,
        id: &'static str,
        title: &str,
        hint: Option<&str>,
        paths: &'static [&'static str],
        default: &str,
        n: usize,
        body: impl IntoElement,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.setting_with(
            id,
            title,
            hint.map(|h| div().child(h.to_owned()).into_any_element()),
            paths,
            default,
            n,
            body,
            p,
            cx,
        )
    }

    /// A setting whose hint is drawn by the page (bold words, links).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn setting_with(
        &self,
        id: &'static str,
        title: &str,
        hint: Option<AnyElement>,
        paths: &'static [&'static str],
        default: &str,
        n: usize,
        body: impl IntoElement,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.setting_ruled(id, title, hint, paths, default, n, n > 0, n > 0, body, p, cx)
    }

    /// The first setting of a group that follows others (the web's `first:pt-0` inside a form of
    /// its own): the rule above it, but no room under the rule. Without a default, like the web's
    /// `badge={false}`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn setting_lead(
        &self,
        id: &'static str,
        title: &str,
        hint: Option<&str>,
        n: usize,
        body: impl IntoElement,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hint = hint.map(|h| div().child(h.to_owned()).into_any_element());
        self.setting_ruled(id, title, hint, &[], "", n, true, false, body, p, cx)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn setting_ruled(
        &self,
        id: &'static str,
        title: &str,
        hint: Option<AnyElement>,
        paths: &'static [&'static str],
        default: &str,
        n: usize,
        rule: bool,
        pad: bool,
        body: impl IntoElement,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let el = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .pt(px(if pad { 24.0 } else { 0.0 }))
            .pb(px(24.0))
            .when(rule, |el| el.border_t_1().border_color(alpha(p.border, 0.7)))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::EXTRA_BOLD).line_height(px(24.0)).child(title.to_owned()),
                            )
                            .when_some(hint, |el, hint| {
                                el.child(
                                    div()
                                        .mt(px(2.0))
                                        .text_sm()
                                        .line_height(px(20.0))
                                        .text_color(p.muted_foreground)
                                        .child(hint),
                                )
                            }),
                    )
                    .child(self.reset_badge(paths, default, p, cx)),
            )
            .child(body);
        let el = self.found_mark(id, el, p);
        motion::rise(
            div().child(el),
            SharedString::from(format!("isetting-{id}")),
            Duration::from_millis(40 * n.min(8) as u64),
            12.0,
        )
        .into_any_element()
    }

    /// Marks an element as a search target: its bounds are kept for scrolling there, and it glows
    /// when it's the one picked.
    pub(super) fn found_mark(&self, id: &'static str, el: Div, p: &Palette) -> AnyElement {
        use gpui_kit::base::ElementExt as _;
        let places = self.places.clone();
        let el = el.on_prepaint(move |bounds, _, _| {
            places.borrow_mut().insert(id, bounds);
        });
        match &self.glow {
            Some((glowing, n)) if *glowing == id => {
                let color = p.primary;
                motion::once(
                    el,
                    SharedString::from(format!("ifound-{id}-{n}")),
                    Duration::from_millis(1800),
                    move |el, t| {
                        // 0..15% in, held to 60%, out by 100%.
                        let k = if t < 0.15 {
                            t / 0.15
                        } else if t < 0.6 {
                            1.0
                        } else {
                            1.0 - (t - 0.6) / 0.4
                        };
                        el.bg(alpha(color, 0.12 * k)).rounded(px(12.0))
                    },
                )
            }
            _ => el.into_any_element(),
        }
    }

    /// A row (or rows) of option cards; the selection glides between them (`Choice`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn choice(
        &self,
        id: &'static str,
        value: i32,
        options: Vec<Opt>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Self, i32, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let width = self.column;
        let gap = 8.0;
        let cols = columns(options.len()).min(options.len().max(1));
        let card_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
        let pick = Rc::new(pick);
        let chosen = options.iter().position(|o| o.value == value);
        let at = chosen.map(|i| ((i % cols) as f32 * (card_w + gap), (i / cols) as f32));
        let x = motion::follow(SharedString::from(format!("ichoice-{id}-x")), at.map_or(0.0, |a| a.0), window, cx);
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
                let value = option.value;
                let pick = pick.clone();
                let hover = alpha(p.primary, 0.3);
                row = row.child(
                    div()
                        .id(SharedString::from(format!("ichoice-{id}-{i}")))
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
                                .when(!active, |el| el.hover(move |s| s.border_color(hover).top(px(-2.0))))
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(move |this, _, window, cx| pick(this, value, window, cx)))
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
                                    SharedString::from(format!("ichoice-{id}-{i}-{active}")),
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
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .child(option.label.clone()),
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

    /// A switch with words; the words flip it too (`Toggle`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn toggle(
        &self,
        id: &'static str,
        on: bool,
        disabled: bool,
        label: &str,
        hint: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
        set: fn(&mut crate::pb::InstanceSettings, bool),
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("itoggle-{id}")))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .when(disabled, |el| el.opacity(0.6))
            .when(!disabled, |el| {
                el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.patch(cx, |d| set(d, !on))))
            })
            .child(
                div()
                    .min_w_0()
                    .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(label.to_owned()))
                    .when(!hint.is_empty(), |el| {
                        el.child(
                            div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(hint.to_owned()),
                        )
                    }),
            )
            .child(switch(
                SharedString::from(format!("itoggle-{id}-switch")),
                on,
                disabled,
                p,
                window,
                cx,
                move |this: &mut Self, on, _, cx| this.patch(cx, |d| set(d, on)),
            ))
            .into_any_element()
    }

    /// A cap that's off ("No limit") or a number; sizes take a unit. The number slides in when
    /// it's switched on (`Cap`).
    pub(super) fn cap(
        &self,
        path: &'static str,
        label: &str,
        bytes: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.cap_with(path, label, bytes, None, p, window, cx)
    }

    /// A cap with its own words for off (`placeholder`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cap_with(
        &self,
        path: &'static str,
        label: &str,
        bytes: bool,
        placeholder: Option<&str>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = self.draft.as_ref().and_then(|d| admin::cap(d, path)).is_some();
        let unit = bytes.then(|| self.units.get(path).copied().unwrap_or(1));
        cap_row(
            &format!("icap-{path}"),
            label,
            on,
            placeholder,
            self.caps.get(path),
            unit,
            p,
            window,
            cx,
            move |this, on, window, cx| this.switch_cap(path, bytes, on, window, cx),
            move |this, n, cx| this.pick_unit(path, n, cx),
        )
    }

    /// A value to copy, under a small label, with a button that copies it (`CopyRow`). `shown` is
    /// what's drawn (streamer mode hides addresses), `value` what's copied.
    pub(super) fn copy_row(
        &self,
        id: &str,
        label: &str,
        shown: &str,
        value: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self.copied.as_deref() == Some(id);
        let (id_owned, value) = (id.to_owned(), value.to_owned());
        let green: gpui_kit::Hsla = gpui_kit::rgb(0x10b981).into();
        let (hover_bg, hover_fg) = (p.background, p.foreground);
        motion::slide_in(
            div()
                .flex()
                .min_w_0()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.2))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(label.to_uppercase()),
                )
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_xl())
                        .bg(alpha(p.muted, 0.6))
                        .py(px(4.0))
                        .pr(px(4.0))
                        .pl(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family("monospace")
                                .text_xs()
                                .child(shown.to_owned()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("{id}-copy")))
                                .flex_none()
                                .size(px(32.0))
                                .rounded(radius_lg())
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_color(if copied { green } else { p.muted_foreground.into() })
                                .when(!copied, |el| el.hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(value.clone()));
                                    this.copied = Some(id_owned.clone());
                                    cx.notify();
                                    let id = id_owned.clone();
                                    cx.spawn_in(window, async move |this, cx| {
                                        cx.background_executor().timer(Duration::from_millis(1400)).await;
                                        let _ = this.update(cx, |this, cx| {
                                            if this.copied.as_deref() == Some(id.as_str()) {
                                                this.copied = None;
                                                cx.notify();
                                            }
                                        });
                                    })
                                    .detach();
                                }))
                                .child(motion::once(
                                    div().child(icon(if copied { "check" } else { "copy" }).size(px(16.0))),
                                    SharedString::from(format!("{id}-copied-{copied}")),
                                    Duration::from_millis(300),
                                    |el, t| el.opacity(1.0 - (1.0 - t).powi(3)),
                                )),
                        ),
                ),
            SharedString::from(format!("{id}-row")),
            -8.0,
        )
        .into_any_element()
    }

    /// A heads-up under a setting, sliding in while it applies.
    pub(super) fn notice(&self, id: &str, text: &str, p: &Palette) -> AnyElement {
        notice_el(id, text, p)
    }

    /// A read-only fact as a little pill: "Port 8080".
    pub(super) fn fact(id: String, label: &str, value: &str, good: bool, n: usize, p: &Palette) -> AnyElement {
        motion::once(
            div()
                .flex()
                .gap(px(4.0))
                .px(px(12.0))
                .py(px(4.0))
                .rounded_full()
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.muted, 0.5))
                .text_xs()
                .line_height(px(16.0))
                .child(div().text_color(p.muted_foreground).child(label.to_owned()))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if good { p.primary } else { p.foreground })
                        .child(value.to_owned()),
                ),
            SharedString::from(id),
            Duration::from_millis(200 + 40 * n as u64 + 260),
            move |el, t| {
                // Pops in a beat after the one before it.
                let start = (200.0 + 40.0 * n as f32) / (460.0 + 40.0 * n as f32);
                let k = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - k).powi(3);
                el.opacity(eased).relative().top(px(4.0 * (1.0 - eased)))
            },
        )
    }

    fn overridden_any(&self, paths: &[&str]) -> bool {
        paths.iter().any(|path| self.overridden(path))
    }

    /// "Default" while a setting follows the operator's environment, otherwise "Changed" and
    /// Reset, which puts the default (`default`, in words) back.
    pub(super) fn reset_badge(
        &self,
        paths: &'static [&'static str],
        _default: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Pages that aren't settings have no default to show.
        if paths.is_empty() {
            return div().into_any_element();
        }
        let key = paths.first().copied().unwrap_or_default();
        let pill = |text: String, bg: gpui_kit::Hsla, fg: gpui_kit::Hsla| {
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
        if !self.overridden_any(paths) {
            return motion::once(
                pill(t("settings.controls.default"), p.muted.into(), p.muted_foreground.into()),
                SharedString::from(format!("idefault-{key}")),
                Duration::from_millis(260),
                |el, t| el.opacity(t),
            );
        }
        let saving = self.saving;
        let (fg, hover) = (p.foreground, p.accent);
        motion::once(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.0))
                .child(pill(t("settings.controls.changed"), alpha(p.primary, 0.15), p.primary.into()))
                .child(
                    div()
                        .id(SharedString::from(format!("ireset-{key}")))
                        .h(px(28.0))
                        .px(px(8.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg)
                        .when(saving, |el| el.opacity(0.5))
                        .when(!saving, |el| {
                            el.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.commit(Vec::new(), paths.iter().map(|p| (*p).to_owned()).collect(), window, cx)
                                },
                            ))
                        })
                        .child(icon("rotate-ccw").size(px(14.0)))
                        .child(t("settings.controls.reset")),
                ),
            SharedString::from(format!("ichanged-{key}")),
            Duration::from_millis(260),
            |el, t| el.opacity(t),
        )
    }
}

/// Slides something in from the side, after `delay` (a list's lines one after another).
pub(super) fn slide_after(
    el: impl IntoElement + gpui_kit::Styled + 'static,
    id: SharedString,
    from: f32,
    delay: Duration,
) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let total = delay + Duration::from_millis(420);
    let start = delay.as_secs_f32() / total.as_secs_f32();
    el.with_animation(id, Animation::new(total), move |el, t| {
        let k = if t <= start { 0.0 } else { ((t - start) / (1.0 - start)).clamp(0.0, 1.0) };
        let eased = 1.0 - (1.0 - k).powi(3);
        el.opacity(eased).relative().left(px(from * (1.0 - eased)))
    })
    .into_any_element()
}

/// The web's `Dialog` over the settings screen: `bg-black/50` behind, and a panel (`max-w-md
/// rounded-3xl border bg-card p-6 shadow-2xl`) springing up with its `DialogHeader` (`text-xl
/// font-extrabold`, the description `text-sm text-muted-foreground`, `mb-5`) and the round close
/// button in its corner. A click outside or on the close button calls `close`.
#[allow(clippy::too_many_arguments)]
pub(super) fn dialog(
    id: &str,
    title: String,
    description: Option<String>,
    body: impl IntoElement,
    p: &Palette,
    cx: &mut Context<View>,
    close: impl Fn(&mut View, &mut Context<View>) + 'static,
) -> AnyElement {
    let close = Rc::new(close);
    let (outside, x) = (close.clone(), close);
    let (hover_bg, hover_fg) = (p.muted, p.foreground);
    let panel = div()
        .id(SharedString::from(format!("{id}-panel")))
        .relative()
        .w(px(448.0))
        .rounded(radius_3xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .text_color(p.foreground)
        .p(px(24.0))
        .shadow(vec![BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.25),
            offset: point(px(0.0), px(25.0)),
            blur_radius: px(50.0),
            spread_radius: px(-12.0),
            inset: false,
        }])
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id(SharedString::from(format!("{id}-x")))
                .absolute()
                .top(px(16.0))
                .right(px(16.0))
                .size(px(32.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.muted_foreground)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    x(this, cx)
                }))
                .child(icon("x").size(px(16.0))),
        )
        .child(
            div()
                .mb(px(20.0))
                .pr(px(32.0))
                .child(div().text_xl().line_height(px(28.0)).font_weight(FontWeight::EXTRA_BOLD).child(title))
                .when_some(description, |el, d| {
                    el.child(div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(d))
                }),
        )
        .child(body);
    motion::fade_in(
        div()
            .id(SharedString::from(id.to_owned()))
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5))
            .on_click(cx.listener(move |this, _, _, cx| outside(this, cx)))
            .child(motion::rise(panel, SharedString::from(format!("{id}-rise")), Duration::ZERO, 40.0)),
        SharedString::from(format!("{id}-fade")),
        Duration::from_millis(200),
    )
    .into_any_element()
}

/// The button pairs at a dialog's foot (`flex justify-end gap-2`).
pub(super) fn dialog_buttons() -> Div {
    div().flex().justify_end().gap(px(8.0))
}

/// The web's `Cap`: a switch, the label (`w-24 font-bold`), then "no limit" (or what the cap
/// falls back to) while it's off, or the number (`h-9 w-28 rounded-lg`) and, for sizes, the unit
/// picker (MB, GB, TB, the pick gliding) while it's on.
#[allow(clippy::too_many_arguments)]
pub(super) fn cap_row(
    id: &str,
    label: &str,
    on: bool,
    placeholder: Option<&str>,
    state: Option<&gpui_kit::Entity<gpui_kit::component::input::InputState>>,
    unit: Option<usize>,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<View>,
    switch: impl Fn(&mut View, bool, &mut Window, &mut Context<View>) + 'static,
    pick: impl Fn(&mut View, usize, &mut Context<View>) + 'static,
) -> AnyElement {
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .min_h(px(36.0))
        .child(switch_in(SharedString::from(format!("{id}-switch")), on, false, p, window, cx, switch))
        .child(
            div()
                .w(px(96.0))
                .flex_none()
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .child(label.to_owned()),
        );
    let Some(state) = state else { return row.into_any_element() };
    if !on {
        row = row.child(motion::slide_in(
            div()
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(placeholder.map_or_else(|| t("settings.controls.noLimit"), str::to_owned)),
            SharedString::from(format!("{id}-off")),
            12.0,
        ));
        return row.into_any_element();
    }
    let field = div()
        .w(px(112.0))
        .h(px(36.0))
        .flex_none()
        .rounded(radius_lg())
        .border_1()
        .border_color(p.border)
        .px(px(11.0))
        .flex()
        .items_center()
        .text_sm()
        .child(div().flex_1().min_w_0().child(gpui_kit::component::input::Input::new(state).appearance(false)));
    let mut amount = div().flex().items_center().gap(px(8.0)).child(field);
    if let Some(unit) = unit {
        let widths: Vec<f32> = UNITS.iter().map(|(name, _)| if name.len() > 1 { 37.0 } else { 30.0 }).collect();
        let left: f32 = widths[..unit].iter().sum();
        let x = motion::follow(SharedString::from(format!("{id}-unit")), left, window, cx);
        let mut units = div().relative().flex().p(px(2.0)).rounded(radius_lg()).bg(p.muted).child(
            div()
                .absolute()
                .top(px(2.0))
                .left(px(2.0 + x))
                .w(px(widths[unit]))
                .h(px(24.0))
                .rounded(radius_md())
                .bg(p.background)
                .shadow(shadow_sm()),
        );
        let pick = Rc::new(pick);
        for (n, (name, _)) in UNITS.iter().enumerate() {
            let pick = pick.clone();
            units = units.child(
                div()
                    .id(SharedString::from(format!("{id}-{name}")))
                    .relative()
                    .w(px(widths[n]))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if n == unit { p.foreground } else { p.muted_foreground })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                    .child(*name),
            );
        }
        amount = amount.child(units);
    }
    row.child(motion::slide_in(amount, SharedString::from(format!("{id}-on")), -12.0)).into_any_element()
}

/// How wide a line of the app's font is drawn, for highlights that glide between words.
pub(super) fn text_width(text: &str, size: f32, weight: FontWeight, window: &Window) -> f32 {
    let mut font = gpui_kit::font(crate::ui::theme::FONT);
    font.weight = weight;
    let run = gpui_kit::TextRun {
        len: text.len(),
        font,
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(window.text_system().shape_line(SharedString::from(text.to_owned()), px(size), &[run], None).width)
}

/// The web's `Segmented` (`inline-flex rounded-xl bg-muted p-1`, each option `rounded-lg px-3
/// py-1.5 text-sm font-bold`), the chosen one's background gliding to it.
pub(super) fn segmented(
    id: &str,
    labels: Vec<String>,
    chosen: usize,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<View>,
    pick: impl Fn(&mut View, usize, &mut Window, &mut Context<View>) + 'static,
) -> AnyElement {
    let widths: Vec<f32> = labels.iter().map(|l| text_width(l, 14.0, FontWeight::BOLD, window).ceil() + 24.0).collect();
    let left: f32 = widths[..chosen.min(widths.len())].iter().sum();
    let x = motion::follow(SharedString::from(format!("{id}-x")), left, window, cx);
    let w =
        motion::follow(SharedString::from(format!("{id}-w")), widths.get(chosen).copied().unwrap_or(0.0), window, cx);
    let pick = Rc::new(pick);
    let mut row = div().relative().flex().flex_none().p(px(4.0)).rounded(radius_xl()).bg(p.muted).child(
        div()
            .absolute()
            .top(px(4.0))
            .bottom(px(4.0))
            .left(px(4.0 + x))
            .w(px(w))
            .rounded(radius_lg())
            .bg(p.background)
            .shadow(shadow_sm()),
    );
    for (n, (label, width)) in labels.into_iter().zip(widths).enumerate() {
        let on = n == chosen;
        let pick = pick.clone();
        let fg = p.foreground;
        row = row.child(
            div()
                .id(SharedString::from(format!("{id}-{n}")))
                .relative()
                .w(px(width))
                .py(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .text_color(if on { p.foreground } else { p.muted_foreground })
                .cursor_pointer()
                .hover(move |s| s.text_color(fg))
                .active(|s| s.opacity(0.85))
                .on_click(cx.listener(move |this, _, window, cx| pick(this, n, window, cx)))
                .child(label),
        );
    }
    row.into_any_element()
}

/// One of the web's `Chips` (`rounded-full border px-3 py-1 text-xs font-bold`): filled in with a
/// check when it's the one picked.
pub(super) fn choice_chip(id: SharedString, label: &str, on: bool, p: &Palette) -> Stateful<Div> {
    let (hover_edge, hover_fg) = (alpha(p.primary, 0.4), p.foreground);
    div()
        .id(id.clone())
        .flex()
        .items_center()
        .px(px(12.0))
        .py(px(4.0))
        .rounded_full()
        .border_1()
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
                    .hover(move |s| s.border_color(hover_edge).text_color(hover_fg))
            }
        })
        .active(|s| s.opacity(0.85))
        .when(on, |el| {
            el.child(motion::once(
                div().mr(px(4.0)).child(icon("check").size(px(12.0))),
                SharedString::from(format!("{id}-check")),
                Duration::from_millis(260),
                |el, t| el.opacity(t),
            ))
        })
        .child(label.to_owned())
}
