use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, Seat, automod, respond, shared, url, users};
use crate::app::App;
use crate::db::{is_unique_violation, query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, Permission, message_service_server::MessageService};
use crate::permissions::{self, Access};
use crate::servers::{self as store, Audit, Payload, UsageChange, load_channel};

/// The longest a message can be, in characters.
pub const MAX_MESSAGE_LENGTH: usize = 4000;
const MAX_ATTACHMENTS: usize = 10;
const MAX_EMBEDS: usize = 10;
/// Most roles one message pings.
const MAX_ROLE_MENTIONS: usize = 50;

/// What's stored in a message's `extras` column.
#[derive(Clone, PartialEq, prost::Message)]
struct Extras {
    #[prost(message, repeated, tag = "1")]
    attachments: Vec<pb::Attachment>,
    #[prost(message, repeated, tag = "2")]
    embeds: Vec<pb::Embed>,
    #[prost(bool, tag = "3")]
    mentions_everyone: bool,
    #[prost(string, repeated, tag = "4")]
    mention_role_ids: Vec<String>,
    #[prost(message, optional, tag = "5")]
    auto_mod: Option<pb::AutoModAlert>,
    #[prost(message, optional, tag = "6")]
    webhook: Option<pb::MessageWebhook>,
}

impl Extras {
    fn of(message: &pb::Message) -> Option<Vec<u8>> {
        let extras = Extras {
            attachments: message.attachments.clone(),
            embeds: message.embeds.clone(),
            mentions_everyone: message.mentions_everyone,
            mention_role_ids: message.mention_role_ids.clone(),
            auto_mod: message.auto_mod.clone(),
            webhook: message.webhook.clone(),
        };
        (extras != Extras::default()).then(|| extras.encode_to_vec())
    }
}

/// Whether `content` says @everyone or @here as a word of its own.
fn says_everyone(content: &str) -> bool {
    let bytes = content.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    content.match_indices('@').any(|(at, _)| {
        if at > 0 && (word(bytes[at - 1]) || bytes[at - 1] == b'@') {
            return false;
        }
        let rest = &bytes[at + 1..];
        ["everyone", "here"].iter().any(|name| {
            rest.len() >= name.len()
                && rest[..name.len()].eq_ignore_ascii_case(name.as_bytes())
                && rest.get(name.len()).is_none_or(|&b| !word(b))
        })
    })
}

/// The ids written as `<@&id>` in `content`, once each, in order.
fn role_tokens(content: &str) -> Vec<&str> {
    let mut ids = Vec::new();
    for (start, _) in content.match_indices("<@&") {
        let rest = &content[start + 3..];
        if let Some(end) = rest.find('>')
            && end > 0
            && end <= 32
            && rest[..end].bytes().all(|b| b.is_ascii_alphanumeric())
            && !ids.contains(&&rest[..end])
        {
            ids.push(&rest[..end]);
        }
    }
    ids
}

/// Who a message pings, given what its author can do in its channel: everyone
/// if they may, and the roles it names that are mentionable or that they may
/// mention anyway.
async fn mentions(
    conn: &turso::Connection,
    server_id: &str,
    access: &Access,
    channel_id: &str,
    content: &str,
) -> Result<(bool, Vec<String>)> {
    let anyone = access.has_in(channel_id, Permission::MentionEveryone);
    let everyone = anyone && says_everyone(content);
    let named = role_tokens(content);
    if named.is_empty() {
        return Ok((everyone, vec![]));
    }
    let roles = permissions::roles(conn, server_id).await?;
    let mut ids = Vec::new();
    for id in named {
        if let Some(role) = roles.iter().find(|r| r.id.eq_ignore_ascii_case(id) && r.id != server_id)
            && (role.mentionable || anyone)
            && ids.len() < MAX_ROLE_MENTIONS
        {
            ids.push(role.id.clone());
        }
    }
    Ok((everyone, ids))
}

const MESSAGE_COLUMNS: &str = "id, channel_id, author_id, content, extras, reply_to_id, created_at, edited_at, kind";

