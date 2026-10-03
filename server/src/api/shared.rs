//! Channels shared between community servers (docs/shared-channels.md).
//!
//! A shared channel has one home: the server it was made in keeps the
//! channel and every message in it. Each guest server shows it as a channel
//! of its own (a row in `channel_links`) and keeps nothing said there: its
//! shard checks what its people may do, by its own roles and overwrites and
//! its own AutoMod, and passes reads and writes on to the home as
//! `cpb::SharedCall`s. The home checks the connection (`channel_guests`),
//! what it allows, who it keeps out (`channel_blocks`), slow mode and its own
//! AutoMod, and passes what happens in the channel back to each guest
//! ([`spawn_shared_fanout`]), which shows it to its people live and stores
//! none of it.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tonic::{Code, Request, Response, Status};

use super::messages::{self, check_slowmode, load_message};
use super::{Api, Seat, automod, respond, users};
use crate::app::App;
use crate::cpb::{self, shared_call::Call};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, parse_id, timestamp};
use crate::node::Account;
use crate::pb::{self, Permission, shared_channel_service_server::SharedChannelService};
use crate::permissions::{self, Access, Bits, bit};
use crate::servers::{self as store, Audit, Payload, ServerDb, load_channel};

/// What a guest server's people may be let do in a shared channel, at most;
/// the home picks which. Seeing it comes with being shown it.
pub const SHAREABLE: Bits = bit(Permission::SendMessages) | bit(Permission::EmbedLinks) | bit(Permission::AttachFiles);
/// How long a share code works.
const CODE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// How many servers one channel is shown in besides its home, for now.
const MAX_GUESTS: usize = 1;
/// Letters and digits that read the same in any font, as invite codes use.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
const CODE_LENGTH: usize = 16;
/// What a guest the home keeps out of a channel is told.
const KEPT_OUT: &str = "this channel's home server has kept you out of it";
/// What a connection that's gone answers, so the other end can let go too.
const GONE: &str = "shared channel";

// ─────────────── What each side keeps ───────────────

/// Home: a server one of its channels is shown in, or asked to be.
#[derive(Clone, Debug)]
struct GuestRow {
    id: String,
    channel_id: String,
    server: pb::SharedServer,
    active: bool,
    allowed: Bits,
    created_at: i64,
}

const GUEST_COLUMNS: &str = "id, channel_id, guest_server_id, guest_name, guest_icon_url, active, allowed, created_at";

fn guest_row(r: &turso::Row) -> turso::Result<GuestRow> {
    Ok(GuestRow {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        server: pb::SharedServer { id: r.get(2)?, name: r.get(3)?, icon_url: r.get(4)? },
        active: r.get::<i64>(5)? != 0,
        allowed: r.get::<i64>(6)? as Bits,
        created_at: r.get(7)?,
    })
}

async fn guest_by_id(conn: &turso::Connection, id: &str) -> Result<Option<GuestRow>> {
    query_one(conn, &format!("SELECT {GUEST_COLUMNS} FROM channel_guests WHERE id = ?1"), [id], guest_row).await
}

async fn guests_of(conn: &turso::Connection, channel_id: &str) -> Result<Vec<GuestRow>> {
    query_all(
        conn,
        &format!("SELECT {GUEST_COLUMNS} FROM channel_guests WHERE channel_id = ?1 ORDER BY created_at"),
        [channel_id],
        guest_row,
    )
    .await
}

async fn all_guests(conn: &turso::Connection) -> Result<Vec<GuestRow>> {
    query_all(conn, &format!("SELECT {GUEST_COLUMNS} FROM channel_guests ORDER BY created_at"), (), guest_row).await
}

/// Guest: one of its channels that shows another server's, or a request for one.
#[derive(Clone, Debug)]
pub(super) struct LinkRow {
    id: String,
    /// None until the home approves.
    channel_id: Option<String>,
    active: bool,
    home: pb::SharedServer,
    home_channel_name: String,
    allowed: Bits,
    created_at: i64,
}

const LINK_COLUMNS: &str = "id, channel_id, active, home_server_id, home_server_name, home_server_icon_url, home_channel_name, allowed, created_at";

fn link_row(r: &turso::Row) -> turso::Result<LinkRow> {
    Ok(LinkRow {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        active: r.get::<i64>(2)? != 0,
        home: pb::SharedServer { id: r.get(3)?, name: r.get(4)?, icon_url: r.get(5)? },
        home_channel_name: r.get(6)?,
        allowed: r.get::<i64>(7)? as Bits,
        created_at: r.get(8)?,
    })
}

async fn link_by_id(conn: &turso::Connection, id: &str) -> Result<Option<LinkRow>> {
    query_one(conn, &format!("SELECT {LINK_COLUMNS} FROM channel_links WHERE id = ?1"), [id], link_row).await
}

async fn all_links(conn: &turso::Connection) -> Result<Vec<LinkRow>> {
    query_all(conn, &format!("SELECT {LINK_COLUMNS} FROM channel_links ORDER BY created_at"), (), link_row).await
}

/// The other server's channel a channel here shows, if it shows one.
pub(super) async fn link_of(conn: &turso::Connection, channel_id: &str) -> Result<Option<LinkRow>> {
    if channel_id.is_empty() {
        return Ok(None);
    }
    query_one(
        conn,
        &format!("SELECT {LINK_COLUMNS} FROM channel_links WHERE channel_id = ?1 AND active = 1"),
        [channel_id],
        link_row,
    )
    .await
}

fn state(active: bool) -> i32 {
    (if active { pb::SharedConnectionState::Active } else { pb::SharedConnectionState::Waiting }) as i32
}

fn home_connection(row: &GuestRow, channel_name: &str) -> pb::SharedConnection {
    pb::SharedConnection {
        id: row.id.clone(),
        home: true,
        channel_id: row.channel_id.clone(),
        home_channel_name: channel_name.to_string(),
        server: Some(row.server.clone()),
        state: state(row.active),
        allowed: permissions::to_list(row.allowed),
        created_at: Some(timestamp(row.created_at)),
    }
}

fn guest_connection(row: &LinkRow) -> pb::SharedConnection {
    pb::SharedConnection {
        id: row.id.clone(),
        home: false,
        channel_id: row.channel_id.clone().unwrap_or_default(),
        home_channel_name: row.home_channel_name.clone(),
        server: Some(row.home.clone()),
        state: state(row.active),
        allowed: permissions::to_list(row.allowed),
        created_at: Some(timestamp(row.created_at)),
    }
}

/// This server as the other end of a connection sees it.
async fn this_server(conn: &turso::Connection, server_id: &str) -> Result<pb::SharedServer> {
    let server = store::load_server(conn).await?;
    Ok(pb::SharedServer { id: server_id.to_string(), name: server.name, icon_url: server.icon_url })
}

fn shared_changed(events: &mut Vec<Payload>) {
    events.push(Payload::SharedChannelsUpdated(pb::SharedChannelsUpdated {}));
}

/// Tells the home channel's watchers who it's shown in now.
async fn channel_changed(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    if let Some(channel) = load_channel(conn, server_id, channel_id).await? {
        events.push(Payload::ChannelUpdated(pb::ChannelUpdated { channel: Some(channel) }));
    }
    Ok(())
}

// ─────────────── Codes ───────────────

/// A fresh share code. It starts with the home server's id, so any server
/// on the instance can find where it leads.
fn new_code(server_id: &str) -> String {
    let mut bytes = [0u8; CODE_LENGTH];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    let random: String = bytes.iter().map(|b| CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()] as char).collect();
    format!("{server_id}-{random}")
}

/// The home server a code is for.
fn code_home(code: &str) -> Result<String> {
    let invalid = || Error::invalid("that isn't a share code");
    let (server, rest) = code.trim().split_once('-').ok_or_else(invalid)?;
    if rest.len() != CODE_LENGTH || !rest.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(invalid());
    }
    parse_id("share code", server).map_err(|_| invalid())
}

fn code_row(r: &turso::Row) -> turso::Result<(pb::ShareCode, String)> {
    Ok((
        pb::ShareCode {
            code: r.get(0)?,
            channel_id: r.get(1)?,
            channel_name: String::new(),
            creator_id: r.get(2)?,
            created_at: Some(timestamp(r.get(3)?)),
            expires_at: Some(timestamp(r.get(4)?)),
        },
        r.get(1)?,
    ))
}

async fn load_code(conn: &turso::Connection, code: &str) -> Result<Option<pb::ShareCode>> {
    let found = query_one(
        conn,
        "SELECT code, channel_id, creator_id, created_at, expires_at FROM share_codes WHERE code = ?1 AND expires_at > ?2",
        (code.trim(), now_ms()),
        code_row,
    )
    .await?;
    Ok(found.map(|(code, _)| code))
}

