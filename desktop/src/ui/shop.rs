//! A server's shop (docs/economy.md), full screen like Mod View, opened from
//! the wallet at the sidebar's foot: your balances with Claim for daily
//! rewards (once a UTC day, the streak beside it), what's for sale (roles in
//! their color, profile items, anything else by name; what you own marked),
//! bought after a confirm at the price shown; your history, a page at a
//! time; and Send for currencies members may pass on. Lists draw only the
//! rows in sight.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

mod gacha;

use crate::core::Core;
use crate::core::dms::now_ms;
use crate::core::economy::{
    self as eco, Economy, NOTE_MAX, amount_text, claimed_today, daily_reward, listing_name, parse_amount, streak_now,
    until_next_day,
};
use crate::core::i18n::{Arg, t, t_with};
use crate::pb::{self, CurrencyAction as Action, LedgerKind, ShopListingKind as Kind};
use crate::ui::economy::{currency_icon, gold, money};
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::settings_controls::{Look, button, field};
use crate::ui::text::{TIGHT, ms_of, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_lg, radius_xl};
use crate::ui::widgets::{avatar, error_line, icon, pal, picture};

/// How tall a listing is, its gap included.
const LISTING_ROW: f32 = 92.0;
/// How tall a line of history is.
const LEDGER_ROW: f32 = 64.0;
/// How long the screen takes to come and go.
const OPENING: Duration = Duration::from_millis(280);
/// The most people the Send screen offers at once.
const MATCHES: usize = 6;

pub enum ShopEvent {
    Close,
    Toast { icon: &'static str, title: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Shop,
    Gacha,
    History,
    Send,
}

pub struct ShopView {
    core: Arc<Core>,
    pub key: String,
    pub server: String,
    screen: Screen,
    list_scroll: UniformListScrollHandle,
    /// The listing asked about before it's bought, and the one being bought.
    confirming: Option<String>,
    buying: Option<String>,
    /// Just bought, to pop it.
    bought: Option<(String, Instant)>,
    /// The banner picked on the gacha screen, a pull being made (how many),
    /// the pull playing out over the window, and whether the drop rates are open.
    picked: Option<String>,
    pulling: Option<i32>,
    show: Option<gacha::Show>,
    rates_open: bool,
    /// A currency whose reward is being claimed, and the last one claimed.
    claiming: Option<String>,
    claimed: Option<(String, i64, Instant)>,
    /// Your history: entries read so far, newest first.
    ledger: Vec<pb::LedgerEntry>,
    /// Everyone the entries name, as the ledger has them.
    named: HashMap<String, pb::User>,
    ledger_more: bool,
    ledger_loading: bool,
    ledger_read: bool,
    /// The history of one currency only (empty for all), as the web's `LedgerList` picks.
    ledger_currency: String,
    ledger_scroll: UniformListScrollHandle,
    /// Sending: the currency, who to, and what's typed.
    send_currency: Option<String>,
    send_to: Option<String>,
    finder: Entity<InputState>,
    amount: Entity<InputState>,
    note: Entity<InputState>,
    sending: bool,
    sent: Option<Instant>,
    error: Option<String>,
    closing: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ShopEvent> for ShopView {}

impl ShopView {
    pub fn new(core: Arc<Core>, key: String, server: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Drawn again when the instance changes (a balance, the shop, someone's roles).
        let mut changes = core.changes();
        let watching = core.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut seen = watching.shared.version();
            while changes.changed().await.is_ok() {
                let now = watching.shared.version();
                let changed = watching.shared.changed_since(seen, &[]);
                seen = now;
                if this
                    .update(cx, |_, cx| {
                        if changed {
                            cx.notify()
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // Claim buttons count down to the next UTC day.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(30)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let finder = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.economy.send.findPlaceholder")));
        let amount = cx.new(|cx| InputState::new(window, cx).placeholder("100"));
        let note = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.economy.send.notePlaceholder")));
        let typed = |this: &mut Self, _: Entity<InputState>, e: &InputEvent, cx: &mut Context<Self>| {
            if matches!(e, InputEvent::Change) {
                this.error = None;
                this.sent = None;
                cx.notify();
            }
        };
        let subscriptions =
            vec![cx.subscribe(&finder, typed), cx.subscribe(&amount, typed), cx.subscribe(&note, typed)];
        // What's sold and what you hold, as they are now.
        let (c, k, s) = (core.clone(), key.clone(), server.clone());
        drop(core.spawn(async move { c.load_economy(&k, &s).await }));
        Self {
            core,
            key,
            server,
            screen: Screen::Shop,
            list_scroll: UniformListScrollHandle::new(),
            confirming: None,
            buying: None,
            bought: None,
            picked: None,
            pulling: None,
            show: None,
            rates_open: false,
            claiming: None,
            claimed: None,
            ledger: Vec::new(),
            named: HashMap::new(),
            ledger_more: false,
            ledger_loading: false,
            ledger_read: false,
            ledger_currency: String::new(),
            ledger_scroll: UniformListScrollHandle::new(),
            send_currency: None,
            send_to: None,
            finder,
            amount,
            note,
            sending: false,
            sent: None,
            error: None,
            closing: None,
            _subscriptions: subscriptions,
        }
    }

    /// Runs `future` on the core and hands its result back to the view.
    fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let rx = self.core.spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = rx.await {
                let _ = this.update(cx, |this, cx| done(this, value, cx));
            }
        })
        .detach();
    }

    /// Escape: back to the shop from another screen, then closes.
    pub fn escape(&mut self, cx: &mut Context<Self>) {
        // A pull showing: Escape skips to the end, then puts it away.
        if let Some(show) = &self.show {
            if show.done() {
                self.show = None;
                cx.notify();
            } else {
                self.skip_show(cx);
            }
            return;
        }
        if self.confirming.take().is_some() {
            cx.notify();
            return;
        }
        if self.screen != Screen::Shop {
            self.go(Screen::Shop, cx);
            return;
        }
        self.close(cx);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing.is_some() {
            return;
        }
        if cx.reduce_motion() {
            cx.emit(ShopEvent::Close);
            return;
        }
        self.closing = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OPENING).await;
            let _ = this.update(cx, |_, cx| cx.emit(ShopEvent::Close));
        })
        .detach();
    }

