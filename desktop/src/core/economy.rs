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
pub const ACTIONS: [Action; 7] = [
    Action::Message,
    Action::ReactionReceived,
    Action::VoiceMinute,
    Action::GameMinute,
    Action::Daily,
    Action::Join,
    Action::Onboarding,
];
/// The most games one GAME_MINUTE rule may name, and how long a name may be.
pub const MOST_GAMES: usize = 50;
pub const GAME_NAME_MAX: usize = 128;

/// Ledger kinds, as the finance page filters them.
pub const KINDS: [LedgerKind; 5] =
    [LedgerKind::Earn, LedgerKind::Grant, LedgerKind::Transfer, LedgerKind::Purchase, LedgerKind::Gacha];

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
    /// Its gacha banners, in order: managers get unavailable ones too.
    pub banners: Vec<pb::GachaBanner>,
    /// Your pulls on each banner, by its id.
    pub progress: HashMap<String, pb::GachaProgress>,
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

    /// The listings members see: the available ones in a currency still here,
    /// gacha-only ones while a banner members see can drop them.
    pub fn for_sale(&self, now: i64) -> Vec<&pb::ShopListing> {
        let banners = self.shown_banners(now);
        self.listings
            .iter()
            .filter(|l| l.available && self.currency(&l.currency_id).is_some())
            .filter(|l| !l.gacha_only || banners.iter().any(|b| b.prizes.iter().any(|p| p.listing_id == l.id)))
            .collect()
    }

    pub fn banner(&self, id: &str) -> Option<&pb::GachaBanner> {
        self.banners.iter().find(|b| b.id == id)
    }

    /// The banners members see: available ones in a currency still here, not closed.
    pub fn shown_banners(&self, now: i64) -> Vec<&pb::GachaBanner> {
        self.banners
            .iter()
            .filter(|b| b.available && self.currency(&b.currency_id).is_some() && openness(b, now) != Openness::Closed)
            .collect()
    }

    /// The banners a listing drops from.
    pub fn banners_with(&self, listing_id: &str, now: i64) -> Vec<&pb::GachaBanner> {
        self.shown_banners(now).into_iter().filter(|b| b.prizes.iter().any(|p| p.listing_id == listing_id)).collect()
    }
}

/// Whether a banner can be pulled on now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Openness {
    /// Open; until when (0 for no end).
    Open(i64),
    /// Opens at this moment.
    Soon(i64),
    Closed,
}

pub fn openness(b: &pb::GachaBanner, now: i64) -> Openness {
    let (starts, ends) = (ms(b.starts_at.as_ref()), ms(b.ends_at.as_ref()));
    if starts > now {
        Openness::Soon(starts)
    } else if ends != 0 && ends <= now {
        Openness::Closed
    } else {
        Openness::Open(ends)
    }
}

/// What `count` pulls cost (1, or the multi-pull), or None for a count the banner doesn't offer.
pub fn pull_cost(b: &pb::GachaBanner, count: i32) -> Option<i64> {
    if count == 1 {
        Some(b.pull_price)
    } else if b.multi_pull >= 2 && count == b.multi_pull {
        Some(if b.multi_pull_price > 0 { b.multi_pull_price } else { b.pull_price.saturating_mul(i64::from(count)) })
    } else {
        None
    }
}

/// How many pulls until pity's guarantee (1 is the next), or None without pity.
pub fn pity_in(b: &pb::GachaBanner, progress: Option<&pb::GachaProgress>) -> Option<i32> {
    (b.pity_pulls > 0).then(|| (b.pity_pulls - progress.map_or(0, |p| p.since_pity)).max(1))
}

/// How many more pulls you may make, or None for no limit.
pub fn pulls_left(b: &pb::GachaBanner, progress: Option<&pb::GachaProgress>) -> Option<i32> {
    (b.max_pulls > 0).then(|| (b.max_pulls - progress.map_or(0, |p| p.pulls)).max(0))
}

/// A rate or chance in millionths as a percentage, with up to four decimals: "3%", "0.6%".
pub fn percent(rate: i32) -> String {
    let text = format!("{:.4}", f64::from(rate) / 10_000.0);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}%")
}

/// The colors tiers without their own take, by rarity (0 rarest), as the web picks them.
pub const TIER_COLORS: [u32; 5] = [0xf59e0b, 0xa855f7, 0x3b82f6, 0x10b981, 0x64748b];

pub fn tier_color(tier: &pb::GachaTier, index: usize) -> u32 {
    tier.color.map_or(TIER_COLORS[index.min(TIER_COLORS.len() - 1)], |c| c as u32)
}