/// Whether messages can go in a channel of this type, and so whether it can be shared.
fn shareable(channel: &pb::Channel) -> bool {
    matches!(pb::ChannelType::try_from(channel.r#type), Ok(pb::ChannelType::Text | pb::ChannelType::Announcement))
}

// ─────────────── Who wrote what ───────────────

/// Which server the authors with these ids are guests from, for those who are.
async fn guests_among(conn: &turso::Connection, ids: &[&str]) -> Result<HashMap<String, String>> {
    let mut ids = ids.to_vec();
    ids.retain(|id| !id.is_empty());
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let rows = query_all(
        conn,
        &format!("SELECT id, guest_of FROM users WHERE guest_of IS NOT NULL AND id IN ({placeholders})"),
        ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
        |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?)),
    )
    .await?;
    Ok(rows.into_iter().collect())
}

/// The servers guests here are from, by id, as they last named themselves.
async fn guest_servers(conn: &turso::Connection) -> Result<HashMap<String, pb::SharedServer>> {
    let rows = query_all(conn, "SELECT guest_server_id, guest_name, guest_icon_url FROM channel_guests", (), |r| {
        Ok(pb::SharedServer { id: r.get(0)?, name: r.get(1)?, icon_url: r.get(2)? })
    })
    .await?;
    Ok(rows.into_iter().map(|s| (s.id.clone(), s)).collect())
}

/// Says who wrote each message and which server they're from: on every
/// message (`home` given, for guests reading the channel), or only on
/// those by people from other servers (for the home's own members).
async fn decorate(
    conn: &turso::Connection,
    home: Option<&pb::SharedServer>,
    messages: &mut [pb::Message],
) -> Result<()> {
    let ids: Vec<&str> = messages.iter().map(|m| m.author_id.as_str()).collect();
    let guests = guests_among(conn, &ids).await?;
    if guests.is_empty() && home.is_none() {
        return Ok(());
    }
    let profiles: HashMap<String, pb::User> = users(conn, &ids).await?.into_iter().map(|u| (u.id.clone(), u)).collect();
    let servers = if guests.is_empty() { HashMap::new() } else { guest_servers(conn).await? };
    for message in messages.iter_mut() {
        if message.shared.is_some() {
            continue;
        }
        let server = match guests.get(&message.author_id) {
            Some(server_id) => servers
                .get(server_id)
                .cloned()
                .unwrap_or_else(|| pb::SharedServer { id: server_id.clone(), ..Default::default() }),
            None => match home {
                Some(home) => home.clone(),
                None => continue,
            },
        };
        let user = if message.webhook.is_some() { None } else { profiles.get(&message.author_id).cloned() };
        message.shared = Some(pb::SharedAuthor { user, server: Some(server) });
    }
    Ok(())
}

/// Marks messages people from other servers wrote here, for this server's
/// own members reading a channel it shares.
pub(super) async fn mark_guests(conn: &turso::Connection, messages: &mut [pb::Message]) -> Result<()> {
    decorate(conn, None, messages).await
}

/// Keeps a guest's profile here, so their messages show who wrote them, and
/// their server's name as it is now.
async fn remember(conn: &turso::Connection, user: &pb::User, server: &pb::SharedServer) -> Result<()> {
    store::upsert_user(conn, user).await?;
    // Someone who's a member here too reads as one of the home's own.
    let member = query_one(conn, "SELECT 1 FROM members WHERE user_id = ?1", [user.id.as_str()], |r| r.get::<i64>(0))
        .await?
        .is_some();
    if !member {
        conn.execute("UPDATE users SET guest_of = ?2 WHERE id = ?1", (user.id.as_str(), server.id.as_str())).await?;
    }
    conn.execute(
        "UPDATE channel_guests SET guest_name = ?2, guest_icon_url = ?3 WHERE guest_server_id = ?1",
        (server.id.as_str(), server.name.as_str(), server.icon_url.as_str()),
    )
    .await?;
    Ok(())
}

async fn blocked(conn: &turso::Connection, channel_id: &str, user_id: &str) -> Result<bool> {
    Ok(query_one(
        conn,
        "SELECT 1 FROM channel_blocks WHERE channel_id = ?1 AND user_id = ?2",
        (channel_id, user_id),
        |r| r.get::<i64>(0),
    )
    .await?
    .is_some())
}

