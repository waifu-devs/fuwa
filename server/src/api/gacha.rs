//! Gacha banners (docs/economy.md): what members pull on for a price, drawing
//! prizes (the shop's listings, or currency) by each tier's rate and each
//! prize's weight, with pity and what happens to duplicates. Banners are
//! whole protobufs in the server file's `gacha_banners`; each member's pulls
//! and pity count are in `gacha_progress`; every pull is kept, with its
//! rolls, in `gacha_pulls`.
//!
//! A pull is one write: it pays, draws with the OS's randomness (never a
//! seed anyone could guess), hands out what came up, and logs it, so a pull
//! that fails half way takes nothing.

use std::collections::{HashMap, HashSet};

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::economy::{Short, Why, amount, credit, currencies, listings, load_currency, load_listing, sellable_role};
use super::media::PictureOwner;
use super::{Api, Seat, respond, text, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{
    self, GachaDuplicates as Duplicates, GachaOutcome as Outcome, LedgerKind, Permission,
    ShopListingKind as ListingKind,
};
use crate::servers::{self as store, Audit, Payload};

/// A rate of 100%, in millionths.
pub(super) const WHOLE: i32 = 1_000_000;
const MAX_BANNERS: usize = 50;
const MAX_NAME: usize = 60;
const MAX_DESCRIPTION: usize = 500;
const MAX_TIERS: usize = 10;
const MAX_TIER_ID: usize = 32;
const MAX_TIER_NAME: usize = 32;
const MAX_PRIZES: usize = 100;
const MAX_MULTI: i32 = 100;
const MAX_WEIGHT: i32 = 1_000_000;
const MAX_PITY: i32 = 10_000;
const MAX_PULLS: i32 = 1_000_000;
/// What one currency prize or refund may give.
const MAX_PRIZE: i64 = 1_000_000_000;
/// A page of pulls.
const PAGE: i32 = 50;
const MAX_PAGE: i32 = 100;

/// A pull's draws, as stored.
#[derive(Clone, PartialEq, prost::Message)]
struct GachaResults {
    #[prost(message, repeated, tag = "1")]
    results: Vec<pb::GachaResult>,
}

/// How many draws a stored pull made.
pub(super) fn draws(bytes: &[u8]) -> i64 {
    GachaResults::decode(bytes).map(|r| r.results.len() as i64).unwrap_or(0)
}

// Banners.

/// Fills in each prize's chance: its tier's rate, shared by weight.
fn with_chances(mut banner: pb::GachaBanner) -> pb::GachaBanner {
    let mut weights: HashMap<String, i64> = HashMap::new();
    for prize in &banner.prizes {
        *weights.entry(prize.tier_id.clone()).or_default() += i64::from(prize.weight);
    }
    let rates: HashMap<String, i64> = banner.tiers.iter().map(|t| (t.id.clone(), i64::from(t.rate))).collect();
    for prize in &mut banner.prizes {
        let rate = rates.get(&prize.tier_id).copied().unwrap_or(0);
        let total = weights.get(&prize.tier_id).copied().unwrap_or(0);
        prize.chance = if total > 0 { (rate * i64::from(prize.weight) / total) as i32 } else { 0 };
    }
    banner
}

fn decode(server_id: &str, bytes: &[u8]) -> Result<pb::GachaBanner> {
    let mut banner = pb::GachaBanner::decode(bytes)?;
    banner.server_id = server_id.to_string();
    Ok(with_chances(banner))
}

/// Every banner, in order.
pub(super) async fn banners(conn: &turso::Connection, server_id: &str) -> Result<Vec<pb::GachaBanner>> {
    query_all(conn, "SELECT banner FROM gacha_banners ORDER BY position, created_at, id", (), |r| r.get::<Vec<u8>>(0))
        .await?
        .iter()
        .map(|bytes| decode(server_id, bytes))
        .collect()
}

async fn load_banner(conn: &turso::Connection, server_id: &str, id: &str) -> Result<pb::GachaBanner> {
    let bytes = query_one(conn, "SELECT banner FROM gacha_banners WHERE id = ?1", [id], |r| r.get::<Vec<u8>>(0))
        .await?
        .ok_or(Error::NotFound("banner"))?;
    decode(server_id, &bytes)
}

async fn save_banner(conn: &turso::Connection, banner: &pb::GachaBanner) -> Result<()> {
    let mut stored = banner.clone();
    stored.server_id.clear();
    for prize in &mut stored.prizes {
        prize.chance = 0;
    }
    let created_at = banner.created_at.as_ref().map(crate::id::millis).unwrap_or_else(now_ms);
    conn.execute(
        "INSERT INTO gacha_banners (id, currency_id, position, banner, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET currency_id = excluded.currency_id, position = excluded.position,
           banner = excluded.banner",
        (banner.id.as_str(), banner.currency_id.as_str(), banner.position, stored.encode_to_vec(), created_at),
    )
    .await?;
    Ok(())
}

/// Every banner, as the event members get.
async fn banners_changed(conn: &turso::Connection, server_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    let banners = banners(conn, server_id).await?;
    events.push(Payload::GachaBannersUpdated(pb::GachaBannersUpdated { banners }));
    Ok(())
}

/// Takes the prizes `gone` picks off every banner, inside the write that
/// deleted what they were (a listing), telling members when any went.
pub(super) async fn forget_prizes(
    conn: &turso::Connection,
    server_id: &str,
    gone: impl Fn(&pb::GachaPrize) -> bool,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let mut changed = false;
    for mut banner in banners(conn, server_id).await? {
        let before = banner.prizes.len();
        banner.prizes.retain(|p| !gone(p));
        if banner.prizes.len() != before {
            save_banner(conn, &banner).await?;
            changed = true;
        }
    }
    if changed {
        banners_changed(conn, server_id, events).await?;
    }
    Ok(())
}

/// A currency is gone (and its listings): the banners priced in it go with
/// their progress and pulls, and the others lose the prizes and refunds in
/// it or its listings.
pub(super) async fn forget_currency(
    conn: &turso::Connection,
    server_id: &str,
    currency_id: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let all = banners(conn, server_id).await?;
    if all.is_empty() {
        return Ok(());
    }
    for banner in all.iter().filter(|b| b.currency_id == currency_id) {
        conn.execute("DELETE FROM gacha_progress WHERE banner_id = ?1", [banner.id.as_str()]).await?;
    }
    conn.execute("DELETE FROM gacha_banners WHERE currency_id = ?1", [currency_id]).await?;
    conn.execute("DELETE FROM gacha_pulls WHERE currency_id = ?1", [currency_id]).await?;
    let listed: HashSet<String> = listings(conn, server_id).await?.into_iter().map(|l| l.id).collect();
    for mut banner in all.into_iter().filter(|b| b.currency_id != currency_id) {
        let before = banner.clone();
        banner.prizes.retain(|p| {
            if p.listing_id.is_empty() { p.currency_id != currency_id } else { listed.contains(&p.listing_id) }
        });
        if banner.refund_currency_id == currency_id {
            banner.duplicates = Duplicates::Keep as i32;
            banner.refund_currency_id.clear();
            banner.refund_amount = 0;
        }
        if banner != before {
            save_banner(conn, &banner).await?;
        }
    }
    banners_changed(conn, server_id, events).await
}

/// A banner as a manager sent it, checked, with new prizes given ids.
/// `access` decides whether roles may be prizes (Manage Roles, and ranked
/// above them).
async fn checked(
    conn: &turso::Connection,
    server_id: &str,
    access: &crate::permissions::Access,
    mut banner: pb::GachaBanner,
) -> Result<pb::GachaBanner> {
    banner.server_id = server_id.to_string();
    banner.name = text("name", &banner.name, 1, MAX_NAME)?;
    banner.description = text("description", &banner.description, 0, MAX_DESCRIPTION)?;
    load_currency(conn, server_id, &banner.currency_id).await?;
    banner.pull_price = amount(banner.pull_price, false)?;
    if banner.multi_pull != 0 && !(2..=MAX_MULTI).contains(&banner.multi_pull) {
        return Err(Error::invalid(format!("a multi-pull is 2 to {MAX_MULTI} pulls, or 0 for none")));
    }
    if banner.multi_pull == 0 {
        banner.multi_pull_price = 0;
    } else if banner.multi_pull_price != 0 {
        banner.multi_pull_price = amount(banner.multi_pull_price, false)?;
    }

    // Tiers, rarest first, their rates making 100%.
    if banner.tiers.is_empty() || banner.tiers.len() > MAX_TIERS {
        return Err(Error::invalid(format!("a banner has 1 to {MAX_TIERS} tiers")));
    }
    let mut tier_ids = HashSet::new();
    let mut total: i64 = 0;
    for tier in &mut banner.tiers {
        tier.id = text("tier id", &tier.id, 1, MAX_TIER_ID)?;
        tier.name = text("tier name", &tier.name, 1, MAX_TIER_NAME)?;
        if !tier_ids.insert(tier.id.clone()) {
            return Err(Error::invalid("each tier needs its own id"));
        }
        if !(0..=WHOLE).contains(&tier.rate) {
            return Err(Error::invalid("a tier's rate is 0% to 100%"));
        }
        if let Some(color) = tier.color {
            tier.color = Some(color & 0xFF_FFFF);
        }
        total += i64::from(tier.rate);
    }
    if total != i64::from(WHOLE) {
        return Err(Error::invalid(format!(
            "the tiers' rates add up to {}%, not 100%",
            total as f64 / f64::from(WHOLE) * 100.0
        )));
    }

    // Prizes.
    if banner.prizes.is_empty() || banner.prizes.len() > MAX_PRIZES {
        return Err(Error::invalid(format!("a banner has 1 to {MAX_PRIZES} prizes")));
    }
    let shop: HashMap<String, pb::ShopListing> =
        listings(conn, server_id).await?.into_iter().map(|l| (l.id.clone(), l)).collect();
    let mut prize_ids = HashSet::new();
    let mut filled = HashSet::new();
    for prize in &mut banner.prizes {
        if prize.id.trim().is_empty() {
            prize.id = new_id();
        }
        if !prize_ids.insert(prize.id.clone()) {
            return Err(Error::invalid("each prize needs its own id"));
        }
        if !tier_ids.contains(&prize.tier_id) {
            return Err(Error::invalid("each prize is in one of the banner's tiers"));
        }
        if !(1..=MAX_WEIGHT).contains(&prize.weight) {
            return Err(Error::invalid(format!("a prize's weight is 1 to {MAX_WEIGHT}")));
        }
        prize.chance = 0;
        match (prize.listing_id.is_empty(), prize.currency_id.is_empty()) {
            (false, true) => {
                let listing = shop.get(&prize.listing_id).ok_or(Error::NotFound("listing"))?;
                prize.amount = 0;
                if listing.kind == ListingKind::Role as i32 {
                    access.require(Permission::ManageRoles)?;
                    let role = sellable_role(conn, server_id, &listing.target_id).await?;
                    if !access.above(i64::from(role.position)) {
                        return Err(Error::denied("you can only give out roles below your own"));
                    }
                }
            }
            (true, false) => {
                load_currency(conn, server_id, &prize.currency_id).await?;
                if !(1..=MAX_PRIZE).contains(&prize.amount) {
                    return Err(Error::invalid(format!("a currency prize is 1 to {MAX_PRIZE}")));
                }
            }
            _ => return Err(Error::invalid("a prize is a shop listing or an amount of a currency")),
        }
        filled.insert(prize.tier_id.clone());
    }
    if let Some(tier) = banner.tiers.iter().find(|t| t.rate > 0 && !filled.contains(&t.id)) {
        return Err(Error::invalid(format!("the {} tier has a rate but no prizes", tier.name)));
    }

    // Pity, duplicates, limits and when it's open.
    if banner.pity_pulls == 0 {
        banner.pity_tier_id.clear();
    } else {
        if !(1..=MAX_PITY).contains(&banner.pity_pulls) {
            return Err(Error::invalid(format!("pity comes after 1 to {MAX_PITY} pulls")));
        }
        if !tier_ids.contains(&banner.pity_tier_id) {
            return Err(Error::invalid("pity needs one of the banner's tiers"));
        }
    }
    match Duplicates::try_from(banner.duplicates) {
        Ok(Duplicates::Refund) => {
            load_currency(conn, server_id, &banner.refund_currency_id).await?;
            if !(1..=MAX_PRIZE).contains(&banner.refund_amount) {
                return Err(Error::invalid(format!("a refund is 1 to {MAX_PRIZE}")));
            }
        }
        Ok(Duplicates::Keep | Duplicates::SkipOwned) => {
            banner.refund_currency_id.clear();
            banner.refund_amount = 0;
        }
        Ok(Duplicates::Unspecified) => {
            banner.duplicates = Duplicates::Keep as i32;
            banner.refund_currency_id.clear();
            banner.refund_amount = 0;
        }
        Err(_) => return Err(Error::invalid("unknown duplicates setting")),
    }
    if !(0..=MAX_PULLS).contains(&banner.max_pulls) {
        return Err(Error::invalid(format!("a member's pulls are limited to 1 to {MAX_PULLS}, or 0 for no limit")));
    }
    if let (Some(starts), Some(ends)) = (&banner.starts_at, &banner.ends_at)
        && crate::id::millis(ends) <= crate::id::millis(starts)
    {
        return Err(Error::invalid("a banner ends after it starts"));
    }
    Ok(with_chances(banner))
}

fn banner_audit(action: pb::AuditAction, before: Option<&pb::GachaBanner>, after: &pb::GachaBanner) -> Audit {
    let blank = pb::GachaBanner::default();
    let before = before.unwrap_or(&blank);
    let mut entry = Audit::new(action, &after.id)
        .change("name", &before.name, &after.name)
        .change("price", before.pull_price, after.pull_price)
        .change("available", before.available, after.available);
    if before.tiers != after.tiers || before.prizes != after.prizes {
        entry = entry.change("rates", "", "changed");
    }
    entry
}

// Drawing.

/// A number from 0 up to (not including) `below`, from the OS's random
/// number generator, without the bias of a plain remainder.
fn roll(below: i64) -> Result<i64> {
    let below = u64::try_from(below).ok().filter(|b| *b > 0).ok_or_else(|| Error::invalid("nothing to roll"))?;
    // The largest multiple of `below` a u64 holds: values from it up are
    // thrown away so each result is as likely as the others.
    let zone = u64::MAX - (u64::MAX % below);
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|_| Error::Unavailable("no randomness to draw with".into()))?;
        let value = u64::from_le_bytes(bytes);
        if value < zone {
            return Ok((value % below) as i64);
        }
    }
}

