//! The server's economy (`EconomyService`, docs/economy.md): its currencies
//! (`currencies`, each with its earn rules as one protobuf blob), what
//! members hold (`currency_balances`) and every change to it
//! (`currency_ledger`), what throttles earning (`currency_earnings`), and the
//! shop (`shop_listings`, with `profile_item_owners` for items bought).
//!
//! Earning runs inside the write that did the thing ([`earn`]), so a message
//! AutoMod blocks earns nothing; time in voice and playing games is
//! credited once a minute by [`spawn_minute_earnings`]. Balances never go below zero, and only their
//! owner (and people who run the economy) see them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, Seat, respond, text, users};
use crate::app::App;
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{
    self, CurrencyAction as Action, LedgerKind, Permission, ShopListingKind as ListingKind,
    economy_service_server::EconomyService,
};
use crate::permissions::Access;
use crate::servers::{self as store, Audit, Payload};

const DAY_MS: i64 = 86_400_000;
const MAX_NAME: usize = 32;
const MAX_LISTING_NAME: usize = 60;
const MAX_DESCRIPTION: usize = 200;
const MAX_NOTE: usize = 200;
const MAX_ICON: usize = 16;
/// What one earn rule may give at once, and what anything may cost or move.
const MAX_EARN: i64 = 1_000_000_000;
const MAX_AMOUNT: i64 = 1_000_000_000_000;
const MAX_COOLDOWN: i32 = 86_400;
const MAX_STREAK_DAYS: i32 = 365;
/// Games one GAME_MINUTE rule may name, and how long a name is (as long as
/// an activity's).
const MAX_GAMES: usize = 50;
const MAX_GAME_NAME: usize = 128;
/// How many people a finance page's rankings show.
const TOP: i64 = 10;
/// A ledger page.
const PAGE: i32 = 50;
const MAX_PAGE: i32 = 100;
/// Every currency's ledger is read for at most this long a period.
const MAX_DAYS: i32 = 365;

/// A currency's earn rules, as stored.
#[derive(Clone, PartialEq, prost::Message)]
struct EarnRules {
    #[prost(message, repeated, tag = "1")]
    rules: Vec<pb::EarnRule>,
}

/// The UTC day `ms` falls in, counted from 1970.
fn day_of(ms: i64) -> i64 {
    ms.div_euclid(DAY_MS)
}

// Currencies.

const CURRENCY_COLUMNS: &str =
    "id, name, icon_emoji, icon_emoji_id, description, earn, transferable, position, created_at";

fn currency_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<(pb::Currency, Option<Vec<u8>>)> + '_ {
    move |r| {
        Ok((
            pb::Currency {
                id: r.get(0)?,
                server_id: server_id.to_string(),
                name: r.get(1)?,
                icon_emoji: r.get(2)?,
                icon_emoji_id: r.get(3)?,
                description: r.get(4)?,
                earn: vec![],
                transferable: r.get::<i64>(6)? != 0,
                position: r.get(7)?,
                created_at: Some(timestamp(r.get(8)?)),
            },
            r.get::<Option<Vec<u8>>>(5)?,
        ))
    }
}

fn with_rules((mut currency, earn): (pb::Currency, Option<Vec<u8>>)) -> Result<pb::Currency> {
    if let Some(bytes) = earn {
        currency.earn = EarnRules::decode(bytes.as_slice())?.rules;
    }
    Ok(currency)
}

/// The server's currencies, in order.
pub(super) async fn currencies(conn: &turso::Connection, server_id: &str) -> Result<Vec<pb::Currency>> {
    query_all(
        conn,
        &format!("SELECT {CURRENCY_COLUMNS} FROM currencies ORDER BY position, created_at, id"),
        (),
        currency_row(server_id),
    )
    .await?
    .into_iter()
    .map(with_rules)
    .collect()
}

async fn load_currency(conn: &turso::Connection, server_id: &str, id: &str) -> Result<pb::Currency> {
    let row = query_one(
        conn,
        &format!("SELECT {CURRENCY_COLUMNS} FROM currencies WHERE id = ?1"),
        [id],
        currency_row(server_id),
    )
    .await?
    .ok_or(Error::NotFound("currency"))?;
    with_rules(row)
}

async fn save_currency(conn: &turso::Connection, c: &pb::Currency) -> Result<()> {
    let earn = (!c.earn.is_empty()).then(|| EarnRules { rules: c.earn.clone() }.encode_to_vec());
    conn.execute(
        "INSERT INTO currencies (id, name, icon_emoji, icon_emoji_id, description, earn, transferable, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET name = excluded.name, icon_emoji = excluded.icon_emoji,
           icon_emoji_id = excluded.icon_emoji_id, description = excluded.description, earn = excluded.earn,
           transferable = excluded.transferable, position = excluded.position",
        (
            c.id.as_str(),
            c.name.as_str(),
            c.icon_emoji.as_str(),
            c.icon_emoji_id.as_str(),
            c.description.as_str(),
            earn,
            c.transferable as i64,
            c.position,
            c.created_at.as_ref().map(crate::id::millis).unwrap_or_else(now_ms),
        ),
    )
    .await?;
    Ok(())
}

/// Every currency the server has now, as the event members get.
async fn currencies_changed(conn: &turso::Connection, server_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    let currencies = currencies(conn, server_id).await?;
    events.push(Payload::CurrenciesUpdated(pb::CurrenciesUpdated { currencies }));
    Ok(())
}

/// Earn rules checked: known actions, one each, in range, and only the
/// fields that mean something for each action.
fn checked_rules(rules: &[pb::EarnRule]) -> Result<Vec<pb::EarnRule>> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        let action = match Action::try_from(rule.action) {
            Ok(Action::Unspecified) | Err(_) => return Err(Error::invalid("an earn rule needs an action")),
            Ok(action) => action,
        };
        if !seen.insert(rule.action) {
            return Err(Error::invalid("a currency has at most one earn rule per action"));
        }
        if !(1..=MAX_EARN).contains(&rule.amount) {
            return Err(Error::invalid(format!("an earn rule gives 1 to {MAX_EARN}")));
        }
        let throttled = matches!(action, Action::Message | Action::ReactionReceived);
        if !(0..=MAX_COOLDOWN).contains(&rule.cooldown_seconds) {
            return Err(Error::invalid("a cooldown is 0 seconds to a day"));
        }
        if !(0..=MAX_AMOUNT).contains(&rule.daily_cap) {
            return Err(Error::invalid("a daily cap is 0 (none) or more"));
        }
        if !(0..=MAX_EARN).contains(&rule.streak_bonus) || !(0..=MAX_STREAK_DAYS).contains(&rule.streak_max_days) {
            return Err(Error::invalid(format!("a streak bonus is 0 to {MAX_EARN}, for up to {MAX_STREAK_DAYS} days")));
        }
        let daily = action == Action::Daily;
        let capped =
            matches!(action, Action::Message | Action::ReactionReceived | Action::VoiceMinute | Action::GameMinute);
        let games = if action == Action::GameMinute { checked_games(&rule.games)? } else { vec![] };
        out.push(pb::EarnRule {
            action: rule.action,
            amount: rule.amount,
            cooldown_seconds: if throttled { rule.cooldown_seconds } else { 0 },
            daily_cap: if capped { rule.daily_cap } else { 0 },
            streak_bonus: if daily { rule.streak_bonus } else { 0 },
            streak_max_days: if daily { rule.streak_max_days } else { 0 },
            games,
        });
    }
    out.sort_by_key(|r| r.action);
    Ok(out)
}