/// Keeps someone from another server out of a channel, inside a write.
async fn keep_out(
    conn: &turso::Connection,
    channel: &pb::Channel,
    user_id: &str,
    guest_server_id: &str,
    actor_id: &str,
    reason: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    if blocked(conn, &channel.id, user_id).await? {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO channel_blocks (channel_id, user_id, guest_server_id, blocked_by_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (channel.id.as_str(), user_id, guest_server_id, actor_id, now_ms()),
    )
    .await?;
    store::audit(
        conn,
        actor_id,
        Audit::new(pb::AuditAction::SharedChannelBlock, user_id).channel(&channel.name).reason(reason),
    )
    .await?;
    shared_changed(events);
    Ok(())
}

/// Runs the home's AutoMod over what someone from another server writes,
/// inside the write. They aren't a member here to time out (and may be one
/// in their own right, whose membership this mustn't touch), so a rule that
/// would time them out keeps them out of the channel instead.
#[allow(clippy::too_many_arguments)]
async fn review_guest(
    conn: &turso::Connection,
    server_id: &str,
    user: &pb::User,
    server: &pb::SharedServer,
    access: &Access,
    channel: &pb::Channel,
    content: &str,
    events: &mut Vec<Payload>,
) -> Result<Option<String>> {
    // Already out until the year 9999, as far as `review` can tell.
    let member = pb::Member {
        user: Some(user.clone()),
        timed_out_until: Some(timestamp(253_402_300_799_000)),
        ..Default::default()
    };
    let verdict = automod::review(conn, server_id, &member, access, channel, content, events).await?;
    let times_out = store::load_automod(conn).await?.iter().any(|rule| {
        rule.enabled
            && !rule.exempt_channel_ids.iter().any(|id| *id == channel.id || *id == channel.parent_id)
            && rule.actions.iter().any(|a| a.kind == pb::AutoModActionKind::TimeOut as i32)
            && crate::automod::check(rule, content).is_some()
    });
    if times_out {
        keep_out(conn, channel, &user.id, &server.id, &user.id, "AutoMod", events).await?;
    }
    Ok(verdict.blocked)
}

// ─────────────── Calls between the two ends ───────────────

/// Whether the other end answered that the connection, or its server, is gone.
fn gone(err: &Error) -> bool {
    match err {
        Error::NotFound(what) => *what == GONE || *what == "server",
        Error::Remote(status) => {
            status.code() == Code::NotFound
                && matches!(status.message(), "shared channel not found" | "server not found")
        }
        _ => false,
    }
}

/// A call from a guest to the home. When the home says the connection is
/// gone, the guest lets go of it too.
async fn to_home(app: &Arc<App>, server_id: &str, link: &LinkRow, call: Call) -> Result<cpb::SharedReply> {
    let call = cpb::SharedCall { server_id: link.home.id.clone(), call: Some(call) };
    match app.shared(call).await {
        Err(err) if gone(&err) => {
            let sdb = app.servers.get(server_id).await?;
            let dropped = sdb.write("", async |conn, events| drop_link(conn, &sdb.id, &link.id, events).await).await?;
            if let Some(channel_id) = dropped {
                app.forget_notifications(server_id, Some(&channel_id), None).await;
            }
            Err(Error::FailedPrecondition("this channel isn't shared with this server anymore".into()))
        }
        other => other,
    }
}

/// Someone here, acting in a channel this server shows from another.
pub(super) async fn guest_of(
    conn: &turso::Connection,
    server_id: &str,
    account: &Account,
    access: &Access,
    link: &LinkRow,
) -> Result<cpb::Guest> {
    let channel_id = link.channel_id.as_deref().unwrap_or_default();
    let user = store::user(conn, &account.id).await?.ok_or_else(|| Error::denied("join this server first"))?;
    Ok(cpb::Guest {
        connection_id: link.id.clone(),
        user: Some(user),
        server: Some(this_server(conn, server_id).await?),
        moderator: access.has_in(channel_id, Permission::ManageMessages),
    })
}

/// A message as this server shows it: in its own channel.
fn shown_here(mut message: pb::Message, server_id: &str, link: &LinkRow) -> pb::Message {
    message.server_id = server_id.to_string();
    message.channel_id = link.channel_id.clone().unwrap_or_default();
    message
}

/// Runs this server's own AutoMod over what one of its people writes in a
/// channel it shows from another, before it goes there.
async fn review_here(
    sdb: &ServerDb,
    member: &pb::Member,
    access: &Access,
    channel_id: &str,
    content: &str,
) -> Result<()> {
    if access.has(Permission::ManageServer) || store::load_automod(&sdb.read()?).await?.is_empty() {
        return Ok(());
    }
    let author_id = member.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let blocked = sdb
        .write(&author_id, async |conn, events| {
            let channel = load_channel(conn, &sdb.id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
            Ok(automod::review(conn, &sdb.id, member, access, &channel, content, events).await?.blocked)
        })
        .await?;
    match blocked {
        Some(why) => Err(Error::denied(why)),
        None => Ok(()),
    }
}

/// Finds whether a message call is about a channel this server shows from
/// another: by the channel the app named, or, for apps that don't name one,
/// by asking each such channel's home for a message not kept here.
pub(super) async fn locate(
    app: &Arc<App>,
    conn: &turso::Connection,
    server_id: &str,
    account: &Account,
    access: &Access,
    channel_id: &str,
    message_id: &str,
) -> Result<Option<(LinkRow, cpb::Guest)>> {
    if !channel_id.is_empty() {
        let Some(link) = link_of(conn, channel_id).await? else { return Ok(None) };
        if !access.can_see(channel_id) {
            return Err(Error::NotFound("message"));
        }
        let guest = guest_of(conn, server_id, account, access, &link).await?;
        return Ok(Some((link, guest)));
    }
    if load_message(conn, server_id, message_id).await?.is_some() {
        return Ok(None);
    }
    for link in all_links(conn).await? {
        if !link.active || !link.channel_id.as_deref().is_some_and(|c| access.can_see(c)) {
            continue;
        }
        let guest = guest_of(conn, server_id, account, access, &link).await?;
        let call = Call::Get(cpb::GuestGet { guest: Some(guest.clone()), message_id: message_id.to_string() });
        if app.shared(cpb::SharedCall { server_id: link.home.id.clone(), call: Some(call) }).await.is_ok() {
            return Ok(Some((link, guest)));
        }
    }
    Ok(None)
}

/// Sends a message in a channel this server shows from another. The caller
/// has checked the member may send it here.
pub(super) async fn guest_send(
    app: &Arc<App>,
    sdb: &ServerDb,
    account: &Account,
    member: &pb::Member,
    access: &Access,
    link: &LinkRow,
    req: pb::SendMessageRequest,
) -> Result<pb::Message> {
    let channel_id = link.channel_id.clone().unwrap_or_default();
    let guest = guest_of(&sdb.read()?, &sdb.id, account, access, link).await?;
    review_here(sdb, member, access, &channel_id, &req.content).await?;
    let call = Call::Send(cpb::GuestSend {
        guest: Some(guest),
        content: req.content,
        attachments: req.attachments,
        embeds: req.embeds,
        reply_to_id: req.reply_to_id,
    });
    let reply = to_home(app, &sdb.id, link, call).await?;
    let message = reply.message.ok_or_else(|| Error::internal("the home server didn't say what it saved"))?;
    Ok(shown_here(message, &sdb.id, link))
}

pub(super) async fn guest_list(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    req: &pb::ListMessagesRequest,
) -> Result<pb::ListMessagesResponse> {
    let call = Call::List(cpb::GuestList {
        guest: Some(guest),
        limit: req.limit,
        before_id: req.before_id.clone(),
        after_id: req.after_id.clone(),
    });
    let mut page = to_home(app, server_id, link, call).await?.page.unwrap_or_default();
    page.messages = page.messages.into_iter().map(|m| shown_here(m, server_id, link)).collect();
    Ok(page)
}

pub(super) async fn guest_get(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    message_id: &str,
) -> Result<pb::GetMessageResponse> {
    let call = Call::Get(cpb::GuestGet { guest: Some(guest), message_id: message_id.to_string() });
    let reply = to_home(app, server_id, link, call).await?;
    Ok(pb::GetMessageResponse { message: reply.message.map(|m| shown_here(m, server_id, link)), author: reply.author })
}

pub(super) async fn guest_edit(
    app: &Arc<App>,
    sdb: &ServerDb,
    member: &pb::Member,
    access: &Access,
    link: &LinkRow,
    guest: cpb::Guest,
    req: &pb::UpdateMessageRequest,
) -> Result<pb::Message> {
    let channel_id = link.channel_id.clone().unwrap_or_default();
    review_here(sdb, member, access, &channel_id, &req.content).await?;
    let call = Call::Edit(cpb::GuestEdit {
        guest: Some(guest),
        message_id: req.message_id.clone(),
        content: req.content.clone(),
    });
    let reply = to_home(app, &sdb.id, link, call).await?;
    let message = reply.message.ok_or_else(|| Error::internal("the home server didn't say what it saved"))?;
    Ok(shown_here(message, &sdb.id, link))
}

pub(super) async fn guest_delete(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    message_id: &str,
) -> Result<()> {
    let call = Call::Delete(cpb::GuestDelete { guest: Some(guest), message_id: message_id.to_string() });
    to_home(app, server_id, link, call).await.map(|_| ())
}

/// Answers a call from the other end of one of this process's servers'
/// shared channels.
pub async fn shared_call(app: &Arc<App>, call: cpb::SharedCall) -> Result<cpb::SharedReply> {
    let sdb = app.servers.get(&call.server_id).await?;
    match call.call.ok_or_else(|| Error::invalid("call is required"))? {
        Call::Lookup(lookup) => home_lookup(&sdb, lookup).await,
        Call::Ask(ask) => home_ask(&sdb, ask).await,
        Call::Send(send) => home_send(app, &sdb, send).await,
        Call::List(list) => home_list(&sdb, list).await,
        Call::Get(get) => home_get(&sdb, get).await,
        Call::Edit(edit) => home_edit(&sdb, edit).await,
        Call::Delete(delete) => home_delete(&sdb, delete).await,
        Call::Left(left) => home_left(&sdb, left).await,
        Call::Approved(approved) => guest_approved(app, &sdb, approved).await,
        Call::Ended(ended) => guest_ended(app, &sdb, ended).await,
        Call::Events(events) => guest_events(app, &sdb, events).await,
        Call::Updated(updated) => guest_updated(&sdb, updated).await,
    }
}

// ─────────────── At the home ───────────────

/// The connection a guest's call comes through, checked: it's approved and
/// it's their server's.
async fn connection(conn: &turso::Connection, guest: &cpb::Guest) -> Result<(GuestRow, pb::User, pb::SharedServer)> {
    let server = guest.server.clone().ok_or_else(|| Error::invalid("server is required"))?;
    let user = guest.user.clone().ok_or_else(|| Error::invalid("user is required"))?;
    let row = guest_by_id(conn, &guest.connection_id)
        .await?
        .filter(|row| row.active && row.server.id == server.id)
        .ok_or(Error::NotFound(GONE))?;
    Ok((row, user, server))
}

async fn home_lookup(sdb: &ServerDb, lookup: cpb::ShareLookup) -> Result<cpb::SharedReply> {
    let conn = sdb.read()?;
    let code = load_code(&conn, &lookup.code).await?.ok_or(Error::NotFound("share code"))?;
    let channel = load_channel(&conn, &sdb.id, &code.channel_id).await?.ok_or(Error::NotFound("share code"))?;
    let guests = guests_of(&conn, &channel.id).await?;
    if guests.iter().any(|g| g.server.id == lookup.guest_server_id) {
        return Err(Error::AlreadyExists("this server already shows that channel, or has asked to".into()));
    }
    Ok(cpb::SharedReply {
        preview: Some(pb::PreviewShareResponse {
            home_server: Some(this_server(&conn, &sdb.id).await?),
            channel_name: channel.name,
            channel_topic: channel.topic,
            region: String::new(),
            checked_by: vec![],
            allowed: permissions::to_list(SHAREABLE),
            expires_at: code.expires_at,
            guest_count: guests.iter().filter(|g| g.active).count() as i32,
        }),
        ..Default::default()
    })
}

async fn home_ask(sdb: &ServerDb, ask: cpb::ShareAsk) -> Result<cpb::SharedReply> {
    let guest = ask.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    if guest.id == sdb.id {
        return Err(Error::invalid("a server can't show its own channel twice"));
    }
    parse_id("connection id", &ask.connection_id)?;
    let connection = sdb
        .write(&ask.asked_by_id, async |conn, events| {
            let code = load_code(conn, &ask.code).await?.ok_or(Error::NotFound("share code"))?;
            let channel = load_channel(conn, &sdb.id, &code.channel_id).await?.ok_or(Error::NotFound("share code"))?;
            if !shareable(&channel) || link_of(conn, &channel.id).await?.is_some() {
                return Err(Error::FailedPrecondition("that channel can't be shared".into()));
            }
            let guests = guests_of(conn, &channel.id).await?;
            if guests.iter().any(|g| g.server.id == guest.id) {
                return Err(Error::AlreadyExists("this server already shows that channel, or has asked to".into()));
            }
            if guests.len() >= MAX_GUESTS {
                return Err(Error::FailedPrecondition(
                    "that channel is already shown in another server; for now a channel is shared with one other server"
                        .into(),
                ));
            }
            let now = now_ms();
            conn.execute(
                "INSERT INTO channel_guests (id, channel_id, guest_server_id, guest_name, guest_icon_url, active, allowed, asked_by_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8)",
                (
                    ask.connection_id.as_str(),
                    channel.id.as_str(),
                    guest.id.as_str(),
                    guest.name.as_str(),
                    guest.icon_url.as_str(),
                    SHAREABLE as i64,
                    ask.asked_by_id.as_str(),
                    now,
                ),
            )
            .await?;
            // Each code lets one server ask.
            conn.execute("DELETE FROM share_codes WHERE code = ?1", [code.code.as_str()]).await?;
            shared_changed(events);
            Ok(pb::SharedConnection {
                id: ask.connection_id.clone(),
                home: false,
                channel_id: channel.id.clone(),
                home_channel_name: channel.name.clone(),
                server: Some(this_server(conn, &sdb.id).await?),
                state: pb::SharedConnectionState::Waiting as i32,
                allowed: permissions::to_list(SHAREABLE),
                created_at: Some(timestamp(now)),
            })
        })
        .await?;
    Ok(cpb::SharedReply { connection: Some(connection), ..Default::default() })
}

async fn home_send(app: &Arc<App>, sdb: &ServerDb, send: cpb::GuestSend) -> Result<cpb::SharedReply> {
    let guest = send.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    let author_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let limits = sdb.limits(&app.settings().limits).await?;
    if let Some(limit) = limits.storage_bytes
        && sdb.storage_bytes() >= limit
    {
        return Err(Error::ResourceExhausted("this channel's home server is out of storage".into()));
    }
    let message = sdb
        .write(&author_id, async |conn, events| {
            let (row, user, server) = connection(conn, &guest).await?;
            if blocked(conn, &row.channel_id, &user.id).await? {
                return Ok(Err(KEPT_OUT.to_string()));
            }
            let channel = load_channel(conn, &sdb.id, &row.channel_id).await?.ok_or(Error::NotFound(GONE))?;
            let access = Access::guest(&channel.id, row.allowed);
            access.require_in(&channel.id, Permission::SendMessages)?;
            if !send.attachments.is_empty() {
                access.require_in(&channel.id, Permission::AttachFiles)?;
            }
            if !send.embeds.is_empty() {
                access.require_in(&channel.id, Permission::EmbedLinks)?;
            }
            if !send.reply_to_id.is_empty() {
                let replied = load_message(conn, &sdb.id, &send.reply_to_id).await?;
                if replied.is_none_or(|m| m.channel_id != channel.id) {
                    return Err(Error::NotFound("message being replied to"));
                }
            }
            remember(conn, &user, &server).await?;
            let verdict = review_guest(conn, &sdb.id, &user, &server, &access, &channel, &send.content, events).await?;
            if let Some(why) = verdict {
                return Ok(Err(why));
            }
            let now = now_ms();
            check_slowmode(conn, &channel, &user.id, now).await?;
            let message = pb::Message {
                id: new_id(),
                server_id: sdb.id.clone(),
                channel_id: channel.id.clone(),
                author_id: user.id.clone(),
                content: send.content.clone(),
                attachments: send.attachments.clone(),
                embeds: send.embeds.clone(),
                reply_to_id: send.reply_to_id.clone(),
                created_at: Some(timestamp(now)),
                kind: pb::MessageKind::Unspecified as i32,
                shared: Some(pb::SharedAuthor { user: Some(user), server: Some(server) }),
                ..Default::default()
            };
            messages::insert_message(conn, &message, now).await?;
            events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message.clone()) }));
            Ok(Ok(message))
        })
        .await?
        .map_err(Error::denied)?;
    Ok(cpb::SharedReply { message: Some(message), ..Default::default() })
}

