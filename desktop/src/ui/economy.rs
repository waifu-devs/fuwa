//! A server's economy in the window (docs/economy.md): how a currency and an
//! amount of it are drawn everywhere, the wallet at the sidebar's foot, and
//! Give currency (Grant Currency), the dialog profile cards, member menus
//! and the finance page open to give a member or a role some, or take it
//! away, with a reason that goes in the audit log. The shop is
//! `ui/shop.rs`; the settings pages `server_settings/economy.rs` and
//! `server_settings/finances.rs`.

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use crate::core::economy::{GrantTo, Icon, NOTE_MAX, amount_text, icon_of, parse_amount};
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Nav};
use crate::ui::motion;
use crate::ui::overlay::{dialog_button, dialog_card, dialog_close, dialog_header, dialog_layer};
use crate::ui::settings_controls::{Look, field};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, error_line, icon, pal, picture};

/// The gold of coins, for what's earned and given.
pub(crate) fn gold(p: &Palette) -> Hsla {
    if p.dark { gpui_kit::rgb(0xfbbf24).into() } else { gpui_kit::rgb(0xd97706).into() }
}

/// A currency's icon, `size` pixels: its custom emoji's picture, or its characters.
pub(crate) fn currency_icon(currency: &pb::Currency, emojis: &[pb::Emoji], size: f32) -> AnyElement {
    match icon_of(currency, emojis) {
        Icon::Picture(url) => {
            use gpui_kit::StyledImage as _;
            picture(url).size(px(size)).flex_none().object_fit(gpui_kit::ObjectFit::Contain).into_any_element()
        }
        Icon::Text(text) => {
            div().flex_none().text_size(px(size * 0.9)).line_height(px(size)).child(text).into_any_element()
        }
    }
}

/// An amount with its currency's icon before it: "🪙 1,250".
pub(crate) fn money(currency: &pb::Currency, emojis: &[pb::Emoji], amount: i64, size: f32) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(size * 0.3))
        .child(currency_icon(currency, emojis, size))
        .child(div().font_weight(FontWeight::BOLD).child(amount_text(amount)))
}

/// The profile items a server sells that you haven't bought, by id, with
/// what they cost as words ("🪙 120", or "120 Gems" for a currency drawn as
/// a picture), for the pickers to lock (docs/economy.md).
pub(crate) fn lock_labels(e: &crate::core::economy::Economy) -> std::collections::HashMap<String, String> {
    crate::core::economy::item_locks(e)
        .into_iter()
        .map(|(item, (price, currency))| {
            let label = match e.currency(&currency) {
                Some(c) if c.icon_emoji_id.is_empty() && !c.icon_emoji.is_empty() => {
                    format!("{} {}", c.icon_emoji, amount_text(price))
                }
                Some(c) => format!("{} {}", amount_text(price), c.name),
                None => amount_text(price),
            };
            (item, label)
        })
        .collect()
}

/// Someone a ledger names: as the server shows them (nickname first) when
/// the app knows them, else as the ledger's own list has them (people who
/// left are only there).
pub(crate) fn person_name(
    i: &crate::core::store::InstanceState,
    server_id: &str,
    user_id: &str,
    named: &std::collections::HashMap<String, pb::User>,
) -> String {
    let known = i.users.contains_key(user_id)
        || i.members.get(server_id).is_some_and(|m| m.iter().any(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)));
    match named.get(user_id) {
        Some(user) if !known => crate::core::store::user_name(user),
        _ => i.display_name(Some(server_id), user_id),
    }
}

/// Something to say a moment ago went well: "Sent!", fading after a while.
pub(crate) fn fresh(at: Option<Instant>, within: Duration) -> bool {
    at.is_some_and(|at| at.elapsed() < within)
}

