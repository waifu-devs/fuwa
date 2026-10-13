//! A server's economy (docs/economy.md): its currencies, how members earn
//! them, its shop, and your wallet there. Loaded with a server's state on
//! instances with the `economy` feature, and kept by its events: the lists
//! come whole (`CurrenciesUpdated`, `ShopUpdated`), a listing or a balance
//! one at a time, and `WalletsChanged` or a purchase reads the wallet again.
//! The calls the shop, the settings pages and Give currency make are here
//! too. Like the web's `fuwa/economy.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::permissions::Access;
use crate::core::store::InstanceState;
use crate::pb::{self, CurrencyAction as Action, LedgerKind, Permission as P, ShopListingKind as Kind};
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// The icon a currency without one shows.
pub const FALLBACK_ICON: &str = "🪙";
pub const NAME_MAX: usize = 32;
pub const DESCRIPTION_MAX: usize = 200;
/// A custom listing's name.
pub const LISTING_NAME_MAX: usize = 60;
/// A transfer's note or a grant's reason.
pub const NOTE_MAX: usize = 200;
/// The most one rule gives at once, and the most a listing costs.
pub const MOST_EARNED: i64 = 1_000_000_000;
pub const MOST_PRICE: i64 = 1_000_000_000_000;
/// The longest cooldown a rule may have: a day.
pub const MOST_COOLDOWN: i32 = 86_400;
/// Ledger entries asked for at a time.
pub const PAGE: i32 = 50;
/// The periods the finance page offers, in days.
pub const PERIODS: [i32; 4] = [7, 30, 90, 365];
const DAY_MS: i64 = 86_400_000;

/// Every action a currency can reward, in the order the editor lists them.
pub const ACTIONS: [Action; 6] =
    [Action::Message, Action::ReactionReceived, Action::VoiceMinute, Action::Daily, Action::Join, Action::Onboarding];

/// Ledger kinds, as the finance page filters them.
pub const KINDS: [LedgerKind; 4] = [LedgerKind::Earn, LedgerKind::Grant, LedgerKind::Transfer, LedgerKind::Purchase];

/// What one server's economy holds for you.
#[derive(Debug, Clone, Default)]
pub struct Economy {
    /// In order (position, then age).
    pub currencies: Vec<pb::Currency>,
    /// Every listing you were sent, in order: managers get unavailable ones too.
    pub listings: Vec<pb::ShopListing>,
    /// Your balances, by currency.
    pub balances: HashMap<String, pb::Balance>,
    /// The server's profile items you bought.
    pub owned_items: HashSet<String>,
}

impl Economy {
    pub fn currency(&self, id: &str) -> Option<&pb::Currency> {
        self.currencies.iter().find(|c| c.id == id)
    }

    pub fn listing(&self, id: &str) -> Option<&pb::ShopListing> {
        self.listings.iter().find(|l| l.id == id)
    }

    /// How much of a currency you have.
    pub fn amount(&self, currency_id: &str) -> i64 {
        self.balances.get(currency_id).map_or(0, |b| b.amount)
    }

    /// The listings members see: the available ones in a currency still here.
    pub fn for_sale(&self) -> Vec<&pb::ShopListing> {
        self.listings.iter().filter(|l| l.available && self.currency(&l.currency_id).is_some()).collect()
    }
}

/// An action's name in the translations.
pub fn action_key(action: Action) -> &'static str {
    match action {
        Action::Unspecified | Action::Message => "desktop.economy.action.message",
        Action::Daily => "desktop.economy.action.daily",
        Action::Join => "desktop.economy.action.join",
        Action::Onboarding => "desktop.economy.action.onboarding",
        Action::ReactionReceived => "desktop.economy.action.reaction",
        Action::VoiceMinute => "desktop.economy.action.voice",
    }
}

/// What an action's rule pays for, under its name in the editor.
pub fn action_hint_key(action: Action) -> &'static str {
    match action {
        Action::Unspecified | Action::Message => "desktop.economy.actionHint.message",
        Action::Daily => "desktop.economy.actionHint.daily",
        Action::Join => "desktop.economy.actionHint.join",
        Action::Onboarding => "desktop.economy.actionHint.onboarding",
        Action::ReactionReceived => "desktop.economy.actionHint.reaction",
        Action::VoiceMinute => "desktop.economy.actionHint.voice",
    }
}

pub fn action_glyph(action: Action) -> &'static str {
    match action {
        Action::Unspecified | Action::Message => "message-square",
        Action::Daily => "calendar-check",
        Action::Join => "door-open",
        Action::Onboarding => "party-popper",
        Action::ReactionReceived => "face-slightly-smiling",
        Action::VoiceMinute => "mic",
    }
}

/// Whether a rule for this action waits between earnings.
pub fn has_cooldown(action: Action) -> bool {
    matches!(action, Action::Message | Action::ReactionReceived)
}