/// A GAME_MINUTE rule's games: trimmed, each once (ignoring case), in the
/// order given.
fn checked_games(games: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for game in games {
        let game = text("game", game, 1, MAX_GAME_NAME)?;
        if seen.insert(game_key(&game)) {
            out.push(game);
        }
    }
    if out.len() > MAX_GAMES {
        return Err(Error::invalid(format!("a rule names at most {MAX_GAMES} games")));
    }
    Ok(out)
}

/// How game names are compared: trimmed, ignoring case.
fn game_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether a GAME_MINUTE rule pays for playing any of `games`.
fn pays_for(rule: &pb::EarnRule, games: &[String]) -> bool {
    !games.is_empty()
        && (rule.games.is_empty() || games.iter().any(|g| rule.games.iter().any(|r| game_key(r) == game_key(g))))
}

/// Applies a change to a currency, checked. Whether anything changed.
async fn apply_change(
    conn: &turso::Connection,
    currency: &mut pb::Currency,
    change: pb::CurrencyChange,
) -> Result<bool> {
    let before = currency.clone();
    if let Some(name) = change.name {
        currency.name = text("name", &name, 1, MAX_NAME)?;
    }
    if let Some(icon) = change.icon_emoji {
        let icon = text("icon", &icon, 0, MAX_ICON)?;
        if icon.chars().any(|c| c.is_whitespace() || c.is_ascii_alphanumeric()) {
            return Err(Error::invalid("the icon is an emoji"));
        }
        currency.icon_emoji = icon;
    }
    if let Some(id) = change.icon_emoji_id {
        let id = id.trim().to_string();
        if !id.is_empty()
            && query_one(conn, "SELECT 1 FROM emojis WHERE id = ?1", [id.as_str()], |r| r.get::<i64>(0))
                .await?
                .is_none()
        {
            return Err(Error::NotFound("emoji"));
        }
        currency.icon_emoji_id = id;
    }
    if let Some(description) = change.description {
        currency.description = text("description", &description, 0, MAX_DESCRIPTION)?;
    }
    if let Some(transferable) = change.transferable {
        currency.transferable = transferable;
    }
    if let Some(position) = change.position {
        currency.position = position;
    }
    if change.replace_earn {
        currency.earn = checked_rules(&change.earn)?;
    }
    Ok(*currency != before)
}

fn currency_audit(action: pb::AuditAction, before: &pb::Currency, after: &pb::Currency) -> Audit {
    let mut entry = Audit::new(action, &after.id)
        .change("name", &before.name, &after.name)
        .change(
            "icon",
            format!("{}{}", before.icon_emoji, before.icon_emoji_id),
            format!("{}{}", after.icon_emoji, after.icon_emoji_id),
        )
        .change("description", &before.description, &after.description)
        .change("transferable", before.transferable, after.transferable)
        .change("position", before.position, after.position);
    if before.earn != after.earn {
        entry = entry.change("earn", "", "changed");
    }
    entry
}

// The shop.

const LISTING_COLUMNS: &str =
    "id, currency_id, kind, target_id, name, description, price, stock, sold, position, available, created_at";

fn listing_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<pb::ShopListing> + '_ {
    move |r| {
        Ok(pb::ShopListing {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            currency_id: r.get(1)?,
            kind: r.get(2)?,
            target_id: r.get(3)?,
            name: r.get(4)?,
            description: r.get(5)?,
            price: r.get(6)?,
            stock: r.get(7)?,
            sold: r.get(8)?,
            position: r.get(9)?,
            available: r.get::<i64>(10)? != 0,
            created_at: Some(timestamp(r.get(11)?)),
        })
    }
}

/// Every listing, in order.
async fn listings(conn: &turso::Connection, server_id: &str) -> Result<Vec<pb::ShopListing>> {
    query_all(
        conn,
        &format!("SELECT {LISTING_COLUMNS} FROM shop_listings ORDER BY position, created_at, id"),
        (),
        listing_row(server_id),
    )
    .await
}

async fn load_listing(conn: &turso::Connection, server_id: &str, id: &str) -> Result<pb::ShopListing> {
    query_one(conn, &format!("SELECT {LISTING_COLUMNS} FROM shop_listings WHERE id = ?1"), [id], listing_row(server_id))
        .await?
        .ok_or(Error::NotFound("listing"))
}

async fn save_listing(conn: &turso::Connection, l: &pb::ShopListing) -> Result<()> {
    conn.execute(
        "INSERT INTO shop_listings (id, currency_id, kind, target_id, name, description, price, stock, sold, position,
           available, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET name = excluded.name, description = excluded.description,
           price = excluded.price, stock = excluded.stock, position = excluded.position,
           available = excluded.available",
        (
            l.id.as_str(),
            l.currency_id.as_str(),
            l.kind,
            l.target_id.as_str(),
            l.name.as_str(),
            l.description.as_str(),
            l.price,
            l.stock,
            l.sold,
            l.position,
            l.available as i64,
            l.created_at.as_ref().map(crate::id::millis).unwrap_or_else(now_ms),
        ),
    )
    .await?;
    Ok(())
}

/// The whole shop, as the event members get.
async fn shop_changed(conn: &turso::Connection, server_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    let listings = listings(conn, server_id).await?;
    events.push(Payload::ShopUpdated(pb::ShopUpdated { listings }));
    Ok(())
}

/// Takes down the listings of a role or profile item that's gone, inside the
/// write that deletes it, telling members when there were any.
pub(super) async fn unlist(
    conn: &turso::Connection,
    server_id: &str,
    kind: ListingKind,
    target_id: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    if kind == ListingKind::ProfileItem {
        conn.execute("DELETE FROM profile_item_owners WHERE item_id = ?1", [target_id]).await?;
    }
    let gone =
        conn.execute("DELETE FROM shop_listings WHERE kind = ?1 AND target_id = ?2", (kind as i64, target_id)).await?;
    if gone > 0 {
        shop_changed(conn, server_id, events).await?;
    }
    Ok(())
}

/// Whether someone may put on one of the server's profile items: anyone,
/// unless the shop sells it, and then only those who bought it.
pub(super) async fn may_wear(conn: &turso::Connection, item_id: &str, account_id: &str) -> Result<()> {
    let listed = query_one(
        conn,
        "SELECT 1 FROM shop_listings WHERE kind = ?1 AND target_id = ?2 LIMIT 1",
        (ListingKind::ProfileItem as i64, item_id),
        |r| r.get::<i64>(0),
    )
    .await?
    .is_some();
    if listed && !owns(conn, item_id, account_id).await? {
        return Err(Error::FailedPrecondition("this item is sold in the server's shop: buy it first".into()));
    }
    Ok(())
}

async fn owns(conn: &turso::Connection, item_id: &str, account_id: &str) -> Result<bool> {
    Ok(query_one(
        conn,
        "SELECT 1 FROM profile_item_owners WHERE item_id = ?1 AND account_id = ?2",
        (item_id, account_id),
        |r| r.get::<i64>(0),
    )
    .await?
    .is_some())
}

// Balances and the ledger.

/// Someone's balance, with the UTC day of their last daily claim.
struct Held {
    balance: pb::Balance,
    daily_day: i64,
}