impl FuwaApp {
    /// The wallet at the sidebar's foot, in a server with currencies: each
    /// currency's icon and your count of it, rolling as it changes. It opens the shop.
    pub(crate) fn wallet_bar(
        &mut self,
        key: &str,
        server: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (currencies, amounts, emojis, claimable) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            if !i.has_currencies(server) {
                return None;
            }
            let e = i.economy(server)?;
            let now = crate::core::dms::now_ms();
            let claimable = e.currencies.iter().any(|c| {
                crate::core::economy::rule(c, pb::CurrencyAction::Daily).is_some()
                    && !crate::core::economy::claimed_today(e.balances.get(&c.id), now)
            });
            Some((
                e.currencies.clone(),
                e.currencies.iter().map(|c| e.amount(&c.id)).collect::<Vec<_>>(),
                i.emojis.get(server).cloned().unwrap_or_default(),
                claimable,
            ))
        })?;
        let p = pal(cx);
        let mut row = div().flex_1().min_w_0().flex().items_center().gap(px(12.0)).overflow_hidden();
        for (c, amount) in currencies.iter().zip(&amounts).take(3) {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(currency_icon(c, &emojis, 16.0))
                    .child(motion::count(
                        format!("wallet|{key}|{server}|{}", c.id),
                        (*amount).max(0) as u64,
                        None,
                        14.0,
                        window,
                        cx,
                    )),
            );
        }
        let (k, s) = (key.to_owned(), server.to_owned());
        let hover = alpha(p.muted, 0.7);
        let gold = gold(&p);
        let bar = div()
            .id("wallet-bar")
            .group("wallet-bar")
            .flex_none()
            .mx(px(8.0))
            .mb(px(6.0))
            .h(px(36.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(alpha(p.border, 0.8))
            .bg(alpha(p.background, 0.4))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(|s| s.scale(0.98))
            .on_click(cx.listener(move |this, _, window, cx| this.open_shop(&k, &s, window, cx)))
            .child(row)
            .when(claimable, |el| {
                // Today's reward waits: a little gift that bobs.
                el.child(motion::ambient(
                    div().text_color(gold).child(icon("gift").size(px(14.0))),
                    "wallet-gift",
                    Duration::from_millis(1600),
                    window,
                    |el, t| el.relative().top(px(-2.0 * (t * std::f32::consts::TAU).sin().abs())),
                ))
            })
            .child(
                div()
                    .id("wallet-store")
                    .text_color(p.muted_foreground)
                    .group_hover("wallet-bar", |s| s.scale(1.12).rotate(gpui_kit::radians(-0.15)))
                    .child(icon("store").size(px(15.0))),
            );
        Some(
            motion::rise(bar, SharedString::from(format!("wallet-in|{key}|{server}")), Duration::ZERO, 6.0)
                .into_any_element(),
        )
    }

    /// The wallet for the server on screen, if it has currencies.
    pub(crate) fn sidebar_wallet(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Nav::Server { key, server } = self.nav.clone() else { return None };
        self.wallet_bar(&key, &server, window, cx)
    }

    /// Opens Give currency for a member, or for a role (empty to pick one in the dialog).
    pub(crate) fn open_give(
        &mut self,
        key: &str,
        server: &str,
        to: GrantTo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_dialog(Dialog::GiveCurrency { key: key.to_owned(), server: server.to_owned(), to }, window, cx);
    }

    pub(crate) fn render_give(
        &mut self,
        key: &str,
        server: &str,
        to: &GrantTo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let (currencies, emojis, roles, user, name, wallet_for) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return Default::default() };
            let e = i.economy(server);
            let mut roles: Vec<pb::Role> = i.roles.get(server).cloned().unwrap_or_default();
            roles.sort_by_key(|r| std::cmp::Reverse(r.position));
            let (user, name) = match to {
                GrantTo::Member(id) => (i.users.get(id).cloned(), i.display_name(Some(server), id)),
                GrantTo::Role(_) => (None, String::new()),
            };
            (
                e.map(|e| e.currencies.clone()).unwrap_or_default(),
                i.emojis.get(server).cloned().unwrap_or_default(),
                roles,
                user,
                name,
                match to {
                    GrantTo::Member(id) => Some(id.clone()),
                    GrantTo::Role(_) => None,
                },
            )
        });
        // A new dialog starts empty, on the first currency, the amount in focus.
        let tag = format!("{key}|{server}|{to:?}");
        if self.giving.for_tag.as_deref() != Some(tag.as_str()) {
            self.giving.for_tag = Some(tag);
            self.giving.take = false;
            self.giving.done = None;
            self.giving.wallet = None;
            self.giving.currency = currencies.first().map(|c| c.id.clone());
            self.giving.role = match to {
                GrantTo::Role(id) if !id.is_empty() => Some(id.clone()),
                _ => None,
            };
            self.giving.amount.update(cx, |s, cx| {
                s.set_value("", window, cx);
                s.focus(window, cx);
            });
            self.giving.reason.update(cx, |s, cx| s.set_value("", window, cx));
            // What they hold now, beside each currency.
            if let Some(uid) = wallet_for {
                let (core, k, sv) = (self.core.clone(), key.to_owned(), server.to_owned());
                self.run(cx, async move { core.wallet_of(&k, &sv, &uid).await }, |this, result, cx| {
                    if let Ok(w) = result {
                        this.giving.wallet = Some(w.balances);
                        cx.notify();
                    }
                });
            }
        }
        let close = cx.listener(|this, _, _, cx| {
            this.dialog = None;
            this.close_dialog(cx);
        });
        if let Some((at, people, total)) = self.giving.done {
            return dialog_layer("give", self.give_done(at, people, total, &p, cx), &p, close, cx);
        }

        let gold = gold(&p);
        let take = self.giving.take;
        let title = match to {
            GrantTo::Member(_) if take => t_with("desktop.economy.give.takeTitle", &[("name", Arg::Str(&name))]),
            GrantTo::Member(_) => t_with("desktop.economy.give.title", &[("name", Arg::Str(&name))]),
            GrantTo::Role(_) => t("desktop.economy.give.roleTitle"),
        };
        let about = match to {
            GrantTo::Member(_) => t("desktop.economy.give.about"),
            GrantTo::Role(_) => t("desktop.economy.give.roleAbout"),
        };
        let mut body = div().flex().flex_col().gap(px(14.0));
        if let GrantTo::Member(_) = to {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.muted, 0.6))
                    .child(avatar(user.as_ref(), 40.0, &p))
                    .child(div().flex_1().min_w_0().font_weight(FontWeight::BOLD).truncate().child(name.clone())),
            );
        } else {
            // Who it reaches: a role, or everyone.
            let mut chips = div().flex().flex_wrap().gap(px(6.0));
            for r in &roles {
                let on = self.giving.role.as_deref() == Some(r.id.as_str());
                let label = if r.id == server { t("desktop.economy.give.everyone") } else { r.name.clone() };
                let dot: Hsla = r.color.map(|c| gpui_kit::rgb(c as u32).into()).unwrap_or(p.muted_foreground.into());
                let id = r.id.clone();
                chips = chips.child(
                    crate::ui::server_settings::chip(SharedString::from(format!("give-role-{}", r.id)), &label, on, &p)
                        .when(r.id != server, |el| el.child(div().ml(px(6.0)).size(px(8.0)).rounded_full().bg(dot)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.giving.role = Some(id.clone());
                            cx.notify();
                        })),
                );
            }
            body = body
                .child(label(t("desktop.economy.give.to")))
                .child(crate::ui::widgets::inner_scroll("give-roles", window, cx).max_h(px(120.0)).child(chips));
        }
        // Give or take.
        let mut modes = div().flex().gap(px(6.0));
        for (n, (taking, key_, glyph)) in
            [(false, "desktop.economy.give.give", "plus"), (true, "desktop.economy.give.take", "minus")]
                .into_iter()
                .enumerate()
        {
            let on = take == taking;
            let tint: Hsla = if taking { p.destructive.into() } else { gold };
            modes = modes.child(
                div()
                    .id(SharedString::from(format!("give-mode-{n}")))
                    .flex_1()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .rounded(radius_xl())
                    .border_1()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .map(|el| {
                        if on {
                            el.border_color(tint.opacity(0.6)).bg(tint.opacity(0.12)).text_color(tint)
                        } else {
                            el.border_color(p.border).text_color(p.muted_foreground)
                        }
                    })
                    .active(|s| s.scale(0.97))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.giving.take = taking;
                        cx.notify();
                    }))
                    .child(icon(glyph).size(px(14.0)))
                    .child(t(key_)),
            );
        }
        body = body.child(modes);
        // Which currency.
        let mut picks = div().flex().flex_wrap().gap(px(6.0));
        for c in &currencies {
            let on = self.giving.currency.as_deref() == Some(c.id.as_str());
            let held = self
                .giving
                .wallet
                .as_ref()
                .and_then(|w| w.iter().find(|b| b.currency_id == c.id))
                .map(|b| amount_text(b.amount));
            let id = c.id.clone();
            let hover = alpha(p.primary, 0.3);
            picks = picks.child(
                div()
                    .id(SharedString::from(format!("give-cur-{}", c.id)))
                    .h(px(34.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded_full()
                    .border_1()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .map(|el| {
                        if on {
                            el.border_color(alpha(p.primary, 0.6)).bg(alpha(p.primary, 0.1))
                        } else {
                            el.border_color(p.border).hover(move |s| s.border_color(hover))
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.giving.currency = Some(id.clone());
                        cx.notify();
                    }))
                    .child(currency_icon(c, &emojis, 16.0))
                    .child(c.name.clone())
                    .when_some(held, |el, held| el.child(div().text_xs().text_color(p.muted_foreground).child(held))),
            );
        }
        body = body.child(label(t("desktop.economy.give.currency"))).child(picks);
        let typed = self.giving.amount.read(cx).value().to_string();
        let amount = parse_amount(&typed).filter(|n| *n > 0);
        body = body
            .child(label(if take { t("desktop.economy.give.amountTake") } else { t("desktop.economy.give.amount") }))
            .child(field(Input::new(&self.giving.amount).appearance(false), &p));
        let reason_len = self.giving.reason.read(cx).value().chars().count();
        body = body
            .child(
                div().flex().items_center().justify_between().child(label(t("desktop.economy.give.reason"))).child(
                    div()
                        .text_xs()
                        .text_color(if reason_len > NOTE_MAX { p.destructive } else { p.muted_foreground })
                        .child(format!("{reason_len}/{NOTE_MAX}")),
                ),
            )
            .child(field(Input::new(&self.giving.reason).appearance(false), &p))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(icon("scroll-text").size(px(14.0)))
                    .child(t("desktop.economy.give.audited")),
            );
        let busy = self.dialog_busy;
        let target_ready = match to {
            GrantTo::Member(_) => true,
            GrantTo::Role(_) => self.giving.role.is_some(),
        };
        let ready =
            !busy && amount.is_some() && self.giving.currency.is_some() && target_ready && reason_len <= NOTE_MAX;
        let action = match (take, amount) {
            (_, None) if take => t("desktop.economy.give.take"),
            (_, None) => t("desktop.economy.give.give"),
            (true, Some(n)) => t_with("desktop.economy.give.takeN", &[("amount", Arg::Str(&amount_text(n)))]),
            (false, Some(n)) => t_with("desktop.economy.give.giveN", &[("amount", Arg::Str(&amount_text(n)))]),
        };
        let panel = dialog_card(false, &p)
            .child(dialog_header(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .size(px(36.0))
                            .flex_none()
                            .rounded(radius_xl())
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gold.opacity(0.15))
                            .text_color(gold)
                            .child(icon("hand-coins").size(px(20.0))),
                    )
                    .child(div().flex_1().min_w_0().child(title)),
                Some(div().child(about).into_any_element()),
                &p,
            ))
            .child(body)
            .when_some(error_line(self.dialog_error.as_deref(), &p), |el, e| el.child(div().mt(px(12.0)).child(e)))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(dialog_button("give-cancel", t("common.cancel"), Look::Ghost, &p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.dialog = None;
                            this.close_dialog(cx);
                        },
                    )))
                    .child(
                        dialog_button("give-send", action, if take { Look::Destructive } else { Look::Primary }, &p)
                            .when(!ready, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.send_give(cx))),
                    ),
            )
            .child(dialog_close("give-close", &p).on_click(cx.listener(|this, _, _, cx| {
                this.dialog = None;
                this.close_dialog(cx);
            })));
        dialog_layer("give", panel, &p, close, cx)
    }

    /// What a grant did: how many it reached and what moved, with coins that pop.
    fn give_done(&self, at: Instant, people: i32, total: i64, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let gold = gold(p);
        dialog_card(false, p)
            .items_center()
            .child(motion::pop(
                div()
                    .size(px(56.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gold.opacity(0.15))
                    .text_color(gold)
                    .child(icon("coins").size(px(28.0))),
                SharedString::from(format!("give-done-{at:?}")),
                0.4,
                -20.0,
                Duration::ZERO,
            ))
            .child(motion::rise(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(t("desktop.economy.give.done")))
                    .child(div().text_sm().text_center().text_color(p.muted_foreground).child(t_with(
                        "desktop.economy.give.doneAbout",
                        &[("count", Arg::Num(i64::from(people))), ("total", Arg::Str(&amount_text(total.abs())))],
                    ))),
                SharedString::from(format!("give-done-words-{at:?}")),
                Duration::from_millis(80),
                8.0,
            ))
            .child(div().mt(px(20.0)).child(
                dialog_button("give-ok", t("desktop.modView.report.done"), Look::Primary, p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.dialog = None;
                        this.close_dialog(cx);
                    },
                )),
            ))
            .into_any_element()
    }

    /// Gives (or takes) what's typed.
    pub(crate) fn send_give(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::GiveCurrency { key, server, to }) = self.dialog.clone() else { return };
        if self.giving.done.is_some() {
            self.dialog = None;
            self.close_dialog(cx);
            return;
        }
        let typed = self.giving.amount.read(cx).value().to_string();
        let reason = self.giving.reason.read(cx).value().to_string();
        let (Some(amount), Some(currency)) = (parse_amount(&typed).filter(|n| *n > 0), self.giving.currency.clone())
        else {
            self.dialog_error = Some(t("desktop.economy.give.amountNeeded"));
            cx.notify();
            return;
        };
        let to = match to {
            GrantTo::Role(_) => match self.giving.role.clone() {
                Some(r) => GrantTo::Role(r),
                None => return,
            },
            member => member,
        };
        if self.dialog_busy || reason.chars().count() > NOTE_MAX {
            return;
        }
        let amount = if self.giving.take { -amount } else { amount };
        self.dialog_busy = true;
        self.dialog_error = None;
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.grant(&key, &server, &currency, &to, amount, &reason).await },
            |this, result, cx| {
                this.dialog_busy = false;
                match result {
                    Ok(res) => this.giving.done = Some((Instant::now(), res.people, res.total)),
                    Err(err) => this.dialog_error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }
}