pub fn kind_key(kind: LedgerKind) -> &'static str {
    match kind {
        LedgerKind::Unspecified | LedgerKind::Earn => "desktop.economy.kind.earn",
        LedgerKind::Grant => "desktop.economy.kind.grant",
        LedgerKind::Transfer => "desktop.economy.kind.transfer",
        LedgerKind::Purchase => "desktop.economy.kind.purchase",
    }
}

fn ms(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000).unwrap_or(0)
}

fn sort_currencies(list: &mut [pb::Currency]) {
    list.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
}

fn sort_listings(list: &mut [pb::ShopListing]) {
    list.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
}

/// The currencies as they are now; balances and listings in ones that went go with them.
pub fn currencies_updated(e: &mut Economy, list: &[pb::Currency]) {
    let mut list = list.to_vec();
    sort_currencies(&mut list);
    let ids: HashSet<&str> = list.iter().map(|c| c.id.as_str()).collect();
    e.balances.retain(|id, _| ids.contains(id.as_str()));
    e.listings.retain(|l| ids.contains(l.currency_id.as_str()));
    e.currencies = list;
}

pub fn shop_updated(e: &mut Economy, list: &[pb::ShopListing]) {
    let mut list = list.to_vec();
    sort_listings(&mut list);
    e.listings = list;
}

/// One listing changed (bought): it takes its place, or joins the shop.
pub fn listing_updated(e: &mut Economy, listing: &pb::ShopListing) {
    match e.listings.iter_mut().find(|l| l.id == listing.id) {
        Some(l) => *l = listing.clone(),
        None => e.listings.push(listing.clone()),
    }
    sort_listings(&mut e.listings);
}

/// One of your balances as it is now. Only yours are ever kept here.
pub fn balance_updated(e: &mut Economy, balance: &pb::Balance, me: &str) {
    if !balance.user_id.is_empty() && balance.user_id != me {
        return;
    }
    e.balances.insert(balance.currency_id.clone(), balance.clone());
}

/// Your wallet, read whole.
pub fn wallet_read(e: &mut Economy, res: &pb::GetWalletResponse) {
    e.balances = res.balances.iter().map(|b| (b.currency_id.clone(), b.clone())).collect();
    e.owned_items = res.owned_item_ids.iter().cloned().collect();
}

/// What a currency's icon is: one of the server's emoji (a picture), or characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    Picture(String),
    Text(String),
}

/// A currency's icon: its custom emoji while the server still has it, else its characters, else 🪙.
pub fn icon_of(currency: &pb::Currency, emojis: &[pb::Emoji]) -> Icon {
    if !currency.icon_emoji_id.is_empty()
        && let Some(e) = emojis.iter().find(|e| e.id == currency.icon_emoji_id)
    {
        return Icon::Picture(e.url.clone());
    }
    if currency.icon_emoji.is_empty() {
        Icon::Text(FALLBACK_ICON.into())
    } else {
        Icon::Text(currency.icon_emoji.clone())
    }
}

/// A currency's rule for an action.
pub fn rule(currency: &pb::Currency, action: Action) -> Option<&pb::EarnRule> {
    currency.earn.iter().find(|r| r.action == action as i32)
}

/// The UTC day a moment is in.
fn day_of(ms: i64) -> i64 {
    ms.div_euclid(DAY_MS)
}

/// Whether today's (UTC) reward was claimed.
pub fn claimed_today(balance: Option<&pb::Balance>, now: i64) -> bool {
    balance.map(|b| ms(b.daily_claimed_at.as_ref())).is_some_and(|at| at > 0 && day_of(at) == day_of(now))
}

/// How long until the next UTC day, when the next claim opens.
pub fn until_next_day(now: i64) -> i64 {
    (day_of(now) + 1) * DAY_MS - now
}

/// The streak a claim now would make: one more when yesterday was claimed (or
/// today, which it stays), else it starts again.
pub fn streak_now(balance: Option<&pb::Balance>, now: i64) -> i32 {
    let Some(b) = balance else { return 1 };
    let at = ms(b.daily_claimed_at.as_ref());
    if at <= 0 {
        return 1;
    }
    match day_of(now) - day_of(at) {
        0 => b.daily_streak.max(1),
        1 => b.daily_streak.max(0) + 1,
        _ => 1,
    }
}

/// What claiming on day `streak` of a streak pays: the amount, plus the
/// bonus for each day after the first, counting up to `streak_max_days`.
pub fn daily_reward(rule: &pb::EarnRule, streak: i32) -> i64 {
    let days = if rule.streak_max_days > 0 { (streak - 1).clamp(0, rule.streak_max_days) } else { 0 };
    rule.amount.saturating_add(rule.streak_bonus.saturating_mul(i64::from(days)))
}

/// A whole number for a person to read: "1,234".
pub fn amount_text(n: i64) -> String {
    crate::core::i18n::number(n)
}

