//! A server's AutoMod rules (`AutoModService`), and acting on what they catch
//! when a message is sent or edited.

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, respond, text};
use crate::automod::{self, providers};
use crate::error::{Error, Result};
use crate::id::{millis, new_id, now_ms, timestamp};
use crate::pb::{
    self, AutoModActionKind as Kind, AutoModLevel as Level, AutoModTrigger as Trigger, Permission,
    auto_mod_service_server::AutoModService,
};
use crate::permissions::Access;
use crate::servers::{self as store, Audit, Payload, UsageChange, load_channel};

/// As many of each kind as Discord allows.
const MAX_KEYWORD_RULES: usize = 6;
const MAX_KEYWORDS: usize = 1000;
const MAX_KEYWORD: usize = 60;
const MAX_ALLOWED: usize = 100;
const MAX_EXEMPT_ROLES: usize = 20;
const MAX_EXEMPT_CHANNELS: usize = 50;
const MAX_BLOCK_MESSAGE: usize = 150;
const MAX_MENTION_LIMIT: i32 = 50;
const MIN_TIME_OUT: i32 = 60;
const MAX_TIME_OUT: i32 = 28 * 24 * 60 * 60;
/// How long a provider rule times people out unless it says.
const DEFAULT_TIME_OUT: i32 = 10 * 60;

/// What a rule is called when nobody names it.
fn default_name(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Keywords => "Blocked words",
        Trigger::MentionSpam => "Mention spam",
        Trigger::Links => "Links",
        Trigger::Provider => "Smart filter",
        Trigger::Unspecified => "Rule",
    }
}

/// A list as it may be saved: trimmed, lowercase, once each, nothing blank.
fn checked_list(what: &str, list: &[String], max_items: usize, max_len: usize) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let item = item.trim().to_lowercase();
        if item.is_empty() || out.contains(&item) {
            continue;
        }
        if item.chars().count() > max_len {
            return Err(Error::invalid(format!("{what} can be at most {max_len} characters each")));
        }
        out.push(item);
    }
    if out.len() > max_items {
        return Err(Error::invalid(format!("a rule can have up to {max_items} {what}")));
    }
    Ok(out)
}

/// A provider rule's labels as they may be saved: each one fuwa knows, once,
/// with a level and a threshold. Labels left out start at their defaults, so
/// a rule with none is the provider's default set.
fn checked_labels(labels: &[pb::AutoModLabelRule]) -> Result<Vec<pb::AutoModLabelRule>> {
    let mut out: Vec<pb::AutoModLabelRule> = Vec::new();
    for given in labels {
        let label =
            providers::label(given.label.trim()).ok_or_else(|| Error::invalid("that isn't a label fuwa knows"))?;
        if out.iter().any(|l| l.label == label.id) {
            return Err(Error::invalid("a rule sets each label once"));
        }
        let level = match Level::try_from(given.level) {
            Ok(Level::Unspecified) | Err(_) => Level::Off,
            Ok(level) => level,
        };
        let threshold = match given.threshold {
            0 => automod::DEFAULT_THRESHOLD,
            t if (50..=99).contains(&t) => t,
            _ => return Err(Error::invalid("a label's threshold is 50 to 99 percent")),
        };
        out.push(pb::AutoModLabelRule { label: label.id.into(), level: level as i32, threshold });
    }
    for label in providers::LABELS {
        if !out.iter().any(|l| l.label == label.id) {
            out.push(pb::AutoModLabelRule {
                label: label.id.into(),
                level: label.default_level as i32,
                threshold: automod::DEFAULT_THRESHOLD,
            });
        }
    }
    out.sort_by_key(|l| providers::LABELS.iter().position(|label| label.id == l.label));
    Ok(out)
}