/// Picks the item a roll lands on, counting up by weight.
fn pick<T>(items: &[(T, i64)], roll: i64) -> Option<&T> {
    let mut at = 0;
    for (item, weight) in items {
        at += weight;
        if roll < at {
            return Some(item);
        }
    }
    None
}

/// What one draw may choose from: the tiers (by index) that can come up,
/// each with its rate, and in each the prizes (by index) that can drop.
struct Draw {
    tiers: Vec<(usize, i64)>,
    prizes: HashMap<usize, Vec<(usize, i64)>>,
    guaranteed: bool,
}

/// The tiers and prizes a draw chooses from, given which prizes can drop;
/// with `pity_tier`, only that tier and rarer ones (when any can come up).
fn candidates(banner: &pb::GachaBanner, can_drop: impl Fn(&pb::GachaPrize) -> bool, pity_tier: Option<usize>) -> Draw {
    let mut prizes: HashMap<usize, Vec<(usize, i64)>> = HashMap::new();
    for (i, prize) in banner.prizes.iter().enumerate() {
        if let Some(t) = banner.tiers.iter().position(|t| t.id == prize.tier_id)
            && can_drop(prize)
        {
            prizes.entry(t).or_default().push((i, i64::from(prize.weight)));
        }
    }
    let tiers: Vec<(usize, i64)> = banner
        .tiers
        .iter()
        .enumerate()
        .filter(|(i, t)| t.rate > 0 && prizes.contains_key(i))
        .map(|(i, t)| (i, i64::from(t.rate)))
        .collect();
    if let Some(pity) = pity_tier {
        let rare: Vec<(usize, i64)> = tiers.iter().copied().filter(|(i, _)| *i <= pity).collect();
        if !rare.is_empty() {
            return Draw { tiers: rare, prizes, guaranteed: true };
        }
    }
    Draw { tiers, prizes, guaranteed: false }
}