fn message_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<(pb::Message, Option<Vec<u8>>)> + '_ {
    move |r| {
        Ok((
            pb::Message {
                id: r.get(0)?,
                server_id: server_id.to_string(),
                channel_id: r.get(1)?,
                author_id: r.get(2)?,
                content: r.get(3)?,
                attachments: vec![],
                embeds: vec![],
                reply_to_id: r.get::<Option<String>>(5)?.unwrap_or_default(),
                created_at: Some(timestamp(r.get(6)?)),
                edited_at: r.get::<Option<i64>>(7)?.map(timestamp),
                kind: r.get(8)?,
                mentions_everyone: false,
                mention_role_ids: vec![],
                auto_mod: None,
                webhook: None,
                shared: None,
            },
            r.get::<Option<Vec<u8>>>(4)?,
        ))
    }
}

/// A message's attachments and embeds, as stored.
pub(super) fn decode_extras(bytes: &[u8]) -> Result<(Vec<pb::Attachment>, Vec<pb::Embed>)> {
    let extras = Extras::decode(bytes)?;
    Ok((extras.attachments, extras.embeds))
}

fn with_extras((mut message, extras): (pb::Message, Option<Vec<u8>>)) -> Result<pb::Message> {
    if let Some(bytes) = extras {
        let extras = Extras::decode(bytes.as_slice())?;
        message.attachments = extras.attachments;
        message.embeds = extras.embeds;
        message.mentions_everyone = extras.mentions_everyone;
        message.mention_role_ids = extras.mention_role_ids;
        message.auto_mod = extras.auto_mod;
        message.webhook = extras.webhook;
    }
    Ok(message)
}

pub(super) async fn load_message(
    conn: &turso::Connection,
    server_id: &str,
    message_id: &str,
) -> Result<Option<pb::Message>> {
    query_one(
        conn,
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1"),
        [message_id],
        message_row(server_id),
    )
    .await?
    .map(with_extras)
    .transpose()
}

pub(super) fn check_content(content: &str, has_extras: bool) -> Result<()> {
    if content.trim().is_empty() && !has_extras {
        return Err(Error::invalid("a message needs text, an attachment or an embed"));
    }
    if content.chars().count() > MAX_MESSAGE_LENGTH {
        return Err(Error::invalid(format!("messages can be at most {MAX_MESSAGE_LENGTH} characters")));
    }
    Ok(())
}

pub(super) fn check_extras(attachments: &mut [pb::Attachment], embeds: &[pb::Embed]) -> Result<()> {
    if attachments.len() > MAX_ATTACHMENTS || embeds.len() > MAX_EMBEDS {
        return Err(Error::invalid(format!(
            "at most {MAX_ATTACHMENTS} attachments and {MAX_EMBEDS} embeds per message"
        )));
    }
    for attachment in attachments.iter_mut() {
        attachment.url = url("attachment url", &attachment.url)?;
        if attachment.url.is_empty() || attachment.filename.chars().count() > 255 {
            return Err(Error::invalid("attachments need a URL and a filename of at most 255 characters"));
        }
        attachment.id = new_id();
    }
    Ok(())
}

/// Refuses members who are timed out.
pub(super) fn check_not_timed_out(member: &pb::Member) -> Result<()> {
    if let Some(until) = &member.timed_out_until {
        let until = crate::id::millis(until);
        if until > now_ms() {
            return Err(Error::denied(format!("you're timed out for {}", wait(until - now_ms()))));
        }
    }
    Ok(())
}