/// A rule as it may be saved, checked against the server it's for. Fields
/// that don't belong to its trigger are cleared.
async fn checked_rule(
    conn: &turso::Connection,
    server_id: &str,
    settings: &crate::settings::Settings,
    rule: pb::AutoModRule,
) -> Result<pb::AutoModRule> {
    let trigger = match Trigger::try_from(rule.trigger) {
        Ok(trigger @ (Trigger::Keywords | Trigger::MentionSpam | Trigger::Links | Trigger::Provider)) => trigger,
        _ => return Err(Error::invalid("a rule looks for keywords, mention spam or links, or asks a provider")),
    };
    let name = match rule.name.trim() {
        "" => default_name(trigger).to_string(),
        name => text("a rule's name", name, 1, 100)?,
    };
    let (keywords, allowed, mention_limit) = match trigger {
        Trigger::Keywords => {
            let keywords = checked_list("keywords", &rule.keywords, MAX_KEYWORDS, MAX_KEYWORD)?;
            if keywords.iter().all(|k| k.trim_matches('*').is_empty()) {
                return Err(Error::invalid("add a word or phrase for the rule to look for"));
            }
            (keywords, checked_list("allowed words", &rule.allowed, MAX_ALLOWED, MAX_KEYWORD)?, 0)
        }
        Trigger::MentionSpam => {
            if !(1..=MAX_MENTION_LIMIT).contains(&rule.mention_limit) {
                return Err(Error::invalid(format!("the ping limit is 1 to {MAX_MENTION_LIMIT}")));
            }
            (vec![], vec![], rule.mention_limit)
        }
        Trigger::Provider => (vec![], vec![], 0),
        _ => {
            let sites: Vec<String> = rule.allowed.iter().map(|s| automod::site(s)).collect();
            if sites.iter().any(|s| !s.is_empty() && !s.contains('.')) {
                return Err(Error::invalid("allowed sites look like example.com"));
            }
            (vec![], checked_list("allowed sites", &sites, MAX_ALLOWED, 253)?, 0)
        }
    };
    let (provider, labels) = match trigger {
        Trigger::Provider => {
            let provider = rule.provider.trim();
            let name = match providers::kind(provider) {
                Some(kind) => kind.name.to_string(),
                None => settings
                    .automod_providers
                    .iter()
                    .find(|s| s.is_custom() && s.id == provider)
                    .map(|s| s.name().to_string())
                    .ok_or_else(|| Error::invalid("pick a provider for the rule"))?,
            };
            // A rule already on keeps its provider when admins switch it off
            // (it then lets messages through); a new or changed one needs it.
            if rule.enabled && settings.automod_provider(provider).is_none() {
                return Err(Error::FailedPrecondition(format!(
                    "{name} isn't turned on for this instance; its admins set providers up"
                )));
            }
            (provider.to_string(), checked_labels(&rule.labels)?)
        }
        _ => (String::new(), vec![]),
    };
    // Only providers that read pictures are shown them.
    let pictures =
        trigger == Trigger::Provider && rule.pictures && (providers::kind(&provider).is_some_and(|kind| kind.pictures));
    let mut rule = rule;
    if trigger == Trigger::Provider {
        let highest = labels.iter().map(|l| l.level).max().unwrap_or_default();
        let has = |kind: Kind| rule.actions.iter().any(|a| a.kind == kind as i32);
        if labels.iter().any(|l| l.level == Level::Flag as i32) && !has(Kind::Alert) {
            return Err(Error::invalid("pick a channel for flagged messages"));
        }
        if highest == Level::TimeOut as i32 && !has(Kind::TimeOut) {
            rule.actions.push(pb::AutoModAction {
                kind: Kind::TimeOut as i32,
                duration_seconds: DEFAULT_TIME_OUT,
                ..Default::default()
            });
        }
    } else if rule.actions.is_empty() {
        return Err(Error::invalid("a rule needs something to do: block, alert or time out"));
    }
    let mut actions: Vec<pb::AutoModAction> = Vec::new();
    for action in rule.actions {
        let kind = Kind::try_from(action.kind).unwrap_or(Kind::Unspecified);
        if actions.iter().any(|a| a.kind == action.kind) {
            return Err(Error::invalid("a rule does each thing once"));
        }
        actions.push(match kind {
            Kind::Block => pb::AutoModAction {
                kind: action.kind,
                message: text("the blocked message's note", &action.message, 0, MAX_BLOCK_MESSAGE)?,
                ..Default::default()
            },
            Kind::Alert => {
                let channel = load_channel(conn, server_id, &action.channel_id)
                    .await?
                    .ok_or_else(|| Error::invalid("pick a channel for alerts"))?;
                if !matches!(
                    pb::ChannelType::try_from(channel.r#type),
                    Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                ) {
                    return Err(Error::invalid("alerts go in a text channel"));
                }
                if channel.shared.as_ref().is_some_and(|s| !s.home) {
                    return Err(Error::invalid("alerts can't go in a channel shared from another server"));
                }
                pb::AutoModAction { kind: action.kind, channel_id: channel.id, ..Default::default() }
            }
            Kind::TimeOut => {
                if !(MIN_TIME_OUT..=MAX_TIME_OUT).contains(&action.duration_seconds) {
                    return Err(Error::invalid("time-outs last a minute to 28 days"));
                }
                pb::AutoModAction { kind: action.kind, duration_seconds: action.duration_seconds, ..Default::default() }
            }
            Kind::Unspecified => return Err(Error::invalid("a rule blocks, alerts or times out")),
        });
    }
    let roles = crate::permissions::roles(conn, server_id).await?;
    let mut exempt_role_ids: Vec<String> = Vec::new();
    for id in &rule.exempt_role_ids {
        let role = roles.iter().find(|r| &r.id == id && r.id != server_id).ok_or(Error::NotFound("role"))?;
        if !exempt_role_ids.contains(&role.id) {
            exempt_role_ids.push(role.id.clone());
        }
    }
    let mut exempt_channel_ids: Vec<String> = Vec::new();
    for id in &rule.exempt_channel_ids {
        let channel = load_channel(conn, server_id, id).await?.ok_or(Error::NotFound("channel"))?;
        if !exempt_channel_ids.contains(&channel.id) {
            exempt_channel_ids.push(channel.id);
        }
    }
    if exempt_role_ids.len() > MAX_EXEMPT_ROLES || exempt_channel_ids.len() > MAX_EXEMPT_CHANNELS {
        return Err(Error::invalid(format!(
            "a rule leaves out up to {MAX_EXEMPT_ROLES} roles and {MAX_EXEMPT_CHANNELS} channels"
        )));
    }
    Ok(pb::AutoModRule {
        id: rule.id,
        server_id: server_id.to_string(),
        name,
        enabled: rule.enabled,
        trigger: trigger as i32,
        keywords,
        allowed,
        mention_limit,
        actions,
        exempt_role_ids,
        exempt_channel_ids,
        creator_id: String::new(),
        created_at: None,
        updated_at: None,
        provider,
        labels,
        pictures,
    })
}

/// What a server's provider rule's provider said about a message.
pub(super) struct Asked {
    rule_id: String,
    provider: String,
    scores: providers::Scores,
}

/// A provider's answer on its way: `None` when it didn't answer (counted in
/// the anonymous report), and the message goes through that rule unchecked.
type Answer = futures::future::Shared<futures::future::BoxFuture<'static, Option<providers::Scores>>>;

/// A message being checked by its server's provider rule.
pub(super) struct Checking {
    rule_id: String,
    provider: String,
    answer: Answer,
}

/// Checks being asked right now, by server, provider and what they ask, so
/// the same message sent again and again (a raid, a spammer) is asked once.
/// The key is the whole of what's asked, compared in full, so no message
/// can pass for another.
type Asking = std::collections::HashMap<(String, String, String, Vec<String>), Answer>;

static ASKING: std::sync::LazyLock<std::sync::Mutex<Asking>> = std::sync::LazyLock::new(Default::default);

fn asking() -> std::sync::MutexGuard<'static, Asking> {
    ASKING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Asks the provider of the server's provider rule about a message, when the
/// rule is on, its provider is set up and the message isn't left alone, and
/// waits for the answer. `pictures` are the links to the message's pictures
/// (attached ones and embeds'), read and shown only when the rule says so
/// and the provider reads pictures. `None` when nothing was asked or the
/// provider didn't answer: the message then goes through that rule
/// unchecked.
#[allow(clippy::too_many_arguments)]
pub(super) async fn ask(
    app: &std::sync::Arc<crate::app::App>,
    sdb: &store::ServerDb,
    member: &pb::Member,
    access: &Access,
    channel_id: &str,
    content: &str,
    pictures: &[String],
) -> Option<Asked> {
    start(app, sdb, member, access, channel_id, content, pictures).await?.wait().await
}

/// Starts asking, as [`ask`] does, without waiting for the answer.
#[allow(clippy::too_many_arguments)]
pub(super) async fn start(
    app: &std::sync::Arc<crate::app::App>,
    sdb: &store::ServerDb,
    member: &pb::Member,
    access: &Access,
    channel_id: &str,
    content: &str,
    pictures: &[String],
) -> Option<Checking> {
    if access.has(Permission::ManageServer) || (content.trim().is_empty() && pictures.is_empty()) {
        return None;
    }
    let conn = sdb.read().ok()?;
    let rules = store::load_automod(&conn).await.ok()?;
    let rule = rules.into_iter().find(|r| r.enabled && r.trigger == Trigger::Provider as i32)?;
    if rule.labels.iter().all(|l| l.level < Level::Flag as i32)
        || rule.exempt_role_ids.iter().any(|id| member.role_ids.contains(id))
    {
        return None;
    }
    let channel = load_channel(&conn, &sdb.id, channel_id).await.ok()??;
    if rule.exempt_channel_ids.iter().any(|id| *id == channel.id || *id == channel.parent_id) {
        return None;
    }
    drop(conn);
    let setup = app.settings().automod_provider(&rule.provider)?.clone();
    let provider = setup.name().to_string();
    let pictures: Vec<String> = if rule.pictures && setup.reads_pictures() { pictures.to_vec() } else { vec![] };
    if content.trim().is_empty() && pictures.is_empty() {
        return None;
    }
    let key = (sdb.id.clone(), setup.id.clone(), content.to_string(), pictures.clone());
    let checking = |answer| Some(Checking { rule_id: rule.id.clone(), provider: provider.clone(), answer });
    if let Some(answer) = asking().get(&key).cloned() {
        return checking(answer);
    }
    if let Check::Capped { per_day, first } = take_check(&sdb.id, app.settings().limits.automod_checks_per_day) {
        crate::reports::server_error("automod_provider_capped", Some(setup.report_id()));
        if first {
            alert_capped(sdb, per_day).await;
        }
        return None;
    }
    // Asked in its own task, so it finishes (and frees its turn) even when
    // whoever sent the message goes away.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let answer: Answer = futures::FutureExt::shared(
        Box::pin(async move { rx.await.ok().flatten() }) as futures::future::BoxFuture<'static, _>
    );
    asking().insert(key.clone(), answer.clone());
    let (app, content) = (app.clone(), content.to_string());
    tokio::spawn(async move {
        let pictures = read_pictures(&app, &pictures).await;
        let answer = providers::check(&setup, &content, &pictures).await.0.ok();
        asking().remove(&key);
        let _ = tx.send(answer);
    });
    checking(answer)
}