/// One member's progress on a banner.
async fn progress(conn: &turso::Connection, banner_id: &str, account_id: &str) -> Result<pb::GachaProgress> {
    let (pulls, since_pity) = query_one(
        conn,
        "SELECT pulls, since_pity FROM gacha_progress WHERE banner_id = ?1 AND account_id = ?2",
        (banner_id, account_id),
        |r| Ok((r.get::<i32>(0)?, r.get::<i32>(1)?)),
    )
    .await?
    .unwrap_or_default();
    Ok(pb::GachaProgress { banner_id: banner_id.to_string(), pulls, since_pity })
}

/// Whether someone has a role or profile item a listing hands out.
async fn has(conn: &turso::Connection, listing: &pb::ShopListing, account_id: &str) -> Result<bool> {
    let sql = match ListingKind::try_from(listing.kind) {
        Ok(ListingKind::Role) => "SELECT 1 FROM member_roles WHERE role_id = ?1 AND user_id = ?2",
        Ok(ListingKind::ProfileItem) => "SELECT 1 FROM profile_item_owners WHERE item_id = ?1 AND account_id = ?2",
        _ => return Ok(false),
    };
    Ok(query_one(conn, sql, (listing.target_id.as_str(), account_id), |r| r.get::<i64>(0)).await?.is_some())
}