/// A number someone typed into an amount box: digits, with any grouping marks
/// or spaces; None when it isn't one.
pub fn parse_amount(text: &str) -> Option<i64> {
    let t = text.trim();
    let (negative, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let clean: String = digits.chars().filter(|c| !matches!(c, ',' | '.' | ' ' | '_' | '\u{a0}')).collect();
    if clean.is_empty() || !clean.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n = clean.parse::<i64>().ok()?;
    Some(if negative { -n } else { n })
}

/// The profile items sold in a server you haven't bought, by id: what the
/// cheapest listing for each costs, and in which currency. While an item is
/// listed (even unavailable), only those who bought it may wear it here.
pub fn item_locks(e: &Economy) -> HashMap<String, (i64, String)> {
    let mut out: HashMap<String, (i64, String)> = HashMap::new();
    for l in &e.listings {
        if l.kind != Kind::ProfileItem as i32 || l.target_id.is_empty() || e.owned_items.contains(&l.target_id) {
            continue;
        }
        let cheaper = out.get(&l.target_id).is_none_or(|(price, _)| l.price < *price);
        if cheaper {
            out.insert(l.target_id.clone(), (l.price, l.currency_id.clone()));
        }
    }
    out
}

/// Whether onboarding's rule lets a role be sold, and you may hand it out:
/// not @everyone, nothing strong in it, below you, with Manage Roles.
pub fn role_sellable(role: &pb::Role, server_id: &str, access: &Access) -> bool {
    role.id != server_id
        && crate::core::onboarding::harmless(&role.permissions)
        && access.has(P::ManageRoles)
        && access.above(role.position)
}

/// A listing's name: its own, else the role's or item's.
pub fn listing_name(l: &pb::ShopListing, roles: &[pb::Role], items: &[pb::ProfileItem]) -> String {
    if !l.name.is_empty() {
        return l.name.clone();
    }
    match Kind::try_from(l.kind).unwrap_or(Kind::Custom) {
        Kind::Role => roles.iter().find(|r| r.id == l.target_id).map(|r| r.name.clone()),
        Kind::ProfileItem => items.iter().find(|i| i.id == l.target_id).map(|i| i.name.clone()),
        _ => None,
    }
    .unwrap_or_default()
}

/// Whether you already have what a listing sells (a role or an item; custom ones are never "had").
pub fn owned(e: &Economy, l: &pb::ShopListing, role_ids: &[String]) -> bool {
    match Kind::try_from(l.kind).unwrap_or(Kind::Custom) {
        Kind::Role => role_ids.contains(&l.target_id),
        Kind::ProfileItem => e.owned_items.contains(&l.target_id),
        _ => false,
    }
}

/// Why you can't buy a listing now, in words' keys; None when you can.
pub fn cannot_buy(e: &Economy, l: &pb::ShopListing, role_ids: &[String]) -> Option<&'static str> {
    if owned(e, l, role_ids) {
        Some("desktop.economy.shop.owned")
    } else if l.stock == Some(0) {
        Some("desktop.economy.shop.soldOut")
    } else if e.amount(&l.currency_id) < l.price {
        Some("desktop.economy.shop.short")
    } else {
        None
    }
}

/// A currency being made or changed, as the editor holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurrencyDraft {
    pub name: String,
    pub icon_emoji: String,
    pub icon_emoji_id: String,
    pub description: String,
    pub transferable: bool,
    /// One per action in [`ACTIONS`] order; `enabled` says whether it's kept.
    pub rules: Vec<RuleDraft>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleDraft {
    pub action: Action,
    pub enabled: bool,
    pub amount: i64,
    pub cooldown_seconds: i32,
    pub daily_cap: i64,
    pub streak_bonus: i64,
    pub streak_max_days: i32,
}

impl RuleDraft {
    fn new(action: Action) -> Self {
        let (amount, cooldown) = match action {
            Action::Message => (1, 60),
            Action::ReactionReceived => (1, 30),
            Action::VoiceMinute => (1, 0),
            Action::Daily => (50, 0),
            _ => (100, 0),
        };
        Self {
            action,
            enabled: false,
            amount,
            cooldown_seconds: cooldown,
            daily_cap: 0,
            streak_bonus: 0,
            streak_max_days: 0,
        }
    }

    fn rule(&self) -> pb::EarnRule {
        let daily = self.action == Action::Daily;
        pb::EarnRule {
            action: self.action as i32,
            amount: self.amount,
            cooldown_seconds: if has_cooldown(self.action) { self.cooldown_seconds } else { 0 },
            daily_cap: self.daily_cap,
            streak_bonus: if daily { self.streak_bonus } else { 0 },
            streak_max_days: if daily { self.streak_max_days } else { 0 },
        }
    }
}

impl CurrencyDraft {
    /// A new currency's: coins, earned by nothing yet.
    pub fn new() -> Self {
        Self { rules: ACTIONS.iter().map(|a| RuleDraft::new(*a)).collect(), ..Default::default() }
    }