/// Starts asking, as [`ask`] does, without waiting: the message is sent at
/// once, and [`Checking::later`] acts on the answer when it comes.
#[allow(clippy::too_many_arguments)]
pub(super) async fn ask_after(
    app: &std::sync::Arc<crate::app::App>,
    sdb: &store::ServerDb,
    member: &pb::Member,
    access: &Access,
    channel_id: &str,
    content: &str,
    pictures: &[String],
) -> Option<Checking> {
    start(app, sdb, member, access, channel_id, content, pictures).await
}

impl Checking {
    /// Waits for the answer.
    pub(super) async fn wait(self) -> Option<Asked> {
        let scores = self.answer.await?;
        Some(Asked { rule_id: self.rule_id, provider: self.provider, scores })
    }

    /// Checks a message already sent when the answer comes, in its own task:
    /// AutoMod then does what the rule says, as it would have before the
    /// message was sent, and takes the message down if the rule blocks it.
    /// A message edited or deleted in the meantime is left alone (an edit
    /// is checked again).
    pub(super) fn later(
        self,
        sdb: std::sync::Arc<store::ServerDb>,
        member: pb::Member,
        message_id: String,
        content: String,
    ) {
        tokio::spawn(async move {
            let Some(asked) = self.wait().await else { return };
            let reviewed = sdb
                .write("", async |conn, events| {
                    review_sent(conn, &sdb.id, &member, &message_id, &content, &asked, events).await
                })
                .await;
            if reviewed.is_err() {
                crate::reports::server_error("automod_late_review_failed", None);
            }
        });
    }
}

