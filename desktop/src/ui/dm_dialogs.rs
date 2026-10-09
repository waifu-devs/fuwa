//! The dialogs about encryption: a conversation's (the web's
//! `dm/EncryptionDialog.tsx`: the safety number and every device that can
//! read it), the padlock that clicks shut at their top (`dm/Padlock.tsx`),
//! and the web's dialog frame they sit in (`ui/dialog.tsx`), which the secure
//! channel's dialog (`ui/secure.rs`) shares.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::dm_view::{glyph_swap, seal, shadow_2xl};
use crate::ui::motion;
use crate::ui::settings_controls::{Look, button};
use crate::ui::text::{TIGHT, WIDER, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::{avatar, icon, pal};

/// The web's dialog: the page dimmed behind it, a card that springs up from a
/// little below, and the round close button in its corner. Clicking outside
/// it, or the button, closes it.
pub(crate) fn dialog_shell(tag: &str, width: f32, body: Div, window: &Window, cx: &mut Context<FuwaApp>) -> AnyElement {
    let p = pal(cx);
    let hover = p.muted;
    let fg = p.foreground;
    let card = div()
        .id(SharedString::from(format!("dialog-card-{tag}")))
        .relative()
        .w(px(width))
        .max_h(window.viewport_size().height * 0.92)
        .overflow_y_scroll()
        .rounded(radius_3xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .text_color(p.foreground)
        .p(px(24.0))
        .shadow(shadow_2xl())
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(body)
        .child(
            div()
                .id("dialog-close")
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
                .hover(move |s| s.bg(hover).text_color(fg))
                .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
                .child(icon("x").size(px(16.0))),
        );
    motion::fade_in(
        div()
            .id(SharedString::from(format!("dialog-scrim-{tag}")))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.0))
            .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5))
            .backdrop_blur(px(crate::ui::overlay::SCRIM_BLUR))
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
            .child(crate::ui::overlay::leaving_pose(
                div().child(motion::dialog_in(card, SharedString::from(format!("dialog-rise-{tag}")))),
                24.0,
                0.97,
                cx,
            )),
        SharedString::from(format!("dialog-fade-{tag}")),
        Duration::from_millis(200),
    )
    .into_any_element()
}