    pub fn of(c: &pb::Currency) -> Self {
        let rules = ACTIONS
            .iter()
            .map(|a| match rule(c, *a) {
                Some(r) => RuleDraft {
                    action: *a,
                    enabled: true,
                    amount: r.amount,
                    cooldown_seconds: r.cooldown_seconds,
                    daily_cap: r.daily_cap,
                    streak_bonus: r.streak_bonus,
                    streak_max_days: r.streak_max_days,
                },
                None => RuleDraft::new(*a),
            })
            .collect();
        Self {
            name: c.name.clone(),
            icon_emoji: c.icon_emoji.clone(),
            icon_emoji_id: c.icon_emoji_id.clone(),
            description: c.description.clone(),
            transferable: c.transferable,
            rules,
        }
    }

    /// The rules it keeps, as the instance takes them.
    pub fn earn(&self) -> Vec<pb::EarnRule> {
        self.rules.iter().filter(|r| r.enabled).map(RuleDraft::rule).collect()
    }

    /// What's wrong with it, as a translation key; None when it can be saved.
    pub fn problem(&self) -> Option<&'static str> {
        let name = self.name.trim().chars().count();
        if name == 0 || name > NAME_MAX {
            return Some("desktop.economy.currency.nameNeeded");
        }
        if self.description.chars().count() > DESCRIPTION_MAX {
            return Some("desktop.economy.currency.descriptionLong");
        }
        for r in self.rules.iter().filter(|r| r.enabled) {
            if !(1..=MOST_EARNED).contains(&r.amount) {
                return Some("desktop.economy.currency.amountRange");
            }
            if !(0..=MOST_COOLDOWN).contains(&r.cooldown_seconds) {
                return Some("desktop.economy.currency.cooldownRange");
            }
            if r.daily_cap < 0 || r.streak_bonus < 0 || r.streak_max_days < 0 {
                return Some("desktop.economy.currency.negative");
            }
        }
        None
    }

    /// What a new currency sends.
    pub fn create(&self, position: i32) -> pb::CurrencyChange {
        pb::CurrencyChange {
            name: Some(self.name.trim().to_owned()),
            icon_emoji: Some(self.icon_emoji.clone()),
            icon_emoji_id: Some(self.icon_emoji_id.clone()),
            description: Some(self.description.trim().to_owned()),
            transferable: Some(self.transferable),
            position: Some(position),
            replace_earn: true,
            earn: self.earn(),
        }
    }

    /// What changed from `before`, only that.
    pub fn change(&self, before: &CurrencyDraft) -> pb::CurrencyChange {
        let differs = |a: &str, b: &str| (a != b).then(|| a.to_owned());
        let replace = self.earn() != before.earn();
        pb::CurrencyChange {
            name: differs(self.name.trim(), &before.name),
            icon_emoji: (self.icon_emoji != before.icon_emoji || self.icon_emoji_id != before.icon_emoji_id)
                .then(|| self.icon_emoji.clone()),
            icon_emoji_id: (self.icon_emoji != before.icon_emoji || self.icon_emoji_id != before.icon_emoji_id)
                .then(|| self.icon_emoji_id.clone()),
            description: differs(self.description.trim(), &before.description),
            transferable: (self.transferable != before.transferable).then_some(self.transferable),
            position: None,
            replace_earn: replace,
            earn: if replace { self.earn() } else { Vec::new() },
        }
    }

    /// How many things changed from `before`, for the save bar.
    pub fn changes(&self, before: &CurrencyDraft) -> usize {
        usize::from(self.name.trim() != before.name)
            + usize::from(self.icon_emoji != before.icon_emoji || self.icon_emoji_id != before.icon_emoji_id)
            + usize::from(self.description.trim() != before.description)
            + usize::from(self.transferable != before.transferable)
            + self
                .rules
                .iter()
                .zip(&before.rules)
                .filter(|(a, b)| a.enabled != b.enabled || (a.enabled && a != b))
                .count()
    }
}

/// A listing being made or changed.
#[derive(Debug, Clone, PartialEq)]
pub struct ListingDraft {
    pub currency_id: String,
    pub kind: Kind,
    pub target_id: String,
    pub name: String,
    pub description: String,
    pub price: i64,
    /// None for no limit.
    pub stock: Option<i64>,
    pub position: i32,
    pub available: bool,
}

impl ListingDraft {
    pub fn new(currency_id: &str, kind: Kind, position: i32) -> Self {
        Self {
            currency_id: currency_id.to_owned(),
            kind,
            target_id: String::new(),
            name: String::new(),
            description: String::new(),
            price: 100,
            stock: None,
            position,
            available: true,
        }
    }

    pub fn of(l: &pb::ShopListing) -> Self {
        Self {
            currency_id: l.currency_id.clone(),
            kind: Kind::try_from(l.kind).unwrap_or(Kind::Custom),
            target_id: l.target_id.clone(),
            name: l.name.clone(),
            description: l.description.clone(),
            price: l.price,
            stock: l.stock,
            position: l.position,
            available: l.available,
        }
    }