async fn held(conn: &turso::Connection, currency_id: &str, account_id: &str) -> Result<Held> {
    let row = query_one(
        conn,
        "SELECT amount, earned, spent, daily_day, daily_at, daily_streak FROM currency_balances
         WHERE currency_id = ?1 AND account_id = ?2",
        (currency_id, account_id),
        |r| {
            Ok((
                r.get::<i64>(0)?,
                r.get::<i64>(1)?,
                r.get::<i64>(2)?,
                r.get::<i64>(3)?,
                r.get::<i64>(4)?,
                r.get::<i32>(5)?,
            ))
        },
    )
    .await?;
    let (amount, earned, spent, daily_day, daily_at, daily_streak) = row.unwrap_or_default();
    Ok(Held {
        balance: pb::Balance {
            currency_id: currency_id.to_string(),
            user_id: account_id.to_string(),
            amount,
            earned,
            spent,
            daily_claimed_at: (daily_at > 0).then(|| timestamp(daily_at)),
            daily_streak,
        },
        daily_day,
    })
}

const LEDGER_COLUMNS: &str = "id, currency_id, account_id, amount, balance_after, kind, action, actor_id, \
     counterpart_id, listing_id, role_id, note, created_at";

fn ledger_row(r: &turso::Row) -> turso::Result<pb::LedgerEntry> {
    Ok(pb::LedgerEntry {
        id: r.get(0)?,
        currency_id: r.get(1)?,
        user_id: r.get(2)?,
        amount: r.get(3)?,
        balance_after: r.get(4)?,
        kind: r.get(5)?,
        action: r.get(6)?,
        actor_id: r.get(7)?,
        counterpart_id: r.get(8)?,
        listing_id: r.get(9)?,
        role_id: r.get(10)?,
        note: r.get(11)?,
        created_at: Some(timestamp(r.get(12)?)),
    })
}

/// Why a balance changes: the ledger entry's details beside who and how much.
#[derive(Clone, Default)]
struct Why<'a> {
    kind: LedgerKind,
    action: Action,
    actor_id: &'a str,
    counterpart_id: &'a str,
    listing_id: &'a str,
    role_id: &'a str,
    note: &'a str,
}

/// How [`credit`] takes more than someone has.
#[derive(Clone, Copy, PartialEq)]
enum Short {
    /// Refuses: a purchase or a transfer.
    Refuse,
    /// Takes what there is: a grant below zero.
    Stop,
}

/// Moves `amount` into someone's balance (out of it, below zero) inside a
/// write, with its ledger entry, and tells them (`announce`). Nothing when
/// nothing moves.
#[allow(clippy::too_many_arguments)]
async fn credit(
    conn: &turso::Connection,
    currency: &pb::Currency,
    account_id: &str,
    amount: i64,
    short: Short,
    why: Why<'_>,
    now: i64,
    announce: bool,
    events: &mut Vec<Payload>,
) -> Result<Option<(pb::Balance, pb::LedgerEntry)>> {
    let mut balance = held(conn, &currency.id, account_id).await?.balance;
    let moved = if amount < 0 {
        if balance.amount < -amount && short == Short::Refuse {
            return Err(Error::FailedPrecondition(format!("you don't have enough {}", currency.name)));
        }
        amount.max(-balance.amount)
    } else {
        amount.min(i64::MAX - balance.amount)
    };
    if moved == 0 {
        return Ok(None);
    }
    balance.amount += moved;
    if moved > 0 {
        balance.earned = balance.earned.saturating_add(moved);
    } else {
        balance.spent = balance.spent.saturating_add(-moved);
    }
    conn.execute(
        "INSERT INTO currency_balances (currency_id, account_id, amount, earned, spent, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (currency_id, account_id) DO UPDATE SET amount = excluded.amount, earned = excluded.earned,
           spent = excluded.spent, updated_at = excluded.updated_at",
        (currency.id.as_str(), account_id, balance.amount, balance.earned, balance.spent, now),
    )
    .await?;
    let entry = pb::LedgerEntry {
        id: new_id(),
        currency_id: currency.id.clone(),
        user_id: account_id.to_string(),
        amount: moved,
        balance_after: balance.amount,
        kind: why.kind as i32,
        action: why.action as i32,
        actor_id: why.actor_id.to_string(),
        counterpart_id: why.counterpart_id.to_string(),
        listing_id: why.listing_id.to_string(),
        role_id: why.role_id.to_string(),
        note: why.note.to_string(),
        created_at: Some(timestamp(now)),
    };
    conn.execute(
        &format!("INSERT INTO currency_ledger ({LEDGER_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"),
        (
            entry.id.as_str(),
            entry.currency_id.as_str(),
            account_id,
            entry.amount,
            entry.balance_after,
            entry.kind,
            entry.action,
            entry.actor_id.as_str(),
            entry.counterpart_id.as_str(),
            entry.listing_id.as_str(),
            entry.role_id.as_str(),
            entry.note.as_str(),
            now,
        ),
    )
    .await?;
    if announce {
        events.push(Payload::BalanceUpdated(pb::BalanceUpdated {
            balance: Some(balance.clone()),
            entry: Some(entry.clone()),
        }));
    }
    Ok(Some((balance, entry)))
}

/// Whether someone gets an economy event only some members get: their own
/// balance, and a purchase for its buyer and those who run the economy.
pub(super) fn event_shown(account_id: &str, access: &Access, payload: &Payload) -> bool {
    match payload {
        Payload::BalanceUpdated(pb::BalanceUpdated { balance: Some(b), .. }) => b.user_id == account_id,
        Payload::PurchaseMade(pb::PurchaseMade { entry: Some(e), .. }) => {
            e.user_id == account_id || access.has(Permission::ManageEconomy)
        }
        _ => false,
    }
}

// Earning.

/// How much a rule gives someone now, noting it: nothing during its
/// cooldown, past its daily cap, or a second time for a once-only action.
async fn allowance(
    conn: &turso::Connection,
    currency_id: &str,
    account_id: &str,
    rule: &pb::EarnRule,
    now: i64,
) -> Result<i64> {
    let day = day_of(now);
    let row = query_one(
        conn,
        "SELECT last_at, day, day_total FROM currency_earnings WHERE currency_id = ?1 AND account_id = ?2 AND action = ?3",
        (currency_id, account_id, rule.action),
        |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)?, r.get::<i64>(2)?)),
    )
    .await?;
    let once = matches!(Action::try_from(rule.action), Ok(Action::Join | Action::Onboarding));
    if once && row.is_some() {
        return Ok(0);
    }
    let today = row.filter(|(_, d, _)| *d == day).map(|(_, _, total)| total).unwrap_or(0);
    if let Some((last_at, _, _)) = row
        && now - last_at < i64::from(rule.cooldown_seconds) * 1000
    {
        return Ok(0);
    }
    let mut amount = rule.amount;
    if rule.daily_cap > 0 {
        amount = amount.min(rule.daily_cap - today);
    }
    if amount <= 0 {
        return Ok(0);
    }
    conn.execute(
        "INSERT INTO currency_earnings (currency_id, account_id, action, last_at, day, day_total)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (currency_id, account_id, action) DO UPDATE SET last_at = excluded.last_at,
           day = excluded.day, day_total = excluded.day_total",
        (currency_id, account_id, rule.action, now, day, today + amount),
    )
    .await?;
    Ok(amount)
}

/// The currencies with a rule for `action`, and the rule.
async fn rules_for(
    conn: &turso::Connection,
    server_id: &str,
    action: Action,
) -> Result<Vec<(pb::Currency, pb::EarnRule)>> {
    Ok(currencies(conn, server_id)
        .await?
        .into_iter()
        .filter_map(|c| {
            let rule = c.earn.iter().find(|r| r.action == action as i32)?.clone();
            Some((c, rule))
        })
        .collect())
}

/// Rewards a member for `action`, in every currency with a rule for it,
/// inside the write that did it. Agents and people who aren't members earn
/// nothing.
pub(super) async fn earn(
    conn: &turso::Connection,
    server_id: &str,
    account_id: &str,
    action: Action,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let rules = rules_for(conn, server_id, action).await?;
    if rules.is_empty() {
        return Ok(());
    }
    earn_by(conn, &rules, account_id, action, now, events).await
}