/// Whether a listing gives something kept for good (a role or profile item),
/// which can be drawn again as a duplicate.
fn kept(listing: &pb::ShopListing) -> bool {
    matches!(ListingKind::try_from(listing.kind), Ok(ListingKind::Role | ListingKind::ProfileItem))
}

/// Pulls `count` times on a banner for someone, inside a write: pays, draws,
/// hands out and logs. Their balances that moved, the pull and their
/// progress after it.
async fn pull_in(
    conn: &turso::Connection,
    server_id: &str,
    account_id: &str,
    req: &pb::PullRequest,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<(pb::GachaPull, pb::GachaProgress, Vec<pb::Balance>)> {
    let banner = load_banner(conn, server_id, &req.banner_id).await?;
    if !banner.available {
        return Err(Error::NotFound("banner"));
    }
    if banner.starts_at.as_ref().is_some_and(|t| crate::id::millis(t) > now) {
        return Err(Error::FailedPrecondition("this banner isn't open yet".into()));
    }
    if banner.ends_at.as_ref().is_some_and(|t| crate::id::millis(t) <= now) {
        return Err(Error::FailedPrecondition("this banner has closed".into()));
    }
    let count = req.count.max(1);
    let price = if count == 1 {
        banner.pull_price
    } else if banner.multi_pull >= 2 && count == banner.multi_pull {
        if banner.multi_pull_price > 0 {
            banner.multi_pull_price
        } else {
            banner.pull_price.saturating_mul(i64::from(count))
        }
    } else {
        return Err(Error::invalid("pull once, or the banner's multi-pull"));
    };
    if req.expected_price != 0 && req.expected_price != price {
        return Err(Error::FailedPrecondition("the price changed: have another look before pulling".into()));
    }
    let mut progress = progress(conn, &banner.id, account_id).await?;
    if banner.max_pulls > 0 && progress.pulls + count > banner.max_pulls {
        let left = (banner.max_pulls - progress.pulls).max(0);
        return Err(Error::FailedPrecondition(if left == 0 {
            "you've made every pull this banner allows".into()
        } else {
            format!("you have {left} pulls left on this banner")
        }));
    }

    let money: HashMap<String, pb::Currency> =
        currencies(conn, server_id).await?.into_iter().map(|c| (c.id.clone(), c)).collect();
    let currency = money.get(&banner.currency_id).ok_or(Error::NotFound("currency"))?;
    let mut balances: HashMap<String, pb::Balance> = HashMap::new();
    let why = Why { kind: LedgerKind::Gacha, banner_id: &banner.id, ..Default::default() };
    let (balance, paid) = credit(conn, currency, account_id, -price, Short::Refuse, why, now, true, events)
        .await?
        .ok_or_else(|| Error::FailedPrecondition(format!("you don't have enough {}", currency.name)))?;
    balances.insert(balance.currency_id.clone(), balance);

    // What the prizes are now: their listings (stock goes down as they drop),
    // the roles the shop may give, and what the puller has.
    let mut shop: HashMap<String, pb::ShopListing> =
        listings(conn, server_id).await?.into_iter().map(|l| (l.id.clone(), l)).collect();
    let roles: HashMap<String, pb::Role> = crate::permissions::roles(conn, server_id)
        .await?
        .into_iter()
        .filter(|r| r.id != server_id && super::join::harmless(r))
        .map(|r| (r.id.clone(), r))
        .collect();
    let mut owned: HashSet<String> = HashSet::new();
    for prize in &banner.prizes {
        if let Some(listing) = shop.get(&prize.listing_id)
            && has(conn, listing, account_id).await?
        {
            owned.insert(listing.id.clone());
        }
    }
    let skip_owned = banner.duplicates == Duplicates::SkipOwned as i32;
    let pity_tier =
        (banner.pity_pulls > 0).then(|| banner.tiers.iter().position(|t| t.id == banner.pity_tier_id)).flatten();

    let mut results = Vec::with_capacity(count as usize);
    let mut got_role = false;
    for _ in 0..count {
        let due = pity_tier.filter(|_| progress.since_pity + 1 >= banner.pity_pulls);
        let draw = candidates(
            &banner,
            |prize| {
                if prize.listing_id.is_empty() {
                    return money.contains_key(&prize.currency_id);
                }
                let Some(listing) = shop.get(&prize.listing_id) else { return false };
                let role_fine = listing.kind != ListingKind::Role as i32 || roles.contains_key(&listing.target_id);
                listing.available
                    && listing.stock != Some(0)
                    && role_fine
                    && !(skip_owned && kept(listing) && owned.contains(&listing.id))
            },
            due,
        );
        let tier_total: i64 = draw.tiers.iter().map(|(_, rate)| rate).sum();
        if tier_total == 0 {
            return Err(Error::FailedPrecondition("nothing on this banner can drop for you right now".into()));
        }
        let tier_roll = roll(tier_total)?;
        let tier_index = *pick(&draw.tiers, tier_roll).ok_or_else(|| Error::invalid("the roll missed"))?;
        let in_tier = &draw.prizes[&tier_index];
        let prize_total: i64 = in_tier.iter().map(|(_, w)| w).sum();
        let prize_roll = roll(prize_total)?;
        let prize = &banner.prizes[*pick(in_tier, prize_roll).ok_or_else(|| Error::invalid("the roll missed"))?];
        let tier = &banner.tiers[tier_index];
        progress.pulls += 1;
        progress.since_pity = if pity_tier.is_some_and(|p| tier_index <= p) { 0 } else { progress.since_pity + 1 };

        let mut result = pb::GachaResult {
            tier_id: tier.id.clone(),
            tier_name: tier.name.clone(),
            prize_id: prize.id.clone(),
            listing_id: prize.listing_id.clone(),
            outcome: Outcome::Won as i32,
            guaranteed: draw.guaranteed,
            tier_roll,
            tier_total,
            prize_roll,
            prize_total,
            ..Default::default()
        };
        if prize.listing_id.is_empty() {
            let won = &money[&prize.currency_id];
            result.prize_name = format!("{} {}", prize.amount, won.name);
            result.currency_id = won.id.clone();
            result.amount = prize.amount;
            let why = Why { kind: LedgerKind::Gacha, banner_id: &banner.id, note: "prize", ..Default::default() };
            if let Some((balance, _)) =
                credit(conn, won, account_id, prize.amount, Short::Stop, why, now, true, events).await?
            {
                balances.insert(balance.currency_id.clone(), balance);
            }
        } else {
            let listing = shop.get(&prize.listing_id).cloned().ok_or(Error::NotFound("listing"))?;
            result.prize_name = prize_name(conn, server_id, &listing, &roles).await?;
            let duplicate = kept(&listing) && owned.contains(&listing.id);
            if duplicate {
                if banner.duplicates == Duplicates::Refund as i32
                    && let Some(refund) = money.get(&banner.refund_currency_id)
                {
                    result.outcome = Outcome::Refunded as i32;
                    result.currency_id = refund.id.clone();
                    result.amount = banner.refund_amount;
                    let why =
                        Why { kind: LedgerKind::Gacha, banner_id: &banner.id, note: "duplicate", ..Default::default() };
                    if let Some((balance, _)) =
                        credit(conn, refund, account_id, banner.refund_amount, Short::Stop, why, now, true, events)
                            .await?
                    {
                        balances.insert(balance.currency_id.clone(), balance);
                    }
                } else {
                    result.outcome = Outcome::Duplicate as i32;
                }
            } else {
                match ListingKind::try_from(listing.kind) {
                    Ok(ListingKind::Role) => {
                        conn.execute(
                            "INSERT INTO member_roles (user_id, role_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                            (account_id, listing.target_id.as_str()),
                        )
                        .await?;
                        got_role = true;
                    }
                    Ok(ListingKind::ProfileItem) => {
                        conn.execute(
                            "INSERT INTO profile_item_owners (item_id, account_id, created_at) VALUES (?1, ?2, ?3)
                             ON CONFLICT DO NOTHING",
                            (listing.target_id.as_str(), account_id, now),
                        )
                        .await?;
                    }
                    _ => {}
                }
                owned.insert(listing.id.clone());
                conn.execute(
                    "UPDATE shop_listings SET sold = sold + 1, stock = stock - 1 WHERE id = ?1",
                    [listing.id.as_str()],
                )
                .await?;
                let listing = load_listing(conn, server_id, &listing.id).await?;
                events.push(Payload::ShopListingUpdated(pb::ShopListingUpdated { listing: Some(listing.clone()) }));
                shop.insert(listing.id.clone(), listing);
            }
        }
        results.push(result);
    }

    if got_role {
        let member = store::member(conn, server_id, account_id).await?.ok_or(Error::NotFound("member"))?;
        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member) }));
    }
    conn.execute(
        "INSERT INTO gacha_progress (banner_id, account_id, pulls, since_pity) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (banner_id, account_id) DO UPDATE SET pulls = excluded.pulls, since_pity = excluded.since_pity",
        (banner.id.as_str(), account_id, progress.pulls, progress.since_pity),
    )
    .await?;
    let pull = pb::GachaPull {
        id: new_id(),
        banner_id: banner.id.clone(),
        banner_name: banner.name.clone(),
        user_id: account_id.to_string(),
        currency_id: banner.currency_id.clone(),
        cost: price,
        ledger_id: paid.id,
        results,
        created_at: Some(timestamp(now)),
    };
    conn.execute(
        "INSERT INTO gacha_pulls (id, banner_id, banner_name, account_id, currency_id, cost, ledger_id, results,
           created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
            pull.id.as_str(),
            pull.banner_id.as_str(),
            pull.banner_name.as_str(),
            account_id,
            pull.currency_id.as_str(),
            pull.cost,
            pull.ledger_id.as_str(),
            GachaResults { results: pull.results.clone() }.encode_to_vec(),
            now,
        ),
    )
    .await?;
    events.push(Payload::GachaPulled(pb::GachaPulled { pull: Some(pull.clone()), progress: Some(progress.clone()) }));
    let mut balances: Vec<pb::Balance> = balances.into_values().collect();
    balances.sort_by(|a, b| a.currency_id.cmp(&b.currency_id));
    Ok((pull, progress, balances))
}