    pub fn problem(&self) -> Option<&'static str> {
        if self.currency_id.is_empty() {
            return Some("desktop.economy.listing.currencyNeeded");
        }
        match self.kind {
            Kind::Custom => {
                let n = self.name.trim().chars().count();
                if n == 0 || n > LISTING_NAME_MAX {
                    return Some("desktop.economy.listing.nameNeeded");
                }
            }
            _ if self.target_id.is_empty() => return Some("desktop.economy.listing.targetNeeded"),
            _ => {}
        }
        if self.description.chars().count() > DESCRIPTION_MAX {
            return Some("desktop.economy.currency.descriptionLong");
        }
        if !(1..=MOST_PRICE).contains(&self.price) {
            return Some("desktop.economy.listing.priceRange");
        }
        if self.stock.is_some_and(|s| s < 0) {
            return Some("desktop.economy.currency.negative");
        }
        None
    }

    pub fn create(&self) -> pb::NewShopListing {
        pb::NewShopListing {
            currency_id: self.currency_id.clone(),
            kind: self.kind as i32,
            target_id: if self.kind == Kind::Custom { String::new() } else { self.target_id.clone() },
            name: self.name.trim().to_owned(),
            description: self.description.trim().to_owned(),
            price: self.price,
            stock: self.stock,
            position: self.position,
            available: self.available,
        }
    }

    pub fn change(&self, before: &ListingDraft) -> pb::ShopListingChange {
        pb::ShopListingChange {
            name: (self.name.trim() != before.name).then(|| self.name.trim().to_owned()),
            description: (self.description.trim() != before.description).then(|| self.description.trim().to_owned()),
            price: (self.price != before.price).then_some(self.price),
            stock: self.stock.filter(|_| self.stock != before.stock),
            unlimited: self.stock.is_none() && before.stock.is_some(),
            position: (self.position != before.position).then_some(self.position),
            available: (self.available != before.available).then_some(self.available),
        }
    }

    pub fn changes(&self, before: &ListingDraft) -> usize {
        usize::from(self.name.trim() != before.name)
            + usize::from(self.description.trim() != before.description)
            + usize::from(self.price != before.price)
            + usize::from(self.stock != before.stock)
            + usize::from(self.position != before.position)
            + usize::from(self.available != before.available)
    }
}

/// Who a grant goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantTo {
    Member(String),
    /// A role; the server's id for @everyone.
    Role(String),
}

impl InstanceState {
    /// A server's economy, once loaded.
    pub fn economy(&self, server_id: &str) -> Option<&Economy> {
        self.economy.get(server_id)
    }

    /// Whether a server has currencies to show (the wallet, the shop).
    pub fn has_currencies(&self, server_id: &str) -> bool {
        self.has("economy") && self.economy(server_id).is_some_and(|e| !e.currencies.is_empty())
    }

    /// Whether you run a server's economy (its settings pages).
    pub fn can_manage_economy(&self, server_id: &str) -> bool {
        self.has("economy") && self.access(server_id).has(P::ManageEconomy)
    }

    /// Whether you may give and take a server's currency.
    pub fn can_grant_currency(&self, server_id: &str) -> bool {
        self.has("economy") && self.access(server_id).has(P::GrantCurrency) && self.has_currencies(server_id)
    }
}

fn economy_of<'a>(i: &'a mut InstanceState, server_id: &str) -> &'a mut Economy {
    i.economy.entry(server_id.to_owned()).or_default()
}

fn me_of(i: &InstanceState) -> String {
    i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default()
}

impl Core {
    /// A server's currencies, shop and your wallet, read whole. An instance
    /// without the economy has none; a call that fails leaves what's known.
    pub async fn load_economy(&self, key: &str, server_id: &str) {
        if !self.shared.read(|s| s.instance(key).is_some_and(|i| i.has("economy"))) {
            return;
        }
        let Some(api) = self.api(key) else { return };
        let id = server_id.to_owned();
        let read = tokio::try_join!(
            rpc!(api.economy(), list_currencies(pb::ListCurrenciesRequest { server_id: id.clone() })),
            rpc!(api.economy(), list_shop(pb::ListShopRequest { server_id: id.clone() })),
            rpc!(api.economy(), get_wallet(pb::GetWalletRequest { server_id: id.clone(), user_id: String::new() })),
        );
        match read {
            Ok((currencies, shop, wallet)) => {
                self.shared.instance(key, |i| {
                    if !i.servers.iter().any(|s| s.id == server_id) {
                        return;
                    }
                    let e = economy_of(i, server_id);
                    currencies_updated(e, &currencies.currencies);
                    shop_updated(e, &shop.listings);
                    wallet_read(e, &wallet);
                });
            }
            Err(e) if e.code == Code::Unimplemented => {}
            Err(_) => {}
        }
    }

