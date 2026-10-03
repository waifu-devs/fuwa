//! The pieces instance settings pages are built from, as in the web's
//! `settings/controls.tsx`: a setting with its "changed" badge, option cards
//! the selection glides between, switches with words, caps that can be off,
//! and heads-ups that slide in.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::InstanceSettingsView;
use crate::core::instance_admin::{self as admin, UNITS};
use crate::ui::motion;
use crate::ui::server_settings::{amber, pill};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::icon;

type View = InstanceSettingsView;

/// One card in a [`InstanceSettingsView::choice`].
pub(super) struct Opt {
    pub value: i32,
    pub label: &'static str,
    pub hint: &'static str,
    pub glyph: &'static str,
    /// Why it can't be picked now, in place of the hint.
    pub disabled: Option<&'static str>,
}

impl Opt {
    pub fn new(value: i32, label: &'static str, hint: &'static str, glyph: &'static str) -> Self {
        Self { value, label, hint, glyph, disabled: None }
    }

    pub fn unless(mut self, blocked: bool, why: &'static str) -> Self {
        if blocked {
            self.disabled = Some(why);
        }
        self
    }
}

/// A switch whose change also gets the window, for boxes it fills.
pub(super) fn switch_in(
    id: SharedString,
    on: bool,
    disabled: bool,
    cx: &mut Context<View>,
    set: impl Fn(&mut View, bool, &mut Window, &mut Context<View>) + 'static,
) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    let set = Rc::new(set);
    Switch::new(id).checked(on).disabled(disabled).on_change(move |checked, window, cx| {
        let (set, checked) = (set.clone(), *checked);
        let _ = entity.update(cx, |this, cx| set(this, checked, window, cx));
    })
}

/// Whether a default reads well beside "Back to the default:"; longer ones get a line of their own.
fn short(default: &str) -> bool {
    default.chars().count() <= 36
}

/// The width the settings column has, which option cards share.
const COLUMN: f32 = 720.0;