/// What a listing is called: its own name, or its role's or item's.
async fn prize_name(
    conn: &turso::Connection,
    server_id: &str,
    listing: &pb::ShopListing,
    roles: &HashMap<String, pb::Role>,
) -> Result<String> {
    if !listing.name.is_empty() {
        return Ok(listing.name.clone());
    }
    Ok(match ListingKind::try_from(listing.kind) {
        Ok(ListingKind::Role) => roles.get(&listing.target_id).map(|r| r.name.clone()).unwrap_or_default(),
        Ok(ListingKind::ProfileItem) => {
            crate::profile_items::get(conn, server_id, &listing.target_id).await?.map(|i| i.name).unwrap_or_default()
        }
        _ => String::new(),
    })
}

fn pull_row(r: &turso::Row) -> turso::Result<(pb::GachaPull, Vec<u8>)> {
    Ok((
        pb::GachaPull {
            id: r.get(0)?,
            banner_id: r.get(1)?,
            banner_name: r.get(2)?,
            user_id: r.get(3)?,
            currency_id: r.get(4)?,
            cost: r.get(5)?,
            ledger_id: r.get(6)?,
            results: vec![],
            created_at: Some(timestamp(r.get(8)?)),
        },
        r.get(7)?,
    ))
}

// The calls, answered for EconomyService.