    /// Reads your wallet in a server again (a grant to a role, a purchase).
    pub async fn reread_wallet(&self, key: &str, server_id: &str) {
        let Some(api) = self.api(key) else { return };
        let req = pb::GetWalletRequest { server_id: server_id.into(), user_id: String::new() };
        let Ok(res) = rpc!(api.economy(), get_wallet(req)).await else { return };
        self.shared.instance(key, |i| {
            if let Some(e) = i.economy.get_mut(server_id) {
                wallet_read(e, &res);
            }
        });
    }

    /// Someone's wallet (Manage Economy or Grant Currency).
    pub async fn wallet_of(&self, key: &str, server_id: &str, user_id: &str) -> Result<pb::GetWalletResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.economy(), get_wallet(pb::GetWalletRequest { server_id: server_id.into(), user_id: user_id.into() }))
            .await
    }

    fn keep_balance(&self, key: &str, server_id: &str, balance: Option<&pb::Balance>) {
        let Some(balance) = balance else { return };
        self.shared.instance(key, |i| {
            let me = me_of(i);
            balance_updated(economy_of(i, server_id), balance, &me);
        });
    }

    /// Takes today's reward in a currency.
    pub async fn claim_daily(
        &self,
        key: &str,
        server_id: &str,
        currency_id: &str,
    ) -> Result<pb::ClaimDailyResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            claim_daily(pb::ClaimDailyRequest { server_id: server_id.into(), currency_id: currency_id.into() })
        )
        .await?;
        self.keep_balance(key, server_id, res.balance.as_ref());
        Ok(res)
    }

    /// Sends some of a currency to another member.
    pub async fn transfer(
        &self,
        key: &str,
        server_id: &str,
        currency_id: &str,
        user_id: &str,
        amount: i64,
        note: &str,
    ) -> Result<pb::TransferResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            transfer(pb::TransferRequest {
                server_id: server_id.into(),
                currency_id: currency_id.into(),
                user_id: user_id.into(),
                amount,
                note: note.trim().chars().take(NOTE_MAX).collect(),
            })
        )
        .await?;
        self.keep_balance(key, server_id, res.balance.as_ref());
        Ok(res)
    }

    /// Buys a listing at the price shown.
    pub async fn buy(&self, key: &str, server_id: &str, listing: &pb::ShopListing) -> Result<pb::BuyResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            buy(pb::BuyRequest {
                server_id: server_id.into(),
                listing_id: listing.id.clone(),
                expected_price: listing.price,
            })
        )
        .await?;
        self.shared.instance(key, |i| {
            let me = me_of(i);
            let e = economy_of(i, server_id);
            if let Some(b) = &res.balance {
                balance_updated(e, b, &me);
            }
            if let Some(l) = &res.listing {
                listing_updated(e, l);
                if l.kind == Kind::ProfileItem as i32 {
                    e.owned_items.insert(l.target_id.clone());
                }
            }
        });
        Ok(res)
    }

    pub async fn create_currency(
        &self,
        key: &str,
        server_id: &str,
        change: pb::CurrencyChange,
    ) -> Result<pb::Currency, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            create_currency(pb::CreateCurrencyRequest { server_id: server_id.into(), currency: Some(change) })
        )
        .await?;
        let currency = res.currency.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no currency."))?;
        self.put_currency(key, server_id, &currency);
        Ok(currency)
    }

    pub async fn update_currency(
        &self,
        key: &str,
        server_id: &str,
        currency_id: &str,
        change: pb::CurrencyChange,
    ) -> Result<pb::Currency, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            update_currency(pb::UpdateCurrencyRequest {
                server_id: server_id.into(),
                currency_id: currency_id.into(),
                change: Some(change),
            })
        )
        .await?;
        let currency = res.currency.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no currency."))?;
        self.put_currency(key, server_id, &currency);
        Ok(currency)
    }

    /// Keeps a currency the instance answered with, before its event arrives.
    fn put_currency(&self, key: &str, server_id: &str, currency: &pb::Currency) {
        self.shared.instance(key, |i| {
            let e = economy_of(i, server_id);
            let mut list = e.currencies.clone();
            list.retain(|c| c.id != currency.id);
            list.push(currency.clone());
            currencies_updated(e, &list);
        });
    }

    pub async fn delete_currency(&self, key: &str, server_id: &str, currency_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.economy(),
            delete_currency(pb::DeleteCurrencyRequest { server_id: server_id.into(), currency_id: currency_id.into() })
        )
        .await?;
        self.shared.instance(key, |i| {
            let e = economy_of(i, server_id);
            let list: Vec<pb::Currency> = e.currencies.iter().filter(|c| c.id != currency_id).cloned().collect();
            currencies_updated(e, &list);
        });
        Ok(())
    }

    pub async fn create_listing(
        &self,
        key: &str,
        server_id: &str,
        listing: pb::NewShopListing,
    ) -> Result<pb::ShopListing, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            create_shop_listing(pb::CreateShopListingRequest { server_id: server_id.into(), listing: Some(listing) })
        )
        .await?;
        let listing = res.listing.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no listing."))?;
        self.shared.instance(key, |i| listing_updated(economy_of(i, server_id), &listing));
        Ok(listing)
    }

    pub async fn update_listing(
        &self,
        key: &str,
        server_id: &str,
        listing_id: &str,
        change: pb::ShopListingChange,
    ) -> Result<pb::ShopListing, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            update_shop_listing(pb::UpdateShopListingRequest {
                server_id: server_id.into(),
                listing_id: listing_id.into(),
                change: Some(change),
            })
        )
        .await?;
        let listing = res.listing.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no listing."))?;
        self.shared.instance(key, |i| listing_updated(economy_of(i, server_id), &listing));
        Ok(listing)
    }

    pub async fn delete_listing(&self, key: &str, server_id: &str, listing_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.economy(),
            delete_shop_listing(pb::DeleteShopListingRequest {
                server_id: server_id.into(),
                listing_id: listing_id.into(),
            })
        )
        .await?;
        self.shared.instance(key, |i| economy_of(i, server_id).listings.retain(|l| l.id != listing_id));
        Ok(())
    }

    /// A page of the ledger, newest first: yours (`user_id` empty), someone's,
    /// or everyone's ("*"), in one currency or all, of some kinds or all.
    #[allow(clippy::too_many_arguments)]
    pub async fn ledger(
        &self,
        key: &str,
        server_id: &str,
        currency_id: &str,
        user_id: &str,
        kinds: &[LedgerKind],
        before: &str,
        limit: i32,
    ) -> Result<pb::ListLedgerResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.economy(),
            list_ledger(pb::ListLedgerRequest {
                server_id: server_id.into(),
                currency_id: currency_id.into(),
                user_id: user_id.into(),
                kinds: kinds.iter().map(|k| *k as i32).collect(),
                before_id: before.into(),
                limit,
            })
        )
        .await
    }

    /// The finance page for one currency over the last `days`.
    pub async fn economy_overview(
        &self,
        key: &str,
        server_id: &str,
        currency_id: &str,
        days: i32,
    ) -> Result<pb::EconomyOverview, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            get_economy_overview(pb::GetEconomyOverviewRequest {
                server_id: server_id.into(),
                currency_id: currency_id.into(),
                days,
            })
        )
        .await?;
        Ok(res.overview.unwrap_or_default())
    }

    /// Gives (or takes, below zero) an amount to a member or everyone with a role.
    pub async fn grant(
        &self,
        key: &str,
        server_id: &str,
        currency_id: &str,
        to: &GrantTo,
        amount: i64,
        reason: &str,
    ) -> Result<pb::GrantResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let (user_id, role_id) = match to {
            GrantTo::Member(id) => (id.clone(), String::new()),
            GrantTo::Role(id) => (String::new(), id.clone()),
        };
        rpc!(
            api.economy(),
            grant(pb::GrantRequest {
                server_id: server_id.into(),
                currency_id: currency_id.into(),
                user_id,
                role_id,
                amount,
                reason: reason.trim().chars().take(NOTE_MAX).collect(),
            })
        )
        .await
    }
}