async fn home_list(sdb: &ServerDb, list: cpb::GuestList) -> Result<cpb::SharedReply> {
    let guest = list.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    let (mut messages, has_more) =
        messages::page(&conn, &sdb.id, &row.channel_id, list.limit, &list.before_id, &list.after_id, true).await?;
    let home = this_server(&conn, &sdb.id).await?;
    decorate(&conn, Some(&home), &mut messages).await?;
    let authors = users(&conn, &messages.iter().map(|m| m.author_id.as_str()).collect::<Vec<_>>()).await?;
    Ok(cpb::SharedReply { page: Some(pb::ListMessagesResponse { messages, authors, has_more }), ..Default::default() })
}

async fn home_get(sdb: &ServerDb, get: cpb::GuestGet) -> Result<cpb::SharedReply> {
    let guest = get.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    let mut message = load_message(&conn, &sdb.id, &get.message_id)
        .await?
        .filter(|m| m.channel_id == row.channel_id && m.kind == pb::MessageKind::Unspecified as i32)
        .ok_or(Error::NotFound("message"))?;
    let home = this_server(&conn, &sdb.id).await?;
    decorate(&conn, Some(&home), std::slice::from_mut(&mut message)).await?;
    let author = store::user(&conn, &message.author_id).await?;
    Ok(cpb::SharedReply { message: Some(message), author, ..Default::default() })
}

async fn home_edit(sdb: &ServerDb, edit: cpb::GuestEdit) -> Result<cpb::SharedReply> {
    let guest = edit.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    let author_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let message = sdb
        .write(&author_id, async |conn, events| {
            let (row, user, server) = connection(conn, &guest).await?;
            if blocked(conn, &row.channel_id, &user.id).await? {
                return Ok(Err(KEPT_OUT.to_string()));
            }
            let mut message = load_message(conn, &sdb.id, &edit.message_id)
                .await?
                .filter(|m| m.channel_id == row.channel_id)
                .ok_or(Error::NotFound("message"))?;
            if message.author_id != user.id {
                return Err(Error::denied("you can only edit your own messages"));
            }
            if message.kind != pb::MessageKind::Unspecified as i32 {
                return Err(Error::invalid("system messages can't be edited"));
            }
            messages::check_content(&edit.content, !message.attachments.is_empty() || !message.embeds.is_empty())?;
            let channel = load_channel(conn, &sdb.id, &row.channel_id).await?.ok_or(Error::NotFound(GONE))?;
            remember(conn, &user, &server).await?;
            if message.content != edit.content {
                let access = Access::guest(&channel.id, row.allowed);
                let verdict =
                    review_guest(conn, &sdb.id, &user, &server, &access, &channel, &edit.content, events).await?;
                if let Some(why) = verdict {
                    return Ok(Err(why));
                }
            }
            let old_size = message.content.len() as i64;
            message.content = edit.content.clone();
            message.edited_at = Some(timestamp(now_ms()));
            messages::save_edit(conn, &message, old_size).await?;
            message.shared = Some(pb::SharedAuthor { user: Some(user), server: Some(server) });
            events.push(Payload::MessageUpdated(pb::MessageUpdated { message: Some(message.clone()) }));
            Ok(Ok(message))
        })
        .await?
        .map_err(Error::denied)?;
    Ok(cpb::SharedReply { message: Some(message), ..Default::default() })
}

async fn home_delete(sdb: &ServerDb, delete: cpb::GuestDelete) -> Result<cpb::SharedReply> {
    let guest = delete.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    let actor_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    sdb.write(&actor_id, async |conn, events| {
        let (row, user, server) = connection(conn, &guest).await?;
        let message = load_message(conn, &sdb.id, &delete.message_id)
            .await?
            .filter(|m| m.channel_id == row.channel_id)
            .ok_or(Error::NotFound("message"))?;
        if message.author_id != user.id {
            // A guest's moderators delete their own server's people's messages.
            let theirs = guest.moderator
                && guests_among(conn, &[message.author_id.as_str()]).await?.get(&message.author_id) == Some(&server.id);
            if !theirs {
                return Err(Error::denied("you can only delete your own messages here, or your server's people's"));
            }
            remember(conn, &user, &server).await?;
            let channel = load_channel(conn, &sdb.id, &row.channel_id).await?.map(|c| c.name).unwrap_or_default();
            store::audit(
                conn,
                &user.id,
                Audit::new(pb::AuditAction::MessageDelete, &message.author_id).channel(channel),
            )
            .await?;
        }
        messages::remove_message(conn, &message).await?;
        events.push(Payload::MessageDeleted(pb::MessageDeleted {
            channel_id: message.channel_id.clone(),
            message_id: message.id.clone(),
        }));
        Ok(())
    })
    .await?;
    Ok(cpb::SharedReply::default())
}

