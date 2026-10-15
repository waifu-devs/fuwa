//! The Finances page (Manage Economy, docs/economy.md): one currency over a
//! period (7, 30, 90 or 365 days). What's held now and by how many, what
//! came in and went out, a bar a day (earned and given up, spent down), who
//! holds, spends and earns the most, what sells, what each action earned,
//! and the whole ledger, a page at a time, by kind. With Grant Currency,
//! "Give to a role" opens the app's Give dialog.

use super::*;
use crate::core::economy::{self as eco, GrantTo, KINDS, PAGE, PERIODS, amount_text, listing_name};
use crate::pb::{CurrencyAction as Action, LedgerKind};
use crate::ui::economy::{currency_icon, gold};
use crate::ui::shop::{ledger_glyph, ledger_line, signed};
use crate::ui::text::ms_of;
use crate::ui::theme::radius_xl;

/// The chart's height.
const CHART: f32 = 120.0;

#[derive(Default)]
pub(super) struct Finances {
    /// The currency shown (the first one until another is picked) and the period.
    currency: Option<String>,
    days: Option<i32>,
    /// The overview, for the currency and period it was asked for.
    overview: Option<((String, i32), pb::EconomyOverview)>,
    asked: Option<(String, i32)>,
    /// The ledger: the kind it's filtered to, what's read, whether there's more.
    kind: Option<LedgerKind>,
    ledger: Vec<pb::LedgerEntry>,
    /// Everyone the entries name, as the ledger has them.
    named: HashMap<String, pb::User>,
    ledger_for: Option<(String, Option<LedgerKind>)>,
    more: bool,
    reading: bool,
    /// The day under the pointer in the chart.
    hovered: Option<usize>,
}

impl ServerSettingsView {
    fn finances_days(&self) -> i32 {
        self.finances.days.unwrap_or(30)
    }