/// Today's Smart filter checks: how many each server asked its provider,
/// and which servers were told they're used up. Counted where the server
/// lives, which is the one place its messages are sent from; kept in memory,
/// so a restart starts the day again.
#[derive(Default)]
struct Checks {
    /// Days since 1970, UTC.
    day: i64,
    used: std::collections::HashMap<String, i64>,
    alerted: std::collections::HashSet<String>,
}

static CHECKS: std::sync::LazyLock<std::sync::Mutex<Checks>> = std::sync::LazyLock::new(Default::default);

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Whether a server's Smart filter may ask its provider.
#[derive(Debug, PartialEq)]
enum Check {
    /// Yes, and it was counted.
    Taken,
    /// No: today's `per_day` checks are used up. `first` the first time
    /// that day, when its moderators get told.
    Capped { per_day: i64, first: bool },
}

/// Counts one of the server's checks for today, unless it already used
/// `per_day` of them (`None` for no cap): then nothing's asked.
fn take_check(server_id: &str, per_day: Option<i64>) -> Check {
    let today = now_ms().div_euclid(DAY_MS);
    let mut checks = CHECKS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if checks.day != today {
        *checks = Checks { day: today, ..Default::default() };
    }
    let used = checks.used.entry(server_id.to_string()).or_default();
    if let Some(cap) = per_day.filter(|cap| *used >= *cap) {
        let first = checks.alerted.insert(server_id.to_string());
        return Check::Capped { per_day: cap, first };
    }
    *used += 1;
    Check::Taken
}

/// Tells the server's moderators that its Smart filter used up today's
/// checks: one alert in its Smart filter rule's alert channel, when the rule
/// alerts. It names no message and no one; a failed write only skips it.
async fn alert_capped(sdb: &store::ServerDb, per_day: i64) {
    let posted = sdb
        .write("", async |conn, events| {
            let rules = store::load_automod(conn).await?;
            let Some(rule) = rules.into_iter().find(|r| r.enabled && r.trigger == Trigger::Provider as i32) else {
                return Ok(());
            };
            let Some(alert) = rule.actions.iter().find(|a| a.kind == Kind::Alert as i32) else { return Ok(()) };
            let Some(channel) = load_channel(conn, &sdb.id, &alert.channel_id).await? else { return Ok(()) };
            let message = pb::Message {
                id: new_id(),
                server_id: sdb.id.clone(),
                channel_id: channel.id.clone(),
                created_at: Some(timestamp(now_ms())),
                kind: pb::MessageKind::AutoModAlert as i32,
                auto_mod: Some(pb::AutoModAlert {
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    trigger: rule.trigger,
                    capped_per_day: per_day,
                    ..Default::default()
                }),
                ..Default::default()
            };
            super::messages::insert_system(conn, &message).await?;
            store::add_usage(conn, UsageChange { messages: 1, messages_sent: 1, ..Default::default() }).await?;
            events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message) }));
            Ok(())
        })
        .await;
    if posted.is_err() {
        crate::reports::server_error("automod_cap_alert_failed", None);
    }
}

/// How many checks the server's Smart filter asked its provider today.
pub(super) fn checks_today(server_id: &str) -> i64 {
    let checks = CHECKS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if checks.day != now_ms().div_euclid(DAY_MS) {
        return 0;
    }
    checks.used.get(server_id).copied().unwrap_or_default()
}