/// Ends a connection at the home, inside a write: its row, and the people
/// from that server kept out of the channel. None if it was already gone.
async fn drop_guest(
    conn: &turso::Connection,
    server_id: &str,
    connection_id: &str,
    events: &mut Vec<Payload>,
) -> Result<Option<GuestRow>> {
    let Some(row) = guest_by_id(conn, connection_id).await? else { return Ok(None) };
    conn.execute("DELETE FROM channel_guests WHERE id = ?1", [row.id.as_str()]).await?;
    conn.execute(
        "DELETE FROM channel_blocks WHERE channel_id = ?1 AND guest_server_id = ?2",
        (row.channel_id.as_str(), row.server.id.as_str()),
    )
    .await?;
    shared_changed(events);
    if row.active {
        channel_changed(conn, server_id, &row.channel_id, events).await?;
    }
    Ok(Some(row))
}

async fn home_left(sdb: &ServerDb, left: cpb::GuestLeft) -> Result<cpb::SharedReply> {
    sdb.write(&left.actor_id, async |conn, events| {
        let Some(row) = guest_by_id(conn, &left.connection_id).await? else { return Ok(()) };
        if row.server.id != left.guest_server_id {
            return Err(Error::NotFound(GONE));
        }
        let channel = load_channel(conn, &sdb.id, &row.channel_id).await?.map(|c| c.name).unwrap_or_default();
        drop_guest(conn, &sdb.id, &row.id, events).await?;
        store::audit(
            conn,
            &left.actor_id,
            Audit::new(pb::AuditAction::SharedChannelDisconnect, &row.server.id).channel(channel).change(
                "server",
                "",
                &row.server.name,
            ),
        )
        .await?;
        Ok(())
    })
    .await?;
    Ok(cpb::SharedReply::default())
}

// ─────────────── At a guest ───────────────

/// Takes a channel this server shows from another away, inside a write: the
/// channel, if the home had approved it, and the link. Returns the channel's id.
async fn drop_link(
    conn: &turso::Connection,
    server_id: &str,
    connection_id: &str,
    events: &mut Vec<Payload>,
) -> Result<Option<String>> {
    let Some(link) = link_by_id(conn, connection_id).await? else { return Ok(None) };
    conn.execute("DELETE FROM channel_links WHERE id = ?1", [link.id.as_str()]).await?;
    shared_changed(events);
    let Some(channel_id) = link.channel_id else { return Ok(None) };
    if load_channel(conn, server_id, &channel_id).await?.is_none() {
        return Ok(None);
    }
    conn.execute("DELETE FROM channel_overwrites WHERE channel_id = ?1", [channel_id.as_str()]).await?;
    conn.execute("DELETE FROM slowmode WHERE channel_id = ?1", [channel_id.as_str()]).await?;
    conn.execute("DELETE FROM channels WHERE id = ?1", [channel_id.as_str()]).await?;
    conn.execute("UPDATE usage SET channels = channels - 1, updated_at = ?1 WHERE id = 1", [now_ms()]).await?;
    events.push(Payload::ChannelDeleted(pb::ChannelDeleted { channel_id: channel_id.clone() }));
    Ok(Some(channel_id))
}

async fn guest_approved(app: &Arc<App>, sdb: &ServerDb, approved: cpb::HomeApproved) -> Result<cpb::SharedReply> {
    let connection = approved.connection.ok_or_else(|| Error::invalid("connection is required"))?;
    let allowed = permissions::from_list(&connection.allowed)? & SHAREABLE;
    let limits = sdb.limits(&app.settings().limits).await?;
    let link = sdb
        .write(&approved.actor_id, async |conn, events| {
            let link = link_by_id(conn, &connection.id).await?.ok_or(Error::NotFound(GONE))?;
            if link.active {
                return Ok(link);
            }
            if let Some(limit) = limits.channels
                && store::usage_count(conn, "channels").await? >= limit
            {
                return Err(Error::ResourceExhausted(format!(
                    "{} can have at most {limit} channels, so it has no room for this one",
                    store::load_server(conn).await?.name
                )));
            }
            let (name, parent_id) = query_one(conn, "SELECT name, parent_id FROM channel_links WHERE id = ?1", [link.id.as_str()], |r| {
                Ok((r.get::<String>(0)?, r.get::<Option<String>>(1)?))
            })
            .await?
            .unwrap_or_default();
            // The category it was meant for may have gone meanwhile.
            let parent_id = match parent_id {
                Some(id) => load_channel(conn, &sdb.id, &id)
                    .await?
                    .filter(|c| c.r#type == pb::ChannelType::Category as i32)
                    .map(|c| c.id),
                None => None,
            };
            let position = query_one(conn, "SELECT coalesce(max(position) + 1, 0) FROM channels", (), |r| r.get::<i64>(0))
                .await?
                .unwrap_or(0);
            let now = now_ms();
            let channel_id = new_id();
            conn.execute(
                "INSERT INTO channels (id, name, type, parent_id, topic, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?6)",
                (channel_id.as_str(), name.as_str(), pb::ChannelType::Text as i64, parent_id.as_deref(), position, now),
            )
            .await?;
            conn.execute("UPDATE usage SET channels = channels + 1, updated_at = ?1 WHERE id = 1", [now]).await?;
            conn.execute(
                "UPDATE channel_links SET active = 1, channel_id = ?2, allowed = ?3, home_channel_name = ?4 WHERE id = ?1",
                (link.id.as_str(), channel_id.as_str(), allowed as i64, connection.home_channel_name.as_str()),
            )
            .await?;
            let channel = load_channel(conn, &sdb.id, &channel_id).await?.ok_or(Error::NotFound("channel"))?;
            events.push(Payload::ChannelCreated(pb::ChannelCreated { channel: Some(channel) }));
            shared_changed(events);
            link_by_id(conn, &link.id).await?.ok_or(Error::NotFound(GONE))
        })
        .await?;
    Ok(cpb::SharedReply { connection: Some(guest_connection(&link)), ..Default::default() })
}

async fn guest_ended(app: &Arc<App>, sdb: &ServerDb, ended: cpb::HomeEnded) -> Result<cpb::SharedReply> {
    let channel_id = sdb
        .write(&ended.actor_id, async |conn, events| drop_link(conn, &sdb.id, &ended.connection_id, events).await)
        .await?;
    if let Some(channel_id) = channel_id {
        app.forget_notifications(&sdb.id, Some(&channel_id), None).await;
    }
    Ok(cpb::SharedReply::default())
}

async fn guest_events(app: &Arc<App>, sdb: &ServerDb, home: cpb::HomeEvents) -> Result<cpb::SharedReply> {
    let link =
        link_by_id(&sdb.read()?, &home.connection_id).await?.filter(|l| l.active).ok_or(Error::NotFound(GONE))?;
    let Some(channel_id) = link.channel_id.clone() else { return Err(Error::NotFound(GONE)) };
    let events: Vec<pb::Event> = home
        .events
        .into_iter()
        .filter_map(|mut event| {
            match event.payload.as_mut()? {
                Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
                | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }) => {
                    m.server_id = sdb.id.clone();
                    m.channel_id = channel_id.clone();
                }
                Payload::MessageDeleted(d) => d.channel_id = channel_id.clone(),
                _ => return None,
            }
            // Not in this server's log: it's shown, not kept.
            event.id = new_id();
            event.server_id = sdb.id.clone();
            event.sequence = 0;
            Some(event)
        })
        .collect();
    app.hub.publish(events);
    Ok(cpb::SharedReply::default())
}

async fn guest_updated(sdb: &ServerDb, updated: cpb::HomeUpdated) -> Result<cpb::SharedReply> {
    let connection = updated.connection.ok_or_else(|| Error::invalid("connection is required"))?;
    let allowed = permissions::from_list(&connection.allowed)? & SHAREABLE;
    sdb.write("", async |conn, events| {
        let changed = conn
            .execute(
                "UPDATE channel_links SET allowed = ?2, home_channel_name = ?3 WHERE id = ?1",
                (connection.id.as_str(), allowed as i64, connection.home_channel_name.as_str()),
            )
            .await?;
        if changed == 0 {
            return Err(Error::NotFound(GONE));
        }
        shared_changed(events);
        Ok(())
    })
    .await?;
    Ok(cpb::SharedReply::default())
}

// ─────────────── When channels and servers go ───────────────

/// The other ends a deleted channel or server leaves behind, to be told.
#[derive(Default)]
pub(super) struct Ended {
    /// Guests showing this server's channels: (connection, guest server).
    guests: Vec<(String, String)>,
    /// Channels here showing other servers': (connection, home server).
    links: Vec<(String, String)>,
}

/// Takes a channel's shared ties out with it, inside the write that deletes
/// it: who it's shown in (at its home), its codes and blocks, or the link
/// (at a guest).
pub(super) async fn take_channel(conn: &turso::Connection, channel_id: &str) -> Result<Ended> {
    let mut ended = Ended::default();
    for row in guests_of(conn, channel_id).await? {
        ended.guests.push((row.id, row.server.id));
    }
    if let Some(link) = query_one(
        conn,
        &format!("SELECT {LINK_COLUMNS} FROM channel_links WHERE channel_id = ?1"),
        [channel_id],
        link_row,
    )
    .await?
    {
        ended.links.push((link.id, link.home.id));
    }
    for table in ["channel_guests", "share_codes", "channel_blocks", "channel_links"] {
        conn.execute(&format!("DELETE FROM {table} WHERE channel_id = ?1"), [channel_id]).await?;
    }
    Ok(ended)
}

