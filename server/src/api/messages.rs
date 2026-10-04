use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, Seat, automod, respond, shared, threads, url, users};
use crate::app::App;
use crate::attachments;
use crate::db::{is_unique_violation, query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::media;
use crate::pb::{self, Permission, message_service_server::MessageService};
use crate::permissions::{self, Access};
use crate::servers::{self as store, Audit, Payload, UsageChange, load_channel};

/// The longest a message can be, in characters.
pub const MAX_MESSAGE_LENGTH: usize = 4000;
const MAX_ATTACHMENTS: usize = 10;
/// Why a file can't go in a channel shared from another server.
pub(super) const NO_SHARED_FILES: &str = "files can't be sent in a channel shared with another server yet";
const MAX_EMBEDS: usize = 10;
/// Most roles one message pings.
const MAX_ROLE_MENTIONS: usize = 50;
/// Most members one message names that it says it mentions.
const MAX_USER_MENTIONS: usize = 50;

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
    #[prost(message, repeated, tag = "7")]
    emojis: Vec<pb::Emoji>,
    #[prost(message, optional, tag = "9")]
    gif: Option<pb::MessageGif>,
    #[prost(string, repeated, tag = "10")]
    mention_user_ids: Vec<String>,
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
            emojis: message.emojis.clone(),
            gif: message.gif.clone(),
            mention_user_ids: message.mention_user_ids.clone(),
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

/// The ids of the custom emoji written as `<:name:id>` or `<a:name:id>` in
/// `content`, once each, in order. One pass: a token is at most 70 bytes, so
/// its end is looked for only that far.
fn emoji_tokens(content: &str) -> Vec<&str> {
    const LONGEST: usize = 70;
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bytes = content.as_bytes();
    for (start, _) in content.match_indices('<') {
        let window = &bytes[start + 1..bytes.len().min(start + 1 + LONGEST)];
        let Some(end) = window.iter().position(|&b| b == b'>') else { continue };
        // Everything before '>' in a token is ASCII, so this slice is on char
        // boundaries whenever it can be one.
        let Ok(token) = std::str::from_utf8(&window[..end]) else { continue };
        let token = token.strip_prefix('a').unwrap_or(token);
        let Some(token) = token.strip_prefix(':') else { continue };
        let Some((name, id)) = token.split_once(':') else { continue };
        let word = |s: &str, min: usize, under: bool| {
            (min..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || (under && b == b'_'))
        };
        if word(name, 2, true) && word(id, 10, false) && seen.insert(id) {
            ids.push(id);
        }
    }
    ids
}

/// The emoji from the author's other servers that `content` uses, of the
/// ones the app sent along, checked (see [`App::check_emojis`]). Emoji of
/// `server_id` itself are left out: everyone there has them already. When
/// checking fails the message still goes, and they show as their names.
async fn outside_emojis(
    app: &App,
    account_id: &str,
    server_id: &str,
    content: &str,
    sent: Vec<pb::Emoji>,
) -> Vec<pb::Emoji> {
    let used: std::collections::HashSet<&str> = emoji_tokens(content).into_iter().collect();
    let mut taken = std::collections::HashSet::new();
    let mut wanted: Vec<pb::Emoji> = Vec::new();
    for emoji in sent {
        if wanted.len() == crate::cluster::calls::MAX_OUTSIDE_EMOJIS {
            break;
        }
        if emoji.server_id != server_id && used.contains(emoji.id.as_str()) && taken.insert(emoji.id.clone()) {
            wanted.push(emoji);
        }
    }
    match app.check_emojis(account_id, wanted).await {
        Ok(emojis) => emojis,
        Err(_) => {
            tracing::warn!("couldn't check emoji from other servers; they show as names");
            vec![]
        }
    }
}

/// An edited message's emoji from other servers: the ones it had that the
/// new text still uses, then newly checked ones.
fn kept_emojis(had: Vec<pb::Emoji>, checked: Vec<pb::Emoji>, content: &str) -> Vec<pb::Emoji> {
    let used: std::collections::HashSet<&str> = emoji_tokens(content).into_iter().collect();
    let mut kept: Vec<pb::Emoji> = Vec::new();
    for emoji in had.into_iter().chain(checked) {
        if used.contains(emoji.id.as_str())
            && !kept.iter().any(|k| k.id == emoji.id)
            && kept.len() < crate::cluster::calls::MAX_OUTSIDE_EMOJIS
        {
            kept.push(emoji);
        }
    }
    kept
}

/// The ids written as `<@id>` or `<@!id>` in `content`, once each, in order.
fn user_tokens(content: &str) -> Vec<&str> {
    let mut ids = Vec::new();
    for (start, _) in content.match_indices("<@") {
        let rest = &content[start + 2..];
        let rest = rest.strip_prefix('!').unwrap_or(rest);
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

/// The members `content` names, for [`pb::Message::mention_user_ids`]: ids
/// of anyone not in the server are left out, so a client can trust them.
async fn mentioned_users(conn: &turso::Connection, content: &str) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    // Looked up one by one, so only so many names are looked at.
    for id in user_tokens(content).into_iter().take(2 * MAX_USER_MENTIONS) {
        if ids.len() >= MAX_USER_MENTIONS {
            break;
        }
        let member =
            query_one(conn, "SELECT user_id FROM members WHERE user_id = ?1", [id], |r| r.get::<String>(0)).await?;
        if let Some(id) = member
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    Ok(ids)
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

const MESSAGE_COLUMNS: &str =
    "id, channel_id, author_id, content, extras, reply_to_id, created_at, edited_at, kind, thread_id, in_channel";

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
                emojis: vec![],
                thread_id: r.get::<Option<String>>(9)?.unwrap_or_default(),
                thread: None,
                also_in_channel: r.get(10)?,
                gif: None,
                mention_user_ids: vec![],
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
        message.emojis = extras.emojis;
        message.gif = extras.gif;
        message.mention_user_ids = extras.mention_user_ids;
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

/// Messages in any channel with ids below `before_id`, newest first: for
/// building the search index.
pub(super) async fn older(
    conn: &turso::Connection,
    server_id: &str,
    before_id: &str,
    limit: i64,
) -> Result<Vec<pb::Message>> {
    query_all(
        conn,
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id < ?1 ORDER BY id DESC LIMIT ?2"),
        (before_id, limit),
        message_row(server_id),
    )
    .await?
    .into_iter()
    .map(with_extras)
    .collect()
}

/// The messages with these ids that are still there, in no order.
pub(super) async fn by_ids(conn: &turso::Connection, server_id: &str, ids: &[&str]) -> Result<Vec<pb::Message>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    query_all(
        conn,
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id IN ({placeholders})"),
        ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
        message_row(server_id),
    )
    .await?
    .into_iter()
    .map(with_extras)
    .collect()
}

pub(super) fn check_content(content: &str, has_extras: bool) -> Result<()> {
    if content.trim().is_empty() && !has_extras {
        return Err(Error::invalid("a message needs text, an attachment, an embed or a GIF"));
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
    for attachment in attachments.iter() {
        if attachment.url.trim().is_empty() || attachment.filename.chars().count() > attachments::MAX_NAME {
            return Err(Error::invalid("attachments need a URL and a filename of at most 255 characters"));
        }
    }
    Ok(())
}

impl Api {
    /// Checks the files a message is sent with: each one an upload of the
    /// sender's for this server, whole, in it once. Fills in what the server
    /// knows of each (its id, kind and size, its link where it's served, a
    /// safe name) and says how many bytes they come to.
    async fn check_attachments(&self, account_id: &str, server_id: &str, files: &mut [pb::Attachment]) -> Result<i64> {
        let mut total = 0;
        let mut seen = Vec::with_capacity(files.len());
        for file in files.iter_mut() {
            let Some(id) = media::id_in_url(file.url.trim()) else {
                return Err(Error::invalid("attach files by uploading them here first"));
            };
            if seen.contains(&id) {
                return Err(Error::invalid("that file is attached twice"));
            }
            let upload = self
                .app
                .check_upload(account_id, pb::MediaPurpose::Attachment, file.url.trim(), Some(server_id))
                .await?
                .ok_or(Error::NotFound("uploaded file; upload it again"))?;
            let sized = upload.content_type.starts_with("image/") || upload.content_type.starts_with("video/");
            let pixels = |n: i32| if sized { n.clamp(0, 65_535) } else { 0 };
            *file = pb::Attachment {
                url: attachments::link(&self.app, server_id, &id),
                filename: attachments::clean_name(&file.filename),
                content_type: upload.content_type,
                size: upload.size,
                width: pixels(file.width),
                height: pixels(file.height),
                id: id.clone(),
            };
            total += upload.size;
            seen.push(id);
        }
        Ok(total)
    }
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
        let left = sent_at + period - now;
        Error::Limited(format!("slow mode is on; you can send again in {}", wait(left)), left)
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
        "INSERT INTO messages (id, channel_id, author_id, content, size, extras, attachment_count, reply_to_id, created_at,
           thread_id, in_channel)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            (!message.thread_id.is_empty()).then_some(message.thread_id.as_str()),
            message.also_in_channel,
        ),
    )
    .await?;
    attachments::add(conn, message, now).await.map_err(|err| match err {
        err if is_unique_violation(&err) => Error::AlreadyExists("that file is already in a message".into()),
        err => err,
    })?;
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
/// caller sends its event, and drops the files it had (their ids come back)
/// once the write is done.
pub(super) async fn remove_message(conn: &turso::Connection, message: &pb::Message) -> Result<Vec<String>> {
    conn.execute("DELETE FROM messages WHERE id = ?1", [message.id.as_str()]).await?;
    let files = attachments::forget_message(conn, &message.id).await?;
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
    Ok(files)
}

/// A page of a channel's messages, oldest first, and whether there are more
/// beyond it: the channel's own (thread replies only when also sent to it), or
/// with `thread_id` the replies in the thread under that message. `plain`
/// leaves out what isn't a message someone wrote (join messages, AutoMod
/// alerts), as other servers showing the channel see it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn page(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
    thread_id: &str,
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
    let (scope, scope_id) = if thread_id.is_empty() {
        ("channel_id = ?1 AND (thread_id IS NULL OR in_channel = 1)", channel_id)
    } else {
        ("thread_id = ?1", thread_id)
    };
    let rows = query_all(
        conn,
        &format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE {scope} {condition} {kinds} ORDER BY id {order} LIMIT ?3"
        ),
        (scope_id, cursor, limit + 1),
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
                if !req.attachments.is_empty() || req.gif.is_some() {
                    access.require_in(&req.channel_id, Permission::AttachFiles)?;
                }
                if !req.embeds.is_empty() {
                    access.require_in(&req.channel_id, Permission::EmbedLinks)?;
                }
                // Only GIFs this instance stored and sealed.
                let gif = req.gif.take().map(|gif| crate::gifs::open_seal(&self.app, &gif)).transpose()?;
                check_content(&req.content, !req.attachments.is_empty() || !req.embeds.is_empty() || gif.is_some())?;
                check_extras(&mut req.attachments, &req.embeds)?;
                check_embed_links(&self.app, &mut req.embeds)?;
                if req.also_send_to_channel && req.thread_id.is_empty() {
                    return Err(Error::invalid("only thread replies are also sent to the channel"));
                }
                if let Some(link) = shared::link_of(&sdb.read()?, &req.channel_id).await? {
                    if !req.attachments.is_empty() {
                        return Err(Error::invalid(NO_SHARED_FILES));
                    }
                    if !req.thread_id.is_empty() {
                        return Err(Error::invalid("threads aren't in channels shared between servers yet"));
                    }
                    if gif.is_some() {
                        return Err(Error::FailedPrecondition(
                            "GIFs can't be sent in channels shared from another server yet".into(),
                        ));
                    }
                    let message = shared::guest_send(&self.app, &sdb, &account, &member, &access, &link, req).await?;
                    return Ok(pb::SendMessageResponse { message: Some(message) });
                }
                let emojis =
                    outside_emojis(&self.app, &account.id, &sdb.id, &req.content, std::mem::take(&mut req.emojis))
                        .await;
                let limits = sdb.limits(&self.app.settings().limits).await?;
                if let Some(limit) = limits.storage_bytes
                    && sdb.storage_bytes() >= limit
                {
                    return Err(Error::ResourceExhausted("this server is out of storage".into()));
                }
                let file_bytes = self.check_attachments(&account.id, &sdb.id, &mut req.attachments).await?;
                if file_bytes > 0
                    && let Some(limit) = limits.attachment_bytes
                    && sdb.usage().await?.attachment_bytes + file_bytes > limit
                {
                    return Err(Error::ResourceExhausted(format!(
                        "this server is out of room for files ({} in all)",
                        media::size_label(limit)
                    )));
                }
                let mut pictures = automod::picture_links(&req.attachments, &req.embeds);
                // The GIF too: providers read its first frame.
                pictures.extend(gif.iter().map(|gif| gif.url.clone()));
                // The Smart filter's provider is asked alongside: the message
                // goes out at once, and its answer is acted on when it comes.
                let (asked, later) = (
                    None,
                    automod::ask_after(&self.app, &sdb, &member, &access, &req.channel_id, &req.content, &pictures)
                        .await,
                );
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
                        let parent = if req.thread_id.is_empty() {
                            None
                        } else {
                            Some(threads::check_reply(conn, &sdb.id, &access, &channel, &req.thread_id).await?)
                        };
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
                        let mention_user_ids = mentioned_users(conn, &req.content).await?;
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
                            emojis: emojis.clone(),
                            thread_id: req.thread_id.clone(),
                            thread: None,
                            also_in_channel: parent.is_some() && req.also_send_to_channel,
                            gif: gif.clone(),
                            mention_user_ids,
                        };
                        // Checked again here, where no other message can take the room meanwhile.
                        if file_bytes > 0
                            && let Some(limit) = limits.attachment_bytes
                            && store::usage_count(conn, "attachment_bytes").await? + file_bytes > limit
                        {
                            return Err(Error::ResourceExhausted(format!(
                                "this server is out of room for files ({} in all)",
                                media::size_label(limit)
                            )));
                        }
                        insert_message(conn, &message, now).await?;
                        events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message.clone()) }));
                        if let Some(parent) = parent {
                            threads::follow_quietly(conn, &parent.id, &account.id).await?;
                            if parent.webhook.is_none() {
                                threads::follow_quietly(conn, &parent.id, &parent.author_id).await?;
                            }
                            threads::refresh(conn, &channel.id, &parent.id, events).await?;
                        }
                        Ok(Ok(message))
                    })
                    .await?
                    .map_err(Error::denied)?;
                for file in &message.attachments {
                    self.app.keep_picture(Some(&file.id), Some(&sdb.id)).await;
                }
                if let Some(checking) = later {
                    checking.later(self.app.clone(), sdb.clone(), member, message.id.clone(), message.content.clone());
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
                threads::attach(&conn, std::slice::from_mut(&mut message)).await?;
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
                    if !req.thread_id.is_empty() {
                        return Err(Error::NotFound("thread"));
                    }
                    let guest = shared::guest_of(&conn, &sdb.id, &account, &access, &link).await?;
                    return shared::guest_list(&self.app, &sdb.id, &link, guest, &req).await;
                }
                let mut parent = None;
                if !req.thread_id.is_empty() {
                    let mut found = load_message(&conn, &sdb.id, &req.thread_id)
                        .await?
                        .filter(|m| m.channel_id == req.channel_id && m.thread_id.is_empty())
                        .ok_or(Error::NotFound("thread"))?;
                    shared::mark_guests(&conn, std::slice::from_mut(&mut found)).await?;
                    threads::attach(&conn, std::slice::from_mut(&mut found)).await?;
                    parent = Some(found);
                }
                let (mut messages, has_more) = page(
                    &conn,
                    &sdb.id,
                    &req.channel_id,
                    &req.thread_id,
                    req.limit,
                    &req.before_id,
                    &req.after_id,
                    false,
                )
                .await?;
                shared::mark_guests(&conn, &mut messages).await?;
                threads::attach(&conn, &mut messages).await?;
                let authors = authors(&conn, &[messages.as_slice(), parent.as_slice()].concat()).await?;
                Ok(pb::ListMessagesResponse { messages, authors, has_more, parent })
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
                let mut req = request.into_inner();
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
                // Too long is refused before anything reads the text.
                check_content(&req.content, true)?;
                // A provider is asked about new text the author wrote, and its
                // answer acted on when it comes.
                let before = load_message(&sdb.read()?, &sdb.id, &req.message_id).await?;
                let sent = std::mem::take(&mut req.emojis);
                let checked = match &before {
                    Some(m) if m.author_id == account.id => {
                        outside_emojis(&self.app, &account.id, &sdb.id, &req.content, sent).await
                    }
                    _ => vec![],
                };
                let (asked, later) = match before {
                    Some(m) if m.author_id == account.id && m.content != req.content => (
                        None,
                        automod::ask_after(&self.app, &sdb, &member, &access, &m.channel_id, &req.content, &[]).await,
                    ),
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
                        check_content(
                            &req.content,
                            !message.attachments.is_empty() || !message.embeds.is_empty() || message.gif.is_some(),
                        )?;
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
                        message.mention_user_ids = mentioned_users(conn, &req.content).await?;
                        message.emojis =
                            kept_emojis(std::mem::take(&mut message.emojis), checked.clone(), &req.content);
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
                        threads::attach(conn, std::slice::from_mut(&mut message)).await?;
                        events.push(Payload::MessageUpdated(pb::MessageUpdated { message: Some(message.clone()) }));
                        Ok(Ok(message))
                    })
                    .await?
                    .map_err(Error::denied)?;
                if let Some(checking) = later {
                    checking.later(self.app.clone(), sdb.clone(), member, message.id.clone(), message.content.clone());
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
                let files = sdb
                    .write(&account.id, async |conn, events| {
                        let message = load_message(conn, &sdb.id, &req.message_id)
                            .await?
                            .filter(|m| access.can_see(&m.channel_id))
                            .ok_or(Error::NotFound("message"))?;
                        if message.author_id != account.id
                            && !access.has_in(&message.channel_id, Permission::ManageMessages)
                        {
                            return Err(Error::denied("you can only delete your own messages"));
                        }
                        let with_thread = threads::may_delete_with_thread(conn, &access, &account.id, &message).await?;
                        conn.execute("DELETE FROM messages WHERE id = ?1", [message.id.as_str()]).await?;
                        let mut files = attachments::forget_message(conn, &message.id).await?;
                        if with_thread && threads::others_replied(conn, &message.id, &account.id).await? {
                            let channel = load_channel(conn, &sdb.id, &message.channel_id)
                                .await?
                                .map(|c| c.name)
                                .unwrap_or_default();
                            store::audit(
                                conn,
                                &account.id,
                                Audit::new(pb::AuditAction::ThreadDelete, &message.author_id).channel(channel),
                            )
                            .await?;
                        }
                        files.extend(
                            threads::after_delete(conn, &message.channel_id, &message.id, &message.thread_id, events)
                                .await?,
                        );
                        if message.author_id != account.id {
                            let channel = load_channel(conn, &sdb.id, &message.channel_id)
                                .await?
                                .map(|c| c.name)
                                .unwrap_or_default();
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
                        Ok(files)
                    })
                    .await?;
                attachments::drop_soon(&self.app, &sdb.id, files);
                Ok(pb::DeleteMessageResponse {})
            }
            .await,
        )
    }

    async fn list_threads(
        &self,
        request: Request<pb::ListThreadsRequest>,
    ) -> Result<Response<pb::ListThreadsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                self.list_threads_impl(&account, request.into_inner()).await
            }
            .await,
        )
    }

    async fn update_thread(
        &self,
        request: Request<pb::UpdateThreadRequest>,
    ) -> Result<Response<pb::UpdateThreadResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                self.update_thread_impl(&account, request.into_inner()).await
            }
            .await,
        )
    }

    async fn follow_thread(
        &self,
        request: Request<pb::FollowThreadRequest>,
    ) -> Result<Response<pb::FollowThreadResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                self.follow_thread_impl(&account, request.into_inner()).await
            }
            .await,
        )
    }

    async fn list_followed_threads(
        &self,
        request: Request<pb::ListFollowedThreadsRequest>,
    ) -> Result<Response<pb::ListFollowedThreadsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                self.list_followed_threads_impl(&account, request.into_inner()).await
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
        assert_eq!(user_tokens("<@AB> <@!AB> <@!CD> <@&EF> <@> <@G-H> <@IJ"), ["AB", "CD"]);
    }
}