/// "12 seconds", "5 minutes", "3 hours", "2 days": roughly how long, rounded up.
fn wait(ms: i64) -> String {
    let seconds = (ms + 999) / 1000;
    let (n, unit) = match seconds {
        ..60 => (seconds, "second"),
        60..3600 => ((seconds + 59) / 60, "minute"),
        3600..86400 => ((seconds + 3599) / 3600, "hour"),
        _ => ((seconds + 86399) / 86400, "day"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Holds a member to a channel's slow mode, inside the write that sends their
/// message: they wait the channel's time between messages. One row per member
/// and channel, so two sent at once clash and the second is turned away.
pub(super) async fn check_slowmode(
    conn: &turso::Connection,
    channel: &pb::Channel,
    user_id: &str,
    now: i64,
) -> Result<()> {
    if channel.slowmode_seconds <= 0 {
        return Ok(());
    }
    let period = i64::from(channel.slowmode_seconds) * 1000;
    let slowed = |sent_at: i64| {
        Error::ResourceExhausted(format!("slow mode is on; you can send again in {}", wait(sent_at + period - now)))
    };
    let last = query_one(
        conn,
        "SELECT sent_at FROM slowmode WHERE channel_id = ?1 AND user_id = ?2",
        (channel.id.as_str(), user_id),
        |r| r.get::<i64>(0),
    )
    .await?;
    match last {
        Some(sent_at) if now < sent_at + period => Err(slowed(sent_at)),
        Some(_) => {
            conn.execute(
                "UPDATE slowmode SET sent_at = ?3 WHERE channel_id = ?1 AND user_id = ?2",
                (channel.id.as_str(), user_id, now),
            )
            .await?;
            Ok(())
        }
        None => match conn
            .execute(
                "INSERT INTO slowmode (channel_id, user_id, sent_at) VALUES (?1, ?2, ?3)",
                (channel.id.as_str(), user_id, now),
            )
            .await
            .map_err(Error::from)
        {
            Ok(_) => Ok(()),
            Err(err) if is_unique_violation(&err) => Err(slowed(now)),
            Err(err) => Err(err),
        },
    }
}

/// What a webhook posts: text and embeds, under a name and picture.
#[derive(Clone)]
pub struct WebhookMessage {
    pub content: String,
    pub embeds: Vec<pb::Embed>,
    pub author: pb::MessageWebhook,
}

/// Checks a webhook's post as a member's message is checked.
pub(super) fn check_webhook_message(app: &App, post: &mut WebhookMessage) -> Result<()> {
    check_content(&post.content, !post.embeds.is_empty())?;
    check_extras(&mut [], &post.embeds)?;
    check_embed_links(app, &mut post.embeds)
}

/// Embeds link only to http(s), and their pictures come through the
/// instance, so nobody who reads the message is seen by the site they're on.
pub(super) fn check_embed_links(app: &App, embeds: &mut [pb::Embed]) -> Result<()> {
    for embed in embeds {
        embed.url = url("embed url", &embed.url)?;
        embed.thumbnail_url = app.picture_link(&url("embed thumbnail", &embed.thumbnail_url)?);
        embed.image_url = app.picture_link(&url("embed image", &embed.image_url)?);
    }
    Ok(())
}

/// Stores a webhook's message in `channel`, inside a write, and sends its
/// event. Webhooks ping nobody but the people they name.
pub(super) async fn insert_webhook_message(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
    post: WebhookMessage,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<pb::Message> {
    let message = pb::Message {
        id: new_id(),
        server_id: server_id.to_string(),
        channel_id: channel_id.to_string(),
        author_id: post.author.webhook_id.clone(),
        content: post.content,
        embeds: post.embeds,
        created_at: Some(timestamp(now)),
        webhook: Some(post.author),
        ..Default::default()
    };
    let size = message.content.len() as i64;
    conn.execute(
        "INSERT INTO messages (id, channel_id, author_id, content, size, extras, attachment_count, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        (
            message.id.as_str(),
            message.channel_id.as_str(),
            message.author_id.as_str(),
            message.content.as_str(),
            size,
            Extras::of(&message),
            now,
        ),
    )
    .await?;
    store::add_usage(conn, UsageChange { messages: 1, messages_sent: 1, message_bytes: size, ..Default::default() })
        .await?;
    events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message.clone()) }));
    Ok(message)
}

/// Stores a message that isn't a plain one (it has no text of its own),
/// inside a write. The caller counts it and sends its event.
pub(super) async fn insert_system(conn: &turso::Connection, message: &pb::Message) -> Result<()> {
    conn.execute(
        "INSERT INTO messages (id, channel_id, author_id, content, size, extras, kind, created_at)
         VALUES (?1, ?2, ?3, '', 0, ?4, ?5, ?6)",
        (
            message.id.as_str(),
            message.channel_id.as_str(),
            message.author_id.as_str(),
            Extras::of(message),
            message.kind as i64,
            message.created_at.as_ref().map_or_else(now_ms, crate::id::millis),
        ),
    )
    .await?;
    Ok(())
}