/// Who a server's enabled provider rule sends a channel's messages to, as
/// "Name (host)", for people about to write there who aren't its members
/// (the shared channel preview). Empty when no provider reads that channel.
pub(super) async fn readers(
    app: &crate::app::App,
    conn: &turso::Connection,
    channel: &pb::Channel,
) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for rule in store::load_automod(conn).await? {
        if !rule.enabled
            || rule.trigger != Trigger::Provider as i32
            || rule.labels.iter().all(|l| l.level < Level::Flag as i32)
            || rule.exempt_channel_ids.iter().any(|id| *id == channel.id || *id == channel.parent_id)
        {
            continue;
        }
        if let Some(setup) = app.settings().automod_provider(&rule.provider) {
            let name = format!("{} ({})", setup.name(), setup.host());
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    Ok(names)
}

/// How long a message waits for its pictures to be read before its provider
/// is asked without the ones still missing.
const PICTURES_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The pictures at `links` a provider can read, in order, at most
/// [`providers::MAX_PICTURES`] and [`providers::MAX_PICTURES_BYTES`] in all.
/// Ones that can't be fetched in [`PICTURES_WAIT`], or aren't a PNG, JPEG or
/// WebP within the provider's limits, are left out; one that couldn't be
/// fetched is counted in the anonymous report.
async fn read_pictures(app: &crate::app::App, links: &[String]) -> Vec<providers::Picture> {
    let mut wanted: Vec<&str> = Vec::new();
    for link in links {
        if !link.is_empty() && !wanted.contains(&link.as_str()) {
            wanted.push(link);
        }
    }
    wanted.truncate(providers::MAX_PICTURES);
    if wanted.is_empty() {
        return vec![];
    }
    let started = std::time::Instant::now();
    let fetched = futures::future::join_all(
        wanted.iter().map(|link| tokio::time::timeout(PICTURES_WAIT, crate::outside::picture(app, link))),
    )
    .await;
    crate::reports::server_timing("automod:pictures", started.elapsed());
    let mut read = Vec::new();
    let mut total = 0;
    for found in fetched {
        let Ok(Some((_, bytes))) = found else {
            crate::reports::server_error("automod_picture_unavailable", None);
            continue;
        };
        if let Some(picture) = providers::Picture::read(bytes)
            && total + picture.bytes.len() <= providers::MAX_PICTURES_BYTES
        {
            total += picture.bytes.len();
            read.push(picture);
        }
    }
    read
}

/// The links to a message's pictures a provider may be shown: attachments
/// that say they're PNG, JPEG or WebP (by type or name), then embeds' images
/// and thumbnails.
pub(super) fn picture_links(attachments: &[pb::Attachment], embeds: &[pb::Embed]) -> Vec<String> {
    let readable = |a: &pb::Attachment| {
        let kind = a.content_type.to_ascii_lowercase();
        let name = a.filename.to_ascii_lowercase();
        matches!(kind.as_str(), "image/png" | "image/jpeg" | "image/jpg" | "image/webp")
            || [".png", ".jpg", ".jpeg", ".webp"].iter().any(|end| name.ends_with(end))
    };
    attachments
        .iter()
        .filter(|a| readable(a))
        .map(|a| a.url.clone())
        .chain(embeds.iter().flat_map(|e| [e.image_url.clone(), e.thumbnail_url.clone()]))
        .filter(|link| !link.is_empty())
        .collect()
}

/// What a caught rule does: its own actions, or for a provider rule the ones
/// the strongest label it reached calls for.
fn effective(rule: &pb::AutoModRule, level: Option<Level>) -> pb::AutoModRule {
    let Some(level) = level else { return rule.clone() };
    let mut rule = rule.clone();
    rule.actions.retain(|action| match Kind::try_from(action.kind).unwrap_or(Kind::Unspecified) {
        Kind::Alert => true,
        Kind::Block => level as i32 >= Level::Block as i32,
        Kind::TimeOut => level == Level::TimeOut,
        Kind::Unspecified => false,
    });
    if level as i32 >= Level::Block as i32 && !rule.actions.iter().any(|a| a.kind == Kind::Block as i32) {
        rule.actions.push(pb::AutoModAction { kind: Kind::Block as i32, ..Default::default() });
    }
    rule
}

/// How many of a server's rules look for `trigger`, besides the rule `except`.
fn count(rules: &[pb::AutoModRule], trigger: Trigger, except: &str) -> usize {
    rules.iter().filter(|r| r.trigger == trigger as i32 && r.id != except).count()
}

/// What AutoMod did about a message.
#[derive(Debug, Default)]
pub(super) struct Verdict {
    /// Why it wasn't sent, for its author; `None` when it was.
    pub blocked: Option<String>,
}

/// Runs a server's rules over a message about to be sent or saved, inside
/// that write: posts alerts and times its author out as the rules say. A
/// provider rule goes by what `ask` brought back before the write. The
/// caller doesn't save the message when it comes back blocked.
#[allow(clippy::too_many_arguments)]
pub(super) async fn review(
    conn: &turso::Connection,
    server_id: &str,
    member: &pb::Member,
    access: &Access,
    channel: &pb::Channel,
    content: &str,
    asked: Option<&Asked>,
    events: &mut Vec<Payload>,
) -> Result<Verdict> {
    // Managers and administrators are trusted, as on Discord. A message
    // with no text (pictures only) is still up to its provider's answer.
    if access.has(Permission::ManageServer) || (content.trim().is_empty() && asked.is_none()) {
        return Ok(Verdict::default());
    }
    let rules = store::load_automod(conn).await?;
    let mut caught: Vec<(pb::AutoModRule, automod::Hit)> = Vec::new();
    for rule in rules {
        let exempt = !rule.enabled
            || rule.exempt_channel_ids.iter().any(|id| *id == channel.id || *id == channel.parent_id)
            || rule.exempt_role_ids.iter().any(|id| member.role_ids.contains(id));
        if exempt {
            continue;
        }
        if rule.trigger == Trigger::Provider as i32 {
            let answer = asked.filter(|a| a.rule_id == rule.id);
            if let Some((level, hit)) = answer.and_then(|a| automod::provider_hit(&rule, &a.scores)) {
                caught.push((effective(&rule, Some(level)), hit));
            }
        } else if let Some(hit) = automod::check(&rule, content) {
            caught.push((rule, hit));
        }
    }
    let blocked = act(conn, server_id, member, channel, content, asked, &caught, events).await?;
    Ok(Verdict { blocked })
}

/// Does what the rules that `caught` a message say: works out what its
/// author is told when one blocks it (returned), times them out and posts
/// the alerts.
#[allow(clippy::too_many_arguments)]
async fn act(
    conn: &turso::Connection,
    server_id: &str,
    member: &pb::Member,
    channel: &pb::Channel,
    content: &str,
    asked: Option<&Asked>,
    caught: &[(pb::AutoModRule, automod::Hit)],
    events: &mut Vec<Payload>,
) -> Result<Option<String>> {
    if caught.is_empty() {
        return Ok(None);
    }
    let author_id = member.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let action = |rule: &pb::AutoModRule, kind: Kind| rule.actions.iter().find(|a| a.kind == kind as i32).cloned();
    let blocked = caught.iter().find_map(|(rule, _)| action(rule, Kind::Block)).map(|block| {
        if block.message.is_empty() {
            "AutoMod: this server doesn't allow that message".to_string()
        } else {
            format!("AutoMod: {}", block.message)
        }
    });
    let now = now_ms();
    let time_out = caught.iter().filter_map(|(rule, _)| action(rule, Kind::TimeOut)).map(|a| a.duration_seconds).max();
    if let Some(seconds) = time_out {
        let until = now + i64::from(seconds) * 1000;
        let current = member.timed_out_until.as_ref().map_or(0, millis);
        if until > current {
            conn.execute("UPDATE members SET timed_out_until = ?2 WHERE user_id = ?1", (author_id.as_str(), until))
                .await?;
            let mut updated = member.clone();
            updated.timed_out_until = Some(timestamp(until));
            events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(updated) }));
            let rule = caught.iter().find(|(r, _)| action(r, Kind::TimeOut).is_some()).map(|(r, _)| r.name.as_str());
            let entry = Audit::new(pb::AuditAction::AutoModTimeOut, &author_id)
                .channel(channel.name.clone())
                .reason(rule.unwrap_or_default())
                .change("timed_out_until", current, until);
            store::audit(conn, &author_id, entry).await?;
        }
    }
    for (rule, hit) in caught {
        let Some(alert) = action(rule, Kind::Alert) else { continue };
        let Some(alert_channel) = load_channel(conn, server_id, &alert.channel_id).await? else { continue };
        let message = pb::Message {
            id: new_id(),
            server_id: server_id.to_string(),
            channel_id: alert_channel.id.clone(),
            author_id: author_id.clone(),
            created_at: Some(timestamp(now)),
            kind: pb::MessageKind::AutoModAlert as i32,
            auto_mod: Some(pb::AutoModAlert {
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
                trigger: rule.trigger,
                channel_id: channel.id.clone(),
                content: content.to_string(),
                matched: hit.matched.clone(),
                blocked: blocked.is_some(),
                timed_out_seconds: action(rule, Kind::TimeOut).map_or(0, |a| a.duration_seconds),
                provider: asked.filter(|a| a.rule_id == rule.id).map(|a| a.provider.clone()).unwrap_or_default(),
                capped_per_day: 0,
            }),
            ..Default::default()
        };
        super::messages::insert_system(conn, &message).await?;
        store::add_usage(conn, UsageChange { messages: 1, messages_sent: 1, ..Default::default() }).await?;
        events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message) }));
    }
    Ok(blocked)
}