/// Every shared tie a server has, before it's deleted.
pub(super) async fn take_server(conn: &turso::Connection) -> Result<Ended> {
    Ok(Ended {
        guests: all_guests(conn).await?.into_iter().map(|row| (row.id, row.server.id)).collect(),
        links: all_links(conn).await?.into_iter().map(|link| (link.id, link.home.id)).collect(),
    })
}

/// Tells the other ends a channel or server went. Each is told once; one
/// that misses it lets go the next time it reaches this server.
pub(super) async fn tell_ended(app: &Arc<App>, server_id: &str, actor_id: &str, ended: Ended) {
    for (connection_id, guest_server_id) in ended.guests {
        let call = Call::Ended(cpb::HomeEnded { connection_id, actor_id: actor_id.to_string() });
        if let Err(err) = app.shared(cpb::SharedCall { server_id: guest_server_id, call: Some(call) }).await {
            tracing::info!(error = %err, "couldn't tell a server a shared channel ended");
        }
    }
    for (connection_id, home_server_id) in ended.links {
        let left =
            cpb::GuestLeft { connection_id, guest_server_id: server_id.to_string(), actor_id: actor_id.to_string() };
        if let Err(err) = app.shared(cpb::SharedCall { server_id: home_server_id, call: Some(Call::Left(left)) }).await
        {
            tracing::info!(error = %err, "couldn't tell a server a shared channel ended");
        }
    }
}

// ─────────────── Passing the channel on to guests ───────────────

/// Where one shared channel's events go: a connection and its guest server.
#[derive(Clone)]
struct Target {
    connection_id: String,
    guest_server_id: String,
}

/// The channels a server here shows in others, and where.
async fn targets(app: &App, server_id: &str) -> HashMap<String, Vec<Target>> {
    let loaded = async {
        let sdb = app.servers.get(server_id).await?;
        let rows = all_guests(&sdb.read()?).await?;
        Ok::<_, Error>(rows)
    }
    .await;
    let mut targets: HashMap<String, Vec<Target>> = HashMap::new();
    match loaded {
        Ok(rows) => {
            for row in rows.into_iter().filter(|r| r.active) {
                targets
                    .entry(row.channel_id)
                    .or_default()
                    .push(Target { connection_id: row.id, guest_server_id: row.server.id });
            }
        }
        Err(err) => tracing::warn!(server = %server_id, error = %err, "couldn't read a server's shared channels"),
    }
    targets
}

/// The channel a message event is about, for the messages guests are shown:
/// ones people wrote, not join messages or AutoMod alerts.
fn message_channel(payload: &Payload) -> Option<&str> {
    match payload {
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) })
            if m.kind == pb::MessageKind::Unspecified as i32 =>
        {
            Some(&m.channel_id)
        }
        Payload::MessageDeleted(d) => Some(&d.channel_id),
        _ => None,
    }
}

/// An event as guests get it: each message says who wrote it, since its
/// author needn't be in the guest server.
async fn for_guests(app: &App, event: &pb::Event) -> Option<pb::Event> {
    let mut event = event.clone();
    if let Some(
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }),
    ) = event.payload.as_mut()
        && m.shared.is_none()
    {
        let decorated = async {
            let sdb = app.servers.get(&event.server_id).await?;
            let conn = sdb.read()?;
            let home = this_server(&conn, &sdb.id).await?;
            decorate(&conn, Some(&home), std::slice::from_mut(m)).await
        }
        .await;
        if let Err(err) = decorated {
            tracing::warn!(server = %event.server_id, error = %err, "couldn't say who wrote a shared message");
            return None;
        }
    }
    Some(event)
}

/// What's waiting to go to one guest server: the home, the connection, the call.
type Outgoing = (String, String, cpb::SharedCall);

/// One guest server's queue: its events go in order, and a slow or missing
/// server holds up only itself.
fn spawn_queue(app: Arc<App>) -> mpsc::UnboundedSender<Outgoing> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
    tokio::spawn(async move {
        while let Some((home_id, connection_id, call)) = rx.recv().await {
            match app.shared(call).await {
                Ok(_) => {}
                // The guest doesn't show the channel anymore: let it go here too.
                Err(err) if gone(&err) => {
                    let healed = async {
                        let sdb = app.servers.get(&home_id).await?;
                        sdb.write("", async |conn, events| drop_guest(conn, &sdb.id, &connection_id, events).await)
                            .await
                    }
                    .await;
                    if let Err(err) = healed {
                        tracing::warn!(server = %home_id, error = %err, "couldn't end a shared channel its guest left");
                    }
                }
                Err(err) => tracing::info!(error = %err, "couldn't show a server what happened in a shared channel"),
            }
        }
    });
    tx
}

/// Passes what happens in the shared channels kept here on to the servers
/// that show them. Runs where servers are kept.
pub fn spawn_shared_fanout(app: Arc<App>) {
    let mut events = app.hub.shared_tap();
    tokio::spawn(async move {
        let mut known: HashMap<String, Arc<HashMap<String, Vec<Target>>>> = HashMap::new();
        let mut queues: HashMap<String, mpsc::UnboundedSender<Outgoing>> = HashMap::new();
        loop {
            let event = tokio::select! {
                event = events.recv() => match event {
                    Some(event) => event,
                    None => break,
                },
                () = app.shutdown.cancelled() => break,
            };
            let Some(payload) = &event.payload else { continue };
            if matches!(
                payload,
                Payload::SharedChannelsUpdated(_) | Payload::ChannelDeleted(_) | Payload::ServerDeleted(_)
            ) {
                known.remove(&event.server_id);
                continue;
            }
            // Events shown in a guest are published with sequence 0 and never
            // passed on again: only a channel's home passes its events on.
            if event.sequence == 0 || !app.servers.holds(&event.server_id) {
                continue;
            }
            let Some(channel_id) = message_channel(payload) else { continue };
            let server_targets = match known.get(&event.server_id) {
                Some(targets) => targets.clone(),
                None => {
                    let loaded = Arc::new(targets(&app, &event.server_id).await);
                    known.insert(event.server_id.clone(), loaded.clone());
                    loaded
                }
            };
            let Some(list) = server_targets.get(channel_id) else { continue };
            let Some(shown) = for_guests(&app, &event).await else { continue };
            for target in list {
                let call = cpb::SharedCall {
                    server_id: target.guest_server_id.clone(),
                    call: Some(Call::Events(cpb::HomeEvents {
                        connection_id: target.connection_id.clone(),
                        events: vec![shown.clone()],
                    })),
                };
                let queue = queues.entry(target.guest_server_id.clone()).or_insert_with(|| spawn_queue(app.clone()));
                let outgoing = (event.server_id.clone(), target.connection_id.clone(), call);
                if let Err(mpsc::error::SendError(outgoing)) = queue.send(outgoing) {
                    let queue = spawn_queue(app.clone());
                    let _ = queue.send(outgoing);
                    queues.insert(target.guest_server_id.clone(), queue);
                }
            }
        }
    });
}

// ─────────────── The service ───────────────

impl Api {
    /// Refuses new shares while the instance has them turned off.
    fn sharing_on(&self) -> Result<()> {
        if self.app.settings().shared_channels {
            Ok(())
        } else {
            Err(Error::FailedPrecondition("this instance has shared channels turned off".into()))
        }
    }
}

/// A connection's guest server's people may do at most these, and never more
/// than [`SHAREABLE`].
fn allowed_from(list: &[i32]) -> Result<Bits> {
    let bits = permissions::from_list(list)?;
    if bits & !(SHAREABLE | bit(Permission::ViewChannels)) != 0 {
        return Err(Error::invalid(
            "a shared channel can let other servers' people send messages, embed links and attach files, nothing more",
        ));
    }
    Ok(bits & SHAREABLE)
}