async fn earn_by(
    conn: &turso::Connection,
    rules: &[(pb::Currency, pb::EarnRule)],
    account_id: &str,
    action: Action,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let kind = query_one(
        conn,
        "SELECT users.kind FROM members JOIN users ON users.id = members.user_id WHERE members.user_id = ?1",
        [account_id],
        |r| r.get::<i32>(0),
    )
    .await?;
    if kind.is_none_or(|k| k == pb::AccountKind::Agent as i32) {
        return Ok(());
    }
    for (currency, rule) in rules {
        let amount = allowance(conn, &currency.id, account_id, rule, now).await?;
        if amount > 0 {
            let why = Why { kind: LedgerKind::Earn, action, ..Default::default() };
            credit(conn, currency, account_id, amount, Short::Stop, why, now, true, events).await?;
        }
    }
    Ok(())
}

/// How often voice and games are credited.
const MINUTE: Duration = Duration::from_secs(60);

/// Every minute: credits the people in each server's voice channels with
/// someone else there, who aren't deafened, under its VOICE_MINUTE rules,
/// and the people playing a game where they share it under its
/// GAME_MINUTE rules.
pub fn spawn_minute_earnings(app: Arc<App>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(MINUTE);
        // A minute missed (a slow directory) isn't paid twice after.
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        every.tick().await;
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = every.tick() => {
                    for scope in app.voice.scopes("") {
                        if scope.starts_with("dm:") {
                            continue;
                        }
                        if credit_voice(&app, &scope).await.is_err() {
                            tracing::warn!("couldn't credit time in voice");
                        }
                    }
                    credit_games(&app).await;
                }
            }
        }
    });
}

/// Credits everyone playing a game in this part's servers.
async fn credit_games(app: &App) {
    let players = match app.playing().await {
        Ok(players) => players,
        Err(err) => {
            tracing::info!(%err, "couldn't learn who's playing; games are credited next minute");
            return;
        }
    };
    let mut by_server: HashMap<String, Vec<(String, Vec<String>)>> = HashMap::new();
    for (server_id, user_id, games) in players {
        by_server.entry(server_id).or_default().push((user_id, games));
    }
    for (server_id, players) in by_server {
        if credit_players(app, &server_id, players).await.is_err() {
            tracing::warn!("couldn't credit time playing games");
        }
    }
}

async fn credit_players(app: &App, server_id: &str, players: Vec<(String, Vec<String>)>) -> Result<()> {
    let Ok(sdb) = app.servers.get(server_id).await else { return Ok(()) };
    let rules = rules_for(&*sdb.read()?, &sdb.id, Action::GameMinute).await?;
    if rules.is_empty() {
        return Ok(());
    }
    // Each person, with the rules their games pass.
    let paid: Vec<(String, Vec<(pb::Currency, pb::EarnRule)>)> = players
        .into_iter()
        .filter_map(|(user_id, games)| {
            let theirs: Vec<_> = rules.iter().filter(|(_, rule)| pays_for(rule, &games)).cloned().collect();
            (!theirs.is_empty()).then_some((user_id, theirs))
        })
        .collect();
    if paid.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    sdb.write("", async |conn, events| {
        for (user_id, rules) in &paid {
            earn_by(conn, rules, user_id, Action::GameMinute, now, events).await?;
        }
        Ok(())
    })
    .await
}

async fn credit_voice(app: &App, server_id: &str) -> Result<()> {
    let mut by_channel: HashMap<String, Vec<pb::VoiceState>> = HashMap::new();
    for place in app.voice.list(server_id) {
        by_channel.entry(place.state.channel_id.clone()).or_default().push(place.state);
    }
    let people: Vec<String> = by_channel
        .into_values()
        .filter(|states| states.len() >= 2)
        .flatten()
        .filter(|s| !s.self_deaf && !s.server_deaf)
        .map(|s| s.user_id)
        .collect();
    if people.is_empty() {
        return Ok(());
    }
    let Ok(sdb) = app.servers.get(server_id).await else { return Ok(()) };
    let rules = rules_for(&*sdb.read()?, &sdb.id, Action::VoiceMinute).await?;
    if rules.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    sdb.write("", async |conn, events| {
        for user_id in &people {
            earn_by(conn, &rules, user_id, Action::VoiceMinute, now, events).await?;
        }
        Ok(())
    })
    .await
}

/// Deletes everything about an account's money in the server, inside the
/// write that forgets them.
pub(crate) async fn forget_account(conn: &turso::Connection, account_id: &str) -> Result<()> {
    for table in ["currency_balances", "currency_earnings", "currency_ledger", "profile_item_owners"] {
        conn.execute(&format!("DELETE FROM {table} WHERE account_id = ?1"), [account_id]).await?;
    }
    Ok(())
}

// Reading the finances.

/// One currency's finances over the last `days` days.
async fn overview(conn: &turso::Connection, currency_id: &str, days: i32, now: i64) -> Result<pb::EconomyOverview> {
    let today = day_of(now);
    let first = today - i64::from(days) + 1;
    let since = first * DAY_MS;
    let (circulating, holders) = query_one(
        conn,
        "SELECT coalesce(sum(amount), 0), count(*) FROM currency_balances WHERE currency_id = ?1 AND amount > 0",
        [currency_id],
        |r| Ok((r.get::<i64>(0)?, r.get::<i32>(1)?)),
    )
    .await?
    .unwrap_or_default();
    let mut o =
        pb::EconomyOverview { currency_id: currency_id.to_string(), circulating, holders, ..Default::default() };
    let totals = query_all(
        conn,
        "SELECT kind, coalesce(sum(CASE WHEN amount > 0 THEN amount ELSE 0 END), 0),
           coalesce(sum(CASE WHEN amount < 0 THEN -amount ELSE 0 END), 0), count(*)
         FROM currency_ledger WHERE currency_id = ?1 AND created_at >= ?2 GROUP BY kind",
        (currency_id, since),
        |r| Ok((r.get::<i32>(0)?, r.get::<i64>(1)?, r.get::<i64>(2)?, r.get::<i64>(3)?)),
    )
    .await?;
    for (kind, inward, outward, count) in totals {
        match LedgerKind::try_from(kind) {
            Ok(LedgerKind::Earn) => o.earned = inward,
            Ok(LedgerKind::Grant) => (o.granted, o.taken) = (inward, outward),
            Ok(LedgerKind::Transfer) => o.transferred = inward,
            Ok(LedgerKind::Purchase) => (o.spent, o.purchases) = (outward, count),
            _ => {}
        }
    }
    o.earned_by_action = query_all(
        conn,
        "SELECT action, sum(amount) FROM currency_ledger WHERE currency_id = ?1 AND created_at >= ?2 AND kind = ?3
         GROUP BY action ORDER BY 2 DESC",
        (currency_id, since, LedgerKind::Earn as i64),
        |r| Ok(pb::ActionTotal { action: r.get(0)?, amount: r.get(1)? }),
    )
    .await?;
    let person = |r: &turso::Row| Ok(pb::PersonTotal { user_id: r.get(0)?, amount: r.get(1)? });
    o.top_holders = query_all(
        conn,
        "SELECT b.account_id, b.amount FROM currency_balances b JOIN members m ON m.user_id = b.account_id
         WHERE b.currency_id = ?1 AND b.amount > 0 ORDER BY b.amount DESC, b.account_id LIMIT ?2",
        (currency_id, TOP),
        person,
    )
    .await?;
    let ranked = |sign: &str, kind: LedgerKind| {
        format!(
            "SELECT l.account_id, sum({sign}l.amount) AS total FROM currency_ledger l
             JOIN members m ON m.user_id = l.account_id
             WHERE l.currency_id = ?1 AND l.created_at >= ?2 AND l.kind = {}
             GROUP BY l.account_id ORDER BY total DESC, l.account_id LIMIT ?3",
            kind as i32
        )
    };
    o.top_spenders = query_all(conn, &ranked("-", LedgerKind::Purchase), (currency_id, since, TOP), person).await?;
    o.top_earners = query_all(conn, &ranked("", LedgerKind::Earn), (currency_id, since, TOP), person).await?;
    o.top_listings = query_all(
        conn,
        "SELECT listing_id, count(*) AS bought, sum(-amount) FROM currency_ledger
         WHERE currency_id = ?1 AND created_at >= ?2 AND kind = ?3
         GROUP BY listing_id ORDER BY bought DESC, listing_id LIMIT ?4",
        (currency_id, since, LedgerKind::Purchase as i64, TOP),
        |r| Ok(pb::ListingTotal { listing_id: r.get(0)?, count: r.get(1)?, amount: r.get(2)? }),
    )
    .await?;
    let rows = query_all(
        conn,
        "SELECT created_at / ?3 AS day,
           coalesce(sum(CASE WHEN kind = ?4 THEN amount ELSE 0 END), 0),
           coalesce(sum(CASE WHEN kind = ?5 AND amount > 0 THEN amount ELSE 0 END), 0),
           coalesce(sum(CASE WHEN kind = ?6 THEN -amount ELSE 0 END), 0)
         FROM currency_ledger WHERE currency_id = ?1 AND created_at >= ?2 GROUP BY day",
        (currency_id, since, DAY_MS, LedgerKind::Earn as i64, LedgerKind::Grant as i64, LedgerKind::Purchase as i64),
        |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)?, r.get::<i64>(2)?, r.get::<i64>(3)?)),
    )
    .await?;
    let mut by_day: BTreeMap<i64, (i64, i64, i64)> = (first..=today).map(|d| (d, (0, 0, 0))).collect();
    for (day, earned, granted, spent) in rows {
        if let Some(slot) = by_day.get_mut(&day) {
            *slot = (earned, granted, spent);
        }
    }
    o.days = by_day
        .into_iter()
        .map(|(day, (earned, granted, spent))| pb::DayTotal {
            day: Some(timestamp(day * DAY_MS)),
            earned,
            granted,
            spent,
        })
        .collect();
    Ok(o)
}