/// [`review`] for a message already sent, with the answer its provider rule
/// gave after it went out (see [`Checking::later`]); the other rules saw it
/// before. Takes the message down when the rule blocks it.
async fn review_sent(
    conn: &turso::Connection,
    server_id: &str,
    member: &pb::Member,
    message_id: &str,
    content: &str,
    asked: &Asked,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let Some(message) = super::messages::load_message(conn, server_id, message_id).await? else { return Ok(()) };
    if super::messages::reviewed_text(&message) != content {
        return Ok(());
    }
    let Some(channel) = load_channel(conn, server_id, &message.channel_id).await? else { return Ok(()) };
    let rules = store::load_automod(conn).await?;
    let Some(rule) = rules.into_iter().find(|r| r.id == asked.rule_id && r.enabled) else { return Ok(()) };
    let exempt = rule.exempt_channel_ids.iter().any(|id| *id == channel.id || *id == channel.parent_id)
        || rule.exempt_role_ids.iter().any(|id| member.role_ids.contains(id));
    let Some((level, hit)) = automod::provider_hit(&rule, &asked.scores).filter(|_| !exempt) else { return Ok(()) };
    let caught = [(effective(&rule, Some(level)), hit)];
    if act(conn, server_id, member, &channel, content, Some(asked), &caught, events).await?.is_some() {
        super::messages::remove_message(conn, &message).await?;
        let author_id = member.user.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
        let entry = Audit::new(pb::AuditAction::AutoModMessageDelete, author_id)
            .channel(channel.name.clone())
            .reason(rule.name.clone());
        store::audit(conn, author_id, entry).await?;
        events.push(Payload::MessageDeleted(pb::MessageDeleted {
            channel_id: message.channel_id.clone(),
            message_id: message.id.clone(),
        }));
        super::threads::after_delete(conn, &message.channel_id, &message.id, &message.thread_id, events).await?;
    }
    Ok(())
}