    /// Asks for the overview when what's shown isn't what's picked.
    fn read_overview(&mut self, currency: &str, cx: &mut Context<Self>) {
        let want = (currency.to_owned(), self.finances_days());
        if self.finances.asked.as_ref() == Some(&want) {
            return;
        }
        self.finances.asked = Some(want.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let (cid, days) = want.clone();
        self.run(cx, async move { core.economy_overview(&key, &sid, &cid, days).await }, move |this, result, cx| {
            if this.finances.asked.as_ref() != Some(&want) {
                return;
            }
            match result {
                Ok(o) => this.finances.overview = Some((want, o)),
                Err(err) => {
                    this.error = Some(err.message);
                    this.finances.asked = None;
                }
            }
            cx.notify();
        });
    }

    /// Reads the ledger's first page for what's picked, or the next one.
    fn read_ledger(&mut self, currency: &str, next: bool, cx: &mut Context<Self>) {
        let want = (currency.to_owned(), self.finances.kind);
        if !next && self.finances.ledger_for.as_ref() == Some(&want) {
            return;
        }
        if next && (self.finances.reading || !self.finances.more) {
            return;
        }
        if !next {
            self.finances.ledger.clear();
            self.finances.more = false;
        }
        self.finances.ledger_for = Some(want.clone());
        self.finances.reading = true;
        let before =
            if next { self.finances.ledger.last().map(|e| e.id.clone()).unwrap_or_default() } else { String::new() };
        let kinds: Vec<LedgerKind> = self.finances.kind.into_iter().collect();
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let cid = want.0.clone();
        self.run(
            cx,
            async move { core.ledger(&key, &sid, &cid, "*", &kinds, &before, PAGE).await },
            move |this, result, cx| {
                if this.finances.ledger_for.as_ref() != Some(&want) {
                    return;
                }
                this.finances.reading = false;
                match result {
                    Ok(page) => {
                        // A page may overlap one read before it: keep each entry once.
                        for e in page.entries {
                            if !this.finances.ledger.iter().any(|x| x.id == e.id) {
                                this.finances.ledger.push(e);
                            }
                        }
                        this.finances.more = page.has_more;
                        this.finances.named.extend(page.users.into_iter().map(|u| (u.id.clone(), u)));
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
    }

    pub(super) fn finances_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (economy, emojis, roles, items, can_give) = self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return Default::default() };
            (
                i.economy(&self.server).cloned().unwrap_or_default(),
                i.emojis.get(&self.server).cloned().unwrap_or_default(),
                i.roles.get(&self.server).cloned().unwrap_or_default(),
                i.server_items.get(&self.server).cloned().unwrap_or_default(),
                i.can_grant_currency(&self.server),
            )
        });
        let Some(currency) = self
            .finances
            .currency
            .as_deref()
            .and_then(|id| economy.currency(id))
            .or_else(|| economy.currencies.first())
            .cloned()
        else {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .py(px(40.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .child(icon("landmark").size(px(28.0)))
                .child(t("desktop.economy.finances.none"))
                .into_any_element();
        };
        self.read_overview(&currency.id, cx);
        self.read_ledger(&currency.id, false, cx);
        let days = self.finances_days();
        let overview =
            self.finances.overview.as_ref().filter(|(k, _)| k.0 == currency.id && k.1 == days).map(|(_, o)| o.clone());
        let name_of = |id: &str| {
            self.core.shared.read(|s| {
                s.instance(&self.key)
                    .map(|i| crate::ui::economy::person_name(i, &self.server, id, &self.finances.named))
                    .unwrap_or_default()
            })
        };

        // The currency, the period and Give to a role.
        let mut picks = div().flex().flex_wrap().items_center().gap(px(6.0));
        if economy.currencies.len() > 1 {
            for c in &economy.currencies {
                let id = c.id.clone();
                picks = picks.child(
                    chip(SharedString::from(format!("fin-cur-{}", c.id)), &c.name, c.id == currency.id, p)
                        .child(div().ml(px(6.0)).child(currency_icon(c, &emojis, 14.0)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.finances.currency = Some(id.clone());
                            this.finances.hovered = None;
                            cx.notify();
                        })),
                );
            }
            picks = picks.child(div().w(px(1.0)).h(px(20.0)).mx(px(4.0)).bg(p.border));
        }
        for d in PERIODS {
            picks = picks.child(
                chip(
                    SharedString::from(format!("fin-days-{d}")),
                    &t_with("desktop.economy.finances.days", &[("count", Arg::Num(i64::from(d)))]),
                    d == days,
                    p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.finances.days = Some(d);
                    this.finances.hovered = None;
                    cx.notify();
                })),
            );
        }
        let mut top = div().flex().flex_wrap().items_center().gap(px(10.0)).child(div().flex_1().child(picks));
        if can_give {
            top = top.child(
                super::pages::act(
                    "fin-give-role",
                    t("desktop.economy.finances.giveRole"),
                    icon("hand-coins").size(px(16.0)),
                    Look::Primary,
                    p,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(ServerSettingsEvent::Give { to: GrantTo::Role(String::new()) });
                })),
            );
        }
        let mut page = div().flex().flex_col().gap(px(24.0)).child(top);

        let Some(o) = overview else {
            return page.child(shimmer_rows(4, p)).into_any_element();
        };

        // The tiles.
        let tiles: [(&str, &str, i64, Hsla); 6] = [
            ("desktop.economy.finances.circulating", "coins", o.circulating, gold(p)),
            ("desktop.economy.finances.earned", "trending-up", o.earned, gpui_kit::rgb(0x10b981).into()),
            ("desktop.economy.finances.granted", "hand-coins", o.granted - o.taken, amber(p)),
            ("desktop.economy.finances.spent", "shopping-bag", o.spent, p.primary.into()),
            ("desktop.economy.finances.transferred", "send", o.transferred, gpui_kit::rgb(0x0ea5e9).into()),
            ("desktop.economy.finances.purchases", "receipt", o.purchases, gpui_kit::rgb(0xa855f7).into()),
        ];
        let mut grid = div().flex().flex_wrap().gap(px(10.0));
        for (n, (key, glyph, value, tint)) in tiles.into_iter().enumerate() {
            let hint = match n {
                0 => Some(t_with("desktop.economy.finances.holders", &[("count", Arg::Num(i64::from(o.holders)))])),
                2 if o.taken > 0 => {
                    Some(t_with("desktop.economy.finances.takenBack", &[("amount", Arg::Str(&amount_text(o.taken)))]))
                }
                _ => None,
            };
            let tile = div()
                .flex_1()
                .min_w(px(150.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(14.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(div().text_color(tint).child(icon(glyph).size(px(14.0))))
                        .child(t(key)),
                )
                .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(motion::count_up(
                    SharedString::from(format!("fin-tile-{n}-{}-{days}", currency.id)),
                    value as f64,
                    Duration::from_millis(n as u64 * 35),
                    |v| amount_text(v.round() as i64),
                )))
                .when_some(hint, |el, h| el.child(div().text_xs().text_color(p.muted_foreground).child(h)));
            grid = grid.child(motion::rise(
                tile,
                SharedString::from(format!("fin-tile-in-{n}-{}-{days}", currency.id)),
                Duration::from_millis(n as u64 * 35),
                8.0,
            ));
        }
        page = page.child(grid);

        // A bar a day.
        page = page.child(self.finance_chart(&o, p, cx));

        // Who and what.
        let people = |title: &str, list: &[pb::PersonTotal], tag: &str| -> AnyElement {
            let mut col = div().flex().flex_col().gap(px(4.0));
            if list.is_empty() {
                col = col
                    .child(div().text_sm().text_color(p.muted_foreground).child(t("desktop.economy.finances.nobody")));
            }
            for (n, row) in list.iter().enumerate() {
                col = col.child(rank_row(
                    n,
                    name_of(&row.user_id),
                    row.amount,
                    &currency,
                    &emojis,
                    p,
                    &format!("{tag}-{n}"),
                ));
            }
            top_card(title, col, p)
        };
        let listings = {
            let mut col = div().flex().flex_col().gap(px(4.0));
            if o.top_listings.is_empty() {
                col = col.child(
                    div().text_sm().text_color(p.muted_foreground).child(t("desktop.economy.finances.nothingSold")),
                );
            }
            for (n, row) in o.top_listings.iter().enumerate() {
                let name = economy
                    .listing(&row.listing_id)
                    .map(|l| listing_name(l, &roles, &items))
                    .unwrap_or_else(|| t("desktop.economy.ledger.boughtGone"));
                let name = format!("{name} × {}", amount_text(row.count));
                col = col.child(rank_row(n, name, row.amount, &currency, &emojis, p, &format!("fin-l-{n}")));
            }
            top_card(&t("desktop.economy.finances.topListings"), col, p)
        };
        let actions = {
            let mut col = div().flex().flex_col().gap(px(8.0));
            let most = o.earned_by_action.iter().map(|a| a.amount).max().unwrap_or(0).max(1);
            if o.earned_by_action.is_empty() {
                col = col.child(
                    div().text_sm().text_color(p.muted_foreground).child(t("desktop.economy.finances.nothingEarned")),
                );
            }
            for row in &o.earned_by_action {
                let a = Action::try_from(row.action).unwrap_or(Action::Message);
                let share = row.amount.max(0) as f32 / most as f32;
                col = col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_sm()
                                .child(icon(eco::action_glyph(a)).size(px(14.0)))
                                .child(div().flex_1().child(t(eco::action_key(a))))
                                .child(div().font_weight(FontWeight::BOLD).child(amount_text(row.amount))),
                        )
                        .child(div().h(px(6.0)).w_full().rounded_full().bg(alpha(p.muted, 0.8)).child(
                            div().h_full().w(gpui_kit::relative(share.clamp(0.02, 1.0))).rounded_full().bg(gold(p)),
                        )),
                );
            }
            top_card(&t("desktop.economy.finances.byAction"), col, p)
        };
        page = page.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(12.0))
                .child(people(&t("desktop.economy.finances.topHolders"), &o.top_holders, "fin-h"))
                .child(people(&t("desktop.economy.finances.topEarners"), &o.top_earners, "fin-e"))
                .child(people(&t("desktop.economy.finances.topSpenders"), &o.top_spenders, "fin-s"))
                .child(listings)
                .child(actions),
        );

        // The whole ledger.
        let mut kinds = div().flex().flex_wrap().gap(px(6.0)).child(
            chip("fin-k-all".into(), &t("desktop.economy.finances.allKinds"), self.finances.kind.is_none(), p)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.finances.kind = None;
                    cx.notify();
                })),
        );
        for k in KINDS {
            kinds = kinds.child(
                chip(
                    SharedString::from(format!("fin-k-{}", k as i32)),
                    &t(eco::kind_key(k)),
                    self.finances.kind == Some(k),
                    p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.finances.kind = Some(k);
                    cx.notify();
                })),
            );
        }
        let mut ledger = div().flex().flex_col().gap(px(6.0));
        for e in &self.finances.ledger {
            let listing = eco::entry_subject(&economy, e, &roles, &items);
            let line = ledger_line(e, &name_of, listing);
            let (amount, tint) = signed(e.amount, p);
            let when = crate::ui::text::ago(ms_of(e.created_at.as_ref()), now_ms());
            let sub = if e.note.is_empty() { when } else { format!("{when} · “{}”", e.note) };
            ledger = ledger.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.background, 0.5))
                    .child(
                        div()
                            .size(px(30.0))
                            .flex_none()
                            .rounded(corner(10.0))
                            .bg(tint.opacity(0.12))
                            .text_color(tint)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(ledger_glyph(e)).size(px(14.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_weight(FontWeight::BOLD).truncate().child(name_of(&e.user_id)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .truncate()
                                    .child(format!("{line} · {sub}")),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_sm()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(tint)
                            .child(currency_icon(&currency, &emojis, 14.0))
                            .child(amount),
                    ),
            );
        }
        if self.finances.ledger.is_empty() && !self.finances.reading {
            ledger = ledger.child(
                div().py(px(12.0)).text_sm().text_color(p.muted_foreground).child(t("desktop.economy.ledger.empty")),
            );
        }
        if self.finances.reading {
            ledger =
                ledger.child(div().flex().justify_center().py(px(8.0)).child(spinner("fin-ledger-spin", 18.0, window)));
        } else if self.finances.more {
            let cid = currency.id.clone();
            ledger = ledger.child(
                div().flex().justify_center().child(
                    soft_button("fin-ledger-more", t("desktop.modView.more"), p)
                        .on_click(cx.listener(move |this, _, _, cx| this.read_ledger(&cid, true, cx))),
                ),
            );
        }
        page.child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(super::pages::heading(
                    &t("desktop.economy.finances.ledger"),
                    Some(&t("desktop.economy.finances.ledgerAbout")),
                    p,
                ))
                .child(kinds)
                .child(ledger),
        )
        .into_any_element()
    }

    /// The days of the period as bars: earned and given above the line, spent below.
    fn finance_chart(&self, o: &pb::EconomyOverview, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let up = |d: &pb::DayTotal| d.earned + d.granted;
        let most = o.days.iter().map(|d| up(d).max(d.spent)).max().unwrap_or(0).max(1) as f32;
        let earned: Hsla = gpui_kit::rgb(0x10b981).into();
        let granted = amber(p);
        let spent: Hsla = p.primary.into();
        let gap = if o.days.len() > 120 {
            0.0
        } else if o.days.len() > 40 {
            1.0
        } else {
            3.0
        };
        let mut bars = div().flex().items_end().gap(px(gap)).h(px(CHART)).w_full();
        let mut below = div().flex().items_start().gap(px(gap)).h(px(CHART * 0.4)).w_full();
        for (n, d) in o.days.iter().enumerate() {
            let hovered = self.finances.hovered == Some(n);
            let h = |v: i64, of: f32| px((v.max(0) as f32 / most * of).max(if v > 0 { 2.0 } else { 0.0 }));
            bars = bars.child(
                div()
                    .id(SharedString::from(format!("fin-day-{n}")))
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .rounded_t(px(3.0))
                    .when(hovered, |el| el.bg(alpha(p.muted, 0.6)))
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                        if *on {
                            this.finances.hovered = Some(n);
                        } else if this.finances.hovered == Some(n) {
                            this.finances.hovered = None;
                        }
                        cx.notify();
                    }))
                    .child(div().w_full().h(h(d.granted, CHART)).bg(granted).rounded_t(px(3.0)))
                    .child(div().w_full().h(h(d.earned, CHART)).bg(earned)),
            );
            below = below.child(div().flex_1().h(h(d.spent, CHART * 0.4)).bg(spent.opacity(0.75)).rounded_b(px(3.0)));
        }
        let shown = self.finances.hovered.and_then(|n| o.days.get(n)).or(o.days.last());
        let caption = shown.map(|d| {
            let day = crate::ui::moderate::stamp(ms_of(d.day.as_ref()));
            t_with(
                "desktop.economy.finances.day",
                &[
                    ("day", Arg::Str(&day)),
                    ("earned", Arg::Str(&amount_text(d.earned))),
                    ("granted", Arg::Str(&amount_text(d.granted))),
                    ("spent", Arg::Str(&amount_text(d.spent))),
                ],
            )
        });
        let key = |c: Hsla, label: String| {
            div().flex().items_center().gap(px(4.0)).child(div().size(px(8.0)).rounded_full().bg(c)).child(label)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(16.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.5))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(div().flex_1().font_weight(FontWeight::BOLD).child(caption.unwrap_or_default()))
                    .child(key(earned, t("desktop.economy.finances.earned")))
                    .child(key(granted, t("desktop.economy.finances.granted")))
                    .child(key(spent, t("desktop.economy.finances.spent"))),
            )
            .child(bars)
            .child(div().h(px(1.0)).w_full().bg(p.border))
            .child(below)
            .into_any_element()
    }
}