fn label(text: String) -> gpui_kit::Div {
    div().text_sm().line_height(px(14.0)).font_weight(FontWeight::BOLD).child(text)
}

/// What Give currency keeps while it's open.
pub struct Giving {
    pub currency: Option<String>,
    /// The role picked, when giving to a role.
    pub role: Option<String>,
    pub take: bool,
    pub amount: Entity<InputState>,
    pub reason: Entity<InputState>,
    /// Which dialog the fields were last emptied for.
    pub for_tag: Option<String>,
    /// The member's balances, once read.
    pub wallet: Option<Vec<pb::Balance>>,
    /// It went through: when, how many it reached and what moved.
    pub done: Option<(Instant, i32, i64)>,
}

impl Giving {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let amount = cx.new(|cx| InputState::new(window, cx).placeholder("100"));
        let reason = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.economy.give.reasonPlaceholder")));
        let subs = vec![
            cx.subscribe(&amount, |this: &mut FuwaApp, _, e: &InputEvent, cx| match e {
                InputEvent::PressEnter { .. } => this.send_give(cx),
                _ => {
                    this.dialog_error = None;
                    cx.notify()
                }
            }),
            cx.subscribe(&reason, |this: &mut FuwaApp, _, e: &InputEvent, cx| match e {
                InputEvent::PressEnter { .. } => this.send_give(cx),
                _ => cx.notify(),
            }),
        ];
        (
            Self { currency: None, role: None, take: false, amount, reason, for_tag: None, wallet: None, done: None },
            subs,
        )
    }
}