#[tonic::async_trait]
impl AutoModService for Api {
    async fn list_auto_mod_rules(
        &self,
        request: Request<pb::ListAutoModRulesRequest>,
    ) -> Result<Response<pb::ListAutoModRulesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.with(&account, &request.get_ref().server_id, Permission::ManageServer).await?.sdb;
                let providers = self
                    .app
                    .settings()
                    .automod_providers
                    .iter()
                    .filter(|setup| setup.usable())
                    .filter_map(|setup| setup.offer())
                    .collect();
                Ok(pb::ListAutoModRulesResponse { rules: store::load_automod(&sdb.read()?).await?, providers })
            }
            .await,
        )
    }

    async fn save_auto_mod_rule(
        &self,
        request: Request<pb::SaveAutoModRuleRequest>,
    ) -> Result<Response<pb::SaveAutoModRuleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let draft = req.rule.unwrap_or_default();
                let settings = self.app.settings();
                let rule = sdb
                    .write(&account.id, async |conn, _events| {
                        let mut rule = checked_rule(conn, &sdb.id, &settings, draft.clone()).await?;
                        let rules = store::load_automod(conn).await?;
                        let before = rules.iter().find(|r| r.id == rule.id).cloned();
                        if !rule.id.is_empty() && before.is_none() {
                            return Err(Error::NotFound("rule"));
                        }
                        let trigger = Trigger::try_from(rule.trigger).unwrap_or(Trigger::Unspecified);
                        let room = if trigger == Trigger::Keywords { MAX_KEYWORD_RULES } else { 1 };
                        if count(&rules, trigger, &rule.id) >= room {
                            return Err(Error::FailedPrecondition(match trigger {
                                Trigger::Keywords => {
                                    format!("a server can have up to {MAX_KEYWORD_RULES} keyword rules")
                                }
                                _ => "a server has one rule of this kind; change that one instead".into(),
                            }));
                        }
                        let now = now_ms();
                        let created_at = before.as_ref().and_then(|b| b.created_at).unwrap_or(timestamp(now));
                        rule.updated_at = Some(timestamp(now));
                        rule.created_at = Some(created_at);
                        rule.creator_id = before.as_ref().map_or_else(|| account.id.clone(), |b| b.creator_id.clone());
                        let action = match &before {
                            Some(_) => pb::AuditAction::AutoModRuleUpdate,
                            None => {
                                rule.id = new_id();
                                pb::AuditAction::AutoModRuleCreate
                            }
                        };
                        conn.execute(
                            "INSERT INTO automod_rules (id, rule, created_at) VALUES (?1, ?2, ?3)
                             ON CONFLICT (id) DO UPDATE SET rule = excluded.rule",
                            (rule.id.as_str(), rule.encode_to_vec(), millis(&created_at)),
                        )
                        .await?;
                        // Keywords stay between managers: the entry says what changed, never which words.
                        let mut entry = Audit::new(action, &rule.id).change(
                            "name",
                            before.as_ref().map(|b| b.name.as_str()).unwrap_or_default(),
                            &rule.name,
                        );
                        if let Some(before) = &before {
                            entry = entry
                                .change("enabled", before.enabled, rule.enabled)
                                .change("keywords", before.keywords.len(), rule.keywords.len())
                                .change("allowed", before.allowed.len(), rule.allowed.len())
                                .change("mention_limit", before.mention_limit, rule.mention_limit)
                                .change("actions", before.actions.len(), rule.actions.len())
                                .change("provider", &before.provider, &rule.provider);
                        }
                        store::audit(conn, &account.id, entry).await?;
                        Ok(rule)
                    })
                    .await?;
                Ok(pb::SaveAutoModRuleResponse { rule: Some(rule) })
            }
            .await,
        )
    }

    async fn delete_auto_mod_rule(
        &self,
        request: Request<pb::DeleteAutoModRuleRequest>,
    ) -> Result<Response<pb::DeleteAutoModRuleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                sdb.write(&account.id, async |conn, _events| {
                    let rules = store::load_automod(conn).await?;
                    let rule = rules.iter().find(|r| r.id == req.rule_id).ok_or(Error::NotFound("rule"))?;
                    conn.execute("DELETE FROM automod_rules WHERE id = ?1", [rule.id.as_str()]).await?;
                    let entry = Audit::new(pb::AuditAction::AutoModRuleDelete, &rule.id).change("name", &rule.name, "");
                    store::audit(conn, &account.id, entry).await?;
                    Ok(())
                })
                .await?;
                Ok(pb::DeleteAutoModRuleResponse {})
            }
            .await,
        )
    }

    async fn test_auto_mod_rule(
        &self,
        request: Request<pb::TestAutoModRuleRequest>,
    ) -> Result<Response<pb::TestAutoModRuleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                if req.content.chars().count() > super::messages::MAX_MESSAGE_LENGTH {
                    return Err(Error::invalid("that's longer than a message can be"));
                }
                let rule = req.rule.unwrap_or_default();
                if rule.trigger == Trigger::Provider as i32 {
                    let settings = self.app.settings();
                    let setup = settings.automod_provider(rule.provider.trim()).ok_or_else(|| {
                        Error::FailedPrecondition("that provider isn't turned on for this instance".into())
                    })?;
                    let rule = pb::AutoModRule { labels: checked_labels(&rule.labels)?, ..rule };
                    if let Check::Capped { per_day, first } =
                        take_check(&sdb.id, settings.limits.automod_checks_per_day)
                    {
                        crate::reports::server_error("automod_provider_capped", Some(setup.report_id()));
                        if first {
                            alert_capped(&sdb, per_day).await;
                        }
                        return Ok(pb::TestAutoModRuleResponse {
                            error: format!(
                                "today's {per_day} Smart filter checks are used up, so it wasn't asked (they come back at midnight UTC)"
                            ),
                            ..Default::default()
                        });
                    }
                    let (answer, took) = providers::check(setup, &req.content, &[]).await;
                    let elapsed_ms = took.as_millis().min(i32::MAX as u128) as i32;
                    return Ok(match answer {
                        Ok(scores) => {
                            let hit = automod::provider_hit(&rule, &scores).map(|(_, hit)| hit);
                            pb::TestAutoModRuleResponse {
                                matched: hit.is_some(),
                                matches: hit.map(|h| h.matched).unwrap_or_default(),
                                error: String::new(),
                                elapsed_ms,
                            }
                        }
                        Err(failure) => {
                            pb::TestAutoModRuleResponse { error: failure.to_string(), elapsed_ms, ..Default::default() }
                        }
                    });
                }
                let hit = automod::check(&rule, &req.content);
                Ok(pb::TestAutoModRuleResponse {
                    matched: hit.is_some(),
                    matches: hit.map(|h| h.matched).unwrap_or_default(),
                    ..Default::default()
                })
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider answer that comes after the message went out still blocks
    /// it: down it comes, with an alert and a time out, as before sending.
    #[tokio::test]
    async fn late_answers_take_blocked_messages_down() {
        let dir = tempfile::tempdir().unwrap();
        let conn = store::scratch(&dir.path().join("s.db")).await;
        conn.execute_batch(
            "INSERT INTO channels (id, name, type, created_at, updated_at) VALUES ('general', 'general', 1, 0, 0);
             INSERT INTO channels (id, name, type, created_at, updated_at) VALUES ('mods', 'mods', 1, 0, 0);",
        )
        .await
        .unwrap();
        let rule = pb::AutoModRule {
            id: "r".into(),
            name: "Smart filter".into(),
            enabled: true,
            trigger: Trigger::Provider as i32,
            labels: vec![pb::AutoModLabelRule { label: "scam".into(), level: Level::TimeOut as i32, threshold: 80 }],
            actions: vec![
                pb::AutoModAction { kind: Kind::Alert as i32, channel_id: "mods".into(), ..Default::default() },
                pb::AutoModAction { kind: Kind::TimeOut as i32, duration_seconds: 600, ..Default::default() },
            ],
            ..Default::default()
        };
        conn.execute("INSERT INTO automod_rules (id, rule, created_at) VALUES ('r', ?1, 0)", [rule.encode_to_vec()])
            .await
            .unwrap();
        let member = pb::Member { user: Some(pb::User { id: "u".into(), ..Default::default() }), ..Default::default() };
        let send = async |id: &str, content: &str| {
            let message = pb::Message {
                id: id.into(),
                channel_id: "general".into(),
                author_id: "u".into(),
                content: content.into(),
                ..Default::default()
            };
            super::super::messages::insert_message(&conn, &message, now_ms()).await.unwrap();
        };
        let scam = Asked { rule_id: "r".into(), provider: "Cloudflare Clef".into(), scores: vec![("scam", 0.97)] };
        let fine = Asked { rule_id: "r".into(), provider: "Cloudflare Clef".into(), scores: vec![("scam", 0.02)] };
        let mut events = Vec::new();

        // Fine: nothing happens.
        send("m1", "hello").await;
        review_sent(&conn, "s", &member, "m1", "hello", &fine, &mut events).await.unwrap();
        assert!(events.is_empty());

        // Edited since it was asked about: the edit is checked on its own.
        send("m2", "free nitro").await;
        review_sent(&conn, "s", &member, "m2", "something else", &scam, &mut events).await.unwrap();
        assert!(events.is_empty());

        // Blocked: taken down, alerted and timed out.
        review_sent(&conn, "s", &member, "m2", "free nitro", &scam, &mut events).await.unwrap();
        assert!(super::super::messages::load_message(&conn, "s", "m2").await.unwrap().is_none());
        assert!(super::super::messages::load_message(&conn, "s", "m1").await.unwrap().is_some());
        let alert = events.iter().find_map(|e| match e {
            Payload::MessageCreated(pb::MessageCreated { message: Some(m) }) => m.auto_mod.clone(),
            _ => None,
        });
        let alert = alert.unwrap();
        assert!(alert.blocked && alert.content == "free nitro" && alert.provider == "Cloudflare Clef", "{alert:?}");
        assert!(events.iter().any(|e| matches!(e, Payload::MessageDeleted(d) if d.message_id == "m2")));
        assert!(events.iter().any(|e| matches!(e, Payload::MemberUpdated(_))));
        let logged: Vec<i32> =
            crate::db::query_all(&conn, "SELECT action FROM audit", (), |r| r.get::<i32>(0)).await.unwrap();
        assert!(logged.contains(&(pb::AuditAction::AutoModMessageDelete as i32)), "{logged:?}");
    }
}