    fn go(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        self.error = None;
        self.confirming = None;
        if screen == Screen::History {
            self.load_ledger(false, cx);
        }
        cx.notify();
    }

    /// Your history from the top, or the next page with `older`.
    fn load_ledger(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.ledger_loading || (older && !self.ledger_more) {
            return;
        }
        let before = if older { self.ledger.last().map(|e| e.id.clone()).unwrap_or_default() } else { String::new() };
        self.ledger_loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let currency = self.ledger_currency.clone();
        let asked = currency.clone();
        self.run(
            cx,
            async move { core.ledger(&key, &sid, &currency, "", &[], &before, eco::PAGE).await },
            move |this, result, cx| {
                this.ledger_loading = false;
                // Another currency was picked meanwhile: its own page is on the way.
                if this.ledger_currency != asked {
                    this.load_ledger(false, cx);
                    return;
                }
                match result {
                    Ok(page) => {
                        if !older {
                            this.ledger.clear();
                        }
                        this.ledger.extend(page.entries);
                        this.named.extend(page.users.into_iter().map(|u| (u.id.clone(), u)));
                        this.ledger_more = page.has_more;
                        this.ledger_read = true;
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn claim(&mut self, currency_id: String, cx: &mut Context<Self>) {
        if self.claiming.is_some() {
            return;
        }
        self.claiming = Some(currency_id.clone());
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let id = currency_id.clone();
        self.run(cx, async move { core.claim_daily(&key, &sid, &id).await }, move |this, result, cx| {
            this.claiming = None;
            match result {
                Ok(res) => {
                    let got = res.entry.map_or(0, |e| e.amount);
                    this.claimed = Some((currency_id, got, Instant::now()));
                    this.ledger_read = false;
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn buy(&mut self, listing: pb::ShopListing, name: String, cx: &mut Context<Self>) {
        if self.buying.is_some() {
            return;
        }
        self.buying = Some(listing.id.clone());
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let id = listing.id.clone();
        self.run(cx, async move { core.buy(&key, &sid, &listing).await }, move |this, result, cx| {
            this.buying = None;
            this.confirming = None;
            match result {
                Ok(_) => {
                    this.bought = Some((id, Instant::now()));
                    this.ledger_read = false;
                    cx.emit(ShopEvent::Toast {
                        icon: "shopping-bag",
                        title: t_with("desktop.economy.shop.boughtToast", &[("name", Arg::Str(&name))]),
                    });
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn pull(&mut self, banner: &pb::GachaBanner, count: i32, cx: &mut Context<Self>) {
        let Some(price) = eco::pull_cost(banner, count) else { return };
        if self.pulling.is_some() {
            return;
        }
        self.pulling = Some(count);
        self.error = None;
        let (core, key, sid, id) = (self.core.clone(), self.key.clone(), self.server.clone(), banner.id.clone());
        let banner = banner.clone();
        self.run(cx, async move { core.pull(&key, &sid, &id, count, price).await }, move |this, result, cx| {
            this.pulling = None;
            match result {
                Ok(res) => {
                    if let Some(pull) = res.pull {
                        this.show = Some(gacha::Show::new(pull, banner, cx.reduce_motion()));
                    }
                    this.ledger_read = false;
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let (Some(currency), Some(to)) = (self.send_currency.clone(), self.send_to.clone()) else { return };
        let Some(amount) = parse_amount(&self.amount.read(cx).value()).filter(|n| *n > 0) else {
            self.error = Some(t("desktop.economy.give.amountNeeded"));
            cx.notify();
            return;
        };
        let note = self.note.read(cx).value().to_string();
        if self.sending || note.chars().count() > NOTE_MAX {
            return;
        }
        self.sending = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.transfer(&key, &sid, &currency, &to, amount, &note).await },
            |this, result, cx| {
                this.sending = false;
                match result {
                    Ok(_) => {
                        this.sent = Some(Instant::now());
                        this.ledger_read = false;
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Everything the screen needs from the store.
    fn facts(&self) -> Facts {
        self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return Facts::default() };
            let economy = i.economy(&self.server).cloned().unwrap_or_default();
            let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
            Facts {
                name: i.server(&self.server).map(|s| s.name.clone()),
                open: i.has_currencies(&self.server),
                role_ids: i.my_member(&self.server).map(|m| m.role_ids.clone()).unwrap_or_default(),
                roles: i.roles.get(&self.server).cloned().unwrap_or_default(),
                items: i.server_list(&self.server).to_vec(),
                emojis: i.emojis.get(&self.server).cloned().unwrap_or_default(),
                members: i
                    .members
                    .get(&self.server)
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m.user.clone())
                    .filter(|u| u.id != me)
                    .collect(),
                economy,
                my_name: i.display_name(Some(&self.server), &me),
                channels: {
                    let access = i.access(&self.server);
                    i.channels
                        .get(&self.server)
                        .into_iter()
                        .flatten()
                        .filter(|c| {
                            matches!(
                                pb::ChannelType::try_from(c.r#type),
                                Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                            ) && c.shared.is_none()
                                && access.has_in(&c.id, pb::Permission::SendMessages)
                        })
                        .map(|c| (c.id.clone(), c.name.clone()))
                        .collect()
                },
                me,
            }
        })
    }

    fn name_of(&self, user_id: &str) -> String {
        self.core.shared.read(|s| {
            s.instance(&self.key)
                .map(|i| crate::ui::economy::person_name(i, &self.server, user_id, &self.named))
                .unwrap_or_default()
        })
    }
}

#[derive(Default)]
struct Facts {
    name: Option<String>,
    open: bool,
    economy: Economy,
    role_ids: Vec<String>,
    roles: Vec<pb::Role>,
    items: Vec<pb::ProfileItem>,
    emojis: Vec<pb::Emoji>,
    /// Everyone here but you, for Send.
    members: Vec<pb::User>,
    me: String,
    /// Your name here, for a pull's picture.
    my_name: String,
    /// Where a pull can be shared: the text channels you can post in (id, name).
    channels: Vec<(String, String)>,
}

/// A countdown to a moment `ms` away: "5h 12m", "12m".
fn countdown(ms: i64) -> String {
    let minutes = (ms.max(0) + 59_999) / 60_000;
    if minutes >= 60 { format!("{}h {}m", minutes / 60, minutes % 60) } else { format!("{minutes}m") }
}

/// A role's color, or the muted text's for none.
fn role_color(role: Option<&pb::Role>, p: &Palette) -> Hsla {
    role.and_then(|r| r.color).map(|c| gpui_kit::rgb(c as u32).into()).unwrap_or(p.muted_foreground.into())
}

/// What a line of history says happened.
pub(crate) fn ledger_line(
    entry: &pb::LedgerEntry,
    name_of: &dyn Fn(&str) -> String,
    listing: Option<String>,
) -> String {
    match LedgerKind::try_from(entry.kind).unwrap_or(LedgerKind::Earn) {
        LedgerKind::Earn | LedgerKind::Unspecified => {
            let action = Action::try_from(entry.action).unwrap_or(Action::Message);
            t_with("desktop.economy.ledger.earn", &[("action", Arg::Str(&t(eco::action_key(action))))])
        }
        LedgerKind::Grant if entry.amount < 0 => {
            t_with("desktop.economy.ledger.taken", &[("name", Arg::Str(&name_of(&entry.actor_id)))])
        }
        LedgerKind::Grant => t_with("desktop.economy.ledger.granted", &[("name", Arg::Str(&name_of(&entry.actor_id)))]),
        LedgerKind::Transfer if entry.amount < 0 => {
            t_with("desktop.economy.ledger.sentTo", &[("name", Arg::Str(&name_of(&entry.counterpart_id)))])
        }
        LedgerKind::Transfer => {
            t_with("desktop.economy.ledger.from", &[("name", Arg::Str(&name_of(&entry.counterpart_id)))])
        }
        LedgerKind::Purchase => match listing {
            Some(name) => t_with("desktop.economy.ledger.bought", &[("name", Arg::Str(&name))]),
            None => t("desktop.economy.ledger.boughtGone"),
        },
        LedgerKind::Gacha => {
            let banner = listing.unwrap_or_else(|| t("chattools.economy.gacha.aBanner"));
            let key = if entry.amount < 0 {
                "desktop.economy.ledger.pulled"
            } else if entry.note == "duplicate" {
                "desktop.economy.ledger.gachaRefund"
            } else {
                "desktop.economy.ledger.gachaWon"
            };
            t_with(key, &[("name", Arg::Str(&banner))])
        }
    }
}

/// A line of history's look: its icon.
pub(crate) fn ledger_glyph(entry: &pb::LedgerEntry) -> &'static str {
    match LedgerKind::try_from(entry.kind).unwrap_or(LedgerKind::Earn) {
        LedgerKind::Earn | LedgerKind::Unspecified => {
            eco::action_glyph(Action::try_from(entry.action).unwrap_or(Action::Message))
        }
        LedgerKind::Grant => "hand-coins",
        LedgerKind::Transfer if entry.amount < 0 => "arrow-up-right",
        LedgerKind::Transfer => "arrow-down-left",
        LedgerKind::Purchase => "shopping-bag",
        LedgerKind::Gacha => "dices",
    }
}

/// An amount in or out: green with a plus, or red with a minus.
pub(crate) fn signed(amount: i64, p: &Palette) -> (String, Hsla) {
    let green: Hsla = gpui_kit::rgb(if p.dark { 0x34d399 } else { 0x059669 }).into();
    if amount < 0 {
        (format!("−{}", amount_text(-amount)), p.destructive.into())
    } else {
        (format!("+{}", amount_text(amount)), green)
    }
}

fn empty(glyph: &'static str, text: &str, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .py(px(48.0))
            .text_sm()
            .text_center()
            .text_color(p.muted_foreground)
            .child(motion::pop(
                div().child(icon(glyph).size(px(32.0))),
                SharedString::from(format!("shop-empty-{glyph}")),
                0.5,
                -12.0,
                Duration::from_millis(60),
            ))
            .child(text.to_owned()),
        SharedString::from(format!("shop-empty-in-{glyph}")),
        Duration::ZERO,
        8.0,
    )
    .into_any_element()
}

impl ShopView {
    /// Your balance in each currency: what it is, how to earn it, today's reward and Send.
    fn balances(&mut self, f: &Facts, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = now_ms();
        let gold = gold(p);
        let mut row = div().flex().flex_wrap().gap(px(12.0));
        for (n, c) in f.economy.currencies.iter().enumerate() {
            let balance = f.economy.balances.get(&c.id);
            let amount = balance.map_or(0, |b| b.amount);
            let mut card = div()
                .w(px(260.0))
                .flex_none()
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.card, 0.8))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div()
                                .size(px(40.0))
                                .flex_none()
                                .rounded(radius_xl())
                                .bg(gold.opacity(0.12))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(currency_icon(c, &f.emojis, 24.0)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().text_color(p.muted_foreground).truncate().child(c.name.clone()))
                                .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(motion::count(
                                    format!("shop-bal-{}", c.id),
                                    amount.max(0) as u64,
                                    None,
                                    20.0,
                                    window,
                                    cx,
                                ))),
                        ),
                )
                .when(!c.description.is_empty(), |el| {
                    el.child(div().text_xs().text_color(p.muted_foreground).child(c.description.clone()))
                });
            // How it's earned, as small chips.
            let earns: Vec<AnyElement> = c
                .earn
                .iter()
                .filter_map(|r| Action::try_from(r.action).ok().map(|a| (a, r)))
                .enumerate()
                .map(|(n, (a, r))| {
                    let tip = eco::earn_line(r);
                    let text = if a == Action::GameMinute {
                        format!("+{} · {}", amount_text(r.amount), eco::games_short(&r.games))
                    } else {
                        format!("+{}", amount_text(r.amount))
                    };
                    div()
                        .id(SharedString::from(format!("shop-earn-{}-{n}", c.id)))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(8.0))
                        .h(px(22.0))
                        .rounded_full()
                        .bg(alpha(p.muted, 0.8))
                        .text_size(px(11.0))
                        .text_color(p.muted_foreground)
                        .child(icon(eco::action_glyph(a)).size(px(12.0)))
                        .child(text)
                        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                        .into_any_element()
                })
                .collect();
            if !earns.is_empty() {
                card = card.child(div().flex().flex_wrap().gap(px(4.0)).children(earns));
            }
            let mut actions = div().flex().items_center().gap(px(6.0));
            if let Some(rule) = eco::rule(c, Action::Daily) {
                let done = claimed_today(balance, now);
                let streak = streak_now(balance, now);
                let busy = self.claiming.as_deref() == Some(c.id.as_str());
                let label = if done {
                    t_with("desktop.economy.shop.claimIn", &[("time", Arg::Str(&countdown(until_next_day(now))))])
                } else {
                    t_with(
                        "desktop.economy.shop.claim",
                        &[("amount", Arg::Str(&amount_text(daily_reward(rule, streak))))],
                    )
                };
                let id = c.id.clone();
                actions = actions.child(
                    button(
                        SharedString::from(format!("shop-claim-{}", c.id)),
                        label,
                        Some("gift"),
                        Look::Primary,
                        true,
                        p,
                    )
                    .rounded(radius_xl())
                    .when(done || busy, |el| el.opacity(0.5))
                    .when(!done && !busy, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| this.claim(id.clone(), cx)))
                    }),
                );
                let shown_streak = if done { balance.map_or(0, |b| b.daily_streak) } else { streak - 1 };
                if shown_streak > 1 {
                    actions =
                        actions.child(div().text_xs().font_weight(FontWeight::BOLD).text_color(gold).child(t_with(
                            "desktop.economy.shop.streak",
                            &[("count", Arg::Num(i64::from(shown_streak)))],
                        )));
                }
            }
            if c.transferable {
                let id = c.id.clone();
                actions = actions.child(
                    button(
                        SharedString::from(format!("shop-send-{}", c.id)),
                        t("desktop.economy.send.title"),
                        Some("send"),
                        Look::Outline,
                        true,
                        p,
                    )
                    .rounded(radius_xl())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send_currency = Some(id.clone());
                        this.go(Screen::Send, cx);
                    })),
                );
            }
            card = card.child(actions);
            if let Some((_, got, at)) =
                self.claimed.as_ref().filter(|(id, _, at)| *id == c.id && at.elapsed() < Duration::from_secs(3))
            {
                card = card.child(motion::pop(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(gold)
                        .child(t_with("desktop.economy.shop.claimed", &[("amount", Arg::Str(&amount_text(*got)))])),
                    SharedString::from(format!("shop-claimed-{}-{at:?}", c.id)),
                    0.6,
                    -8.0,
                    Duration::ZERO,
                ));
            }
            row = row.child(motion::rise(
                card,
                SharedString::from(format!("shop-cur-in-{}", c.id)),
                Duration::from_millis(40 * n.min(6) as u64),
                10.0,
            ));
        }
        row.into_any_element()
    }

    /// One listing: what it is, its price, stock and Buy (asked once before).
    fn listing_row(
        &self,
        l: &pb::ShopListing,
        f: &Facts,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = Kind::try_from(l.kind).unwrap_or(Kind::Custom);
        let name = listing_name(l, &f.roles, &f.items);
        let currency = f.economy.currency(&l.currency_id).cloned().unwrap_or_default();
        let owned = eco::owned(&f.economy, l, &f.role_ids);
        let why_not = eco::cannot_buy(&f.economy, l, &f.role_ids);
        let confirming = self.confirming.as_deref() == Some(l.id.as_str());
        let buying = self.buying.as_deref() == Some(l.id.as_str());
        let face: AnyElement = match kind {
            Kind::Role => {
                let role = f.roles.iter().find(|r| r.id == l.target_id);
                let tint = role_color(role, p);
                div()
                    .size(px(44.0))
                    .rounded(radius_xl())
                    .bg(tint.opacity(0.15))
                    .text_color(tint)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("shield").size(px(22.0)))
                    .into_any_element()
            }
            Kind::ProfileItem => {
                let item = f.items.iter().find(|i| i.id == l.target_id);
                let base = div()
                    .size(px(44.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.primary, 0.12))
                    .text_color(p.primary)
                    .flex()
                    .items_center()
                    .justify_center();
                match item.filter(|i| !i.picture_url.is_empty()) {
                    Some(i) => base.child(picture(i.picture_url.clone()).size(px(36.0))).into_any_element(),
                    None => base.child(icon("sparkles").size(px(22.0))).into_any_element(),
                }
            }
            _ => div()
                .size(px(44.0))
                .rounded(radius_xl())
                .bg(gold(p).opacity(0.12))
                .text_color(gold(p))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("package").size(px(22.0)))
                .into_any_element(),
        };
        let kind_word = match kind {
            Kind::Role => t("desktop.economy.kind.role"),
            Kind::ProfileItem => t("desktop.economy.kind.item"),
            _ => t("desktop.economy.kind.custom"),
        };
        let name_color = if kind == Kind::Role {
            role_color(f.roles.iter().find(|r| r.id == l.target_id), p)
        } else {
            p.foreground.into()
        };
        let stock: Option<(String, Hsla)> = match l.stock {
            Some(0) => Some((t("desktop.economy.shop.soldOut"), p.destructive.into())),
            Some(n) => {
                Some((t_with("desktop.economy.shop.left", &[("count", Arg::Num(n))]), p.muted_foreground.into()))
            }
            None => None,
        };
        let action: AnyElement = if owned {
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(10.0))
                .h(px(28.0))
                .rounded_full()
                .bg(alpha(p.primary, 0.12))
                .text_color(p.primary)
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .child(icon("check").size(px(14.0)))
                .child(t("desktop.economy.shop.owned"))
                .into_any_element()
        } else if l.gacha_only {
            let from: Vec<String> = f.economy.banners_with(&l.id, now_ms()).iter().map(|b| b.name.clone()).collect();
            let tip = t_with("chattools.economy.gacha.dropsFrom", &[("banners", Arg::Str(&from.join(", ")))]);
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id(SharedString::from(format!("shop-gacha-only-{}", l.id)))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(8.0))
                        .h(px(22.0))
                        .rounded_full()
                        .bg(alpha(p.primary, 0.12))
                        .text_color(p.primary)
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .child(icon("dices").size(px(12.0)))
                        .child(t("desktop.economy.shop.gachaOnly"))
                        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx)),
                )
                .child(
                    button(
                        SharedString::from(format!("shop-try-{}", l.id)),
                        t("chattools.economy.gacha.tryLuck"),
                        Some("dices"),
                        Look::Primary,
                        true,
                        p,
                    )
                    .rounded(radius_xl())
                    .on_click(cx.listener(|this, _, _, cx| this.go(Screen::Gacha, cx))),
                )
                .into_any_element()
        } else if confirming {
            let (listing, nm) = (l.clone(), name.clone());
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    button("shop-buy-no", t("common.cancel"), None, Look::Ghost, true, p)
                        .rounded(radius_xl())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming = None;
                            cx.notify();
                        })),
                )
                .child(
                    button(
                        "shop-buy-yes",
                        if buying { t("desktop.economy.shop.buying") } else { t("desktop.economy.shop.confirm") },
                        None,
                        Look::Primary,
                        true,
                        p,
                    )
                    .rounded(radius_xl())
                    .when(buying, |el| el.opacity(0.5).child(spinner("shop-buy-spin", 14.0, window)))
                    .on_click(cx.listener(move |this, _, _, cx| this.buy(listing.clone(), nm.clone(), cx))),
                )
                .into_any_element()
        } else {
            let id = l.id.clone();
            button(
                SharedString::from(format!("shop-buy-{}", l.id)),
                why_not.map(t).unwrap_or_else(|| t("desktop.economy.shop.buy")),
                Some("shopping-bag"),
                Look::Primary,
                true,
                p,
            )
            .rounded(radius_xl())
            .when(why_not.is_some(), |el| el.opacity(0.5))
            .when(why_not.is_none(), |el| {
                el.on_click(cx.listener(move |this, _, _, cx| {
                    this.confirming = Some(id.clone());
                    cx.notify();
                }))
            })
            .into_any_element()
        };
        let hover = alpha(p.primary, 0.3);
        let card = div()
            .id(SharedString::from(format!("shop-l-{}", l.id)))
            .h(px(LISTING_ROW - 10.0))
            .px(px(14.0))
            .flex()
            .items_center()
            .gap(px(14.0))
            .rounded(radius_2xl())
            .border_1()
            .map(|el| {
                if confirming {
                    el.border_color(alpha(p.primary, 0.5)).bg(alpha(p.primary, 0.06))
                } else {
                    el.border_color(p.border).bg(alpha(p.card, 0.8)).hover(move |s| s.border_color(hover))
                }
            })
            .child(face)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().font_weight(FontWeight::BOLD).truncate().text_color(name_color).child(name))
                            .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(kind_word)),
                    )
                    .when(!l.description.is_empty(), |el| {
                        el.child(div().text_sm().truncate().text_color(p.muted_foreground).child(l.description.clone()))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .text_sm()
                            .child(money(&currency, &f.emojis, l.price, 16.0))
                            .when_some(stock, |el, (s, c)| el.child(div().text_xs().text_color(c).child(s))),
                    ),
            )
            .child(action);
        match self.bought.as_ref().filter(|(id, at)| *id == l.id && at.elapsed() < Duration::from_secs(2)) {
            Some((_, at)) => {
                motion::pop(card, SharedString::from(format!("shop-bought-{at:?}")), 0.94, 0.0, Duration::ZERO)
                    .into_any_element()
            }
            None => card.into_any_element(),
        }
    }

    fn shop_screen(&mut self, f: &Facts, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let balances = self.balances(f, p, window, cx);
        let listings: Vec<pb::ShopListing> = f.economy.for_sale(now_ms()).into_iter().cloned().collect();
        let list: AnyElement = if listings.is_empty() {
            empty("store", &t("desktop.economy.shop.empty"), p)
        } else {
            let rows = std::rc::Rc::new(listings);
            let facts = std::rc::Rc::new(Facts {
                name: None,
                open: true,
                economy: f.economy.clone(),
                role_ids: f.role_ids.clone(),
                roles: f.roles.clone(),
                items: f.items.clone(),
                emojis: f.emojis.clone(),
                members: Vec::new(),
                me: f.me.clone(),
                my_name: String::new(),
                channels: Vec::new(),
            });
            uniform_list(
                "shop-listings",
                rows.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let p = pal(cx);
                    range
                        .filter_map(|n| rows.get(n))
                        .map(|l| {
                            div()
                                .h(px(LISTING_ROW))
                                .pb(px(10.0))
                                .child(this.listing_row(l, &facts, &p, window, cx))
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.list_scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(balances)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(t("desktop.economy.shop.forSale").to_uppercase(), crate::ui::text::WIDE)),
            )
            .child(list)
            .into_any_element()
    }

    fn ledger_row(&self, e: &pb::LedgerEntry, f: &Facts, p: &Palette) -> AnyElement {
        let currency = f.economy.currency(&e.currency_id).cloned().unwrap_or_default();
        let listing = eco::entry_subject(&f.economy, e, &f.roles, &f.items);
        let name_of = |id: &str| self.name_of(id);
        let line = ledger_line(e, &name_of, listing);
        let (amount, tint) = signed(e.amount, p);
        let when = crate::ui::text::ago(ms_of(e.created_at.as_ref()), now_ms());
        div()
            .h(px(LEDGER_ROW - 6.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded(radius_xl())
            .bg(alpha(p.card, 0.6))
            .child(
                div()
                    .size(px(34.0))
                    .flex_none()
                    .rounded(radius_lg())
                    .bg(tint.opacity(0.12))
                    .text_color(tint)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(ledger_glyph(e)).size(px(16.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).truncate().child(line))
                    .child(div().text_xs().truncate().text_color(p.muted_foreground).child(if e.note.is_empty() {
                        when
                    } else {
                        format!("{when} · “{}”", e.note)
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_sm()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(tint)
                            .child(currency_icon(&currency, &f.emojis, 14.0))
                            .child(amount),
                    )
                    .child(div().text_xs().text_color(p.muted_foreground).child(t_with(
                        "desktop.economy.ledger.after",
                        &[("amount", Arg::Str(&amount_text(e.balance_after)))],
                    ))),
            )
            .into_any_element()
    }

    fn history_screen(&mut self, f: &Facts, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.ledger_read && !self.ledger_loading {
            self.load_ledger(false, cx);
        }
        let list = self.ledger_list(f, p, window, cx);
        // With more than one currency, which one's history (the web's chips).
        if f.economy.currencies.len() < 2 {
            return list;
        }
        let mut chips = div().flex().flex_wrap().gap(px(6.0));
        let all = std::iter::once((String::new(), t("chattools.economy.allCurrencies"), None));
        for (id, name, currency) in
            all.chain(f.economy.currencies.iter().map(|c| (c.id.clone(), c.name.clone(), Some(c))))
        {
            let on = self.ledger_currency == id;
            let hover = alpha(p.primary, 0.3);
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("ledger-cur-{id}")))
                    .h(px(30.0))
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
                        if this.ledger_currency != id {
                            this.ledger_currency = id.clone();
                            this.ledger.clear();
                            this.ledger_more = false;
                            this.ledger_read = false;
                            this.load_ledger(false, cx);
                        }
                    }))
                    .when_some(currency, |el, c| el.child(currency_icon(c, &f.emojis, 14.0)))
                    .child(name),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(div().flex_none().child(chips))
            .child(list)
            .into_any_element()
    }

    fn ledger_list(&mut self, f: &Facts, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.ledger.is_empty() {
            return if self.ledger_loading || !self.ledger_read {
                crate::ui::server_settings::shimmer_rows(6, p).into_any_element()
            } else {
                empty("receipt", &t("desktop.economy.ledger.empty"), p)
            };
        }
        let rows = std::rc::Rc::new(self.ledger.clone());
        let more = self.ledger_more;
        let facts = std::rc::Rc::new(Facts {
            economy: f.economy.clone(),
            roles: f.roles.clone(),
            items: f.items.clone(),
            emojis: f.emojis.clone(),
            ..Facts::default()
        });
        let _ = window;
        uniform_list(
            "shop-ledger",
            rows.len() + usize::from(more),
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let p = pal(cx);
                range
                    .map(|n| match rows.get(n) {
                        Some(e) => {
                            div().h(px(LEDGER_ROW)).pb(px(6.0)).child(this.ledger_row(e, &facts, &p)).into_any_element()
                        }
                        None => {
                            let loading = this.ledger_loading;
                            div()
                                .h(px(LEDGER_ROW))
                                .pt(px(10.0))
                                .flex()
                                .justify_center()
                                .child(
                                    button(
                                        "shop-ledger-more",
                                        t("desktop.modView.more"),
                                        None,
                                        Look::Outline,
                                        false,
                                        &p,
                                    )
                                    .rounded(radius_xl())
                                    .when(loading, |el| {
                                        el.opacity(0.5).child(spinner("shop-ledger-spin", 16.0, window))
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.load_ledger(true, cx))),
                                )
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.ledger_scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    fn send_screen(&mut self, f: &Facts, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let sendable: Vec<&pb::Currency> = f.economy.currencies.iter().filter(|c| c.transferable).collect();
        if sendable.is_empty() {
            return empty("send", &t("desktop.economy.send.none"), p);
        }
        if self.send_currency.as_ref().is_none_or(|id| !sendable.iter().any(|c| c.id == *id)) {
            self.send_currency = sendable.first().map(|c| c.id.clone());
        }
        let label = |text: String| div().text_sm().font_weight(FontWeight::BOLD).child(text);
        let mut col = div().max_w(px(520.0)).flex().flex_col().gap(px(14.0));
        // Which currency, with what you have of it.
        let mut picks = div().flex().flex_wrap().gap(px(6.0));
        for c in &sendable {
            let on = self.send_currency.as_deref() == Some(c.id.as_str());
            let id = c.id.clone();
            let hover = alpha(p.primary, 0.3);
            picks = picks.child(
                div()
                    .id(SharedString::from(format!("send-cur-{}", c.id)))
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
                        this.send_currency = Some(id.clone());
                        cx.notify();
                    }))
                    .child(currency_icon(c, &f.emojis, 16.0))
                    .child(c.name.clone())
                    .child(div().text_xs().text_color(p.muted_foreground).child(amount_text(f.economy.amount(&c.id)))),
            );
        }
        col = col.child(label(t("desktop.economy.give.currency"))).child(picks);
        // Who to: picked, or found by name.
        col = col.child(label(t("desktop.economy.send.to")));
        match self.send_to.clone() {
            Some(uid) => {
                let user = f.members.iter().find(|u| u.id == uid).cloned();
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .p(px(10.0))
                        .rounded(radius_xl())
                        .bg(alpha(p.muted, 0.6))
                        .child(avatar(user.as_ref(), 32.0, p))
                        .child(
                            div().flex_1().min_w_0().font_weight(FontWeight::BOLD).truncate().child(self.name_of(&uid)),
                        )
                        .child(
                            button("send-to-change", t("desktop.economy.send.change"), None, Look::Ghost, true, p)
                                .rounded(radius_xl())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.send_to = None;
                                    cx.notify();
                                })),
                        ),
                );
            }
            None => {
                let query = self.finder.read(cx).value().to_lowercase();
                let found: Vec<&pb::User> = f
                    .members
                    .iter()
                    .filter(|u| {
                        query.is_empty()
                            || u.username.to_lowercase().contains(&query)
                            || u.display_name.to_lowercase().contains(&query)
                    })
                    .take(MATCHES)
                    .collect();
                let mut list = div().flex().flex_col().gap(px(2.0));
                for u in found {
                    let id = u.id.clone();
                    let hover = alpha(p.muted, 0.7);
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("send-who-{}", u.id)))
                            .h(px(40.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .rounded(radius_lg())
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .active(|s| s.scale(0.99))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send_to = Some(id.clone());
                                cx.notify();
                            }))
                            .child(avatar(Some(u), 26.0, p))
                            .child(div().text_sm().font_weight(FontWeight::BOLD).truncate().child(self.name_of(&u.id)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .truncate()
                                    .child(format!("@{}", u.username)),
                            ),
                    );
                }
                col = col.child(field(Input::new(&self.finder).appearance(false), p)).child(list);
            }
        }
        let have = self.send_currency.as_deref().map_or(0, |c| f.economy.amount(c));
        let typed = parse_amount(&self.amount.read(cx).value()).filter(|n| *n > 0);
        let too_much = typed.is_some_and(|n| n > have);
        col = col
            .child(
                div().flex().items_center().justify_between().child(label(t("desktop.economy.give.amount"))).child(
                    div()
                        .text_xs()
                        .text_color(if too_much { p.destructive } else { p.muted_foreground })
                        .child(t_with("desktop.economy.send.have", &[("amount", Arg::Str(&amount_text(have)))])),
                ),
            )
            .child(field(Input::new(&self.amount).appearance(false), p))
            .child(label(t("desktop.economy.send.note")))
            .child(field(Input::new(&self.note).appearance(false), p));
        let ready = !self.sending && typed.is_some() && !too_much && self.send_to.is_some();
        let sent = crate::ui::economy::fresh(self.sent, Duration::from_secs(3));
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    button(
                        "send-go",
                        if self.sending { t("desktop.economy.send.sending") } else { t("desktop.economy.send.title") },
                        Some("send"),
                        Look::Primary,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .when(!ready, |el| el.opacity(0.5))
                    .when(ready, |el| el.on_click(cx.listener(|this, _, _, cx| this.send(cx)))),
                )
                .when(sent, |el| {
                    el.child(motion::pop(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(gold(p))
                            .child(icon("circle-check").size(px(16.0)))
                            .child(t("desktop.economy.send.sent")),
                        SharedString::from(format!("send-sent-{:?}", self.sent)),
                        0.6,
                        -8.0,
                        Duration::ZERO,
                    ))
                }),
        );
        let _ = window;
        div().id("shop-send-scroll").flex_1().min_h_0().overflow_y_scroll().pb(px(24.0)).child(col).into_any_element()
    }
}