/// One of the top lists' cards.
fn top_card(title: &str, body: Div, p: &Palette) -> AnyElement {
    div()
        .flex_1()
        .min_w(px(240.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .p(px(14.0))
        .rounded(corner(16.0))
        .border_1()
        .border_color(p.border)
        .bg(alpha(p.background, 0.5))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(body)
        .into_any_element()
}

/// A place in a top list: its number, who or what, how much.
fn rank_row(
    n: usize,
    name: String,
    amount: i64,
    currency: &pb::Currency,
    emojis: &[pb::Emoji],
    p: &Palette,
    id: &str,
) -> AnyElement {
    let medal: Option<Hsla> = match n {
        0 => Some(gold(p)),
        1 => Some(gpui_kit::rgb(0x94a3b8).into()),
        2 => Some(gpui_kit::rgb(0xb45309).into()),
        _ => None,
    };
    motion::rise(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_sm()
            .child(
                div()
                    .size(px(20.0))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .bg(medal.map(|m| m.opacity(0.18)).unwrap_or(alpha(p.muted, 0.8)))
                    .text_color(medal.unwrap_or(p.muted_foreground.into()))
                    .child((n + 1).to_string()),
            )
            .child(div().flex_1().min_w_0().truncate().child(name))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .font_weight(FontWeight::BOLD)
                    .child(currency_icon(currency, emojis, 13.0))
                    .child(amount_text(amount)),
            ),
        SharedString::from(id.to_owned()),
        Duration::from_millis((n.min(9) * 25) as u64),
        6.0,
    )
    .into_any_element()
}