pub(super) async fn list_banners(
    api: &Api,
    request: Request<pb::ListGachaBannersRequest>,
) -> Result<Response<pb::ListGachaBannersResponse>, Status> {
    respond(
        async {
            let account = api.account(request.metadata()).await?;
            let sdb = api.membership(&account, &request.get_ref().server_id).await?.sdb;
            let conn = sdb.read()?;
            let banners = banners(&conn, &sdb.id).await?;
            let progress = query_all(
                &conn,
                "SELECT banner_id, pulls, since_pity FROM gacha_progress WHERE account_id = ?1",
                [account.id.as_str()],
                |r| Ok(pb::GachaProgress { banner_id: r.get(0)?, pulls: r.get(1)?, since_pity: r.get(2)? }),
            )
            .await?
            .into_iter()
            .filter(|p| banners.iter().any(|b| b.id == p.banner_id))
            .collect();
            Ok(pb::ListGachaBannersResponse { banners, progress })
        }
        .await,
    )
}

pub(super) async fn pull(api: &Api, request: Request<pb::PullRequest>) -> Result<Response<pb::PullResponse>, Status> {
    respond(
        async {
            let account = api.account(request.metadata()).await?;
            let req = request.into_inner();
            let Seat { sdb, access, .. } = api.membership(&account, &req.server_id).await?;
            access.require_not_timed_out()?;
            let now = now_ms();
            let (pull, progress, balances) = sdb
                .write(&account.id, async |conn, events| pull_in(conn, &sdb.id, &account.id, &req, now, events).await)
                .await?;
            Ok(pb::PullResponse { pull: Some(pull), progress: Some(progress), balances })
        }
        .await,
    )
}