impl Render for ShopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let f = self.facts();
        let Some(server_name) = f.name.clone().filter(|_| f.open) else {
            // The server went away, or its currencies did.
            cx.defer_in(window, |_, _, cx| cx.emit(ShopEvent::Close));
            return div().into_any_element();
        };
        let width = f32::from(window.viewport_size().width);
        let height = window.viewport_size().height;
        let aside = 240.0 + ((width - 1072.0).max(0.0) / 2.0);
        let gold = gold(&p);

        let nav_row = |id: &'static str, glyph: &'static str, label: String, on: bool| {
            let hover = alpha(p.muted, 0.7);
            let group = SharedString::from(id);
            div()
                .id(id)
                .group(group.clone())
                .h(px(32.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(radius_lg())
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(if on { p.primary } else { p.muted_foreground })
                .cursor_pointer()
                .when(!on, |el| el.hover(move |s| s.bg(hover)))
                .active(|s| s.scale(0.97))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-icon")))
                        .flex_none()
                        .group_hover(group, |s| s.rotate(gpui_kit::radians(-12f32.to_radians())).scale(1.1))
                        .child(icon(glyph).size(px(16.0))),
                )
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let gacha = !f.economy.shown_banners(now_ms()).is_empty();
        if !gacha && self.screen == Screen::Gacha {
            self.screen = Screen::Shop;
        }
        let mut screens = vec![(Screen::Shop, "shop-nav-shop", "store", t("desktop.economy.shop.title"))];
        if gacha {
            screens.push((Screen::Gacha, "shop-nav-gacha", "dices", t("chattools.economy.tab.gacha")));
        }
        screens.push((Screen::History, "shop-nav-history", "receipt", t("desktop.economy.ledger.title")));
        screens.push((Screen::Send, "shop-nav-send", "send", t("desktop.economy.send.title")));
        let at = screens.iter().position(|s| s.0 == self.screen).unwrap_or(0);
        let hl = motion::follow("shop-nav-hl", at as f32 * 34.0, window, cx);
        let mut rows = div().relative().flex().flex_col().gap(px(2.0)).child(
            div().absolute().left_0().right_0().top(px(hl)).h(px(32.0)).rounded(radius_lg()).bg(alpha(p.primary, 0.15)),
        );
        for (screen, id, glyph, label) in screens {
            rows = rows.child(
                nav_row(id, glyph, label, self.screen == screen)
                    .on_click(cx.listener(move |this, _, _, cx| this.go(screen, cx))),
            );
        }
        // Your balances, small, under where you are.
        let mut held = div().flex().flex_col().gap(px(6.0)).px(px(10.0));
        for c in &f.economy.currencies {
            held = held.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_sm()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(currency_icon(c, &f.emojis, 16.0))
                            .child(div().text_color(p.muted_foreground).truncate().child(c.name.clone())),
                    )
                    .child(div().font_weight(FontWeight::BOLD).child(motion::count(
                        format!("shop-side-{}", c.id),
                        f.economy.amount(&c.id).max(0) as u64,
                        None,
                        14.0,
                        window,
                        cx,
                    ))),
            );
        }
        let menu = div()
            .w(px(240.0))
            .flex_none()
            .pt(px(64.0))
            .pr(px(12.0))
            .pl(px(20.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(
                div()
                    .px(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(icon("store").size(px(18.0)).text_color(gold))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(t("desktop.economy.shop.title")),
                            ),
                    )
                    .child(div().text_xs().text_color(p.muted_foreground).truncate().child(server_name)),
            )
            .child(rows)
            .child(div().h(px(1.0)).mx(px(10.0)).bg(p.border))
            .child(held);

        let (title, about) = match self.screen {
            Screen::Shop => (t("desktop.economy.shop.title"), t("desktop.economy.shop.about")),
            Screen::Gacha => (t("chattools.economy.tab.gacha"), t("desktop.economy.gacha.about")),
            Screen::History => (t("desktop.economy.ledger.title"), t("desktop.economy.ledger.about")),
            Screen::Send => (t("desktop.economy.send.title"), t("desktop.economy.send.about")),
        };
        let body = match self.screen {
            Screen::Shop => self.shop_screen(&f, &p, window, cx),
            Screen::Gacha => self.gacha_screen(&f, &p, window, cx),
            Screen::History => self.history_screen(&f, &p, window, cx),
            Screen::Send => self.send_screen(&f, &p, window, cx),
        };
        let content = div()
            .flex_1()
            .min_w_0()
            .h(height)
            .pl(px(40.0))
            .pr(px(40.0 + 64.0))
            .pt(px(64.0))
            .pb(px(24.0))
            .flex()
            .flex_col()
            .bg(p.chat_surface)
            .child(
                div()
                    .mb(px(20.0))
                    .child(
                        div()
                            .text_2xl()
                            .line_height(px(32.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(tracked(title, TIGHT)),
                    )
                    .child(div().mt(px(4.0)).text_sm().text_color(p.muted_foreground).child(about)),
            )
            .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(div().mb(px(12.0)).child(e)))
            .child(motion::rise(
                div().flex_1().min_h_0().flex().flex_col().child(body),
                SharedString::from(format!("shop-page-{:?}", self.screen)),
                Duration::ZERO,
                14.0,
            ));

        let (hover_bg, hover_fg, hover_ring) = (p.muted, p.foreground, alpha(p.foreground, 0.4));
        let close = div()
            .id("shop-close")
            .group("shop-close")
            .absolute()
            .top(px(64.0))
            .right(px(24.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .id("shop-close-ring")
                    .size(px(40.0))
                    .rounded_full()
                    .border_2()
                    .border_color(p.border)
                    .text_color(p.muted_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .group_hover("shop-close", move |s| s.bg(hover_bg).text_color(hover_fg).border_color(hover_ring))
                    .active(|s| s.scale(0.9))
                    .child(
                        div()
                            .id("shop-close-x")
                            .group_hover("shop-close", |s| s.rotate(gpui_kit::radians(std::f32::consts::FRAC_PI_2)))
                            .child(icon("x").size(px(20.0))),
                    ),
            )
            .child(div().text_size(px(10.4)).font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("ESC"));

        let show = self.pull_show(&f, &p, window, cx);
        let behind = crate::ui::backdrop::layers(&crate::ui::theme::backdrop(cx), &p, window, cx);
        let screen = div()
            .id("shop")
            .absolute()
            .inset_0()
            .min_w(px(width))
            .min_h(height)
            .occlude()
            .flex()
            .bg(p.background)
            .text_color(p.foreground)
            .line_height(gpui_kit::relative(1.5))
            .when_some(behind, |el, behind| el.child(behind))
            .child(
                div()
                    .flex_none()
                    .w(px(aside))
                    .h(height)
                    .flex()
                    .justify_end()
                    .items_start()
                    .bg(p.side_surface)
                    .border_r_1()
                    .border_color(p.border)
                    .child(menu),
            )
            .child(content)
            .child(close)
            .when_some(show, |el, show| el.child(show));
        // It comes in as Mod View does, from a little larger, fading up, and goes the same way.
        use gpui_kit::{Animation, AnimationExt as _};
        let (id, out) = match self.closing {
            Some(at) => (SharedString::from(format!("shop-closing-{at:?}")), true),
            None => (SharedString::from("shop-opening"), false),
        };
        let (w, h) = (width, f32::from(height));
        screen
            .with_animation(id, Animation::new(OPENING).with_easing(gpui_kit::ease_out_quint()), move |el, t| {
                let k = if out { 1.0 - t } else { t };
                let (dx, dy) = (w * 0.02 * (1.0 - k), h * 0.02 * (1.0 - k));
                el.opacity(k).top(px(-dy)).bottom(px(-dy)).left(px(-dx)).right(px(-dx))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdowns_round_up_to_the_minute() {
        assert_eq!(countdown(61_000), "2m");
        assert_eq!(countdown(3 * 3_600_000 + 5 * 60_000), "3h 5m");
        assert_eq!(countdown(0), "0m");
    }
}