/// The web's switch (`h-5 w-8`): the primary when on, the input color when off; the thumb springs across.
pub(crate) fn web_switch(
    id: &'static str,
    on: bool,
    disabled: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<FuwaApp>,
    set: impl Fn(&mut FuwaApp, bool, &mut Context<FuwaApp>) + 'static,
) -> impl IntoElement {
    let x = motion::follow(SharedString::from(format!("{id}-x")), if on { 13.0 } else { 1.0 }, window, cx);
    div()
        .id(id)
        .flex_none()
        .relative()
        .w(px(32.0))
        .h(px(20.0))
        .rounded_full()
        // The color crosses over as the thumb does (`transition-colors`).
        .bg(crate::ui::theme::mix(p.border, p.primary, ((x - 1.0) / 12.0).clamp(0.0, 1.0)))
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

/// The web's `DialogHeader`: the title, and what it's about under it.
pub(crate) fn dialog_header(title: AnyElement, description: Option<AnyElement>, p: &Palette) -> Div {
    div()
        .mb(px(20.0))
        .pr(px(32.0))
        .child(div().text_xl().line_height(px(28.0)).font_weight(FontWeight::EXTRA_BOLD).child(title))
        .when_some(description, |el, d| {
            el.child(div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(d))
        })
}

/// A padlock that clicks shut as it appears, with a ring going out and a few
/// sparkles around it (the web's `Padlock`): "only you can read this".
pub(crate) fn padlock(tag: &str, p: &Palette) -> AnyElement {
    let s = seal(p);
    let green = s.icon;
    let card: Hsla = p.card.into();
    let disc = s.green;
    motion::once(
        div().relative().size(px(64.0)),
        SharedString::from(format!("padlock-{tag}")),
        Duration::from_millis(2100),
        move |el, t| {
            // Its own clock, in seconds since it showed (the web's delays start at 0.1s).
            let at = |start: f32, length: f32| ((t * 2.1 - start) / length).clamp(0.0, 1.0);
            let ease = |k: f32| 1.0 - (1.0 - k).powi(3);
            // The disc grows past its size and settles.
            let k = at(0.1, 0.6);
            let scale = if k < 0.6 { 0.4 + 0.75 * ease(k / 0.6) } else { 1.15 - 0.15 * ease((k - 0.6) / 0.4) };
            let disc_size = 64.0 * scale;
            // The ring goes out once the lock is shut.
            let r = at(0.65, 1.1);
            let ring_size = 64.0 * (1.0 + 0.6 * ease(r));
            let ring_alpha = if r <= 0.0 { 0.0 } else { 0.8 * (1.0 - r) };
            // The body springs in; the shackle drops into it; the keyhole pops.
            let b = at(0.1, 0.45);
            let body = (0.6 + 0.4 * ease(b) + (b * std::f32::consts::PI).sin() * 0.12).min(1.12);
            let drop = at(0.25, 0.55);
            let shackle_y = if drop < 0.4 {
                -4.0
            } else if drop < 0.8 {
                -4.0 + 4.6 * ((drop - 0.4) / 0.4)
            } else {
                0.6 - 0.6 * ((drop - 0.8) / 0.2)
            };
            let hole = ease(at(0.7, 0.35));
            let mut el = el
                .child(
                    div()
                        .absolute()
                        .left(px(32.0 - disc_size / 2.0))
                        .top(px(32.0 - disc_size / 2.0))
                        .size(px(disc_size))
                        .rounded_full()
                        .bg(Hsla { a: 0.15 * b.clamp(0.2, 1.0), ..disc }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(32.0 - ring_size / 2.0))
                        .top(px(32.0 - ring_size / 2.0))
                        .size(px(ring_size))
                        .rounded_full()
                        .border_1()
                        .border_color(Hsla { a: 0.4 * ring_alpha / 0.8, ..disc }),
                )
                // The lock, on the web's 32px drawing in the middle of the 64px mark.
                .child(
                    div()
                        .absolute()
                        .left(px(16.0 + 9.2))
                        .top(px(16.0 + 3.7 + shackle_y))
                        .w(px(13.6))
                        .h(px(12.0))
                        .border_t_2()
                        .border_l_2()
                        .border_r_2()
                        .border_color(green)
                        .rounded_t(px(6.8)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(16.0 + 16.0 - 9.5 * body))
                        .top(px(16.0 + 20.5 - 7.0 * body))
                        .w(px(19.0 * body))
                        .h(px(14.0 * body))
                        .rounded(px(4.0 * body))
                        .bg(green),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(16.0 + 16.0 - 2.1 * hole))
                        .top(px(16.0 + 20.5 - 2.1 * hole))
                        .size(px(4.2 * hole))
                        .rounded_full()
                        .bg(card),
                );
            // Three sparkles, one after another.
            for (n, (x, y)) in [(0.08, 0.18), (0.82, 0.26), (0.70, 0.84)].into_iter().enumerate() {
                let k = at(0.8 + n as f32 * 0.12, 1.2);
                if k <= 0.0 || k >= 1.0 {
                    continue;
                }
                let grow = if k < 0.5 { 1.2 * ease(k / 0.5) } else { 1.2 - 0.3 * ((k - 0.5) / 0.5) };
                let fade = if k < 0.5 { k / 0.5 } else { 1.0 - (k - 0.5) / 0.5 };
                el = el.child(
                    div()
                        .absolute()
                        .left(px(64.0 * x))
                        .top(px(64.0 * y))
                        .text_size(px(10.4 * grow))
                        .line_height(px(12.0))
                        .text_color(Hsla { a: fade, ..disc })
                        .child("✦"),
                );
            }
            el
        },
    )
}

/// A conversation's devices as the instance listed them, and when.
type Listed = (Instant, HashMap<String, pb::Device>);