pub(super) async fn list_pulls(
    api: &Api,
    request: Request<pb::ListGachaPullsRequest>,
) -> Result<Response<pb::ListGachaPullsResponse>, Status> {
    respond(
        async {
            let account = api.account(request.metadata()).await?;
            let req = request.into_inner();
            let Seat { sdb, access, .. } = api.membership(&account, &req.server_id).await?;
            let everyone = req.user_id == "*";
            let mine = req.user_id.is_empty() || req.user_id == account.id;
            if !mine {
                access.require(Permission::ManageEconomy)?;
            }
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
                let user_id = if mine { account.id.as_str() } else { req.user_id.as_str() };
                param(user_id.into(), &mut filters, "account_id = ?");
            }
            if !req.banner_id.is_empty() {
                param(req.banner_id.as_str().into(), &mut filters, "banner_id = ?");
            }
            if !req.before_id.is_empty() {
                param(req.before_id.as_str().into(), &mut filters, "id < ?");
            }
            params.push(i64::from(limit + 1).into());
            let conn = sdb.read()?;
            let rows = query_all(
                &conn,
                &format!(
                    "SELECT id, banner_id, banner_name, account_id, currency_id, cost, ledger_id, results, created_at
                     FROM gacha_pulls WHERE {} ORDER BY id DESC LIMIT ?{}",
                    filters.join(" AND "),
                    params.len()
                ),
                params,
                pull_row,
            )
            .await?;
            let has_more = rows.len() > limit as usize;
            let mut pulls = Vec::with_capacity(rows.len());
            for (mut pull, bytes) in rows.into_iter().take(limit as usize) {
                pull.results = GachaResults::decode(bytes.as_slice())?.results;
                pulls.push(pull);
            }
            let named: Vec<&str> = pulls.iter().map(|p| p.user_id.as_str()).collect();
            let users = users(&conn, &named).await?;
            Ok(pb::ListGachaPullsResponse { pulls, has_more, users })
        }
        .await,
    )
}

/// A banner's art as it's being saved: unchanged (`had`), cleared, or a
/// picture just uploaded here for the server's shop, handed back to keep
/// once the banner is saved. Never a link to another site, which every
/// member's app would then fetch.
async fn checked_art(
    api: &Api,
    account: &crate::node::Account,
    server_id: &str,
    url: &str,
    had: &str,
) -> Result<(String, Option<pb::Media>)> {
    let url = url.trim();
    if url.is_empty() || url == had {
        return Ok((url.to_string(), None));
    }
    let upload = api
        .check_upload(account, pb::MediaPurpose::ShopArt, url, Some(server_id))
        .await?
        .ok_or_else(|| Error::invalid("upload the banner's picture here rather than linking to it"))?;
    Ok((upload.url.clone(), Some(upload)))
}

pub(super) async fn create_banner(
    api: &Api,
    request: Request<pb::CreateGachaBannerRequest>,
) -> Result<Response<pb::CreateGachaBannerResponse>, Status> {
    respond(
        async {
            let account = api.account(request.metadata()).await?;
            let req = request.into_inner();
            let Seat { sdb, access, .. } = api.with(&account, &req.server_id, Permission::ManageEconomy).await?;
            let mut banner = req.banner.ok_or_else(|| Error::invalid("banner is needed"))?;
            banner.id = new_id();
            banner.created_at = Some(timestamp(now_ms()));
            let (art, upload) = checked_art(api, &account, &sdb.id, &banner.art_url, "").await?;
            banner.art_url = art;
            let written = sdb
                .write(&account.id, async |conn, events| {
                    let count = query_one(conn, "SELECT count(*) FROM gacha_banners", (), |r| r.get::<i64>(0))
                        .await?
                        .unwrap_or(0);
                    if count as usize >= MAX_BANNERS {
                        return Err(Error::ResourceExhausted(format!("a server has at most {MAX_BANNERS} banners")));
                    }
                    let banner = checked(conn, &sdb.id, &access, banner.clone()).await?;
                    save_banner(conn, &banner).await?;
                    let entry = banner_audit(pb::AuditAction::GachaBannerCreate, None, &banner);
                    store::audit(conn, &account.id, entry).await?;
                    banners_changed(conn, &sdb.id, events).await?;
                    Ok(banner)
                })
                .await;
            // Nothing saved: the picture just checked in goes again.
            let banner = match written {
                Ok(banner) => banner,
                Err(err) => {
                    if upload.is_some() {
                        api.drop_picture(&banner.art_url, "", PictureOwner::Server(&sdb.id)).await;
                    }
                    return Err(err);
                }
            };
            api.keep_picture(upload.as_ref().map(|u| u.id.as_str()), Some(&sdb.id)).await;
            Ok(pb::CreateGachaBannerResponse { banner: Some(banner) })
        }
        .await,
    )
}