/// What a line of history is about: the listing bought, or the banner pulled on.
pub fn entry_subject(
    e: &Economy,
    entry: &pb::LedgerEntry,
    roles: &[pb::Role],
    items: &[pb::ProfileItem],
) -> Option<String> {
    if entry.kind == LedgerKind::Gacha as i32 {
        return e.banner(&entry.banner_id).map(|b| b.name.clone());
    }
    e.listing(&entry.listing_id).map(|l| listing_name(l, roles, items))
}

/// What a prize is called: its listing's name or its currency amount.
pub fn prize_name(e: &Economy, prize: &pb::GachaPrize, roles: &[pb::Role], items: &[pb::ProfileItem]) -> String {
    if !prize.listing_id.is_empty() {
        return e.listing(&prize.listing_id).map(|l| listing_name(l, roles, items)).unwrap_or_default();
    }
    let currency = e.currency(&prize.currency_id).map(|c| c.name.as_str()).unwrap_or_default();
    format!("{} {currency}", amount_text(prize.amount))
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
        Action::GameMinute => "desktop.economy.action.game",
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
        Action::GameMinute => "desktop.economy.actionHint.game",
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
        Action::GameMinute => "gamepad-2",
    }
}

/// Whether the editor offers an action on an instance: the game rule only
/// where the instance knows it (`economy-games`).
pub fn offered(action: Action, i: &InstanceState) -> bool {
    action != Action::GameMinute || i.has("economy-games")
}

/// The games a GAME_MINUTE rule names, from what was typed: one per line
/// (names may hold commas, "Warhammer 40,000"), trimmed, each once (ignoring case), in order. Too
/// many or too long is left for [`CurrencyDraft::problem`] to say.
pub fn parse_games(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split('\n')
        .map(str::trim)
        .filter(|g| !g.is_empty() && seen.insert(g.to_lowercase()))
        .map(str::to_owned)
        .collect()
}

/// The games as the editor's box shows them, one per line.
pub fn games_text(games: &[String]) -> String {
    games.join("\n")
}

/// How a rule pays, for members: the action's name, or for the game rule
/// which games and how much a minute.
pub fn earn_line(rule: &pb::EarnRule) -> String {
    use crate::core::i18n::{Arg, t, t_with};
    let action = Action::try_from(rule.action).unwrap_or(Action::Message);
    let amount = amount_text(rule.amount);
    if action != Action::GameMinute {
        return t_with(
            "desktop.economy.earn.line",
            &[("action", Arg::Str(&t(action_key(action)))), ("amount", Arg::Str(&amount))],
        );
    }
    if rule.games.is_empty() {
        t_with("desktop.economy.earn.anyGame", &[("amount", Arg::Str(&amount))])
    } else {
        t_with(
            "desktop.economy.earn.games",
            &[("games", Arg::Str(&rule.games.join(", "))), ("amount", Arg::Str(&amount))],
        )
    }
}