/// Starts reading your wallet in a server again.
pub fn spawn_wallet(core: &Arc<Core>, key: &str, server_id: &str) {
    let (c, key, sid) = (core.clone(), key.to_owned(), server_id.to_owned());
    drop(core.spawn(async move { c.reread_wallet(&key, &sid).await }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(ms: i64) -> prost_types::Timestamp {
        prost_types::Timestamp { seconds: ms / 1000, nanos: 0 }
    }

    fn currency(id: &str, position: i32) -> pb::Currency {
        pb::Currency { id: id.into(), name: id.into(), position, ..Default::default() }
    }

    fn listing(id: &str, currency: &str, kind: Kind, target: &str, price: i64) -> pb::ShopListing {
        pb::ShopListing {
            id: id.into(),
            currency_id: currency.into(),
            kind: kind as i32,
            target_id: target.into(),
            price,
            available: true,
            ..Default::default()
        }
    }

    #[test]
    fn events_apply_twice_alike() {
        let mut e = Economy::default();
        let list = [currency("b", 1), currency("a", 0)];
        currencies_updated(&mut e, &list);
        currencies_updated(&mut e, &list);
        assert_eq!(e.currencies.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        let l = listing("l1", "a", Kind::Custom, "", 5);
        listing_updated(&mut e, &l);
        listing_updated(&mut e, &l);
        assert_eq!(e.listings.len(), 1);
        let b = pb::Balance { currency_id: "a".into(), user_id: "me".into(), amount: 7, ..Default::default() };
        balance_updated(&mut e, &b, "me");
        balance_updated(&mut e, &b, "me");
        assert_eq!(e.amount("a"), 7);
        // Someone else's balance is never kept as yours.
        balance_updated(&mut e, &pb::Balance { amount: 99, user_id: "you".into(), ..b.clone() }, "me");
        assert_eq!(e.amount("a"), 7);
        // A currency that goes takes its balances and listings.
        currencies_updated(&mut e, &[currency("b", 1)]);
        assert_eq!(e.amount("a"), 0);
        assert!(e.listings.is_empty());
    }

    #[test]
    fn daily_claims_follow_utc_days() {
        let day = DAY_MS;
        let now = 100 * day + 5_000;
        let b = |at: i64, streak: i32| pb::Balance {
            daily_claimed_at: Some(ts(at)),
            daily_streak: streak,
            ..Default::default()
        };
        assert!(!claimed_today(None, now));
        assert!(claimed_today(Some(&b(100 * day + 1_000, 1)), now));
        assert!(!claimed_today(Some(&b(99 * day + 80_000_000, 1)), now));
        assert_eq!(until_next_day(now), day - 5_000);
        assert_eq!(streak_now(Some(&b(99 * day, 3)), now), 4);
        assert_eq!(streak_now(Some(&b(97 * day, 3)), now), 1);
        assert_eq!(streak_now(None, now), 1);
        let rule = pb::EarnRule { amount: 10, streak_bonus: 5, streak_max_days: 3, ..Default::default() };
        assert_eq!(daily_reward(&rule, 1), 10);
        assert_eq!(daily_reward(&rule, 3), 20);
        assert_eq!(daily_reward(&rule, 9), 25);
        let flat = pb::EarnRule { amount: 10, streak_bonus: 5, ..Default::default() };
        assert_eq!(daily_reward(&flat, 9), 10);
    }

    #[test]
    fn icons_fall_back() {
        let emoji = pb::Emoji { id: "E1".into(), url: "https://x/e1".into(), ..Default::default() };
        let mut c = currency("a", 0);
        assert_eq!(icon_of(&c, &[]), Icon::Text(FALLBACK_ICON.into()));
        c.icon_emoji = "💎".into();
        assert_eq!(icon_of(&c, &[]), Icon::Text("💎".into()));
        c.icon_emoji_id = "E1".into();
        assert_eq!(icon_of(&c, std::slice::from_ref(&emoji)), Icon::Picture("https://x/e1".into()));
        // Deleted: back to its characters.
        assert_eq!(icon_of(&c, &[]), Icon::Text("💎".into()));
    }

    #[test]
    fn locked_items_are_the_unbought_listed_ones() {
        let mut e = Economy {
            listings: vec![
                listing("1", "a", Kind::ProfileItem, "item", 50),
                listing("2", "a", Kind::ProfileItem, "item", 30),
                listing("3", "a", Kind::ProfileItem, "mine", 10),
                listing("4", "a", Kind::Role, "role", 10),
            ],
            ..Default::default()
        };
        e.owned_items.insert("mine".into());
        let locks = item_locks(&e);
        assert_eq!(locks.len(), 1);
        assert_eq!(locks["item"].0, 30);
        let held = vec!["role".to_owned()];
        assert!(owned(&e, &e.listings[3], &held));
        assert_eq!(cannot_buy(&e, &e.listings[0], &held), Some("desktop.economy.shop.short"));
    }

    #[test]
    fn drafts_send_only_what_changed() {
        let mut c = currency("a", 0);
        c.earn = vec![pb::EarnRule {
            action: Action::Message as i32,
            amount: 2,
            cooldown_seconds: 60,
            ..Default::default()
        }];
        let before = CurrencyDraft::of(&c);
        let mut after = before.clone();
        assert_eq!(after.changes(&before), 0);
        after.transferable = true;
        let change = after.change(&before);
        assert_eq!(change.transferable, Some(true));
        assert!(change.name.is_none() && !change.replace_earn);
        after.rules[0].amount = 3;
        let change = after.change(&before);
        assert!(change.replace_earn);
        assert_eq!(change.earn[0].amount, 3);
        assert_eq!(after.changes(&before), 2);
        after.name = String::new();
        assert!(after.problem().is_some());

        let mut l = ListingDraft::new("a", Kind::Custom, 0);
        assert_eq!(l.problem(), Some("desktop.economy.listing.nameNeeded"));
        l.name = "A hug".into();
        assert_eq!(l.problem(), None);
        let before = l.clone();
        l.stock = Some(3);
        assert_eq!(l.change(&before).stock, Some(3));
        let limited = l.clone();
        l.stock = None;
        let change = l.change(&limited);
        assert!(change.unlimited && change.stock.is_none());
    }

    #[test]
    fn amounts_read_as_people_type_them() {
        assert_eq!(parse_amount("1,234"), Some(1234));
        assert_eq!(parse_amount(" -50 "), Some(-50));
        assert_eq!(parse_amount("12a"), None);
        assert_eq!(parse_amount(""), None);
    }
}