pub(super) async fn update_banner(
    api: &Api,
    request: Request<pb::UpdateGachaBannerRequest>,
) -> Result<Response<pb::UpdateGachaBannerResponse>, Status> {
    respond(
        async {
            let account = api.account(request.metadata()).await?;
            let req = request.into_inner();
            let Seat { sdb, access, .. } = api.with(&account, &req.server_id, Permission::ManageEconomy).await?;
            let mut banner = req.banner.ok_or_else(|| Error::invalid("banner is needed"))?;
            let had = load_banner(&*sdb.read()?, &sdb.id, &banner.id).await?.art_url;
            let (art, upload) = checked_art(api, &account, &sdb.id, &banner.art_url, &had).await?;
            banner.art_url = art;
            let written = sdb
                .write(&account.id, async |conn, events| {
                    let before = load_banner(conn, &sdb.id, &banner.id).await?;
                    let mut banner = banner.clone();
                    banner.created_at = before.created_at;
                    let banner = checked(conn, &sdb.id, &access, banner).await?;
                    if banner == before {
                        return Ok((banner, before));
                    }
                    // Pity counts toward a tier: when it changes, everyone starts again.
                    if banner.pity_tier_id != before.pity_tier_id || banner.pity_pulls != before.pity_pulls {
                        conn.execute(
                            "UPDATE gacha_progress SET since_pity = 0 WHERE banner_id = ?1",
                            [banner.id.as_str()],
                        )
                        .await?;
                    }
                    save_banner(conn, &banner).await?;
                    let entry = banner_audit(pb::AuditAction::GachaBannerUpdate, Some(&before), &banner);
                    store::audit(conn, &account.id, entry).await?;
                    banners_changed(conn, &sdb.id, events).await?;
                    Ok((banner, before))
                })
                .await;
            let (banner, before) = match written {
                Ok(written) => written,
                Err(err) => {
                    if upload.is_some() {
                        api.drop_picture(&banner.art_url, &had, PictureOwner::Server(&sdb.id)).await;
                    }
                    return Err(err);
                }
            };
            api.keep_picture(upload.as_ref().map(|u| u.id.as_str()), Some(&sdb.id)).await;
            // What the write replaced, read inside it.
            api.drop_picture(&before.art_url, &banner.art_url, PictureOwner::Server(&sdb.id)).await;
            Ok(pb::UpdateGachaBannerResponse { banner: Some(banner) })
        }
        .await,
    )
}

pub(super) async fn delete_banner(
    api: &Api,
    request: Request<pb::DeleteGachaBannerRequest>,
) -> Result<Response<pb::DeleteGachaBannerResponse>, Status> {
    respond(
        async {
            let account = api.account(request.metadata()).await?;
            let req = request.into_inner();
            let sdb = api.with(&account, &req.server_id, Permission::ManageEconomy).await?.sdb;
            let gone = sdb
                .write(&account.id, async |conn, events| {
                    let banner = load_banner(conn, &sdb.id, &req.banner_id).await?;
                    conn.execute("DELETE FROM gacha_banners WHERE id = ?1", [banner.id.as_str()]).await?;
                    conn.execute("DELETE FROM gacha_progress WHERE banner_id = ?1", [banner.id.as_str()]).await?;
                    let entry =
                        Audit::new(pb::AuditAction::GachaBannerDelete, &banner.id).change("name", &banner.name, "");
                    store::audit(conn, &account.id, entry).await?;
                    banners_changed(conn, &sdb.id, events).await?;
                    Ok(banner)
                })
                .await?;
            api.drop_picture(&gone.art_url, "", PictureOwner::Server(&sdb.id)).await;
            Ok(pb::DeleteGachaBannerResponse {})
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banner() -> pb::GachaBanner {
        let tier = |id: &str, rate| pb::GachaTier { id: id.into(), name: id.into(), color: None, rate };
        let prize = |id: &str, tier: &str, weight| pb::GachaPrize {
            id: id.into(),
            tier_id: tier.into(),
            currency_id: "c".into(),
            amount: 1,
            weight,
            ..Default::default()
        };
        pb::GachaBanner {
            tiers: vec![tier("ssr", 10_000), tier("sr", 90_000), tier("r", 900_000)],
            prizes: vec![prize("a", "ssr", 1), prize("b", "sr", 1), prize("c", "sr", 3), prize("d", "r", 1)],
            ..Default::default()
        }
    }

    #[test]
    fn chances_share_a_tier_by_weight() {
        let chances: Vec<i32> = with_chances(banner()).prizes.iter().map(|p| p.chance).collect();
        assert_eq!(chances, vec![10_000, 22_500, 67_500, 900_000]);
    }

    #[test]
    fn rolls_stay_below_and_cover_everything() {
        let mut seen = [false; 3];
        for _ in 0..1000 {
            let r = roll(3).unwrap();
            assert!((0..3).contains(&r));
            seen[r as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
        assert!(roll(0).is_err());
    }

    #[test]
    fn picking_counts_up_by_weight() {
        let items = [("a", 1), ("b", 3), ("c", 6)];
        assert_eq!(pick(&items, 0), Some(&"a"));
        assert_eq!(pick(&items, 1), Some(&"b"));
        assert_eq!(pick(&items, 3), Some(&"b"));
        assert_eq!(pick(&items, 4), Some(&"c"));
        assert_eq!(pick(&items, 9), Some(&"c"));
        assert_eq!(pick(&items, 10), None);
    }

    #[test]
    fn prizes_that_cant_drop_give_their_chance_to_the_rest() {
        let b = banner();
        // The SSR tier's only prize is out: the other tiers share the roll.
        let draw = candidates(&b, |p| p.id != "a", None);
        assert_eq!(draw.tiers, vec![(1, 90_000), (2, 900_000)]);
        assert_eq!(draw.prizes[&1], vec![(1, 1), (2, 3)]);
        assert!(!draw.guaranteed);
    }

    #[test]
    fn pity_draws_from_its_tier_and_rarer() {
        let b = banner();
        let draw = candidates(&b, |_| true, Some(1));
        assert_eq!(draw.tiers, vec![(0, 10_000), (1, 90_000)]);
        assert!(draw.guaranteed);
        // Nothing left at that rarity: an ordinary draw.
        let draw = candidates(&b, |p| p.tier_id == "r", Some(1));
        assert_eq!(draw.tiers, vec![(2, 900_000)]);
        assert!(!draw.guaranteed);
    }
}