/// Whose wallet or ledger a request asks for: yours, or someone else's with
/// any of `may`.
fn whose(account_id: &str, asked: &str, access: &Access, may: &[Permission]) -> Result<String> {
    if asked.is_empty() || asked == account_id {
        return Ok(account_id.to_string());
    }
    if may.iter().any(|p| access.has(*p)) {
        Ok(asked.to_string())
    } else {
        Err(Error::denied("you can only see your own wallet"))
    }
}

/// A checked amount for something that moves currency: 1 or more (or, when
/// `signed`, anything but 0) up to the most anything moves.
fn amount(value: i64, signed: bool) -> Result<i64> {
    let fine = if signed {
        value != 0 && value.unsigned_abs() <= MAX_AMOUNT as u64
    } else {
        (1..=MAX_AMOUNT).contains(&value)
    };
    if fine {
        Ok(value)
    } else {
        Err(Error::invalid(format!("an amount is {} up to {MAX_AMOUNT}", if signed { "not 0, and" } else { "1" })))
    }
}

/// A role the shop may sell: one of the server's (not @everyone) that carries
/// nothing that moderates or manages the server.
async fn sellable_role(conn: &turso::Connection, server_id: &str, role_id: &str) -> Result<pb::Role> {
    let role = crate::permissions::roles(conn, server_id)
        .await?
        .into_iter()
        .find(|r| r.id == role_id)
        .ok_or(Error::NotFound("role"))?;
    if role.id == server_id {
        return Err(Error::invalid("everyone has @everyone"));
    }
    if !super::join::harmless(&role) {
        return Err(Error::FailedPrecondition(
            "a role in the shop can't carry permissions that moderate or manage the server".into(),
        ));
    }
    Ok(role)
}