#[tonic::async_trait]
impl SharedChannelService for Api {
    async fn create_share_code(
        &self,
        request: Request<pb::CreateShareCodeRequest>,
    ) -> Result<Response<pb::CreateShareCodeResponse>, Status> {
        respond(
            async {
                self.sharing_on()?;
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                access.require_in(&req.channel_id, Permission::ManageChannels)?;
                let code = sdb
                    .write(&account.id, async |conn, _events| {
                        let channel =
                            load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                        if !shareable(&channel) || link_of(conn, &channel.id).await?.is_some() {
                            return Err(Error::invalid(
                                "only text and announcement channels of this server's own can be shared",
                            ));
                        }
                        if guests_of(conn, &channel.id).await?.len() >= MAX_GUESTS {
                            return Err(Error::FailedPrecondition(
                                "this channel is already shared with another server; for now a channel is shared with one other server"
                                    .into(),
                            ));
                        }
                        let now = now_ms();
                        let code = pb::ShareCode {
                            code: new_code(&sdb.id),
                            channel_id: channel.id.clone(),
                            channel_name: channel.name.clone(),
                            creator_id: account.id.clone(),
                            created_at: Some(timestamp(now)),
                            expires_at: Some(timestamp(now + CODE_TTL_MS)),
                        };
                        conn.execute(
                            "INSERT INTO share_codes (code, channel_id, creator_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                            (code.code.as_str(), channel.id.as_str(), account.id.as_str(), now, now + CODE_TTL_MS),
                        )
                        .await?;
                        // Codes are secrets, like invites: an audit entry, no event.
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::ShareCodeCreate, &code.code).channel(&channel.name),
                        )
                        .await?;
                        Ok(code)
                    })
                    .await?;
                Ok(pb::CreateShareCodeResponse { code: Some(code) })
            }
            .await,
        )
    }

    async fn delete_share_code(
        &self,
        request: Request<pb::DeleteShareCodeRequest>,
    ) -> Result<Response<pb::DeleteShareCodeResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                sdb.write(&account.id, async |conn, _events| {
                    let (_, channel_id) = query_one(
                        conn,
                        "SELECT code, channel_id, creator_id, created_at, expires_at FROM share_codes WHERE code = ?1",
                        [req.code.trim()],
                        code_row,
                    )
                    .await?
                    .ok_or(Error::NotFound("share code"))?;
                    conn.execute("DELETE FROM share_codes WHERE code = ?1", [req.code.trim()]).await?;
                    let channel = load_channel(conn, &sdb.id, &channel_id).await?.map(|c| c.name).unwrap_or_default();
                    store::audit(
                        conn,
                        &account.id,
                        Audit::new(pb::AuditAction::ShareCodeDelete, req.code.trim()).channel(channel),
                    )
                    .await
                })
                .await?;
                Ok(pb::DeleteShareCodeResponse {})
            }
            .await,
        )
    }

    async fn preview_share(
        &self,
        request: Request<pb::PreviewShareRequest>,
    ) -> Result<Response<pb::PreviewShareResponse>, Status> {
        respond(
            async {
                self.sharing_on()?;
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                let home = code_home(&req.code)?;
                if home == sdb.id {
                    return Err(Error::invalid("that code is for one of this server's own channels"));
                }
                let lookup = cpb::ShareLookup { code: req.code.trim().to_string(), guest_server_id: sdb.id.clone() };
                let reply = self
                    .app
                    .shared(cpb::SharedCall { server_id: home, call: Some(Call::Lookup(lookup)) })
                    .await
                    .map_err(|err| if gone(&err) { Error::NotFound("share code") } else { err })?;
                reply.preview.ok_or(Error::NotFound("share code"))
            }
            .await,
        )
    }

    async fn accept_share(
        &self,
        request: Request<pb::AcceptShareRequest>,
    ) -> Result<Response<pb::AcceptShareResponse>, Status> {
        respond(
            async {
                self.sharing_on()?;
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                if req.parent_id.is_empty() {
                    access.require(Permission::ManageChannels)?;
                } else {
                    access.require_in(&req.parent_id, Permission::ManageChannels)?;
                }
                let home = code_home(&req.code)?;
                if home == sdb.id {
                    return Err(Error::invalid("that code is for one of this server's own channels"));
                }
                let name = if req.name.trim().is_empty() { None } else { Some(super::channels::channel_name(&req.name, pb::ChannelType::Text)?) };
                let conn = sdb.read()?;
                if !req.parent_id.is_empty() {
                    let parent = load_channel(&conn, &sdb.id, &req.parent_id).await?.ok_or(Error::NotFound("parent channel"))?;
                    if parent.r#type != pb::ChannelType::Category as i32 {
                        return Err(Error::invalid("channels can only sit inside a category"));
                    }
                }
                let ask = cpb::ShareAsk {
                    code: req.code.trim().to_string(),
                    connection_id: new_id(),
                    guest: Some(this_server(&conn, &sdb.id).await?),
                    asked_by_id: account.id.clone(),
                };
                drop(conn);
                let reply = self
                    .app
                    .shared(cpb::SharedCall { server_id: home.clone(), call: Some(Call::Ask(ask)) })
                    .await
                    .map_err(|err| if gone(&err) { Error::NotFound("share code") } else { err })?;
                let asked = reply.connection.ok_or_else(|| Error::internal("the home server didn't say what it took"))?;
                let home_server = asked.server.clone().unwrap_or_default();
                let saved = sdb
                    .write(&account.id, async |conn, events| {
                        let now = now_ms();
                        conn.execute(
                            "INSERT INTO channel_links (id, active, home_server_id, home_server_name, home_server_icon_url, home_channel_id, home_channel_name, allowed, name, parent_id, asked_by_id, created_at)
                             VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                            (
                                asked.id.as_str(),
                                home.as_str(),
                                home_server.name.as_str(),
                                home_server.icon_url.as_str(),
                                asked.channel_id.as_str(),
                                asked.home_channel_name.as_str(),
                                (permissions::from_list(&asked.allowed)? & SHAREABLE) as i64,
                                name.as_deref().unwrap_or(&asked.home_channel_name),
                                (!req.parent_id.is_empty()).then_some(req.parent_id.as_str()),
                                account.id.as_str(),
                                now,
                            ),
                        )
                        .await?;
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::SharedChannelRequest, &home)
                                .channel(&asked.home_channel_name)
                                .change("server", "", &home_server.name),
                        )
                        .await?;
                        shared_changed(events);
                        link_by_id(conn, &asked.id).await?.ok_or(Error::NotFound(GONE))
                    })
                    .await;
                let link = match saved {
                    Ok(link) => link,
                    Err(err) => {
                        // Take the request back, so the home isn't left waiting on nothing.
                        let left = cpb::GuestLeft {
                            connection_id: asked.id.clone(),
                            guest_server_id: sdb.id.clone(),
                            actor_id: account.id.clone(),
                        };
                        let _ = self.app.shared(cpb::SharedCall { server_id: home, call: Some(Call::Left(left)) }).await;
                        return Err(err);
                    }
                };
                Ok(pb::AcceptShareResponse { connection: Some(guest_connection(&link)) })
            }
            .await,
        )
    }

    async fn review_share(
        &self,
        request: Request<pb::ReviewShareRequest>,
    ) -> Result<Response<pb::ReviewShareResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                let conn = sdb.read()?;
                let row = guest_by_id(&conn, &req.connection_id).await?.ok_or(Error::NotFound("request"))?;
                access.require_in(&row.channel_id, Permission::ManageChannels)?;
                if row.active {
                    return Err(Error::FailedPrecondition("that channel is already shared with that server".into()));
                }
                let channel =
                    load_channel(&conn, &sdb.id, &row.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                drop(conn);
                if !req.approve {
                    let ended = sdb
                        .write(&account.id, async |conn, events| {
                            let row = drop_guest(conn, &sdb.id, &req.connection_id, events).await?;
                            store::audit(
                                conn,
                                &account.id,
                                Audit::new(pb::AuditAction::SharedChannelDisconnect, row.as_ref().map(|r| r.server.id.clone()).unwrap_or_default())
                                    .channel(&channel.name)
                                    .change("server", "", row.as_ref().map(|r| r.server.name.as_str()).unwrap_or_default()),
                            )
                            .await?;
                            Ok(row)
                        })
                        .await?;
                    if let Some(row) = ended {
                        tell_ended(&self.app, &sdb.id, &account.id, Ended { guests: vec![(row.id, row.server.id)], links: vec![] }).await;
                    }
                    return Ok(pb::ReviewShareResponse { connection: None });
                }
                // The guest makes its channel first, so an approval it can't
                // take (no room for another channel, say) doesn't stand here.
                let mut offered = home_connection(&row, &channel.name);
                offered.server = Some(this_server(&sdb.read()?, &sdb.id).await?);
                let approved = cpb::HomeApproved { connection: Some(offered), actor_id: account.id.clone() };
                self.app
                    .shared(cpb::SharedCall { server_id: row.server.id.clone(), call: Some(Call::Approved(approved)) })
                    .await?;
                let saved = sdb
                    .write(&account.id, async |conn, events| {
                        let changed = conn
                            .execute(
                                "UPDATE channel_guests SET active = 1, approved_by_id = ?2 WHERE id = ?1 AND active = 0",
                                (row.id.as_str(), account.id.as_str()),
                            )
                            .await?;
                        if changed == 0 {
                            return Err(Error::NotFound("request"));
                        }
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::SharedChannelApprove, &row.server.id)
                                .channel(&channel.name)
                                .change("server", "", &row.server.name),
                        )
                        .await?;
                        shared_changed(events);
                        channel_changed(conn, &sdb.id, &channel.id, events).await?;
                        guest_by_id(conn, &row.id).await?.ok_or(Error::NotFound("request"))
                    })
                    .await;
                match saved {
                    Ok(row) => Ok(pb::ReviewShareResponse { connection: Some(home_connection(&row, &channel.name)) }),
                    Err(err) => {
                        tell_ended(&self.app, &sdb.id, &account.id, Ended { guests: vec![(row.id, row.server.id)], links: vec![] }).await;
                        Err(err)
                    }
                }
            }
            .await,
        )
    }

    async fn list_connections(
        &self,
        request: Request<pb::ListConnectionsRequest>,
    ) -> Result<Response<pb::ListConnectionsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let Seat { sdb, .. } =
                    self.with(&account, &request.get_ref().server_id, Permission::ManageServer).await?;
                let conn = sdb.read()?;
                let names: HashMap<String, String> =
                    store::load_channels(&conn, &sdb.id).await?.into_iter().map(|c| (c.id, c.name)).collect();
                let name = |id: &str| names.get(id).cloned().unwrap_or_default();
                let mut connections: Vec<pb::SharedConnection> =
                    all_guests(&conn).await?.iter().map(|row| home_connection(row, &name(&row.channel_id))).collect();
                connections.extend(all_links(&conn).await?.iter().map(guest_connection));
                let codes = query_all(
                    &conn,
                    "SELECT code, channel_id, creator_id, created_at, expires_at FROM share_codes WHERE expires_at > ?1 ORDER BY created_at DESC",
                    [now_ms()],
                    code_row,
                )
                .await?
                .into_iter()
                .map(|(mut code, channel_id)| {
                    code.channel_name = name(&channel_id);
                    code
                })
                .collect();
                let rows = query_all(
                    &conn,
                    "SELECT channel_id, user_id, guest_server_id, created_at FROM channel_blocks ORDER BY created_at DESC",
                    (),
                    |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?, r.get::<String>(2)?, r.get::<i64>(3)?)),
                )
                .await?;
                let servers = guest_servers(&conn).await?;
                let profiles: HashMap<String, pb::User> =
                    users(&conn, &rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>())
                        .await?
                        .into_iter()
                        .map(|u| (u.id.clone(), u))
                        .collect();
                let blocks = rows
                    .into_iter()
                    .map(|(channel_id, user_id, server_id, at)| pb::ChannelBlock {
                        channel_id,
                        user: profiles.get(&user_id).cloned(),
                        server: Some(
                            servers
                                .get(&server_id)
                                .cloned()
                                .unwrap_or(pb::SharedServer { id: server_id, ..Default::default() }),
                        ),
                        created_at: Some(timestamp(at)),
                    })
                    .collect();
                Ok(pb::ListConnectionsResponse { connections, codes, blocks })
            }
            .await,
        )
    }

    async fn update_connection(
        &self,
        request: Request<pb::UpdateConnectionRequest>,
    ) -> Result<Response<pb::UpdateConnectionResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                let allowed = allowed_from(&req.allowed)?;
                let row = guest_by_id(&sdb.read()?, &req.connection_id).await?.ok_or(Error::NotFound("connection"))?;
                access.require_in(&row.channel_id, Permission::ManageChannels)?;
                let (row, channel_name) = sdb
                    .write(&account.id, async |conn, events| {
                        let before = guest_by_id(conn, &req.connection_id).await?.ok_or(Error::NotFound("connection"))?;
                        conn.execute(
                            "UPDATE channel_guests SET allowed = ?2 WHERE id = ?1",
                            (before.id.as_str(), allowed as i64),
                        )
                        .await?;
                        let channel = load_channel(conn, &sdb.id, &before.channel_id).await?.map(|c| c.name).unwrap_or_default();
                        let label = |bits: Bits| {
                            permissions::to_list(bits)
                                .into_iter()
                                .filter_map(|p| Permission::try_from(p).ok().map(permissions::label))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };
                        let entry = Audit::new(pb::AuditAction::SharedChannelUpdate, &before.server.id)
                            .channel(&channel)
                            .change("allowed", label(before.allowed), label(allowed));
                        if !entry.changes.is_empty() {
                            store::audit(conn, &account.id, entry).await?;
                        }
                        shared_changed(events);
                        let row = guest_by_id(conn, &before.id).await?.ok_or(Error::NotFound("connection"))?;
                        Ok((row, channel))
                    })
                    .await?;
                let connection = home_connection(&row, &channel_name);
                if row.active {
                    let call = Call::Updated(cpb::HomeUpdated { connection: Some(connection.clone()) });
                    if let Err(err) =
                        self.app.shared(cpb::SharedCall { server_id: row.server.id.clone(), call: Some(call) }).await
                    {
                        tracing::info!(error = %err, "couldn't tell a server what its people may do in a shared channel");
                    }
                }
                Ok(pb::UpdateConnectionResponse { connection: Some(connection) })
            }
            .await,
        )
    }

    async fn disconnect(
        &self,
        request: Request<pb::DisconnectRequest>,
    ) -> Result<Response<pb::DisconnectResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                let (ended, channel_id) = sdb
                    .write(&account.id, async |conn, events| {
                        // At the home: the server it's shown in goes.
                        if let Some(row) = guest_by_id(conn, &req.connection_id).await? {
                            let channel =
                                load_channel(conn, &sdb.id, &row.channel_id).await?.map(|c| c.name).unwrap_or_default();
                            drop_guest(conn, &sdb.id, &row.id, events).await?;
                            store::audit(
                                conn,
                                &account.id,
                                Audit::new(pb::AuditAction::SharedChannelDisconnect, &row.server.id)
                                    .channel(channel)
                                    .change("server", "", &row.server.name),
                            )
                            .await?;
                            return Ok((Ended { guests: vec![(row.id, row.server.id)], links: vec![] }, None));
                        }
                        // At a guest: its channel goes.
                        let link = link_by_id(conn, &req.connection_id).await?.ok_or(Error::NotFound("connection"))?;
                        let channel_id = drop_link(conn, &sdb.id, &link.id, events).await?;
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::SharedChannelDisconnect, &link.home.id)
                                .channel(&link.home_channel_name)
                                .change("server", "", &link.home.name),
                        )
                        .await?;
                        Ok((Ended { guests: vec![], links: vec![(link.id, link.home.id)] }, channel_id))
                    })
                    .await?;
                if let Some(channel_id) = channel_id {
                    self.app.forget_notifications(&sdb.id, Some(&channel_id), None).await;
                }
                tell_ended(&self.app, &sdb.id, &account.id, ended).await;
                Ok(pb::DisconnectResponse {})
            }
            .await,
        )
    }

    async fn block_from_channel(
        &self,
        request: Request<pb::BlockFromChannelRequest>,
    ) -> Result<Response<pb::BlockFromChannelResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::KickMembers).await?;
                access.require_in(&req.channel_id, Permission::ViewChannels)?;
                sdb.write(&account.id, async |conn, events| {
                    let channel = load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                    if req.blocked {
                        let guest_of = guests_among(conn, &[req.user_id.as_str()]).await?;
                        let Some(server_id) = guest_of.get(&req.user_id) else {
                            return Err(Error::invalid(
                                "only people from other servers can be kept out of a shared channel; members here are kicked or banned",
                            ));
                        };
                        keep_out(conn, &channel, &req.user_id, server_id, &account.id, "", events).await
                    } else {
                        let removed = conn
                            .execute(
                                "DELETE FROM channel_blocks WHERE channel_id = ?1 AND user_id = ?2",
                                (channel.id.as_str(), req.user_id.as_str()),
                            )
                            .await?;
                        if removed == 0 {
                            return Err(Error::NotFound("block"));
                        }
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::SharedChannelUnblock, &req.user_id).channel(&channel.name),
                        )
                        .await?;
                        shared_changed(events);
                        Ok(())
                    }
                })
                .await?;
                Ok(pb::BlockFromChannelResponse {})
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_name_their_home() {
        let server = new_id();
        let code = new_code(&server);
        assert_eq!(code_home(&code).unwrap(), server);
        assert_eq!(code_home(&format!("  {code} ")).unwrap(), server);
        assert!(code_home("nope").is_err());
        assert!(code_home(&format!("{server}-short")).is_err());
        assert!(code_home(&format!("not-an-id-{}", "a".repeat(CODE_LENGTH))).is_err());
    }

    #[test]
    fn guests_get_no_more_than_shareable() {
        assert_eq!(allowed_from(&[Permission::SendMessages as i32]).unwrap(), bit(Permission::SendMessages));
        assert_eq!(allowed_from(&[Permission::ViewChannels as i32]).unwrap(), 0);
        assert!(allowed_from(&[Permission::ManageMessages as i32]).is_err());
        assert!(allowed_from(&[Permission::MentionEveryone as i32]).is_err());
    }
}