/// Posts "someone joined" in the server's system channel, if it has one,
/// inside the write that adds them.
pub(super) async fn post_join(
    conn: &turso::Connection,
    server: &pb::Server,
    user_id: &str,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<()> {
    if server.system_channel_id.is_empty() {
        return Ok(());
    }
    let Some(channel) = load_channel(conn, &server.id, &server.system_channel_id).await? else {
        return Ok(());
    };
    let message = pb::Message {
        id: new_id(),
        server_id: server.id.clone(),
        channel_id: channel.id,
        author_id: user_id.to_string(),
        created_at: Some(timestamp(now)),
        kind: pb::MessageKind::MemberJoined as i32,
        ..Default::default()
    };
    insert_system(conn, &message).await?;
    store::add_usage(conn, UsageChange { messages: 1, messages_sent: 1, ..Default::default() }).await?;
    events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message) }));
    Ok(())
}

/// Stores a member's message, inside a write, and counts it. The caller
/// sends its event.
pub(super) async fn insert_message(conn: &turso::Connection, message: &pb::Message, now: i64) -> Result<()> {
    let size = message.content.len() as i64;
    let attachment_count = message.attachments.len() as i64;
    conn.execute(
        "INSERT INTO messages (id, channel_id, author_id, content, size, extras, attachment_count, reply_to_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
            message.id.as_str(),
            message.channel_id.as_str(),
            message.author_id.as_str(),
            message.content.as_str(),
            size,
            Extras::of(message),
            attachment_count,
            (!message.reply_to_id.is_empty()).then_some(message.reply_to_id.as_str()),
            now,
        ),
    )
    .await?;
    store::add_usage(
        conn,
        UsageChange {
            messages: 1,
            messages_sent: 1,
            message_bytes: size,
            attachments: attachment_count,
            ..Default::default()
        },
    )
    .await
}

/// Saves a message's new text, inside a write, and counts the change. The
/// caller sends its event.
pub(super) async fn save_edit(conn: &turso::Connection, message: &pb::Message, old_size: i64) -> Result<()> {
    let now = message.edited_at.as_ref().map_or_else(now_ms, crate::id::millis);
    conn.execute(
        "UPDATE messages SET content = ?2, size = ?3, edited_at = ?4, extras = ?5 WHERE id = ?1",
        (message.id.as_str(), message.content.as_str(), message.content.len() as i64, now, Extras::of(message)),
    )
    .await?;
    store::add_usage(conn, UsageChange { message_bytes: message.content.len() as i64 - old_size, ..Default::default() })
        .await
}

/// Deletes a message, inside a write, and takes it off the totals. The
/// caller sends its event.
pub(super) async fn remove_message(conn: &turso::Connection, message: &pb::Message) -> Result<()> {
    conn.execute("DELETE FROM messages WHERE id = ?1", [message.id.as_str()]).await?;
    store::add_usage(
        conn,
        UsageChange {
            messages: -1,
            message_bytes: -(message.content.len() as i64),
            attachments: -(message.attachments.len() as i64),
            ..Default::default()
        },
    )
    .await
}

/// A page of a channel's messages, oldest first, and whether there are more
/// beyond it. `plain` leaves out what isn't a message someone wrote (join
/// messages, AutoMod alerts), as other servers showing the channel see it.
pub(super) async fn page(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
    limit: i32,
    before_id: &str,
    after_id: &str,
    plain: bool,
) -> Result<(Vec<pb::Message>, bool)> {
    let limit = if limit <= 0 { 50 } else { limit.min(100) } as i64;
    let (condition, cursor, newest_first) = match (before_id.is_empty(), after_id.is_empty()) {
        (false, _) => ("AND id < ?2", before_id, true),
        (true, false) => ("AND id > ?2", after_id, false),
        (true, true) => ("AND ?2 = ''", "", true),
    };
    let order = if newest_first { "DESC" } else { "ASC" };
    let kinds = if plain { "AND kind = 0" } else { "" };
    let rows = query_all(
        conn,
        &format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE channel_id = ?1 {condition} {kinds} ORDER BY id {order} LIMIT ?3"
        ),
        (channel_id, cursor, limit + 1),
        message_row(server_id),
    )
    .await?;
    let has_more = rows.len() as i64 > limit;
    let mut messages = rows.into_iter().take(limit as usize).map(with_extras).collect::<Result<Vec<_>>>()?;
    if newest_first {
        messages.reverse();
    }
    Ok((messages, has_more))
}