#[tonic::async_trait]
impl EconomyService for Api {
    async fn list_currencies(
        &self,
        request: Request<pb::ListCurrenciesRequest>,
    ) -> Result<Response<pb::ListCurrenciesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.membership(&account, &request.get_ref().server_id).await?.sdb;
                Ok(pb::ListCurrenciesResponse { currencies: currencies(&*sdb.read()?, &sdb.id).await? })
            }
            .await,
        )
    }

    async fn list_shop(&self, request: Request<pb::ListShopRequest>) -> Result<Response<pb::ListShopResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.membership(&account, &request.get_ref().server_id).await?.sdb;
                Ok(pb::ListShopResponse { listings: listings(&*sdb.read()?, &sdb.id).await? })
            }
            .await,
        )
    }

    async fn get_wallet(
        &self,
        request: Request<pb::GetWalletRequest>,
    ) -> Result<Response<pb::GetWalletResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                let user_id =
                    whose(&account.id, &req.user_id, &access, &[Permission::ManageEconomy, Permission::GrantCurrency])?;
                let conn = sdb.read()?;
                let mut balances = Vec::new();
                for currency in currencies(&conn, &sdb.id).await? {
                    balances.push(held(&conn, &currency.id, &user_id).await?.balance);
                }
                let owned_item_ids = query_all(
                    &conn,
                    "SELECT item_id FROM profile_item_owners WHERE account_id = ?1 ORDER BY created_at",
                    [user_id.as_str()],
                    |r| r.get::<String>(0),
                )
                .await?;
                Ok(pb::GetWalletResponse { balances, owned_item_ids })
            }
            .await,
        )
    }

    async fn list_ledger(
        &self,
        request: Request<pb::ListLedgerRequest>,
    ) -> Result<Response<pb::ListLedgerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                let everyone = req.user_id == "*";
                if everyone {
                    access.require(Permission::ManageEconomy)?;
                }
                let user_id = if everyone {
                    String::new()
                } else {
                    whose(&account.id, &req.user_id, &access, &[Permission::ManageEconomy])?
                };
                let limit = match req.limit {
                    0 => PAGE,
                    n => n.clamp(1, MAX_PAGE),
                };
                let mut filters = vec!["1 = 1".to_string()];
                let mut params: Vec<turso::Value> = Vec::new();
                let mut param = |value: turso::Value, filters: &mut Vec<String>, sql: &str| {
                    params.push(value);
                    filters.push(sql.replace('?', &format!("?{}", params.len())));
                };
                if !everyone {
                    param(user_id.as_str().into(), &mut filters, "account_id = ?");
                }
                if !req.currency_id.is_empty() {
                    param(req.currency_id.as_str().into(), &mut filters, "currency_id = ?");
                }
                if !req.before_id.is_empty() {
                    param(req.before_id.as_str().into(), &mut filters, "id < ?");
                }
                if !req.kinds.is_empty() {
                    let kinds: Vec<String> =
                        req.kinds.iter().filter(|k| LedgerKind::try_from(**k).is_ok()).map(|k| k.to_string()).collect();
                    filters
                        .push(format!("kind IN ({})", if kinds.is_empty() { "-1".into() } else { kinds.join(", ") }));
                }
                params.push(i64::from(limit + 1).into());
                let conn = sdb.read()?;
                let mut entries = query_all(
                    &conn,
                    &format!(
                        "SELECT {LEDGER_COLUMNS} FROM currency_ledger WHERE {} ORDER BY id DESC LIMIT ?{}",
                        filters.join(" AND "),
                        params.len()
                    ),
                    params,
                    ledger_row,
                )
                .await?;
                let has_more = entries.len() > limit as usize;
                entries.truncate(limit as usize);
                let named: Vec<&str> = entries
                    .iter()
                    .flat_map(|e| [e.user_id.as_str(), e.actor_id.as_str(), e.counterpart_id.as_str()])
                    .collect();
                let users = users(&conn, &named).await?;
                Ok(pb::ListLedgerResponse { entries, has_more, users })
            }
            .await,
        )
    }

    async fn claim_daily(
        &self,
        request: Request<pb::ClaimDailyRequest>,
    ) -> Result<Response<pb::ClaimDailyResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_not_timed_out()?;
                if account.kind == pb::AccountKind::Agent {
                    return Err(Error::denied("agents don't claim daily rewards"));
                }
                let now = now_ms();
                let (balance, entry) = sdb
                    .write(&account.id, async |conn, events| {
                        let currency = load_currency(conn, &sdb.id, &req.currency_id).await?;
                        let rule =
                            currency.earn.iter().find(|r| r.action == Action::Daily as i32).cloned().ok_or_else(
                                || Error::FailedPrecondition(format!("{} has no daily reward", currency.name)),
                            )?;
                        let today = day_of(now);
                        let before = held(conn, &currency.id, &account.id).await?;
                        if before.daily_day == today {
                            return Err(Error::FailedPrecondition(
                                "you've claimed today's reward already; come back tomorrow".into(),
                            ));
                        }
                        let streak = if before.daily_day == today - 1 { before.balance.daily_streak + 1 } else { 1 };
                        let bonus_days = i64::from((streak - 1).min(rule.streak_max_days).max(0));
                        let amount = rule.amount.saturating_add(rule.streak_bonus.saturating_mul(bonus_days));
                        let why = Why { kind: LedgerKind::Earn, action: Action::Daily, ..Default::default() };
                        let (mut balance, entry) =
                            credit(conn, &currency, &account.id, amount, Short::Stop, why, now, false, events)
                                .await?
                                .ok_or_else(|| Error::FailedPrecondition("you can't hold any more".into()))?;
                        conn.execute(
                            "UPDATE currency_balances SET daily_day = ?3, daily_at = ?4, daily_streak = ?5
                             WHERE currency_id = ?1 AND account_id = ?2",
                            (currency.id.as_str(), account.id.as_str(), today, now, streak),
                        )
                        .await?;
                        balance.daily_claimed_at = Some(timestamp(now));
                        balance.daily_streak = streak;
                        events.push(Payload::BalanceUpdated(pb::BalanceUpdated {
                            balance: Some(balance.clone()),
                            entry: Some(entry.clone()),
                        }));
                        Ok((balance, entry))
                    })
                    .await?;
                Ok(pb::ClaimDailyResponse { entry: Some(entry), balance: Some(balance) })
            }
            .await,
        )
    }

    async fn transfer(&self, request: Request<pb::TransferRequest>) -> Result<Response<pb::TransferResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_not_timed_out()?;
                let amount = amount(req.amount, false)?;
                let note = text("note", &req.note, 0, MAX_NOTE)?;
                if req.user_id == account.id {
                    return Err(Error::invalid("send it to someone else"));
                }
                let now = now_ms();
                let (balance, entry) = sdb
                    .write(&account.id, async |conn, events| {
                        let currency = load_currency(conn, &sdb.id, &req.currency_id).await?;
                        if !currency.transferable {
                            return Err(Error::FailedPrecondition(format!(
                                "{} can't be sent between members",
                                currency.name
                            )));
                        }
                        store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                        let out = Why {
                            kind: LedgerKind::Transfer,
                            counterpart_id: &req.user_id,
                            note: &note,
                            ..Default::default()
                        };
                        let sent = credit(conn, &currency, &account.id, -amount, Short::Refuse, out, now, true, events)
                            .await?
                            .ok_or_else(|| {
                                Error::FailedPrecondition(format!("you don't have enough {}", currency.name))
                            })?;
                        let into = Why {
                            kind: LedgerKind::Transfer,
                            counterpart_id: &account.id,
                            note: &note,
                            ..Default::default()
                        };
                        credit(conn, &currency, &req.user_id, amount, Short::Stop, into, now, true, events).await?;
                        Ok(sent)
                    })
                    .await?;
                Ok(pb::TransferResponse { entry: Some(entry), balance: Some(balance) })
            }
            .await,
        )
    }

    async fn buy(&self, request: Request<pb::BuyRequest>) -> Result<Response<pb::BuyResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_not_timed_out()?;
                let now = now_ms();
                let (balance, entry, listing) = sdb
                    .write(&account.id, async |conn, events| {
                        let listing = load_listing(conn, &sdb.id, &req.listing_id).await?;
                        if !listing.available {
                            return Err(Error::NotFound("listing"));
                        }
                        if req.expected_price != 0 && req.expected_price != listing.price {
                            return Err(Error::FailedPrecondition(
                                "the price changed: have another look before buying".into(),
                            ));
                        }
                        if listing.stock == Some(0) {
                            return Err(Error::FailedPrecondition("it's sold out".into()));
                        }
                        let currency = load_currency(conn, &sdb.id, &listing.currency_id).await?;
                        match ListingKind::try_from(listing.kind) {
                            Ok(ListingKind::Role) => {
                                let role = sellable_role(conn, &sdb.id, &listing.target_id).await?;
                                let given = conn
                                    .execute(
                                        "INSERT INTO member_roles (user_id, role_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                                        (account.id.as_str(), role.id.as_str()),
                                    )
                                    .await?;
                                if given == 0 {
                                    return Err(Error::AlreadyExists("you have this role already".into()));
                                }
                                let member =
                                    store::member(conn, &sdb.id, &account.id).await?.ok_or(Error::NotFound("member"))?;
                                events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member) }));
                            }
                            Ok(ListingKind::ProfileItem) => {
                                crate::profile_items::get(conn, &sdb.id, &listing.target_id)
                                    .await?
                                    .ok_or(Error::NotFound("profile item"))?;
                                let given = conn
                                    .execute(
                                        "INSERT INTO profile_item_owners (item_id, account_id, created_at)
                                         VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                                        (listing.target_id.as_str(), account.id.as_str(), now),
                                    )
                                    .await?;
                                if given == 0 {
                                    return Err(Error::AlreadyExists("you have this item already".into()));
                                }
                            }
                            _ => {}
                        }
                        let why = Why { kind: LedgerKind::Purchase, listing_id: &listing.id, ..Default::default() };
                        let (balance, entry) =
                            credit(conn, &currency, &account.id, -listing.price, Short::Refuse, why, now, true, events)
                                .await?
                                .ok_or_else(|| Error::FailedPrecondition(format!("you don't have enough {}", currency.name)))?;
                        conn.execute(
                            "UPDATE shop_listings SET sold = sold + 1, stock = stock - 1 WHERE id = ?1",
                            [listing.id.as_str()],
                        )
                        .await?;
                        let listing = load_listing(conn, &sdb.id, &listing.id).await?;
                        events.push(Payload::ShopListingUpdated(pb::ShopListingUpdated { listing: Some(listing.clone()) }));
                        events.push(Payload::PurchaseMade(pb::PurchaseMade {
                            entry: Some(entry.clone()),
                            listing: Some(listing.clone()),
                        }));
                        Ok((balance, entry, listing))
                    })
                    .await?;
                Ok(pb::BuyResponse { entry: Some(entry), balance: Some(balance), listing: Some(listing) })
            }
            .await,
        )
    }

    async fn create_currency(
        &self,
        request: Request<pb::CreateCurrencyRequest>,
    ) -> Result<Response<pb::CreateCurrencyResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEconomy).await?.sdb;
                let change = req.currency.ok_or_else(|| Error::invalid("currency is needed"))?;
                if change.name.is_none() {
                    return Err(Error::invalid("a currency needs a name"));
                }
                let currency = sdb
                    .write(&account.id, async |conn, events| {
                        let blank = pb::Currency {
                            id: new_id(),
                            server_id: sdb.id.clone(),
                            created_at: Some(timestamp(now_ms())),
                            ..Default::default()
                        };
                        let mut currency = blank.clone();
                        apply_change(conn, &mut currency, change.clone()).await?;
                        save_currency(conn, &currency).await?;
                        let entry = currency_audit(pb::AuditAction::CurrencyCreate, &blank, &currency);
                        store::audit(conn, &account.id, entry).await?;
                        currencies_changed(conn, &sdb.id, events).await?;
                        Ok(currency)
                    })
                    .await?;
                Ok(pb::CreateCurrencyResponse { currency: Some(currency) })
            }
            .await,
        )
    }

    async fn update_currency(
        &self,
        request: Request<pb::UpdateCurrencyRequest>,
    ) -> Result<Response<pb::UpdateCurrencyResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEconomy).await?.sdb;
                let change = req.change.unwrap_or_default();
                let currency = sdb
                    .write(&account.id, async |conn, events| {
                        let before = load_currency(conn, &sdb.id, &req.currency_id).await?;
                        let mut currency = before.clone();
                        if !apply_change(conn, &mut currency, change.clone()).await? {
                            return Ok(currency);
                        }
                        save_currency(conn, &currency).await?;
                        let entry = currency_audit(pb::AuditAction::CurrencyUpdate, &before, &currency);
                        store::audit(conn, &account.id, entry).await?;
                        currencies_changed(conn, &sdb.id, events).await?;
                        Ok(currency)
                    })
                    .await?;
                Ok(pb::UpdateCurrencyResponse { currency: Some(currency) })
            }
            .await,
        )
    }

    async fn delete_currency(
        &self,
        request: Request<pb::DeleteCurrencyRequest>,
    ) -> Result<Response<pb::DeleteCurrencyResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEconomy).await?.sdb;
                // Sweeps balances other writes may be adding to.
                sdb.write_alone(&account.id, async |conn, events| {
                    let currency = load_currency(conn, &sdb.id, &req.currency_id).await?;
                    for table in ["currency_balances", "currency_earnings", "currency_ledger", "shop_listings"] {
                        conn.execute(&format!("DELETE FROM {table} WHERE currency_id = ?1"), [currency.id.as_str()])
                            .await?;
                    }
                    conn.execute("DELETE FROM currencies WHERE id = ?1", [currency.id.as_str()]).await?;
                    let entry =
                        Audit::new(pb::AuditAction::CurrencyDelete, &currency.id).change("name", &currency.name, "");
                    store::audit(conn, &account.id, entry).await?;
                    currencies_changed(conn, &sdb.id, events).await?;
                    shop_changed(conn, &sdb.id, events).await
                })
                .await?;
                Ok(pb::DeleteCurrencyResponse {})
            }
            .await,
        )
    }

    async fn create_shop_listing(
        &self,
        request: Request<pb::CreateShopListingRequest>,
    ) -> Result<Response<pb::CreateShopListingResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageEconomy).await?;
                let new = req.listing.ok_or_else(|| Error::invalid("listing is needed"))?;
                let kind = match ListingKind::try_from(new.kind) {
                    Ok(ListingKind::Unspecified) | Err(_) => return Err(Error::invalid("a listing needs a kind")),
                    Ok(kind) => kind,
                };
                if kind == ListingKind::Role {
                    access.require(Permission::ManageRoles)?;
                }
                let min_name = if kind == ListingKind::Custom { 1 } else { 0 };
                let listing = pb::ShopListing {
                    id: new_id(),
                    server_id: sdb.id.clone(),
                    currency_id: new.currency_id.clone(),
                    kind: kind as i32,
                    target_id: if kind == ListingKind::Custom {
                        String::new()
                    } else {
                        new.target_id.trim().to_string()
                    },
                    name: text("name", &new.name, min_name, MAX_LISTING_NAME)?,
                    description: text("description", &new.description, 0, MAX_DESCRIPTION)?,
                    price: amount(new.price, false)?,
                    stock: new.stock.map(|s| s.clamp(0, MAX_AMOUNT)),
                    sold: 0,
                    position: new.position,
                    available: new.available,
                    created_at: Some(timestamp(now_ms())),
                };
                let listing = sdb
                    .write(&account.id, async |conn, events| {
                        let currency = load_currency(conn, &sdb.id, &listing.currency_id).await?;
                        match kind {
                            ListingKind::Role => {
                                let role = sellable_role(conn, &sdb.id, &listing.target_id).await?;
                                if !access.above(i64::from(role.position)) {
                                    return Err(Error::denied("you can only sell roles below your own"));
                                }
                            }
                            ListingKind::ProfileItem => {
                                crate::profile_items::get(conn, &sdb.id, &listing.target_id)
                                    .await?
                                    .ok_or(Error::NotFound("profile item"))?;
                            }
                            _ => {}
                        }
                        save_listing(conn, &listing).await?;
                        let entry = Audit::new(pb::AuditAction::ShopListingCreate, &listing.id)
                            .change("name", "", &listing.name)
                            .change("target", "", &listing.target_id)
                            .change("currency", "", &currency.name)
                            .change("price", "", listing.price);
                        store::audit(conn, &account.id, entry).await?;
                        shop_changed(conn, &sdb.id, events).await?;
                        Ok(listing.clone())
                    })
                    .await?;
                Ok(pb::CreateShopListingResponse { listing: Some(listing) })
            }
            .await,
        )
    }

    async fn update_shop_listing(
        &self,
        request: Request<pb::UpdateShopListingRequest>,
    ) -> Result<Response<pb::UpdateShopListingResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEconomy).await?.sdb;
                let change = req.change.unwrap_or_default();
                let listing = sdb
                    .write(&account.id, async |conn, events| {
                        let before = load_listing(conn, &sdb.id, &req.listing_id).await?;
                        let mut listing = before.clone();
                        if let Some(name) = &change.name {
                            let min = if before.kind == ListingKind::Custom as i32 { 1 } else { 0 };
                            listing.name = text("name", name, min, MAX_LISTING_NAME)?;
                        }
                        if let Some(description) = &change.description {
                            listing.description = text("description", description, 0, MAX_DESCRIPTION)?;
                        }
                        if let Some(price) = change.price {
                            listing.price = amount(price, false)?;
                        }
                        if change.unlimited {
                            listing.stock = None;
                        } else if let Some(stock) = change.stock {
                            listing.stock = Some(stock.clamp(0, MAX_AMOUNT));
                        }
                        if let Some(position) = change.position {
                            listing.position = position;
                        }
                        if let Some(available) = change.available {
                            listing.available = available;
                        }
                        if listing == before {
                            return Ok(listing);
                        }
                        save_listing(conn, &listing).await?;
                        let stock = |s: Option<i64>| s.map(|s| s.to_string()).unwrap_or_default();
                        let entry = Audit::new(pb::AuditAction::ShopListingUpdate, &listing.id)
                            .change("name", &before.name, &listing.name)
                            .change("description", &before.description, &listing.description)
                            .change("price", before.price, listing.price)
                            .change("stock", stock(before.stock), stock(listing.stock))
                            .change("position", before.position, listing.position)
                            .change("available", before.available, listing.available);
                        store::audit(conn, &account.id, entry).await?;
                        shop_changed(conn, &sdb.id, events).await?;
                        Ok(listing)
                    })
                    .await?;
                Ok(pb::UpdateShopListingResponse { listing: Some(listing) })
            }
            .await,
        )
    }

    async fn delete_shop_listing(
        &self,
        request: Request<pb::DeleteShopListingRequest>,
    ) -> Result<Response<pb::DeleteShopListingResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEconomy).await?.sdb;
                sdb.write(&account.id, async |conn, events| {
                    let listing = load_listing(conn, &sdb.id, &req.listing_id).await?;
                    conn.execute("DELETE FROM shop_listings WHERE id = ?1", [listing.id.as_str()]).await?;
                    let entry = Audit::new(pb::AuditAction::ShopListingDelete, &listing.id)
                        .change("name", &listing.name, "")
                        .change("target", &listing.target_id, "");
                    store::audit(conn, &account.id, entry).await?;
                    shop_changed(conn, &sdb.id, events).await
                })
                .await?;
                Ok(pb::DeleteShopListingResponse {})
            }
            .await,
        )
    }

    async fn get_economy_overview(
        &self,
        request: Request<pb::GetEconomyOverviewRequest>,
    ) -> Result<Response<pb::GetEconomyOverviewResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEconomy).await?.sdb;
                let days = match req.days {
                    0 => 30,
                    n => n.clamp(1, MAX_DAYS),
                };
                let conn = sdb.read()?;
                let currency = load_currency(&conn, &sdb.id, &req.currency_id).await?;
                let overview = overview(&conn, &currency.id, days, now_ms()).await?;
                Ok(pb::GetEconomyOverviewResponse { overview: Some(overview) })
            }
            .await,
        )
    }

    async fn grant(&self, request: Request<pb::GrantRequest>) -> Result<Response<pb::GrantResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::GrantCurrency).await?.sdb;
                let amount = amount(req.amount, true)?;
                let reason = text("reason", &req.reason, 0, MAX_NOTE)?;
                let now = now_ms();
                match (req.user_id.is_empty(), req.role_id.is_empty()) {
                    (false, true) => {
                        let moved = sdb
                            .write(&account.id, async |conn, events| {
                                let currency = load_currency(conn, &sdb.id, &req.currency_id).await?;
                                store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                                let why = Why {
                                    kind: LedgerKind::Grant,
                                    actor_id: &account.id,
                                    note: &reason,
                                    ..Default::default()
                                };
                                let moved =
                                    credit(conn, &currency, &req.user_id, amount, Short::Stop, why, now, true, events)
                                        .await?
                                        .map(|(_, entry)| entry.amount)
                                        .unwrap_or(0);
                                if moved != 0 {
                                    let entry = Audit::new(pb::AuditAction::CurrencyGrant, &req.user_id)
                                        .reason(&reason)
                                        .change("currency", "", &currency.name)
                                        .change("amount", "", moved);
                                    store::audit(conn, &account.id, entry).await?;
                                }
                                Ok(moved)
                            })
                            .await?;
                        Ok(pb::GrantResponse { people: i32::from(moved != 0), total: moved })
                    }
                    (true, false) => {
                        // Sweeps the role's members' balances, which their own
                        // messages may be adding to.
                        let (people, total) = sdb
                            .write_alone(&account.id, async |conn, events| {
                                let currency = load_currency(conn, &sdb.id, &req.currency_id).await?;
                                let role = crate::permissions::roles(conn, &sdb.id)
                                    .await?
                                    .into_iter()
                                    .find(|r| r.id == req.role_id)
                                    .ok_or(Error::NotFound("role"))?;
                                let ids = if role.id == sdb.id {
                                    query_all(conn, "SELECT user_id FROM members", (), |r| r.get::<String>(0)).await?
                                } else {
                                    query_all(
                                        conn,
                                        "SELECT user_id FROM member_roles WHERE role_id = ?1",
                                        [role.id.as_str()],
                                        |r| r.get::<String>(0),
                                    )
                                    .await?
                                };
                                let why = Why {
                                    kind: LedgerKind::Grant,
                                    actor_id: &account.id,
                                    role_id: &role.id,
                                    note: &reason,
                                    ..Default::default()
                                };
                                let (mut people, mut total) = (0i32, 0i64);
                                for id in &ids {
                                    if let Some((_, entry)) = credit(
                                        conn,
                                        &currency,
                                        id,
                                        amount,
                                        Short::Stop,
                                        why.clone(),
                                        now,
                                        false,
                                        events,
                                    )
                                    .await?
                                    {
                                        people += 1;
                                        total = total.saturating_add(entry.amount);
                                    }
                                }
                                let entry = Audit::new(pb::AuditAction::CurrencyGrant, &role.id)
                                    .role(&role.name)
                                    .reason(&reason)
                                    .change("currency", "", &currency.name)
                                    .change("amount", "", amount);
                                store::audit(conn, &account.id, entry).await?;
                                events.push(Payload::WalletsChanged(pb::WalletsChanged {
                                    currency_id: currency.id.clone(),
                                }));
                                Ok((people, total))
                            })
                            .await?;
                        Ok(pb::GrantResponse { people, total })
                    }
                    _ => Err(Error::invalid("grant to a member or to a role")),
                }
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_rule(games: &[&str]) -> pb::EarnRule {
        pb::EarnRule {
            action: Action::GameMinute as i32,
            amount: 1,
            games: games.iter().map(|g| g.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn game_rules_pay_for_the_games_they_name() {
        let played = |games: &[&str]| games.iter().map(|g| g.to_string()).collect::<Vec<_>>();
        let named = game_rule(&["Celeste", "Hollow Knight"]);
        assert!(pays_for(&named, &played(&["celeste"])));
        assert!(pays_for(&named, &played(&["Tetris", " HOLLOW KNIGHT "])));
        assert!(!pays_for(&named, &played(&["Tetris"])));
        // No games named: any game pays, but not playing nothing.
        let any = game_rule(&[]);
        assert!(pays_for(&any, &played(&["Tetris"])));
        assert!(!pays_for(&any, &[]));
    }

    #[test]
    fn game_lists_are_checked() {
        let games = |n: usize| (0..n).map(|i| format!("Game {i}")).collect::<Vec<_>>();
        assert_eq!(checked_games(&games(MAX_GAMES)).unwrap().len(), MAX_GAMES);
        assert!(checked_games(&games(MAX_GAMES + 1)).is_err());
        // Repeats don't count against the limit.
        let mut repeated = games(MAX_GAMES);
        repeated.push("GAME 0".into());
        assert_eq!(checked_games(&repeated).unwrap().len(), MAX_GAMES);
        assert!(checked_games(&["".into()]).is_err());
        assert!(checked_games(&["x".repeat(MAX_GAME_NAME + 1)]).is_err());
        // Only game rules keep games (rules come back in action order).
        let rules = checked_rules(&[pb::EarnRule { games: vec!["Celeste".into()], ..game_rule(&[]) }, {
            pb::EarnRule { action: Action::Join as i32, games: vec!["Celeste".into()], ..game_rule(&[]) }
        }])
        .unwrap();
        assert_eq!(rules.iter().map(|r| r.games.len()).collect::<Vec<_>>(), vec![0, 1]);
    }
}