/// A game rule's games, short enough for a chip: the first few, then how
/// many more; "any game" when it names none.
pub fn games_short(games: &[String]) -> String {
    use crate::core::i18n::{Arg, t, t_with};
    const SHOWN: usize = 3;
    if games.is_empty() {
        return t("desktop.economy.earn.anyGameShort");
    }
    let mut out = games.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if games.len() > SHOWN {
        out.push(' ');
        out.push_str(&t_with("desktop.economy.earn.more", &[("count", Arg::Num((games.len() - SHOWN) as i64))]));
    }
    out
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
        LedgerKind::Gacha => "desktop.economy.kind.gacha",
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
    e.banners.retain(|b| ids.contains(b.currency_id.as_str()));
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

fn sort_banners(list: &mut [pb::GachaBanner]) {
    list.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
}

pub fn banners_updated(e: &mut Economy, list: &[pb::GachaBanner]) {
    let mut list = list.to_vec();
    sort_banners(&mut list);
    e.banners = list;
}

/// Your progress on a banner, unless what's kept is further along.
pub fn progress_updated(e: &mut Economy, progress: &pb::GachaProgress) {
    if e.progress.get(&progress.banner_id).is_some_and(|had| had.pulls > progress.pulls) {
        return;
    }
    e.progress.insert(progress.banner_id.clone(), progress.clone());
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
    } else if l.gacha_only {
        Some("desktop.economy.shop.gachaOnly")
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
    /// GAME_MINUTE: the games that pay, none for any.
    pub games: Vec<String>,
}

impl RuleDraft {
    fn new(action: Action) -> Self {
        let (amount, cooldown) = match action {
            Action::Message => (1, 60),
            Action::ReactionReceived => (1, 30),
            Action::VoiceMinute | Action::GameMinute => (1, 0),
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
            games: Vec::new(),
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
            games: if self.action == Action::GameMinute { self.games.clone() } else { Vec::new() },
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
                    games: r.games.clone(),
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
            if r.action == Action::GameMinute
                && (r.games.len() > MOST_GAMES || r.games.iter().any(|g| g.chars().count() > GAME_NAME_MAX))
            {
                return Some("desktop.economy.currency.gamesRange");
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
    /// Won only from gacha banners, never bought.
    pub gacha_only: bool,
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
            gacha_only: false,
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
            gacha_only: l.gacha_only,
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
            gacha_only: self.gacha_only,
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
            gacha_only: (self.gacha_only != before.gacha_only).then_some(self.gacha_only),
        }
    }

    pub fn changes(&self, before: &ListingDraft) -> usize {
        usize::from(self.name.trim() != before.name)
            + usize::from(self.description.trim() != before.description)
            + usize::from(self.price != before.price)
            + usize::from(self.stock != before.stock)
            + usize::from(self.position != before.position)
            + usize::from(self.available != before.available)
            + usize::from(self.gacha_only != before.gacha_only)
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
        self.load_banners(key, server_id).await;
    }

    /// A server's gacha banners and your pulls on them, on instances with the `gacha` feature.
    pub async fn load_banners(&self, key: &str, server_id: &str) {
        if !self.shared.read(|s| s.instance(key).is_some_and(|i| i.has("gacha"))) {
            return;
        }
        let Some(api) = self.api(key) else { return };
        let req = pb::ListGachaBannersRequest { server_id: server_id.into() };
        let Ok(res) = rpc!(api.economy(), list_gacha_banners(req)).await else { return };
        self.shared.instance(key, |i| {
            if !i.servers.iter().any(|s| s.id == server_id) {
                return;
            }
            let e = economy_of(i, server_id);
            banners_updated(e, &res.banners);
            e.progress = res.progress.iter().map(|p| (p.banner_id.clone(), p.clone())).collect();
        });
    }

    /// Pulls on a banner once or its multi-pull, at the price shown.
    pub async fn pull(
        &self,
        key: &str,
        server_id: &str,
        banner_id: &str,
        count: i32,
        price: i64,
    ) -> Result<pb::PullResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.economy(),
            pull(pb::PullRequest {
                server_id: server_id.into(),
                banner_id: banner_id.into(),
                count,
                expected_price: price,
            })
        )
        .await?;
        self.shared.instance(key, |i| {
            let me = me_of(i);
            let e = economy_of(i, server_id);
            for b in &res.balances {
                balance_updated(e, b, &me);
            }
            if let Some(p) = &res.progress {
                progress_updated(e, p);
            }
        });
        Ok(res)
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

    fn banner() -> pb::GachaBanner {
        pb::GachaBanner {
            id: "b".into(),
            currency_id: "a".into(),
            pull_price: 100,
            multi_pull: 10,
            multi_pull_price: 900,
            tiers: vec![
                pb::GachaTier { id: "t1".into(), name: "Gold".into(), color: None, rate: 30_000 },
                pb::GachaTier { id: "t2".into(), name: "Plain".into(), color: Some(0x123456), rate: 970_000 },
            ],
            prizes: vec![pb::GachaPrize {
                id: "p".into(),
                tier_id: "t1".into(),
                listing_id: "l1".into(),
                ..Default::default()
            }],
            pity_pulls: 50,
            pity_tier_id: "t1".into(),
            available: true,
            ..Default::default()
        }
    }

    #[test]
    fn banners_price_pity_limits_and_rates() {
        let b = banner();
        assert_eq!(pull_cost(&b, 1), Some(100));
        assert_eq!(pull_cost(&b, 10), Some(900));
        assert_eq!(pull_cost(&pb::GachaBanner { multi_pull_price: 0, ..b.clone() }, 10), Some(1000));
        assert_eq!(pull_cost(&b, 5), None);
        let progress = pb::GachaProgress { banner_id: "b".into(), pulls: 70, since_pity: 20 };
        assert_eq!(pity_in(&b, None), Some(50));
        assert_eq!(pity_in(&b, Some(&progress)), Some(30));
        assert_eq!(pity_in(&pb::GachaBanner { pity_pulls: 0, ..b.clone() }, Some(&progress)), None);
        assert_eq!(pulls_left(&b, Some(&progress)), None);
        assert_eq!(pulls_left(&pb::GachaBanner { max_pulls: 60, ..b.clone() }, Some(&progress)), Some(0));
        assert_eq!(pulls_left(&pb::GachaBanner { max_pulls: 100, ..b.clone() }, Some(&progress)), Some(30));
        assert_eq!(percent(1_000_000), "100%");
        assert_eq!(percent(30_000), "3%");
        assert_eq!(percent(6_000), "0.6%");
        assert_eq!(percent(1), "0.0001%");
        assert_eq!(tier_color(&b.tiers[0], 0), TIER_COLORS[0]);
        assert_eq!(tier_color(&b.tiers[1], 1), 0x123456);
        assert_eq!(tier_color(&b.tiers[0], 99), TIER_COLORS[4]);
    }

    #[test]
    fn banners_show_while_open_and_gacha_only_listings_with_them() {
        let now = 1_000_000_000_000;
        let at = |ms: i64| Some(prost_types::Timestamp { seconds: ms / 1000, nanos: 0 });
        assert_eq!(openness(&banner(), now), Openness::Open(0));
        assert_eq!(
            openness(&pb::GachaBanner { starts_at: at(now + 5000), ..banner() }, now),
            Openness::Soon(now + 5000)
        );
        assert_eq!(openness(&pb::GachaBanner { ends_at: at(now), ..banner() }, now), Openness::Closed);
        let mut e = Economy::default();
        currencies_updated(&mut e, &[currency("a", 0)]);
        let mut only = listing("l1", "a", Kind::Custom, "", 5);
        only.gacha_only = true;
        shop_updated(&mut e, &[only.clone(), listing("l2", "a", Kind::Custom, "", 5)]);
        assert_eq!(e.for_sale(now).len(), 1);
        assert_eq!(cannot_buy(&e, &only, &[]), Some("desktop.economy.shop.gachaOnly"));
        banners_updated(&mut e, &[banner(), pb::GachaBanner { id: "c".into(), available: false, ..banner() }]);
        assert_eq!(e.shown_banners(now).len(), 1);
        assert_eq!(e.for_sale(now).len(), 2);
        assert_eq!(e.banners_with("l1", now).len(), 1);
        // A banner in a currency that goes, goes with it.
        currencies_updated(&mut e, &[]);
        assert!(e.banners.is_empty());
        // Progress never goes back.
        progress_updated(&mut e, &pb::GachaProgress { banner_id: "b".into(), pulls: 5, since_pity: 5 });
        progress_updated(&mut e, &pb::GachaProgress { banner_id: "b".into(), pulls: 3, since_pity: 3 });
        assert_eq!(e.progress["b"].pulls, 5);
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
    fn games_read_one_per_line() {
        assert_eq!(
            parse_games("Celeste\n hollow knight \n\nCELESTE\nWarhammer 40,000"),
            ["Celeste", "hollow knight", "Warhammer 40,000"]
        );
        assert!(parse_games(" \n  ").is_empty());
        assert_eq!(parse_games(&games_text(&["A".into(), "B".into()])), ["A", "B"]);

        let mut draft = CurrencyDraft::new();
        draft.name = "Coins".into();
        let game = draft.rules.iter_mut().find(|r| r.action == Action::GameMinute).unwrap();
        assert_eq!(game.amount, 1);
        game.enabled = true;
        game.games = parse_games("Celeste\nHades");
        assert_eq!(draft.problem(), None);
        let earn = draft.earn();
        assert_eq!(earn[0].action, Action::GameMinute as i32);
        assert_eq!(earn[0].games, ["Celeste", "Hades"]);
        // Read back, it's the same draft.
        let c = pb::Currency { name: "Coins".into(), earn: earn.clone(), ..Default::default() };
        assert_eq!(CurrencyDraft::of(&c).earn(), earn);

        let game = draft.rules.iter_mut().find(|r| r.action == Action::GameMinute).unwrap();
        game.games = (0..=MOST_GAMES).map(|n| format!("Game {n}")).collect();
        assert_eq!(draft.problem(), Some("desktop.economy.currency.gamesRange"));
        let game = draft.rules.iter_mut().find(|r| r.action == Action::GameMinute).unwrap();
        game.games = vec!["x".repeat(GAME_NAME_MAX + 1)];
        assert_eq!(draft.problem(), Some("desktop.economy.currency.gamesRange"));
        let game = draft.rules.iter_mut().find(|r| r.action == Action::GameMinute).unwrap();
        game.games = vec!["x".repeat(GAME_NAME_MAX)];
        assert_eq!(draft.problem(), None);
        // Other rules never carry games.
        let message = draft.rules.iter_mut().find(|r| r.action == Action::Message).unwrap();
        message.enabled = true;
        message.games = vec!["Celeste".into()];
        assert!(draft.earn().iter().find(|r| r.action == Action::Message as i32).unwrap().games.is_empty());
    }

    #[test]
    fn amounts_read_as_people_type_them() {
        assert_eq!(parse_amount("1,234"), Some(1234));
        assert_eq!(parse_amount(" -50 "), Some(-50));
        assert_eq!(parse_amount("12a"), None);
        assert_eq!(parse_amount(""), None);
    }
}