thread_local! {
    /// Each conversation's devices as the instance listed them, and when.
    static DEVICES: RefCell<HashMap<String, Listed>> = RefCell::new(HashMap::new());
    /// The safety number was just copied, and when.
    static COPIED: RefCell<Option<Instant>> = const { RefCell::new(None) };
    /// Marking it verified (or not) is on its way.
    static BUSY: RefCell<bool> = const { RefCell::new(false) };
}

/// A device id as four groups of four, like a key fingerprint.
fn fingerprint(id: &str) -> String {
    let head: Vec<char> = id.chars().take(16).collect();
    if head.len() < 16 {
        return id.to_owned();
    }
    head.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

/// A safety number as groups of five.
fn groups(number: &str) -> Vec<String> {
    let chars: Vec<char> = number.chars().collect();
    chars.chunks(5).filter(|c| c.len() == 5).map(|c| c.iter().collect()).collect()
}

impl FuwaApp {
    /// How a conversation is kept private, and the means to check it: the
    /// safety number both people should see the same, and every device that
    /// can read it.
    pub(crate) fn render_encryption(
        &mut self,
        key: &str,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let s = seal(&p);
        let (conversation, me, my_device, safety, verified, members) = self.core.shared.read(|st| {
            let Some(i) = st.instance(key) else { return Default::default() };
            (
                i.dms.conversations.iter().find(|c| c.id == id).cloned(),
                i.me.clone(),
                i.dms.device_id.clone(),
                i.dms.safety.get(id).cloned().unwrap_or_default(),
                i.dms.verified.get(id).cloned().unwrap_or_default(),
                i.dms.members.get(id).cloned().unwrap_or_default(),
            )
        });
        let users = conversation.map(|c| c.users).unwrap_or_default();
        let me_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let partner = users.iter().find(|u| u.id != me_id).cloned();
        let them = partner.as_ref().map_or_else(|| t("dms-calls.dm.view.them"), crate::core::store::user_name);
        self.fetch_devices(key, id, &users, cx);
        let devices = DEVICES.with(|d| d.borrow().get(id).map(|(_, list)| list.clone()).unwrap_or_default());
        let is_verified = !safety.is_empty() && verified == safety;
        let changed = !verified.is_empty() && !safety.is_empty() && verified != safety;
        let groups = groups(&safety);
        let busy = BUSY.with(|b| *b.borrow());
        let copied = COPIED.with(|c| c.borrow().is_some_and(|at| at.elapsed() < Duration::from_millis(1200)));

        let chip = |id: &'static str, glyph: &str, label: String, bg: Hsla, fg: Hsla| {
            crate::ui::motion::spring_in(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded_full()
                    .bg(bg)
                    .text_color(fg)
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(icon(glyph).size(px(14.0)))
                    .child(label),
                id,
                (600.0, 16.0),
                Duration::ZERO,
                |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(0.5 + 0.5 * t),
            )
        };
        let state_chip = if is_verified {
            Some(chip(
                "safety-chip-verified",
                "badge-check",
                t("dms-calls.dm.trust.verified"),
                Hsla { a: 0.15, ..s.green },
                s.pill,
            ))
        } else if changed {
            Some(chip(
                "safety-chip-changed",
                "shield-alert",
                t("dms-calls.dm.encryption.changed"),
                Hsla { a: 0.15, ..s.amber },
                s.amber_text,
            ))
        } else {
            None
        };
        let number: AnyElement = if groups.is_empty() {
            div()
                .mt(px(8.0))
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(t("dms-calls.dm.encryption.noNumber"))
                .into_any_element()
        } else {
            let mut grid = div().mt(px(12.0)).flex().flex_wrap().gap_y(px(8.0));
            for (n, group) in groups.iter().enumerate() {
                grid = grid.child(
                    // Each group flips up into place, one after another (the web's `rotateX: -90`,
                    // drawn here as the group unfolding from a line).
                    div().w(gpui_kit::relative(0.25)).flex().justify_center().child(motion::spring_in(
                        div()
                            .font_family("monospace")
                            .text_size(px(16.0))
                            .line_height(px(24.0))
                            .font_weight(FontWeight::BOLD)
                            .child(tracked(group.clone(), WIDER)),
                        SharedString::from(format!("safety-{safety}-{n}")),
                        (520.0, 34.0),
                        Duration::from_millis(150 + 35 * n as u64),
                        |el, t| el.opacity(t.clamp(0.0, 1.0)).scale_y(t.max(0.0)).translate_y(px((1.0 - t) * 6.0)),
                    )),
                );
            }
            grid.into_any_element()
        };
        let (k, c) = (key.to_owned(), id.to_owned());
        let next = if is_verified { String::new() } else { safety.clone() };
        let actions = (!groups.is_empty()).then(|| {
            let joined = groups.join(" ");
            div()
                .mt(px(12.0))
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .child(
                    button(
                        "safety-mark",
                        t(if is_verified { "dms-calls.dm.encryption.clear" } else { "dms-calls.dm.encryption.mark" }),
                        None,
                        if is_verified { Look::Outline } else { Look::Primary },
                        true,
                        &p,
                    )
                    .rounded(radius_xl())
                    .font_weight(FontWeight::BOLD)
                    .when(busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if BUSY.with(|b| *b.borrow()) {
                            return;
                        }
                        BUSY.with(|b| *b.borrow_mut() = true);
                        let (core, k, c, next) = (this.core.clone(), k.clone(), c.clone(), next.clone());
                        this.run(cx, async move { core.verify_conversation(&k, &c, &next).await }, |_, _, cx| {
                            BUSY.with(|b| *b.borrow_mut() = false);
                            cx.notify();
                        });
                        cx.notify();
                    })),
                )
                .child(
                    // Its icon pops to a tick and back (`stiffness: 700, damping: 22`).
                    button("safety-copy", "", None, Look::Ghost, true, &p)
                        .px(px(10.0))
                        .gap(px(6.0))
                        .child({
                            let glyph = glyph_swap("safety-copy-icon", if copied { "check" } else { "copy" }, 16.0)
                                .pop(0.3, (700.0, 22.0));
                            if copied { glyph.color(p.primary) } else { glyph }
                        })
                        .child(t(if copied {
                            "dms-calls.dm.encryption.copied"
                        } else {
                            "dms-calls.dm.encryption.copy"
                        }))
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(joined.clone()));
                            COPIED.with(|c| *c.borrow_mut() = Some(Instant::now()));
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_millis(1250)).await;
                                let _ = this.update(cx, |_, cx| cx.notify());
                            })
                            .detach();
                            cx.notify();
                        })),
                )
        });
        let section = div()
            .p(px(16.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("dms-calls.dm.encryption.safety")),
                    )
                    .children(state_chip),
            )
            .child(number)
            .child(div().mt(px(12.0)).text_xs().line_height(px(19.5)).text_color(p.muted_foreground).child(
                if changed {
                    t_with("dms-calls.dm.encryption.changedText", &[("name", Arg::Str(&them))])
                } else {
                    t_with("dms-calls.dm.encryption.compare", &[("name", Arg::Str(&them))])
                },
            ))
            .children(actions);

        let mut people = div().flex().flex_col().gap(px(12.0));
        for user in &users {
            let own: Vec<_> = members.iter().filter(|m| m.user_id == user.id).collect();
            let mut list = div().flex().flex_col().gap(px(4.0));
            for (n, m) in own.iter().enumerate() {
                let device = devices.get(&m.device_id);
                let (glyph, label) = crate::ui::settings_account::device_label(device.map_or("", |d| d.label.as_str()));
                let name = if device.is_some() { label } else { t("dms-calls.dm.encryption.signedOut") };
                let mine = m.device_id == my_device;
                let hover = alpha(p.muted, 0.6);
                list = list.child(motion::slide_in(
                    div()
                        .id(SharedString::from(format!("device-{}", m.device_id)))
                        .group("device-row")
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(radius_xl())
                        .hover(move |st| st.bg(hover))
                        // The device's tile tips a little while its row is pointed at.
                        .child(
                            div()
                                .id("device-tile")
                                .group_hover("device-row", |st| st.rotate(gpui_kit::radians(-6f32.to_radians())))
                                .size(px(32.0))
                                .flex_none()
                                .rounded(radius_lg())
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(if mine { alpha(p.primary, 0.15) } else { p.muted.into() })
                                .text_color(if mine { p.primary } else { p.muted_foreground })
                                .child(icon(glyph).size(px(16.0))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .text_sm()
                                        .line_height(px(20.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(div().min_w_0().truncate().child(name))
                                        .when(mine, |el| {
                                            el.child(
                                                div()
                                                    .flex_none()
                                                    .px(px(6.0))
                                                    .rounded_full()
                                                    .bg(alpha(p.primary, 0.15))
                                                    .text_size(px(10.4))
                                                    .line_height(px(15.6))
                                                    .text_color(p.primary)
                                                    .child(t("dms-calls.dm.encryption.thisDevice")),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .font_family("monospace")
                                        .text_size(px(11.2))
                                        .line_height(px(16.8))
                                        .text_color(p.muted_foreground)
                                        .child(fingerprint(&m.device_id)),
                                ),
                        ),
                    SharedString::from(format!("device-in-{}-{n}", m.device_id)),
                    -8.0,
                ));
            }
            people = people.child(
                div()
                    .child(
                        div()
                            .mb(px(4.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_xs()
                            .line_height(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(avatar(Some(user), 20.0, &p))
                            .child(if user.id == me_id {
                                t("dms-calls.dm.encryption.you")
                            } else {
                                crate::core::store::user_name(user)
                            }),
                    )
                    .when(own.is_empty(), |el| {
                        el.child(
                            div()
                                .pl(px(28.0))
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(t("dms-calls.dm.encryption.noDevices")),
                        )
                    })
                    .child(list),
            );
        }
        let description = t_with("dms-calls.dm.encryption.description", &[("name", Arg::Str(&them))]);
        let body = div()
            .flex()
            .flex_col()
            .child(div().mb(px(16.0)).flex().justify_center().child(padlock("dm", &p)))
            .child(dialog_header(
                tracked(t("dms-calls.dm.encrypted"), TIGHT).into_any_element(),
                Some(description.into_any_element()),
                &p,
            ))
            .child(section)
            .child(
                div()
                    .mt(px(16.0))
                    .child(
                        div()
                            .mb(px(8.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("dms-calls.dm.encryption.devices")),
                    )
                    .child(people)
                    .child(
                        div()
                            .mt(px(12.0))
                            .text_xs()
                            .line_height(px(19.5))
                            .text_color(p.muted_foreground)
                            .child(t("dms-calls.dm.encryption.footer")),
                    ),
            );
        dialog_shell("encryption", 512.0, body, window, cx)
    }

    /// Asks the instance for the conversation's devices when the dialog opens (again after a while).
    fn fetch_devices(&self, key: &str, id: &str, users: &[pb::User], cx: &mut Context<Self>) {
        let fresh = DEVICES.with(|d| d.borrow().get(id).is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(30)));
        if fresh || users.is_empty() {
            return;
        }
        // Noted now, so a frame drawn meanwhile doesn't ask again.
        DEVICES.with(|d| {
            let mut d = d.borrow_mut();
            let list = d.get(id).map(|(_, list)| list.clone()).unwrap_or_default();
            d.insert(id.to_owned(), (Instant::now(), list));
        });
        let (core, key, place) = (self.core.clone(), key.to_owned(), id.to_owned());
        let ids: Vec<String> = users.iter().map(|u| u.id.clone()).collect();
        self.run(cx, async move { core.devices_of(&key, ids).await }, move |_, result, cx| {
            if let Ok(list) = result {
                let list = list.into_iter().map(|d| (d.id.clone(), d)).collect();
                DEVICES.with(|d| d.borrow_mut().insert(place.clone(), (Instant::now(), list)));
                cx.notify();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_ids_read_in_groups() {
        assert_eq!(fingerprint("0123456789abcdef0011"), "0123 4567 89ab cdef");
        assert_eq!(fingerprint("short"), "short");
        assert_eq!(groups("1234567890"), vec!["12345", "67890"]);
    }
}