impl InstanceSettingsView {
    /// One setting: its title and hint, whether it follows the default (with the way back when it
    /// doesn't), and its controls. It rises in `n` beats after the page.
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
        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .py(px(22.0))
                .border_b_1()
                .border_color(alpha(p.border, 0.7))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title.to_owned()))
                                .when_some(hint, |el, hint| {
                                    el.child(
                                        div()
                                            .mt(px(2.0))
                                            .text_sm()
                                            .text_color(p.muted_foreground)
                                            .child(hint.to_owned()),
                                    )
                                })
                                .when(self.overridden_any(paths) && !short(default), |el| {
                                    el.child(
                                        div()
                                            .mt(px(2.0))
                                            .text_xs()
                                            .text_color(p.muted_foreground)
                                            .child(format!("The default is {default}.")),
                                    )
                                }),
                        )
                        .child(self.reset_badge(paths, default, p, cx)),
                )
                .child(body),
            SharedString::from(format!("isetting-{id}")),
            Duration::from_millis(40 * n as u64),
            12.0,
        )
        .into_any_element()
    }

    /// A row of option cards; the selection glides between them.
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
        let gap = 8.0;
        let n = options.len().max(1) as f32;
        let width = (COLUMN - gap * (n - 1.0)) / n;
        let at = options.iter().position(|o| o.value == value);
        let target = at.map_or(0.0, |i| i as f32 * (width + gap));
        let x = motion::follow(SharedString::from(format!("ichoice-{id}")), target, window, cx);
        let pick = Rc::new(pick);
        let mut row = div().relative().flex().gap(px(gap)).when(at.is_some(), |el| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(x))
                    .w(px(width))
                    .rounded(corner(12.0))
                    .border_2()
                    .border_color(alpha(p.primary, 0.5))
                    .bg(alpha(p.primary, 0.1)),
            )
        });
        for (i, option) in options.into_iter().enumerate() {
            let active = Some(i) == at;
            let disabled = option.disabled.is_some();
            let value = option.value;
            let tile = div()
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(9.0))
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
                        // The picked one's icon turns into place, as on the web.
                        if active { el.relative().top(px(-5.0 * (t * std::f32::consts::PI).sin())) } else { el }
                    },
                ));
            let card = div()
                .id(SharedString::from(format!("ichoice-{id}-{i}")))
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(6.0))
                .p(px(12.0))
                .rounded(corner(12.0))
                .border_1()
                .border_color(if active { alpha(p.primary, 0.0) } else { p.border.into() })
                .when(disabled, |el| el.opacity(0.5))
                .when(!disabled, |el| {
                    let hover = alpha(p.primary, 0.3);
                    let pick = pick.clone();
                    el.cursor_pointer()
                        .hover(move |s| s.border_color(hover).top(px(-2.0)))
                        .active(|s| s.top(px(1.0)))
                        .on_click(cx.listener(move |this, _, window, cx| pick(this, value, window, cx)))
                })
                .child(tile)
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(option.label))
                .child(div().text_xs().text_color(p.muted_foreground).child(option.disabled.unwrap_or(option.hint)));
            row = row.child(card);
        }
        row.into_any_element()
    }

    /// A switch with words; the words flip it too.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn toggle(
        &self,
        id: &'static str,
        on: bool,
        disabled: bool,
        label: &str,
        hint: &str,
        p: &Palette,
        cx: &mut Context<Self>,
        set: fn(&mut crate::pb::InstanceSettings, bool),
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("itoggle-{id}")))
            .flex()
            .items_center()
            .gap(px(16.0))
            .when(disabled, |el| el.opacity(0.6))
            .when(!disabled, |el| {
                el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.patch(cx, |d| set(d, !on))))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(label.to_owned()))
                    .child(div().text_xs().text_color(p.muted_foreground).child(hint.to_owned())),
            )
            .child(crate::ui::server_settings::switch(
                SharedString::from(format!("itoggle-{id}-switch")),
                on,
                disabled,
                cx,
                move |this: &mut Self, on, cx| this.patch(cx, |d| set(d, on)),
            ))
            .into_any_element()
    }

    /// A cap that's off ("No limit") or a number; sizes take a unit. The number slides in when
    /// it's switched on.
    pub(super) fn cap(
        &self,
        path: &'static str,
        label: &str,
        bytes: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = self.draft.as_ref().and_then(|d| admin::cap(d, path)).is_some();
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .min_h(px(36.0))
            .child(switch_in(SharedString::from(format!("icap-{path}")), on, false, cx, move |this, on, window, cx| {
                this.switch_cap(path, bytes, on, window, cx)
            }))
            .child(div().w(px(96.0)).flex_none().text_sm().font_weight(FontWeight::BOLD).child(label.to_owned()));
        let Some(state) = self.caps.get(path) else { return row.into_any_element() };
        if !on {
            row = row.child(motion::slide_in(
                div().text_sm().text_color(p.muted_foreground).child("No limit"),
                SharedString::from(format!("icap-{path}-off")),
                12.0,
            ));
            return row.into_any_element();
        }
        let mut amount = div().flex().items_center().gap(px(8.0)).child(div().w(px(112.0)).child(Input::new(state)));
        if bytes {
            let unit = self.units.get(path).copied().unwrap_or(1);
            let x = motion::follow(SharedString::from(format!("icap-{path}-unit")), unit as f32 * 40.0, window, cx);
            let mut units = div().relative().flex().p(px(2.0)).rounded(corner(9.0)).bg(p.muted).child(
                div()
                    .absolute()
                    .top(px(2.0))
                    .left(px(2.0 + x))
                    .w(px(40.0))
                    .h(px(26.0))
                    .rounded(corner(7.0))
                    .bg(p.background),
            );
            for (n, (name, _)) in UNITS.iter().enumerate() {
                units = units.child(
                    div()
                        .id(SharedString::from(format!("icap-{path}-{name}")))
                        .relative()
                        .w(px(40.0))
                        .h(px(26.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if n == unit { p.foreground } else { p.muted_foreground })
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_unit(path, n, cx)))
                        .child(*name),
                );
            }
            amount = amount.child(units);
        }
        row.child(motion::slide_in(amount, SharedString::from(format!("icap-{path}-on")), -12.0)).into_any_element()
    }

    /// A heads-up under a setting, sliding in while it applies.
    pub(super) fn notice(&self, id: &str, text: &str, p: &Palette) -> AnyElement {
        motion::rise(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .rounded(corner(12.0))
                .bg(amber(p).opacity(0.1))
                .text_xs()
                .text_color(amber(p))
                .child(text.to_owned()),
            SharedString::from(format!("inotice-{id}")),
            Duration::ZERO,
            -6.0,
        )
        .into_any_element()
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

    /// "Default" while a setting follows the operator's environment, otherwise "Changed" and the
    /// way back.
    pub(super) fn reset_badge(
        &self,
        paths: &'static [&'static str],
        default: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = paths.first().copied().unwrap_or_default();
        if !self.overridden_any(paths) {
            return motion::rise(
                pill("DEFAULT", p.muted_foreground.into()),
                SharedString::from(format!("instance-default-{key}")),
                Duration::ZERO,
                -4.0,
            )
            .into_any_element();
        }
        let saving = self.saving;
        motion::rise(
            div().flex_none().flex().items_center().gap(px(6.0)).child(pill("CHANGED", p.primary.into())).child(
                div()
                    .id(SharedString::from(format!("instance-reset-{key}")))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .h(px(26.0))
                    .rounded_full()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover({
                        let (bg, fg) = (alpha(p.primary, 0.1), p.foreground);
                        move |s| s.bg(bg).text_color(fg)
                    })
                    .when(saving, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.commit(Vec::new(), paths.iter().map(|p| (*p).to_owned()).collect(), window, cx)
                    }))
                    .child(icon("rotate-ccw").size(px(13.0)))
                    .child(if short(default) {
                        format!("Back to the default: {default}")
                    } else {
                        "Back to the default".to_owned()
                    }),
            ),
            SharedString::from(format!("instance-changed-{key}")),
            Duration::ZERO,
            -4.0,
        )
        .into_any_element()
    }
}