async fn authors(conn: &turso::Connection, messages: &[pb::Message]) -> Result<Vec<pb::User>> {
    users(conn, &messages.iter().map(|m| m.author_id.as_str()).collect::<Vec<_>>()).await
}

#[tonic::async_trait]
impl MessageService for Api {
    async fn send_message(
        &self,
        request: Request<pb::SendMessageRequest>,
    ) -> Result<Response<pb::SendMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let mut req = request.into_inner();
                let Seat { sdb, member, access } = self.membership(&account, &req.server_id).await?;
                check_not_timed_out(&member)?;
                access.require_in(&req.channel_id, Permission::SendMessages)?;
                if !req.attachments.is_empty() {
                    access.require_in(&req.channel_id, Permission::AttachFiles)?;
                }
                if !req.embeds.is_empty() {
                    access.require_in(&req.channel_id, Permission::EmbedLinks)?;
                }
                check_content(&req.content, !req.attachments.is_empty() || !req.embeds.is_empty())?;
                check_extras(&mut req.attachments, &req.embeds)?;
                check_embed_links(&self.app, &mut req.embeds)?;
                if let Some(link) = shared::link_of(&sdb.read()?, &req.channel_id).await? {
                    let message = shared::guest_send(&self.app, &sdb, &account, &member, &access, &link, req).await?;
                    return Ok(pb::SendMessageResponse { message: Some(message) });
                }
                let limits = sdb.limits(&self.app.settings().limits).await?;
                if let Some(limit) = limits.storage_bytes
                    && sdb.storage_bytes() >= limit
                {
                    return Err(Error::ResourceExhausted("this server is out of storage".into()));
                }
                let pictures = automod::picture_links(&req.attachments, &req.embeds);
                let (asked, later) =
                    automod::ask_soon(&self.app, &sdb, &member, &access, &req.channel_id, &req.content, &pictures)
                        .await;
                let message = sdb
                    .write(&account.id, async |conn, events| {
                        let channel =
                            load_channel(conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                        if !matches!(
                            pb::ChannelType::try_from(channel.r#type),
                            Ok(pb::ChannelType::Text | pb::ChannelType::Announcement | pb::ChannelType::Thread)
                        ) {
                            return Err(Error::invalid("messages can only go in text channels"));
                        }
                        if !req.reply_to_id.is_empty() {
                            let replied = load_message(conn, &sdb.id, &req.reply_to_id).await?;
                            if replied.is_none_or(|m| m.channel_id != channel.id) {
                                return Err(Error::NotFound("message being replied to"));
                            }
                        }
                        let verdict = automod::review(
                            conn,
                            &sdb.id,
                            &member,
                            &access,
                            &channel,
                            &req.content,
                            asked.as_ref(),
                            events,
                        )
                        .await?;
                        if let Some(why) = verdict.blocked {
                            return Ok(Err(why));
                        }
                        let now = now_ms();
                        let exempt = access.has_in(&channel.id, Permission::ManageMessages)
                            || access.has_in(&channel.id, Permission::ManageChannels);
                        if !exempt {
                            check_slowmode(conn, &channel, &account.id, now).await?;
                        }
                        let (mentions_everyone, mention_role_ids) =
                            mentions(conn, &sdb.id, &access, &channel.id, &req.content).await?;
                        let message = pb::Message {
                            id: new_id(),
                            server_id: sdb.id.clone(),
                            channel_id: channel.id.clone(),
                            author_id: account.id.clone(),
                            content: req.content.clone(),
                            attachments: req.attachments.clone(),
                            embeds: req.embeds.clone(),
                            reply_to_id: req.reply_to_id.clone(),
                            created_at: Some(timestamp(now)),
                            edited_at: None,
                            kind: pb::MessageKind::Unspecified as i32,
                            mentions_everyone,
                            mention_role_ids,
                            auto_mod: None,
                            webhook: None,
                            shared: None,
                        };
                        insert_message(conn, &message, now).await?;
                        events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message.clone()) }));
                        Ok(Ok(message))
                    })
                    .await?
                    .map_err(Error::denied)?;
                if let Some(checking) = later {
                    checking.later(sdb.clone(), member, message.id.clone(), message.content.clone());
                }
                Ok(pb::SendMessageResponse { message: Some(message) })
            }
            .await,
        )
    }

    async fn get_message(
        &self,
        request: Request<pb::GetMessageRequest>,
    ) -> Result<Response<pb::GetMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                let conn = sdb.read()?;
                if let Some((link, guest)) =
                    shared::locate(&self.app, &conn, &sdb.id, &account, &access, &req.channel_id, &req.message_id)
                        .await?
                {
                    return shared::guest_get(&self.app, &sdb.id, &link, guest, &req.message_id).await;
                }
                let mut message = load_message(&conn, &sdb.id, &req.message_id)
                    .await?
                    .filter(|m| access.can_see(&m.channel_id))
                    .ok_or(Error::NotFound("message"))?;
                shared::mark_guests(&conn, std::slice::from_mut(&mut message)).await?;
                let author = authors(&conn, std::slice::from_ref(&message)).await?.into_iter().next();
                Ok(pb::GetMessageResponse { message: Some(message), author })
            }
            .await,
        )
    }

    async fn list_messages(
        &self,
        request: Request<pb::ListMessagesRequest>,
    ) -> Result<Response<pb::ListMessagesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_in(&req.channel_id, Permission::ViewChannels)?;
                let conn = sdb.read()?;
                load_channel(&conn, &sdb.id, &req.channel_id).await?.ok_or(Error::NotFound("channel"))?;
                if let Some(link) = shared::link_of(&conn, &req.channel_id).await? {
                    let guest = shared::guest_of(&conn, &sdb.id, &account, &access, &link).await?;
                    return shared::guest_list(&self.app, &sdb.id, &link, guest, &req).await;
                }
                let (mut messages, has_more) =
                    page(&conn, &sdb.id, &req.channel_id, req.limit, &req.before_id, &req.after_id, false).await?;
                shared::mark_guests(&conn, &mut messages).await?;
                let authors = authors(&conn, &messages).await?;
                Ok(pb::ListMessagesResponse { messages, authors, has_more })
            }
            .await,
        )
    }

    async fn update_message(
        &self,
        request: Request<pb::UpdateMessageRequest>,
    ) -> Result<Response<pb::UpdateMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, member, access } = self.membership(&account, &req.server_id).await?;
                check_not_timed_out(&member)?;
                let located = shared::locate(
                    &self.app,
                    &sdb.read()?,
                    &sdb.id,
                    &account,
                    &access,
                    &req.channel_id,
                    &req.message_id,
                )
                .await?;
                if let Some((link, guest)) = located {
                    check_content(&req.content, true)?;
                    let message = shared::guest_edit(&self.app, &sdb, &member, &access, &link, guest, &req).await?;
                    return Ok(pb::UpdateMessageResponse { message: Some(message) });
                }
                // A provider is asked before the write, about new text the author wrote.
                let before = load_message(&sdb.read()?, &sdb.id, &req.message_id).await?;
                let (asked, later) = match before {
                    Some(m) if m.author_id == account.id && m.content != req.content => {
                        automod::ask_soon(&self.app, &sdb, &member, &access, &m.channel_id, &req.content, &[]).await
                    }
                    _ => (None, None),
                };
                let message = sdb
                    .write(&account.id, async |conn, events| {
                        let mut message = load_message(conn, &sdb.id, &req.message_id)
                            .await?
                            .filter(|m| access.can_see(&m.channel_id))
                            .ok_or(Error::NotFound("message"))?;
                        if message.author_id != account.id {
                            return Err(Error::denied("you can only edit your own messages"));
                        }
                        if message.kind != pb::MessageKind::Unspecified as i32 {
                            return Err(Error::invalid("system messages can't be edited"));
                        }
                        check_content(&req.content, !message.attachments.is_empty() || !message.embeds.is_empty())?;
                        if message.content != req.content {
                            let channel = load_channel(conn, &sdb.id, &message.channel_id)
                                .await?
                                .ok_or(Error::NotFound("channel"))?;
                            let verdict = automod::review(
                                conn,
                                &sdb.id,
                                &member,
                                &access,
                                &channel,
                                &req.content,
                                asked.as_ref(),
                                events,
                            )
                            .await?;
                            if let Some(why) = verdict.blocked {
                                return Ok(Err(why));
                            }
                        }
                        let now = now_ms();
                        let growth = req.content.len() as i64 - message.content.len() as i64;
                        (message.mentions_everyone, message.mention_role_ids) =
                            mentions(conn, &sdb.id, &access, &message.channel_id, &req.content).await?;
                        message.content = req.content.clone();
                        message.edited_at = Some(timestamp(now));
                        conn.execute(
                            "UPDATE messages SET content = ?2, size = ?3, edited_at = ?4, extras = ?5 WHERE id = ?1",
                            (
                                message.id.as_str(),
                                req.content.as_str(),
                                req.content.len() as i64,
                                now,
                                Extras::of(&message),
                            ),
                        )
                        .await?;
                        store::add_usage(conn, UsageChange { message_bytes: growth, ..Default::default() }).await?;
                        events.push(Payload::MessageUpdated(pb::MessageUpdated { message: Some(message.clone()) }));
                        Ok(Ok(message))
                    })
                    .await?
                    .map_err(Error::denied)?;
                if let Some(checking) = later {
                    checking.later(sdb.clone(), member, message.id.clone(), message.content.clone());
                }
                Ok(pb::UpdateMessageResponse { message: Some(message) })
            }
            .await,
        )
    }

    async fn delete_message(
        &self,
        request: Request<pb::DeleteMessageRequest>,
    ) -> Result<Response<pb::DeleteMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_not_timed_out()?;
                let located = shared::locate(
                    &self.app,
                    &sdb.read()?,
                    &sdb.id,
                    &account,
                    &access,
                    &req.channel_id,
                    &req.message_id,
                )
                .await?;
                if let Some((link, guest)) = located {
                    shared::guest_delete(&self.app, &sdb.id, &link, guest, &req.message_id).await?;
                    return Ok(pb::DeleteMessageResponse {});
                }
                sdb.write(&account.id, async |conn, events| {
                    let message = load_message(conn, &sdb.id, &req.message_id)
                        .await?
                        .filter(|m| access.can_see(&m.channel_id))
                        .ok_or(Error::NotFound("message"))?;
                    if message.author_id != account.id
                        && !access.has_in(&message.channel_id, Permission::ManageMessages)
                    {
                        return Err(Error::denied("you can only delete your own messages"));
                    }
                    conn.execute("DELETE FROM messages WHERE id = ?1", [message.id.as_str()]).await?;
                    if message.author_id != account.id {
                        let channel =
                            load_channel(conn, &sdb.id, &message.channel_id).await?.map(|c| c.name).unwrap_or_default();
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::MessageDelete, &message.author_id).channel(channel),
                        )
                        .await?;
                    }
                    store::add_usage(
                        conn,
                        UsageChange {
                            messages: -1,
                            message_bytes: -(message.content.len() as i64),
                            attachments: -(message.attachments.len() as i64),
                            ..Default::default()
                        },
                    )
                    .await?;
                    events.push(Payload::MessageDeleted(pb::MessageDeleted {
                        channel_id: message.channel_id.clone(),
                        message_id: message.id.clone(),
                    }));
                    Ok(())
                })
                .await?;
                Ok(pb::DeleteMessageResponse {})
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_everyone_and_roles() {
        assert!(says_everyone("@everyone look"));
        assert!(says_everyone("hey @Here!"));
        assert!(!says_everyone("mail@everyone.com"));
        assert!(!says_everyone("@everyoneelse"));
        assert!(!says_everyone("@@here"));
        assert_eq!(role_tokens("<@&ABC> and <@&ABC>, <@&> <@&D-E> <@&FG>"), ["ABC", "FG"]);
    }
}
