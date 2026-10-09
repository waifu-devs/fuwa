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
use crate::cluster::calls::MAX_OUTSIDE_EMOJIS;
use crate::cpb::{self, shared_call::Call};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::federation;
use crate::id::{new_id, now_ms, parse_id, timestamp};
use crate::node::Account;
use crate::pb::{self, Permission, shared_channel_service_server::SharedChannelService};
use crate::permissions::{self, Access, Bits, bit};
use crate::servers::{self as store, Audit, Payload, ServerDb, load_channel};

/// What a guest server's people may be let do in a shared channel, at most;
/// the home picks which. Seeing it comes with being shown it.
pub const SHAREABLE: Bits = bit(Permission::SendMessages)
    | bit(Permission::EmbedLinks)
    | bit(Permission::AttachFiles)
    | bit(Permission::CreatePolls)
    | bit(Permission::CreateThreads)
    | bit(Permission::AddReactions);
/// The same for a server on another instance: as much, now that files
/// cross instances too ([`crate::shared_files`]).
const SHAREABLE_ELSEWHERE: Bits = SHAREABLE;
/// Most reactions a message from another instance shows.
const MAX_REACTIONS: usize = 100;
/// How long a share code works.
const CODE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Requests from one other instance a server keeps waiting, at most.
const MAX_WAITING_FROM_INSTANCE: i64 = 20;
/// How many servers one channel is shown in besides its home, for now.
const MAX_GUESTS: usize = 1;
/// Letters and digits that read the same in any font, as invite codes use.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
const CODE_LENGTH: usize = 16;
/// Most custom emoji a home hands a guest's picker, and a guest takes from
/// a home on another instance.
const MAX_HOME_EMOJIS: usize = 1000;
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
    /// The guest's instance, when it's another one.
    instance: Instance,
}

const GUEST_COLUMNS: &str = "id, channel_id, guest_server_id, guest_name, guest_icon_url, active, allowed, created_at, instance, instance_fingerprint";

fn guest_row(r: &turso::Row) -> turso::Result<GuestRow> {
    Ok(GuestRow {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        server: store::shared_server(r.get(2)?, r.get(3)?, r.get(4)?),
        active: r.get::<i64>(5)? != 0,
        allowed: r.get::<i64>(6)? as Bits,
        created_at: r.get(7)?,
        instance: Instance::read(r, 8)?,
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
    /// The home's instance, when it's another one.
    instance: Instance,
}

const LINK_COLUMNS: &str = "id, channel_id, active, home_server_id, home_server_name, home_server_icon_url, home_channel_name, allowed, created_at, instance, instance_fingerprint";

fn link_row(r: &turso::Row) -> turso::Result<LinkRow> {
    Ok(LinkRow {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        active: r.get::<i64>(2)? != 0,
        home: store::shared_server(r.get(3)?, r.get(4)?, r.get(5)?),
        home_channel_name: r.get(6)?,
        allowed: r.get::<i64>(7)? as Bits,
        created_at: r.get(8)?,
        instance: Instance::read(r, 9)?,
    })
}

impl LinkRow {
    /// The channel here that shows the home's, once approved.
    pub(super) fn channel_id(&self) -> &str {
        self.channel_id.as_deref().unwrap_or_default()
    }

    /// Whether the channel's home is on another instance.
    pub(super) fn elsewhere(&self) -> bool {
        self.instance.origin.is_some()
    }
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
        checked_by: vec![],
        instance: row.instance.shown(),
        fingerprint: row.instance.fingerprint.clone().unwrap_or_default(),
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
        checked_by: vec![],
        instance: row.instance.shown(),
        fingerprint: row.instance.fingerprint.clone().unwrap_or_default(),
    }
}

/// This server as the other end of a connection sees it.
async fn this_server(conn: &turso::Connection, server_id: &str) -> Result<pb::SharedServer> {
    let server = store::load_server(conn).await?;
    Ok(store::shared_server(server_id.to_string(), server.name, server.icon_url))
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

// ─────────────── Other instances ───────────────

/// The instance at the other end of a connection, when it isn't this one.
#[derive(Clone, Debug, Default, PartialEq)]
struct Instance {
    /// Its origin (https://chat.example.com).
    origin: Option<String>,
    /// Its key's fingerprint, as pinned when the share was asked.
    fingerprint: Option<String>,
}

impl Instance {
    fn read(r: &turso::Row, at: usize) -> turso::Result<Self> {
        Ok(Self { origin: r.get(at)?, fingerprint: r.get(at + 1)? })
    }

    /// Where a call came from: this instance, or the one that sent it.
    fn of(call: &cpb::SharedCall) -> Self {
        if call.from_instance.is_empty() {
            return Self::default();
        }
        Self { origin: Some(call.from_instance.clone()), fingerprint: Some(call.from_fingerprint.clone()) }
    }

    /// What a server there may be let do, at most.
    fn shareable(&self) -> Bits {
        if self.origin.is_some() { SHAREABLE_ELSEWHERE } else { SHAREABLE }
    }

    /// Its host as people read it; empty for this instance.
    fn shown(&self) -> String {
        self.origin.as_deref().map(federation::display).unwrap_or_default().to_string()
    }
}

/// Pictures in what another instance (at `origin`) sends: kept only when
/// they're that instance's own, and then as links through this instance's
/// picture proxy (`link`, [`crate::outside`]), so apps here never fetch
/// anything from another instance, and nothing anywhere else is fetched for it.
pub struct Pictures<'a> {
    pub origin: &'a str,
    pub link: &'a dyn Fn(&str) -> String,
}

impl Pictures<'_> {
    /// The link apps here load a picture from there at, or empty for none.
    fn of(&self, url: &str) -> String {
        if is_on(url, self.origin) { (self.link)(url) } else { String::new() }
    }
}

/// Whether `url` is a plain link to something at `origin` (no password in
/// it, not overlong).
fn is_on(url: &str, origin: &str) -> bool {
    let (Ok(url), Ok(origin)) = (reqwest::Url::parse(url), reqwest::Url::parse(origin)) else { return false };
    url.as_str().len() <= 2048
        && matches!(url.scheme(), "https" | "http")
        && url.username().is_empty()
        && url.password().is_none()
        && url.origin() == origin.origin()
}

/// A picture of this instance's as it leaves for another, which fetches it
/// through its own proxy: only this instance's own (an upload, or a link it
/// fetches itself), never a link to anywhere else.
fn own_picture(url: &str, public_url: &str) -> String {
    if is_on(url, public_url) { url.to_string() } else { String::new() }
}

/// A server on another instance as this one keeps it: its id under that
/// instance's address, its name on one line and clipped, and its icon
/// through this instance's proxy.
fn their_server(server: Option<&mut pb::SharedServer>, at: &str, pictures: &Pictures) -> Result<()> {
    let server = server.ok_or_else(|| Error::invalid("server is required"))?;
    server.id = format!("{}@{at}", parse_id("server", &server.id)?);
    shown_server(server, pictures);
    Ok(())
}

/// A server another instance named, cleaned up as [`their_server`] does,
/// and marked with its instance when it isn't this one.
fn shown_server(server: &mut pb::SharedServer, pictures: &Pictures) {
    server.name = one_line(&server.name, 100);
    server.icon_url = if server.id.contains('@') { pictures.of(&server.icon_url) } else { String::new() };
    server.instance = store::shared_server(server.id.clone(), String::new(), String::new()).instance;
}

/// An id in what another instance (at `at`) sends about a channel: its own
/// ids, kept under its address, or this instance's own (under `own`), read
/// back. A third instance's are refused: only two instances share a channel.
fn from_there(id: &str, at: &str, own: &str) -> Result<String> {
    match id.split_once('@') {
        None => Ok(format!("{}@{at}", parse_id("id", id)?)),
        Some((id, there)) if there == own => parse_id("id", id),
        Some(_) => Err(Error::invalid("that names a third instance")),
    }
}

/// A person as another instance sent them: their id read by [`from_there`],
/// their names on one line and clipped, and their avatar through this
/// instance's proxy. This instance's own people keep only their id: how
/// they look is this instance's to say ([`own_people`]), never the other's.
fn their_user(user: &mut pb::User, at: &str, own: &str, pictures: &Pictures) -> Result<()> {
    let id = from_there(&user.id, at, own)?;
    *user = if id.contains('@') {
        pb::User {
            id,
            username: one_line(&user.username, 32),
            display_name: one_line(&user.display_name, 64),
            avatar_url: pictures.of(&user.avatar_url),
            kind: user.kind,
            ..Default::default()
        }
    } else {
        pb::User { id, ..Default::default() }
    };
    Ok(())
}

/// Someone as they leave this instance for another: who they are and
/// their avatar, if it's on this instance, and nothing more (no status,
/// banner or anything else of theirs).
fn plain_user(user: &pb::User, public_url: &str) -> pb::User {
    pb::User {
        id: user.id.clone(),
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        avatar_url: own_picture(&user.avatar_url, public_url),
        kind: user.kind,
        ..Default::default()
    }
}

/// Someone in a guest server on another instance, acting in the channel:
/// always that instance's own people and server.
fn their_guest(guest: Option<&mut cpb::Guest>, at: &str, pictures: &Pictures) -> Result<()> {
    let guest = guest.ok_or_else(|| Error::invalid("guest is required"))?;
    parse_id("connection id", &guest.connection_id)?;
    let user = guest.user.as_mut().ok_or_else(|| Error::invalid("user is required"))?;
    parse_id("user", &user.id)?;
    their_user(user, at, "", pictures)?;
    their_server(guest.server.as_mut(), at, pictures)
}

/// A message from a home on another instance, as this one shows it: its
/// people and servers read by [`from_there`], its pictures (avatars, icons,
/// custom emoji, a GIF, link previews') only through this instance's proxy,
/// and no files.
fn their_message(message: &mut pb::Message, at: &str, own: &str, pictures: &Pictures) -> Result<()> {
    parse_id("message", &message.id)?;
    if !message.reply_to_id.is_empty() {
        parse_id("message", &message.reply_to_id)?;
    }
    if message.kind != pb::MessageKind::Unspecified as i32 {
        return Err(Error::invalid("only messages people wrote are shown in shared channels"));
    }
    message.author_id = from_there(&message.author_id, at, own)?;
    message.content = message.content.chars().take(messages::MAX_MESSAGE_LENGTH).collect();
    message.attachments = their_files(&message.attachments);
    message.embeds = message.embeds.iter().take(10).map(|e| their_embed(e, pictures)).collect();
    message.auto_mod = None;
    if !message.thread_id.is_empty() {
        parse_id("message", &message.thread_id)?;
    }
    message.thread = message.thread.take().map(|summary| their_summary(summary, at, own)).transpose()?;
    message.emojis = message.emojis.iter().take(MAX_OUTSIDE_EMOJIS).filter_map(|e| their_emoji(e, pictures)).collect();
    message.gif = message.gif.as_ref().and_then(|gif| their_gif(gif, pictures));
    message.poll = message.poll.take().map(|poll| their_poll(poll, at, own)).transpose()?;
    message.reactions = their_reactions(std::mem::take(&mut message.reactions), pictures);
    if let Some(webhook) = &mut message.webhook {
        webhook.webhook_id.clear();
        webhook.name = one_line(&webhook.name, 80);
        webhook.avatar_url = pictures.of(&webhook.avatar_url);
    }
    if let Some(shared) = &mut message.shared {
        if let Some(user) = &mut shared.user {
            their_user(user, at, own, pictures)?;
            // Who wrote it is the author, never someone else named here.
            if user.id != message.author_id {
                return Err(Error::invalid("that names someone other than who wrote it"));
            }
        }
        if let Some(server) = &mut shared.server {
            server.id = from_there(&server.id, at, own)?;
            shown_server(server, pictures);
        }
    }
    no_pings(message);
    Ok(())
}

/// A message's reactions from another instance: each one as
/// [`super::reactions::arrived`] reads it, custom emoji's pictures through
/// this instance's proxy, and at most as many as a message could show.
fn their_reactions(reactions: Vec<pb::Reaction>, pictures: &Pictures) -> Vec<pb::Reaction> {
    reactions.into_iter().take(MAX_REACTIONS).filter_map(|r| their_reaction(r, pictures)).collect()
}

fn their_reaction(reaction: pb::Reaction, pictures: &Pictures) -> Option<pb::Reaction> {
    let me = reaction.me;
    let mut reaction = super::reactions::arrived(reaction)?;
    if !reaction.emoji_id.is_empty() {
        reaction.emoji_url = pictures.of(&reaction.emoji_url);
        if reaction.emoji_url.is_empty() {
            return None;
        }
    }
    Some(pb::Reaction { me, ..reaction })
}

/// A poll in what another instance (at `at`) sends: as [`super::polls::arrived`]
/// clips it, with who ended it read by [`from_there`].
fn their_poll(poll: pb::Poll, at: &str, own: &str) -> Result<pb::Poll> {
    super::polls::arrived(poll, |id| from_there(id, at, own))
}

/// One of this server's own people, as a home on another instance named
/// them: shown as this instance has them, and only if they're one of this
/// server's people (a home can't put words in anyone else's mouth here).
async fn own_person(conn: &turso::Connection, user_id: &str) -> Result<pb::User> {
    let member = query_one(conn, "SELECT 1 FROM members WHERE user_id = ?1", [user_id], |r| r.get::<i64>(0)).await?;
    match member {
        Some(_) => store::user(conn, user_id).await?.ok_or_else(|| Error::invalid("that names someone not here")),
        None => Err(Error::invalid("that names someone not here")),
    }
}

/// A message from a home on another instance, as [`their_message`] read it,
/// checked against what this server knows: its own people are shown as
/// this instance has them, and only if they're its people; the only
/// servers named are the home and this one, each with its own people.
async fn own_people(conn: &turso::Connection, server_id: &str, home_id: &str, message: &mut pb::Message) -> Result<()> {
    let ours = !message.author_id.contains('@');
    if let Some(server) = message.shared.as_ref().and_then(|s| s.server.as_ref()) {
        let named = if ours { server_id } else { home_id };
        if server.id != named {
            return Err(Error::invalid("that names another server"));
        }
    }
    if ours {
        let user = own_person(conn, &message.author_id).await?;
        message.webhook = None;
        message.shared = Some(pb::SharedAuthor { user: Some(user), server: Some(this_server(conn, server_id).await?) });
    }
    Ok(())
}

/// The people a home on another instance listed, as [`own_people`] shows
/// them: this server's own as this instance has them, and no one else here.
async fn own_authors(conn: &turso::Connection, authors: Vec<pb::User>) -> Result<Vec<pb::User>> {
    let mut shown = Vec::with_capacity(authors.len());
    for author in authors {
        if author.id.contains('@') {
            shown.push(author);
        } else if let Ok(user) = own_person(conn, &author.id).await {
            shown.push(user);
        }
    }
    Ok(shown)
}

/// A thread's summary from a home on another instance: counts that make
/// sense, and at most five people, read like authors.
fn their_summary(summary: pb::ThreadSummary, at: &str, own: &str) -> Result<pb::ThreadSummary> {
    Ok(pb::ThreadSummary {
        reply_count: summary.reply_count.max(0),
        last_reply_at: summary.last_reply_at,
        participant_ids: summary
            .participant_ids
            .iter()
            .take(5)
            .map(|id| from_there(id, at, own))
            .collect::<Result<_>>()?,
        locked: summary.locked,
    })
}

/// A custom emoji in a message from another instance: its name and its
/// picture through this instance's proxy, or none when there's no picture
/// to show (the message shows its name).
fn their_emoji(emoji: &pb::Emoji, pictures: &Pictures) -> Option<pb::Emoji> {
    let name_ok =
        (2..=32).contains(&emoji.name.len()) && emoji.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let url = pictures.of(&emoji.url);
    (parse_id("emoji", &emoji.id).is_ok() && name_ok && !url.is_empty()).then(|| pb::Emoji {
        id: emoji.id.clone(),
        name: emoji.name.clone(),
        url,
        animated: emoji.animated,
        ..Default::default()
    })
}

/// A guest's own server's emoji from another instance, as [`their_emoji`]
/// reads them, each still naming that server (under its address).
fn their_emojis(emojis: &[pb::Emoji], at: &str, pictures: &Pictures) -> Vec<pb::Emoji> {
    emojis
        .iter()
        .take(MAX_OUTSIDE_EMOJIS)
        .filter_map(|emoji| {
            let server_id = format!("{}@{at}", parse_id("server", &emoji.server_id).ok()?);
            Some(pb::Emoji { server_id, ..their_emoji(emoji, pictures)? })
        })
        .collect()
}

/// A GIF in a message from another instance, through this instance's proxy.
fn their_gif(gif: &pb::MessageGif, pictures: &Pictures) -> Option<pb::MessageGif> {
    let url = pictures.of(&gif.url);
    (!url.is_empty()).then(|| pb::MessageGif {
        url,
        width: gif.width.clamp(0, 4096),
        height: gif.height.clamp(0, 4096),
        title: one_line(&gif.title, 200),
        provider: gif.provider,
        seal: String::new(),
    })
}

/// A link preview from another instance: its words, its link, and its
/// pictures through this instance's proxy.
fn their_embed(embed: &pb::Embed, pictures: &Pictures) -> pb::Embed {
    pb::Embed {
        title: one_line(&embed.title, 256),
        description: embed.description.chars().take(4096).collect(),
        url: if embed.url.starts_with("https://") { embed.url.chars().take(2048).collect() } else { String::new() },
        color: embed.color,
        fields: embed
            .fields
            .iter()
            .take(25)
            .map(|f| pb::EmbedField {
                name: one_line(&f.name, 256),
                value: f.value.chars().take(1024).collect(),
                inline: f.inline,
            })
            .collect(),
        thumbnail_url: pictures.of(&embed.thumbnail_url),
        image_url: pictures.of(&embed.image_url),
    }
}

/// What a guest on another instance sends to be said: text, links and
/// files, each with its ticket (fetched from there and checked by the home,
/// [`take_files`]), and no previews it made (apps here would fetch their
/// pictures from wherever it found them).
fn their_text(send: &mut cpb::GuestSend) -> Result<()> {
    if send.files.len() != send.attachments.len() {
        return Err(Error::invalid("each file needs its ticket"));
    }
    for file in &mut send.attachments {
        let id = crate::media::parse_id(&file.id).ok_or_else(|| Error::invalid("a file's id doesn't read"))?;
        *file = pb::Attachment {
            id,
            filename: crate::attachments::clean_name(&file.filename),
            size: file.size,
            width: file.width,
            height: file.height,
            voice: file.voice.take(),
            ..Default::default()
        };
    }
    send.embeds.clear();
    messages::check_content(&send.content, !send.attachments.is_empty())
}

/// The files of a message from a home on another instance, as this one
/// shows them: at most ten, each named by its id there, its name cleaned
/// up and its kind one fuwa knows. Read through this instance
/// ([`crate::shared_files`]), never from there; [`files_here`] links them.
fn their_files(files: &[pb::Attachment]) -> Vec<pb::Attachment> {
    files
        .iter()
        .filter_map(|file| {
            let id = crate::media::parse_id(&file.id)?;
            let kind = crate::media::ATTACHMENT_TYPES.iter().find(|kind| **kind == file.content_type);
            let pixels = |n: i32| n.clamp(0, 65_535);
            let voice = file.voice.clone().filter(|v| v.waveform.len() <= 128);
            (file.size > 0).then(|| pb::Attachment {
                id,
                filename: crate::attachments::clean_name(&file.filename),
                content_type: kind.copied().unwrap_or(crate::media::OCTET_STREAM).to_string(),
                size: file.size,
                width: pixels(file.width),
                height: pixels(file.height),
                voice,
                url: String::new(),
            })
        })
        .take(10)
        .collect()
}

/// A message's files from a home on another instance, linked for people
/// here: through this instance, signed, for a day.
fn files_here(app: &App, link: &LinkRow, message: &mut pb::Message) {
    if link.elsewhere() {
        for file in &mut message.attachments {
            file.url = crate::shared_files::link(app, &link.home.id, file);
        }
    }
}

/// Text another instance sent, on one line and clipped.
fn one_line(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

/// What a home on another instance lets this one's people do, at most.
fn elsewhere_allowed(list: &[i32]) -> Result<Vec<i32>> {
    Ok(permissions::to_list(permissions::from_list(list)? & SHAREABLE_ELSEWHERE))
}

/// A call from another instance, as this one reads it: every server and
/// person it speaks for is that instance's ("<id>@<instance>"), so it can't
/// speak for this instance's own or a third one's.
pub fn arrived(
    mut call: cpb::SharedCall,
    origin: &str,
    own: &str,
    fingerprint: &str,
    link: &dyn Fn(&str) -> String,
) -> Result<cpb::SharedCall> {
    let pictures = Pictures { origin, link };
    let at = federation::address(origin);
    let own = federation::address(own);
    let message_id = |id: &str| if id.is_empty() { Ok(()) } else { parse_id("message", id).map(|_| ()) };
    let theirs = |id: &str| -> Result<String> { Ok(format!("{}@{at}", parse_id("id", id)?)) };
    // An instance admin there acting with the operator's token has no account.
    let actor = |id: &str| if id.is_empty() { Ok(format!("@{at}")) } else { theirs(id) };
    parse_id("server", &call.server_id)?;
    match call.call.as_mut() {
        Some(Call::Lookup(lookup)) => lookup.guest_server_id = theirs(&lookup.guest_server_id)?,
        Some(Call::Ask(ask)) => {
            ask.asked_by_id = actor(&ask.asked_by_id)?;
            their_server(ask.guest.as_mut(), at, &pictures)?;
        }
        Some(Call::Left(left)) => {
            left.guest_server_id = theirs(&left.guest_server_id)?;
            left.actor_id = actor(&left.actor_id)?;
        }
        Some(Call::Readers(readers)) => readers.guest_server_id = theirs(&readers.guest_server_id)?,
        Some(Call::Approved(approved)) => {
            let connection = approved.connection.as_mut().ok_or_else(|| Error::invalid("connection is required"))?;
            their_server(connection.server.as_mut(), at, &pictures)?;
            connection.allowed = elsewhere_allowed(&connection.allowed)?;
            connection.home_channel_name = one_line(&connection.home_channel_name, 100);
            approved.actor_id = actor(&approved.actor_id)?;
        }
        Some(Call::Ended(ended)) => ended.actor_id = actor(&ended.actor_id)?,
        Some(Call::Updated(updated)) => {
            let connection = updated.connection.as_mut().ok_or_else(|| Error::invalid("connection is required"))?;
            their_server(connection.server.as_mut(), at, &pictures)?;
            connection.allowed = elsewhere_allowed(&connection.allowed)?;
            connection.home_channel_name = one_line(&connection.home_channel_name, 100);
        }
        // To the home: someone there acting in the channel.
        Some(Call::Send(send)) => {
            their_guest(send.guest.as_mut(), at, &pictures)?;
            their_text(send)?;
            message_id(&send.reply_to_id)?;
            send.emojis = their_emojis(&send.emojis, at, &pictures);
        }
        Some(Call::List(list)) => {
            their_guest(list.guest.as_mut(), at, &pictures)?;
            message_id(&list.before_id)?;
            message_id(&list.after_id)?;
        }
        Some(Call::Get(get)) => {
            their_guest(get.guest.as_mut(), at, &pictures)?;
            parse_id("message", &get.message_id)?;
        }
        Some(Call::Edit(edit)) => {
            their_guest(edit.guest.as_mut(), at, &pictures)?;
            parse_id("message", &edit.message_id)?;
            messages::check_content(&edit.content, false)?;
            edit.emojis = their_emojis(&edit.emojis, at, &pictures);
        }
        Some(Call::Delete(delete)) => {
            their_guest(delete.guest.as_mut(), at, &pictures)?;
            parse_id("message", &delete.message_id)?;
        }
        Some(Call::Poll(poll)) => {
            their_guest(poll.guest.as_mut(), at, &pictures)?;
            messages::check_content(&poll.content, true)?;
            poll.poll.as_ref().ok_or_else(|| Error::invalid("poll is required"))?;
            message_id(&poll.reply_to_id)?;
            poll.emojis = their_emojis(&poll.emojis, at, &pictures);
        }
        Some(Call::Emojis(emojis)) => their_guest(emojis.guest.as_mut(), at, &pictures)?,
        Some(Call::Reply(reply)) => {
            let send = reply.send.as_mut().ok_or_else(|| Error::invalid("send is required"))?;
            their_guest(send.guest.as_mut(), at, &pictures)?;
            their_text(send)?;
            message_id(&send.reply_to_id)?;
            send.emojis = their_emojis(&send.emojis, at, &pictures);
            parse_id("message", &reply.thread_id)?;
        }
        Some(Call::Thread(thread)) => {
            their_guest(thread.guest.as_mut(), at, &pictures)?;
            parse_id("message", &thread.thread_id)?;
            message_id(&thread.before_id)?;
            message_id(&thread.after_id)?;
        }
        Some(Call::Threads(threads)) => {
            their_guest(threads.guest.as_mut(), at, &pictures)?;
            message_id(&threads.after_thread_id)?;
        }
        Some(Call::Follow(follow)) => {
            their_guest(follow.guest.as_mut(), at, &pictures)?;
            parse_id("message", &follow.thread_id)?;
        }
        Some(Call::Followed(followed)) => their_guest(followed.guest.as_mut(), at, &pictures)?,
        Some(Call::Vote(vote)) => {
            their_guest(vote.guest.as_mut(), at, &pictures)?;
            parse_id("message", &vote.message_id)?;
            if vote.answer_ids.len() > super::polls::MAX_ANSWERS {
                return Err(Error::invalid("pick answers from the poll, each once"));
            }
        }
        Some(Call::EndPoll(end)) => {
            their_guest(end.guest.as_mut(), at, &pictures)?;
            parse_id("message", &end.message_id)?;
        }
        Some(Call::React(react)) => {
            their_guest(react.guest.as_mut(), at, &pictures)?;
            parse_id("message", &react.message_id)?;
            super::reactions::check_emoji(&react.emoji, &react.emoji_id)?;
        }
        Some(Call::Reactors(reactors)) => {
            their_guest(reactors.guest.as_mut(), at, &pictures)?;
            parse_id("message", &reactors.message_id)?;
            super::reactions::check_emoji(&reactors.emoji, &reactors.emoji_id)?;
            if !reactors.after_id.is_empty() {
                reactors.after_id = from_there(&reactors.after_id, at, own)?;
            }
        }
        Some(Call::Voters(voters)) => {
            their_guest(voters.guest.as_mut(), at, &pictures)?;
            parse_id("message", &voters.message_id)?;
            // The page after someone as that instance names them: its own
            // people under its address, this one's as they are here.
            if !voters.after_id.is_empty() {
                voters.after_id = from_there(&voters.after_id, at, own)?;
            }
        }
        // To a guest: what just happened in the channel.
        Some(Call::Events(home)) => {
            let mut shown = Vec::new();
            for mut event in std::mem::take(&mut home.events) {
                match event.payload.as_mut() {
                    Some(
                        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
                        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }),
                    ) => their_message(m, at, own, &pictures)?,
                    Some(Payload::MessageDeleted(deleted)) => {
                        parse_id("message", &deleted.message_id)?;
                    }
                    Some(Payload::ThreadUpdated(updated)) => {
                        parse_id("message", &updated.thread_id)?;
                        let summary = updated.thread.take().ok_or_else(|| Error::invalid("thread is required"))?;
                        updated.thread = Some(their_summary(summary, at, own)?);
                    }
                    Some(Payload::PollUpdated(updated)) => {
                        parse_id("message", &updated.message_id)?;
                        let poll = updated.poll.take().ok_or_else(|| Error::invalid("poll is required"))?;
                        updated.poll = Some(their_poll(poll, at, own)?);
                        // Anonymous polls and ends name no one: kept empty.
                        if !updated.voter_id.is_empty() {
                            updated.voter_id = from_there(&updated.voter_id, at, own)?;
                        }
                        updated.voter_answer_ids.truncate(super::polls::MAX_ANSWERS);
                    }
                    Some(Payload::ReactionUpdated(updated)) => {
                        parse_id("message", &updated.message_id)?;
                        if !updated.thread_id.is_empty() {
                            parse_id("message", &updated.thread_id)?;
                        }
                        updated.user_id = from_there(&updated.user_id, at, own)?;
                        let reaction = updated.reaction.take().ok_or_else(|| Error::invalid("reaction is required"))?;
                        // Counts only: never `me`, which is each reader's own.
                        let reaction = their_reaction(reaction, &pictures)
                            .ok_or_else(|| Error::invalid("that isn't a reaction"))?;
                        updated.reaction = Some(pb::Reaction { me: false, ..reaction });
                    }
                    Some(Payload::ReactionsCleared(cleared)) => {
                        parse_id("message", &cleared.message_id)?;
                        if !cleared.thread_id.is_empty() {
                            parse_id("message", &cleared.thread_id)?;
                        }
                        if !(cleared.emoji.is_empty() && cleared.emoji_id.is_empty()) {
                            super::reactions::check_emoji(&cleared.emoji, &cleared.emoji_id)?;
                        }
                    }
                    _ => continue,
                }
                event.actor_id.clear();
                shown.push(event);
            }
            home.events = shown;
        }
        _ => {
            return Err(Error::FailedPrecondition(
                "channels shared between instances can't do that; this instance may run an older fuwa".into(),
            ));
        }
    }
    call.from_instance = origin.to_string();
    call.from_fingerprint = fingerprint.to_string();
    Ok(call)
}

/// For a share asked from another instance, as [`arrived`] read it: the
/// call that takes the request back, should this instance not keep it.
pub fn undo(call: &cpb::SharedCall) -> Option<cpb::SharedCall> {
    let Some(Call::Ask(ask)) = &call.call else { return None };
    let left = cpb::GuestLeft {
        connection_id: ask.connection_id.clone(),
        guest_server_id: ask.guest.as_ref().map(|g| g.id.clone()).unwrap_or_default(),
        actor_id: ask.asked_by_id.clone(),
    };
    Some(cpb::SharedCall { call: Some(Call::Left(left)), ..call.clone() })
}

/// Another instance's answer to `call`, as this one reads it: its servers
/// under its address, and only what that call answers with.
pub fn returned(
    call: &cpb::SharedCall,
    reply: cpb::SharedReply,
    origin: &str,
    own: &str,
    fingerprint: &str,
    link: &dyn Fn(&str) -> String,
) -> Result<cpb::SharedReply> {
    let pictures = Pictures { origin, link };
    let at = federation::address(origin);
    let own = federation::address(own);
    let mut out = cpb::SharedReply::default();
    match &call.call {
        Some(Call::Send(_) | Call::Get(_) | Call::Edit(_) | Call::Poll(_) | Call::Reply(_)) => {
            if let Some(mut message) = reply.message {
                their_message(&mut message, at, own, &pictures)?;
                out.message = Some(message);
            }
            if let Some(mut author) = reply.author {
                their_user(&mut author, at, own, &pictures)?;
                out.author = Some(author);
            }
            return Ok(out);
        }
        Some(Call::List(_) | Call::Thread(_)) => {
            let mut page = reply.page.unwrap_or_default();
            for message in &mut page.messages {
                their_message(message, at, own, &pictures)?;
            }
            // One author a message, and the thread's parent's on a thread's page.
            let thread = matches!(&call.call, Some(Call::Thread(_)));
            page.authors.truncate(page.messages.len() + usize::from(thread));
            // Only a thread's page has the message it's under.
            page.parent = match (&call.call, page.parent) {
                (Some(Call::Thread(_)), Some(mut parent)) => {
                    their_message(&mut parent, at, own, &pictures)?;
                    Some(parent)
                }
                _ => None,
            };
            for author in &mut page.authors {
                their_user(author, at, own, &pictures)?;
            }
            out.page = Some(page);
            return Ok(out);
        }
        Some(Call::Vote(_) | Call::EndPoll(_)) => {
            out.poll = reply.poll.map(|poll| their_poll(poll, at, own)).transpose()?;
            return Ok(out);
        }
        Some(Call::React(_)) => {
            out.reaction = reply.reaction.and_then(|r| their_reaction(r, &pictures));
            return Ok(out);
        }
        Some(Call::Reactors(_)) => {
            let mut reactors = reply.reactors.unwrap_or_default();
            reactors.users.truncate(100);
            for user in &mut reactors.users {
                their_user(user, at, own, &pictures)?;
            }
            out.reactors = Some(reactors);
            return Ok(out);
        }
        Some(Call::Threads(_)) => {
            let mut page = reply.threads.unwrap_or_default();
            page.threads.truncate(50);
            for message in &mut page.threads {
                their_message(message, at, own, &pictures)?;
            }
            // Each thread's author and up to five people who replied.
            page.authors.truncate(page.threads.len() * 6);
            for author in &mut page.authors {
                their_user(author, at, own, &pictures)?;
            }
            if !page.next_after_thread_id.is_empty() {
                parse_id("message", &page.next_after_thread_id)?;
            }
            out.threads = Some(page);
            return Ok(out);
        }
        Some(Call::Followed(_)) => {
            out.thread_ids = reply
                .thread_ids
                .iter()
                .take(super::threads::MAX_FOLLOWED as usize)
                .filter_map(|id| parse_id("message", id).ok())
                .collect();
            return Ok(out);
        }
        Some(Call::Emojis(_)) => {
            out.emojis = reply.emojis.iter().take(MAX_HOME_EMOJIS).filter_map(|e| their_emoji(e, &pictures)).collect();
            return Ok(out);
        }
        Some(Call::Voters(_)) => {
            let mut voters = reply.voters.unwrap_or_default();
            voters.users.truncate(100);
            for user in &mut voters.users {
                their_user(user, at, own, &pictures)?;
            }
            out.voters = Some(voters);
            return Ok(out);
        }
        _ => {}
    }
    match (&call.call, reply.preview, reply.connection) {
        (Some(Call::Lookup(_)), Some(mut preview), _) => {
            their_server(preview.home_server.as_mut(), at, &pictures)?;
            // Its regions are that instance's, which this one can't name.
            preview.region.clear();
            preview.allowed = elsewhere_allowed(&preview.allowed)?;
            preview.channel_name = one_line(&preview.channel_name, 100);
            preview.channel_topic = preview.channel_topic.chars().take(1024).collect();
            preview.instance = federation::display(origin).to_string();
            preview.fingerprint = fingerprint.to_string();
            out.preview = Some(preview);
        }
        (Some(Call::Ask(_)), _, Some(mut connection)) => {
            their_server(connection.server.as_mut(), at, &pictures)?;
            connection.allowed = elsewhere_allowed(&connection.allowed)?;
            connection.home_channel_name = one_line(&connection.home_channel_name, 100);
            connection.instance = federation::display(origin).to_string();
            connection.fingerprint = fingerprint.to_string();
            out.connection = Some(connection);
        }
        (Some(Call::Readers(_)), _, _) => {
            out.checked_by = reply.checked_by.into_iter().take(20).map(|name| one_line(&name, 200)).collect();
        }
        _ => {}
    }
    Ok(out)
}

/// Checks a call from another instance is one it may make here: federation
/// is on (and sharing, for a new share), and a call to a guest comes from
/// its home's instance.
async fn from_elsewhere(app: &App, sdb: &ServerDb, call: &cpb::SharedCall, from: &Instance) -> Result<()> {
    let settings = app.settings();
    let asking = matches!(call.call, Some(Call::Lookup(_) | Call::Ask(_)));
    if !settings.federation || (asking && !settings.shared_channels) {
        return Err(Error::FailedPrecondition("this instance doesn't share channels with other instances".into()));
    }
    let connection_id = match &call.call {
        Some(Call::Approved(cpb::HomeApproved { connection: Some(connection), .. })) => &connection.id,
        Some(Call::Updated(cpb::HomeUpdated { connection: Some(connection) })) => &connection.id,
        Some(Call::Ended(ended)) => &ended.connection_id,
        Some(Call::Events(home)) => &home.connection_id,
        // At the home, the guest's server id (under its instance) is checked.
        _ => return Ok(()),
    };
    let link = link_by_id(&*sdb.read()?, connection_id).await?;
    let home = match &call.call {
        Some(Call::Approved(cpb::HomeApproved { connection: Some(c), .. })) => c.server.as_ref().map(|s| s.id.as_str()),
        _ => None,
    };
    match link {
        Some(link) if link.instance.origin == from.origin && home.is_none_or(|home| home == link.home.id) => Ok(()),
        _ => Err(Error::NotFound(GONE)),
    }
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

/// What a pasted code names.
#[derive(Debug, PartialEq)]
struct Pasted {
    /// The code as the home keeps it, without its instance.
    code: String,
    /// The home server: its id, or "<id>@<instance>" on another instance.
    home: String,
    /// The home's instance, as an origin, when it's another one.
    origin: Option<String>,
}

/// The home server a code is for, and its instance when the code names
/// another one ("<code>@<host>").
fn code_home(app: &App, code: &str) -> Result<Pasted> {
    let (code, server, at) = split_code(code)?;
    let origin = match at {
        None => None,
        Some(at) => {
            // Only read here: the part that keeps the instance's key refuses
            // private addresses (unless it allows them) before calling.
            let origin = federation::origin(at, true).map_err(Error::invalid)?;
            // A code for this instance, pasted with its host.
            (federation::named_origin(app).as_deref() != Some(origin.as_str())).then_some(origin)
        }
    };
    if origin.is_some() && !app.settings().federation {
        return Err(Error::FailedPrecondition("this instance doesn't share channels with other instances".into()));
    }
    let home = match &origin {
        Some(origin) => format!("{server}@{}", federation::address(origin)),
        None => server,
    };
    Ok(Pasted { code: code.to_string(), home, origin })
}

/// A code's parts: the code as its home keeps it, the home server's id, and
/// the instance it names, if any.
fn split_code(code: &str) -> Result<(&str, String, Option<&str>)> {
    let invalid = || Error::invalid("that isn't a share code");
    let (code, at) = match code.trim().split_once('@') {
        Some((code, at)) => (code, Some(at)),
        None => (code.trim(), None),
    };
    let (server, rest) = code.split_once('-').ok_or_else(invalid)?;
    if rest.len() != CODE_LENGTH || !rest.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(invalid());
    }
    let server = parse_id("share code", server).map_err(|_| invalid())?;
    Ok((code, server, at))
}

/// A code as admins copy it: with this instance's host when servers on
/// other instances may use it.
fn shown_code(app: &App, code: &str, other_instances: bool) -> String {
    match federation::named_origin(app) {
        Some(own) if other_instances => format!("{code}@{}", federation::address(&own)),
        _ => code.to_string(),
    }
}

/// A stored code, the channel it's for and whether other instances may use it.
fn code_row(r: &turso::Row) -> turso::Result<(pb::ShareCode, String, bool)> {
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
        r.get::<i64>(5)? != 0,
    ))
}

const CODE_COLUMNS: &str = "code, channel_id, creator_id, created_at, expires_at, other_instances";

/// A code that still works, and whether servers on other instances may use it.
async fn load_code(conn: &turso::Connection, code: &str) -> Result<Option<(pb::ShareCode, bool)>> {
    let found = query_one(
        conn,
        &format!("SELECT {CODE_COLUMNS} FROM share_codes WHERE code = ?1 AND expires_at > ?2"),
        (code.trim(), now_ms()),
        code_row,
    )
    .await?;
    Ok(found.map(|(code, _, other_instances)| (code, other_instances)))
}

/// A code that works for the server asking: any code for a server on this
/// instance, only one made for other instances for a server on another.
async fn usable_code(conn: &turso::Connection, code: &str, from: &Instance) -> Result<pb::ShareCode> {
    match load_code(conn, code).await? {
        Some((code, other_instances)) if other_instances || from.origin.is_none() => Ok(code),
        _ => Err(Error::NotFound("share code")),
    }
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
        Ok(store::shared_server(r.get(0)?, r.get(1)?, r.get(2)?))
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

/// Whether someone new from a server on another instance is turned away:
/// that instance names its people, so once one of them was kept out of
/// the channel, or it brought as many as it may (`most`,
/// FUWA_LIMIT_SHARED_REMOTE_PEOPLE), no one new from it joins in (those
/// already here still can).
async fn newcomer_refused(
    conn: &turso::Connection,
    channel_id: &str,
    user_id: &str,
    guest_server_id: &str,
    most: Option<i64>,
) -> Result<Option<&'static str>> {
    let known = query_one(conn, "SELECT 1 FROM users WHERE id = ?1", [user_id], |r| r.get::<i64>(0)).await?.is_some();
    if known {
        return Ok(None);
    }
    let kept_out = query_one(
        conn,
        "SELECT 1 FROM channel_blocks WHERE channel_id = ?1 AND guest_server_id = ?2 LIMIT 1",
        (channel_id, guest_server_id),
        |r| r.get::<i64>(0),
    )
    .await?
    .is_some();
    if kept_out {
        return Ok(Some("someone from your server was kept out of this channel, so no one new from it can join in"));
    }
    let Some(most) = most else { return Ok(None) };
    let people =
        query_one(conn, "SELECT COUNT(*) FROM users WHERE guest_of = ?1", [guest_server_id], |r| r.get::<i64>(0))
            .await?
            .unwrap_or(0);
    if people >= most {
        return Ok(Some("this channel has as many people from your server as it takes"));
    }
    Ok(None)
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
    text: automod::Text<'_>,
    asked: Option<&automod::Asked>,
    events: &mut Vec<Payload>,
) -> Result<Option<String>> {
    // Already out until the year 9999, as far as `review` can tell.
    let member = pb::Member {
        user: Some(user.clone()),
        timed_out_until: Some(timestamp(253_402_300_799_000)),
        ..Default::default()
    };
    let verdict = automod::review(conn, server_id, &member, access, channel, text, asked, events).await?;
    let times_out = store::load_automod(conn).await?.iter().any(|rule| {
        rule.enabled
            && !rule.exempt_channel_ids.iter().any(|id| *id == channel.id || *id == channel.parent_id)
            && rule.actions.iter().any(|a| a.kind == pb::AutoModActionKind::TimeOut as i32)
            && crate::automod::check_text(rule, text).is_some()
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
    let call = cpb::SharedCall { server_id: link.home.id.clone(), call: Some(call), ..Default::default() };
    match app.shared(call).await {
        Err(Error::Unavailable(_)) if link.instance.origin.is_some() => Err(Error::Unavailable(format!(
            "can't reach {} right now",
            store::shared_server(link.home.id.clone(), String::new(), String::new()).instance
        ))),
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
    app: &App,
    conn: &turso::Connection,
    server_id: &str,
    account: &Account,
    access: &Access,
    link: &LinkRow,
) -> Result<cpb::Guest> {
    let channel_id = link.channel_id.as_deref().unwrap_or_default();
    let mut user = store::user(conn, &account.id).await?.ok_or_else(|| Error::denied("join this server first"))?;
    if link.instance.origin.is_some() {
        user = plain_user(&user, &app.settings().public_url);
    }
    Ok(cpb::Guest {
        connection_id: link.id.clone(),
        user: Some(user),
        server: Some(this_server(conn, server_id).await?),
        moderator: access.has_in(channel_id, Permission::ManageMessages),
    })
}

/// A message as this server shows it: in its own channel.
fn shown_here(app: &App, mut message: pb::Message, server_id: &str, link: &LinkRow) -> pb::Message {
    message.server_id = server_id.to_string();
    message.channel_id = link.channel_id.clone().unwrap_or_default();
    no_pings(&mut message);
    files_here(app, link, &mut message);
    message
}

/// Messages a home answered with, as this server shows them: checked by
/// [`own_people`] when the home is on another instance (one that doesn't
/// pass is left out), then [`shown_here`].
async fn shown_from(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    messages: Vec<pb::Message>,
) -> Result<Vec<pb::Message>> {
    let mut shown = Vec::with_capacity(messages.len());
    let conn = match link.instance.origin {
        Some(_) => Some(app.servers.get(server_id).await?.read()?),
        None => None,
    };
    for mut message in messages {
        if let Some(conn) = &conn
            && own_people(conn, server_id, &link.home.id, &mut message).await.is_err()
        {
            continue;
        }
        shown.push(shown_here(app, message, server_id, link));
    }
    Ok(shown)
}

/// The one message a home answered with, as [`shown_from`] shows it.
async fn shown_one(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    message: Option<pb::Message>,
) -> Result<pb::Message> {
    let message = message.ok_or_else(|| Error::internal("the home server didn't say what it saved"))?;
    shown_from(app, server_id, link, vec![message])
        .await?
        .pop()
        .ok_or_else(|| Error::FailedPrecondition("the home server answered with someone who isn't here".into()))
}

/// @everyone, @here and role pings belong to the server they were said in:
/// the home's roles mean nothing here, and its @everyone isn't this
/// server's people. Nor are an agent's buttons and interactions relayed
/// between servers yet.
fn no_pings(message: &mut pb::Message) {
    message.mentions_everyone = false;
    message.mention_role_ids.clear();
    message.mention_user_ids.clear();
    message.components.clear();
    message.interaction = None;
}

/// Runs this server's own AutoMod over what one of its people writes in a
/// channel it shows from another, before it goes there.
async fn review_here(
    app: &Arc<App>,
    sdb: &ServerDb,
    member: &pb::Member,
    access: &Access,
    channel_id: &str,
    text: automod::Text<'_>,
    pictures: &[String],
) -> Result<()> {
    if access.has(Permission::ManageServer) || store::load_automod(&*sdb.read()?).await?.is_empty() {
        return Ok(());
    }
    let asked = automod::ask(app, sdb, member, access, channel_id, text, pictures).await;
    let author_id = member.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let blocked = sdb
        .write(&author_id, async |conn, events| {
            let channel = load_channel(conn, &sdb.id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
            Ok(automod::review(conn, &sdb.id, member, access, &channel, text, asked.as_ref(), events).await?.blocked)
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
        let guest = guest_of(app, conn, server_id, account, access, &link).await?;
        return Ok(Some((link, guest)));
    }
    if load_message(conn, server_id, message_id).await?.is_some() {
        return Ok(None);
    }
    for link in all_links(conn).await? {
        // A home on another instance is only asked about a channel the app
        // named: it'd learn which messages this one looks up, and could say
        // yes to any.
        if !link.active
            || link.instance.origin.is_some()
            || !link.channel_id.as_deref().is_some_and(|c| access.can_see(c))
        {
            continue;
        }
        let guest = guest_of(app, conn, server_id, account, access, &link).await?;
        let call = Call::Get(cpb::GuestGet { guest: Some(guest.clone()), message_id: message_id.to_string() });
        if app
            .shared(cpb::SharedCall { server_id: link.home.id.clone(), call: Some(call), ..Default::default() })
            .await
            .is_ok()
        {
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
    let guest = guest_of(app, &*sdb.read()?, &sdb.id, account, access, link).await?;
    let pictures = automod::picture_links(&req.attachments, &req.embeds, &[]);
    let reviewed = messages::reviewed_text(&pb::Message {
        content: req.content.clone(),
        embeds: req.embeds.clone(),
        attachments: req.attachments.clone(),
        poll: req.poll.as_ref().map(|new| super::polls::check(new, now_ms())).transpose()?,
        ..Default::default()
    })
    .into_owned();
    let text = automod::Text { all: &reviewed, content: &req.content };
    review_here(app, sdb, member, access, &channel_id, text, &pictures).await?;
    let files: Vec<String> = req.attachments.iter().map(|file| file.id.clone()).collect();
    let emojis = own_emojis(app, &*sdb.read()?, &sdb.id, &req.content, link).await?;
    // A poll goes as a call of its own, which a home too old for polls here
    // refuses instead of keeping the message without it.
    let thread = (!req.thread_id.is_empty()).then(|| {
        (req.thread_id.clone(), req.also_send_to_channel, access.has_in(&channel_id, Permission::CreateThreads))
    });
    let call = match req.poll {
        Some(poll) => Call::Poll(cpb::GuestPoll {
            guest: Some(guest),
            content: req.content,
            poll: Some(poll),
            reply_to_id: req.reply_to_id,
            emojis,
        }),
        None => Call::Send(cpb::GuestSend {
            guest: Some(guest),
            content: req.content,
            attachments: req.attachments,
            embeds: req.embeds,
            reply_to_id: req.reply_to_id,
            emojis,
            ..Default::default()
        }),
    };
    // A reply in a thread goes as a call of its own, which a home too old
    // for threads here refuses instead of putting it in the channel.
    let call = match (call, thread) {
        (Call::Send(send), Some((thread_id, also_in_channel, may_start))) => {
            Call::Reply(cpb::GuestReply { send: Some(send), thread_id, also_in_channel, may_start })
        }
        (call, _) => call,
    };
    let reply = to_home(app, &sdb.id, link, call).await?;
    // The home has its own copies now; on a split instance this server's
    // shard lets go of its own, and so does one process for a home on
    // another instance. One that went wrong is left to the sweeps.
    if matches!(app.link, crate::app::Link::Shard(_)) {
        for id in &files {
            crate::cluster::pictures::drop(app, &sdb.id, id).await;
        }
    } else if link.elsewhere() && !files.is_empty() {
        crate::attachments::drop_soon(app, &sdb.id, files);
    }
    shown_one(app, &sdb.id, link, reply.message).await
}

pub(super) async fn guest_list(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    req: &pb::ListMessagesRequest,
) -> Result<pb::ListMessagesResponse> {
    let call = if req.thread_id.is_empty() {
        Call::List(cpb::GuestList {
            guest: Some(guest),
            limit: req.limit,
            before_id: req.before_id.clone(),
            after_id: req.after_id.clone(),
        })
    } else {
        Call::Thread(cpb::GuestThread {
            guest: Some(guest),
            thread_id: req.thread_id.clone(),
            limit: req.limit,
            before_id: req.before_id.clone(),
            after_id: req.after_id.clone(),
        })
    };
    let mut page = to_home(app, server_id, link, call).await?.page.unwrap_or_default();
    page.messages = shown_from(app, server_id, link, std::mem::take(&mut page.messages)).await?;
    page.parent = match page.parent.take() {
        Some(parent) if !req.thread_id.is_empty() => shown_from(app, server_id, link, vec![parent]).await?.pop(),
        _ => None,
    };
    if link.instance.origin.is_some() {
        let conn = app.servers.get(server_id).await?.read()?;
        page.authors = own_authors(&conn, std::mem::take(&mut page.authors)).await?;
    }
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
    let message = shown_one(app, server_id, link, reply.message).await?;
    let author = match reply.author {
        Some(author) if link.instance.origin.is_some() => {
            let conn = app.servers.get(server_id).await?.read()?;
            own_authors(&conn, vec![author]).await?.pop()
        }
        author => author,
    };
    Ok(pb::GetMessageResponse { message: Some(message), author })
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
    review_here(app, sdb, member, access, &channel_id, automod::Text::plain(&req.content), &[]).await?;
    let emojis = own_emojis(app, &*sdb.read()?, &sdb.id, &req.content, link).await?;
    let call = Call::Edit(cpb::GuestEdit {
        guest: Some(guest),
        message_id: req.message_id.clone(),
        content: req.content.clone(),
        emojis,
    });
    let reply = to_home(app, &sdb.id, link, call).await?;
    shown_one(app, &sdb.id, link, reply.message).await
}

/// The custom emoji of the channel's home server, for the picker in a
/// channel this server shows from it.
pub(super) async fn guest_emojis(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
) -> Result<Vec<pb::Emoji>> {
    let call = Call::Emojis(cpb::GuestEmojis { guest: Some(guest) });
    Ok(to_home(app, server_id, link, call).await?.emojis)
}

/// A page of the threads in a channel this server shows from another: its
/// home keeps them, and counts the search for the guest.
pub(super) async fn guest_threads(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    req: &pb::ListThreadsRequest,
) -> Result<pb::ListThreadsResponse> {
    let call = Call::Threads(cpb::GuestThreads {
        guest: Some(guest),
        query: req.query.clone(),
        archived: req.archived,
        limit: req.limit,
        after_thread_id: req.after_thread_id.clone(),
    });
    let mut page = to_home(app, server_id, link, call).await?.threads.unwrap_or_default();
    page.threads = shown_from(app, server_id, link, std::mem::take(&mut page.threads)).await?;
    if link.elsewhere() {
        let conn = app.servers.get(server_id).await?.read()?;
        page.authors = own_authors(&conn, std::mem::take(&mut page.authors)).await?;
    }
    Ok(page)
}

pub(super) async fn guest_follow(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    thread_id: &str,
    follow: bool,
) -> Result<()> {
    let call = Call::Follow(cpb::GuestFollow { guest: Some(guest), thread_id: thread_id.to_string(), follow });
    to_home(app, server_id, link, call).await.map(|_| ())
}

/// How long a home gets to say which of its threads someone follows.
const FOLLOWED_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The threads a guest follows in a channel this server shows from another,
/// as its home keeps them; none when it doesn't answer in time.
pub(super) async fn guest_followed(
    app: &Arc<App>,
    server_id: &str,
    link: LinkRow,
    guest: cpb::Guest,
) -> Result<Vec<String>> {
    let call = Call::Followed(cpb::GuestFollowed { guest: Some(guest) });
    match tokio::time::timeout(FOLLOWED_WAIT, to_home(app, server_id, &link, call)).await {
        Ok(reply) => Ok(reply?.thread_ids),
        Err(_) => Ok(vec![]),
    }
}

/// The channels this server shows from others, approved and in use.
pub(super) async fn links_in(conn: &turso::Connection) -> Result<Vec<LinkRow>> {
    Ok(all_links(conn).await?.into_iter().filter(|l| l.active && l.channel_id.is_some()).collect())
}

/// This server's own custom emoji that `content` uses, for a home that
/// doesn't have them: read from this server, so never another server's the
/// author belongs to, and only this instance's own pictures leave for
/// another instance.
async fn own_emojis(
    app: &App,
    conn: &turso::Connection,
    server_id: &str,
    content: &str,
    link: &LinkRow,
) -> Result<Vec<pb::Emoji>> {
    let ids: Vec<String> =
        messages::emoji_tokens(content).into_iter().take(MAX_OUTSIDE_EMOJIS).map(str::to_string).collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let public_url = &app.settings().public_url;
    let mut emojis = store::load_emojis_by_id(conn, server_id, &ids).await?;
    for emoji in &mut emojis {
        *emoji = shown_emoji(std::mem::take(emoji));
        if link.elsewhere() {
            emoji.url = own_picture(&emoji.url, public_url);
        }
    }
    emojis.retain(|emoji| !emoji.url.is_empty());
    Ok(emojis)
}

/// An emoji as another server sees it: what shows it, not who made it.
fn shown_emoji(emoji: pb::Emoji) -> pb::Emoji {
    pb::Emoji {
        id: emoji.id,
        server_id: emoji.server_id,
        name: emoji.name,
        url: emoji.url,
        animated: emoji.animated,
        ..Default::default()
    }
}

/// A guest's emoji as the home keeps them with its message: not the home
/// server's own (everyone shows those from its list, [`home_emojis`]); on
/// this instance only pictures of this instance's uploads (the guest's
/// server read them, and this keeps a slip there from putting any other
/// link in a message); and only ones the text uses. From another instance
/// [`arrived`] has already put them through this instance's proxy.
fn guest_emojis_kept(
    had: Vec<pb::Emoji>,
    sent: Vec<pb::Emoji>,
    content: &str,
    home_id: &str,
    public_url: &str,
    elsewhere: bool,
) -> Vec<pb::Emoji> {
    let uploads = format!("{}/media/", public_url.trim_end_matches('/'));
    let sent = sent
        .into_iter()
        .filter(|e| {
            e.server_id != home_id
                && parse_id("emoji", &e.id).is_ok()
                && (elsewhere || (e.url.starts_with(&uploads) && crate::media::id_in_url(&e.url).is_some()))
        })
        .map(shown_emoji)
        .collect();
    messages::kept_emojis(had, sent, content)
}

/// Puts the home server's own custom emoji that each message uses on it,
/// for guests, whose apps only know their own server's: one read for the
/// lot, at most [`MAX_OUTSIDE_EMOJIS`] on a message. One since deleted
/// shows as its name, as it does here.
async fn home_emojis(conn: &turso::Connection, home_id: &str, messages: &mut [pb::Message]) -> Result<()> {
    let mut wanted: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for message in messages.iter() {
        for id in messages::emoji_tokens(&message.content).into_iter().take(MAX_OUTSIDE_EMOJIS) {
            if !message.emojis.iter().any(|e| e.id == id) && seen.insert(id) {
                wanted.push(id.to_string());
            }
        }
    }
    if wanted.is_empty() {
        return Ok(());
    }
    let found: HashMap<String, pb::Emoji> = store::load_emojis_by_id(conn, home_id, &wanted)
        .await?
        .into_iter()
        .map(|emoji| (emoji.id.clone(), shown_emoji(emoji)))
        .collect();
    for message in messages {
        for id in messages::emoji_tokens(&message.content) {
            if message.emojis.len() >= MAX_OUTSIDE_EMOJIS {
                break;
            }
            if let Some(emoji) = found.get(id)
                && !message.emojis.iter().any(|e| e.id == id)
            {
                message.emojis.push(emoji.clone());
            }
        }
    }
    Ok(())
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

/// Votes in a poll in a channel this server shows from another. The caller
/// has checked the voter may, here.
pub(super) async fn guest_vote(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    message_id: &str,
    answer_ids: &[u32],
) -> Result<pb::Poll> {
    let call = Call::Vote(cpb::GuestVote {
        guest: Some(guest),
        message_id: message_id.to_string(),
        answer_ids: answer_ids.to_vec(),
    });
    to_home(app, server_id, link, call)
        .await?
        .poll
        .ok_or_else(|| Error::internal("the home server didn't say how the poll stands"))
}

pub(super) async fn guest_end_poll(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    message_id: &str,
) -> Result<pb::Poll> {
    let call = Call::EndPoll(cpb::GuestEndPoll { guest: Some(guest), message_id: message_id.to_string() });
    to_home(app, server_id, link, call)
        .await?
        .poll
        .ok_or_else(|| Error::internal("the home server didn't say how the poll stands"))
}

/// Who voted for an answer of a poll in a channel this server shows from
/// another: this server's own people as this instance has them, when the
/// home is on another instance.
pub(super) async fn guest_poll_voters(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    req: &pb::ListPollVotersRequest,
) -> Result<pb::ListPollVotersResponse> {
    let call = Call::Voters(cpb::GuestPollVoters {
        guest: Some(guest),
        message_id: req.message_id.clone(),
        answer_id: req.answer_id,
        limit: req.limit,
        after_id: req.after_id.clone(),
    });
    let mut voters = to_home(app, server_id, link, call).await?.voters.unwrap_or_default();
    if link.instance.origin.is_some() {
        let conn = app.servers.get(server_id).await?.read()?;
        voters.users = own_authors(&conn, std::mem::take(&mut voters.users)).await?;
    }
    Ok(voters)
}

/// Reacts to a message in a channel this server shows from another, or
/// takes the reaction off. The caller has checked the member may, here.
pub(super) async fn guest_react(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    req: &pb::ReactRequest,
) -> Result<pb::Reaction> {
    let call = Call::React(cpb::GuestReact {
        guest: Some(guest),
        message_id: req.message_id.clone(),
        emoji: req.emoji.clone(),
        emoji_id: req.emoji_id.clone(),
        reacted: req.reacted,
    });
    to_home(app, server_id, link, call)
        .await?
        .reaction
        .ok_or_else(|| Error::internal("the home server didn't say how the reaction stands"))
}

/// Who reacted to a message in a channel this server shows from another:
/// this server's own people as this instance has them, when the home is on
/// another instance.
pub(super) async fn guest_reactors(
    app: &Arc<App>,
    server_id: &str,
    link: &LinkRow,
    guest: cpb::Guest,
    req: &pb::ListReactorsRequest,
) -> Result<pb::ListReactorsResponse> {
    let call = Call::Reactors(cpb::GuestReactors {
        guest: Some(guest),
        message_id: req.message_id.clone(),
        emoji: req.emoji.clone(),
        emoji_id: req.emoji_id.clone(),
        limit: req.limit,
        after_id: req.after_id.clone(),
    });
    let mut reactors = to_home(app, server_id, link, call).await?.reactors.unwrap_or_default();
    if link.instance.origin.is_some() {
        let conn = app.servers.get(server_id).await?.read()?;
        reactors.users = own_authors(&conn, std::mem::take(&mut reactors.users)).await?;
    }
    Ok(reactors)
}

/// Answers a call from the other end of one of this process's servers'
/// shared channels.
pub async fn shared_call(app: &Arc<App>, call: cpb::SharedCall) -> Result<cpb::SharedReply> {
    let sdb = app.servers.get(&call.server_id).await?;
    let from = Instance::of(&call);
    if from.origin.is_some() {
        from_elsewhere(app, &sdb, &call, &from).await?;
    }
    match call.call.ok_or_else(|| Error::invalid("call is required"))? {
        Call::Lookup(lookup) => home_lookup(app, &sdb, lookup, &from).await,
        Call::Ask(ask) => home_ask(&sdb, ask, &from).await,
        // Boxed, the big ones: their futures would make this one too big for a stack.
        Call::Send(send) => Box::pin(home_send(app, &sdb, send, None, None)).await,
        Call::List(list) => home_list(app, &sdb, list).await,
        Call::Get(get) => home_get(app, &sdb, get).await,
        Call::Edit(edit) => Box::pin(home_edit(app, &sdb, edit)).await,
        Call::Delete(delete) => home_delete(app, &sdb, delete).await,
        Call::Poll(poll) => Box::pin(home_poll(app, &sdb, poll)).await,
        Call::Vote(vote) => home_vote(app, &sdb, vote).await,
        Call::EndPoll(end) => home_end_poll(&sdb, end).await,
        Call::Voters(voters) => home_voters(app, &sdb, voters).await,
        Call::React(react) => home_react(app, &sdb, react).await,
        Call::Reactors(reactors) => home_reactors(app, &sdb, reactors).await,
        Call::Emojis(emojis) => home_emoji_list(app, &sdb, emojis).await,
        Call::Reply(reply) => {
            let send = reply.send.ok_or_else(|| Error::invalid("send is required"))?;
            let thread = Reply {
                thread_id: reply.thread_id,
                also_in_channel: reply.also_in_channel,
                may_start: reply.may_start,
            };
            Box::pin(home_send(app, &sdb, send, None, Some(thread))).await
        }
        Call::Thread(thread) => Box::pin(home_thread(app, &sdb, thread)).await,
        Call::Threads(threads) => Box::pin(home_threads(app, &sdb, threads)).await,
        Call::Follow(follow) => home_follow(app, &sdb, follow).await,
        Call::Followed(followed) => home_followed(&sdb, followed).await,
        Call::Left(left) => home_left(&sdb, left).await,
        Call::Approved(approved) => guest_approved(app, &sdb, approved).await,
        Call::Ended(ended) => guest_ended(app, &sdb, ended).await,
        Call::Events(events) => guest_events(app, &sdb, events).await,
        Call::Updated(updated) => guest_updated(&sdb, updated).await,
        Call::Readers(readers) => home_readers(app, &sdb, readers).await,
        // Only an instance admin here asks these; never another instance.
        Call::AdminList(_) | Call::AdminEnd(_) if from.origin.is_some() => Err(Error::NotFound(GONE)),
        Call::AdminList(_) => {
            Ok(cpb::SharedReply { connections: Box::pin(connections_of(app, &sdb)).await?, ..Default::default() })
        }
        Call::AdminEnd(end) => {
            // Boxed: ending it calls the other end, which comes back through here.
            Box::pin(end_connection(app, &sdb, &end.actor_id, &end.connection_id, true)).await?;
            Ok(cpb::SharedReply::default())
        }
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

async fn home_lookup(app: &App, sdb: &ServerDb, lookup: cpb::ShareLookup, from: &Instance) -> Result<cpb::SharedReply> {
    let conn = sdb.read()?;
    let code = usable_code(&conn, &lookup.code, from).await?;
    let channel = load_channel(&conn, &sdb.id, &code.channel_id).await?.ok_or(Error::NotFound("share code"))?;
    let guests = guests_of(&conn, &channel.id).await?;
    if guests.iter().any(|g| g.server.id == lookup.guest_server_id) {
        return Err(Error::AlreadyExists("this server already shows that channel, or has asked to".into()));
    }
    // The preview names every outside provider that would read the guest's
    // people's messages, before their admins agree.
    let checked_by = automod::readers(app, &conn, &channel).await?;
    // Where its messages are kept, so the guest's admins know before asking.
    let region = store::load_server(&conn).await?.region;
    Ok(cpb::SharedReply {
        preview: Some(pb::PreviewShareResponse {
            home_server: Some(this_server(&conn, &sdb.id).await?),
            channel_name: channel.name,
            channel_topic: channel.topic,
            region,
            checked_by,
            allowed: permissions::to_list(from.shareable()),
            expires_at: code.expires_at,
            guest_count: guests.iter().filter(|g| g.active).count() as i32,
            instance: String::new(),
            fingerprint: String::new(),
        }),
        ..Default::default()
    })
}

/// Who the home's AutoMod sends a shared channel's messages to, for the
/// guest's admins. Only the server at the other end of the connection asks.
async fn home_readers(app: &App, sdb: &ServerDb, readers: cpb::GuestReaders) -> Result<cpb::SharedReply> {
    let conn = sdb.read()?;
    let row = guest_by_id(&conn, &readers.connection_id)
        .await?
        .filter(|row| row.server.id == readers.guest_server_id)
        .ok_or(Error::NotFound(GONE))?;
    let channel = load_channel(&conn, &sdb.id, &row.channel_id).await?.ok_or(Error::NotFound(GONE))?;
    let checked_by = automod::readers(app, &conn, &channel).await?;
    Ok(cpb::SharedReply { checked_by, ..Default::default() })
}

/// Asks each home who its AutoMod sends the channel's messages to, all at
/// once. A home that doesn't answer in time leaves its list empty.
async fn ask_readers(app: &Arc<App>, server_id: &str, connections: &mut [pb::SharedConnection]) {
    let asks: Vec<_> = connections
        .iter()
        .map(|c| {
            let call = cpb::SharedCall {
                server_id: c.server.as_ref().map(|s| s.id.clone()).unwrap_or_default(),
                call: Some(Call::Readers(cpb::GuestReaders {
                    connection_id: c.id.clone(),
                    guest_server_id: server_id.to_string(),
                })),
                ..Default::default()
            };
            tokio::time::timeout(READERS_WAIT, app.shared(call))
        })
        .collect();
    for (connection, answer) in connections.iter_mut().zip(futures::future::join_all(asks).await) {
        match answer {
            Ok(Ok(reply)) => connection.checked_by = reply.checked_by,
            _ => crate::reports::server_error("shared_readers", Some("SharedChannels/ListConnections")),
        }
    }
}

/// How long the Shared channels page waits for each home's providers.
const READERS_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

async fn home_ask(sdb: &ServerDb, ask: cpb::ShareAsk, from: &Instance) -> Result<cpb::SharedReply> {
    let guest = ask.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    if guest.id == sdb.id {
        return Err(Error::invalid("a server can't show its own channel twice"));
    }
    parse_id("connection id", &ask.connection_id)?;
    let connection = sdb
        .write(&ask.asked_by_id, async |conn, events| {
            let code = usable_code(conn, &ask.code, from).await?;
            let channel = load_channel(conn, &sdb.id, &code.channel_id).await?.ok_or(Error::NotFound("share code"))?;
            // The guest picks the connection's id; it can't be one this
            // server already has the other way round.
            if link_by_id(conn, &ask.connection_id).await?.is_some() {
                return Err(Error::AlreadyExists("that connection id is taken".into()));
            }
            if !shareable(&channel) || link_of(conn, &channel.id).await?.is_some() {
                return Err(Error::FailedPrecondition("that channel can't be shared".into()));
            }
            let guests = guests_of(conn, &channel.id).await?;
            if guests.iter().any(|g| g.server.id == guest.id) {
                return Err(Error::AlreadyExists("this server already shows that channel, or has asked to".into()));
            }
            if let Some(origin) = &from.origin {
                let waiting = query_one(
                    conn,
                    "SELECT count(*) FROM channel_guests WHERE instance = ?1 AND active = 0",
                    [origin.as_str()],
                    |r| r.get::<i64>(0),
                )
                .await?
                .unwrap_or(0);
                if waiting >= MAX_WAITING_FROM_INSTANCE {
                    return Err(Error::ResourceExhausted(
                        "this server has too many requests waiting from your instance; its admins have to answer some first"
                            .into(),
                    ));
                }
            }
            if guests.len() >= MAX_GUESTS {
                return Err(Error::FailedPrecondition(
                    "that channel is already shown in another server; for now a channel is shared with one other server"
                        .into(),
                ));
            }
            let now = now_ms();
            conn.execute(
                "INSERT INTO channel_guests (id, channel_id, guest_server_id, guest_name, guest_icon_url, active, allowed, asked_by_id, created_at, instance, instance_fingerprint)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10)",
                (
                    ask.connection_id.as_str(),
                    channel.id.as_str(),
                    guest.id.as_str(),
                    guest.name.as_str(),
                    guest.icon_url.as_str(),
                    from.shareable() as i64,
                    ask.asked_by_id.as_str(),
                    now,
                    from.origin.as_deref(),
                    from.fingerprint.as_deref(),
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
                allowed: permissions::to_list(from.shareable()),
                created_at: Some(timestamp(now)),
                checked_by: vec![],
                instance: String::new(),
                fingerprint: String::new(),
            })
        })
        .await?;
    Ok(cpb::SharedReply { connection: Some(connection), ..Default::default() })
}

/// A guest's new message with a poll: as [`home_send`] takes any, once the
/// poll is checked as the home's own people's are.
async fn home_poll(app: &Arc<App>, sdb: &ServerDb, poll: cpb::GuestPoll) -> Result<cpb::SharedReply> {
    let new = poll.poll.as_ref().ok_or_else(|| Error::invalid("poll is required"))?;
    let checked = super::polls::check(new, now_ms())?;
    let send = cpb::GuestSend {
        guest: poll.guest,
        content: poll.content,
        reply_to_id: poll.reply_to_id,
        emojis: poll.emojis,
        ..Default::default()
    };
    home_send(app, sdb, send, Some(checked), None).await
}

/// Where a guest's reply in a thread goes ([`cpb::GuestReply`]).
struct Reply {
    thread_id: String,
    also_in_channel: bool,
    may_start: bool,
}

async fn home_send(
    app: &Arc<App>,
    sdb: &ServerDb,
    send: cpb::GuestSend,
    poll: Option<pb::Poll>,
    thread: Option<Reply>,
) -> Result<cpb::SharedReply> {
    let guest = send.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    let author_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let limits = sdb.limits(&app.settings().limits).await?;
    if let Some(limit) = limits.storage_bytes
        && sdb.storage_bytes() >= limit
    {
        return Err(Error::ResourceExhausted("this channel's home server is out of storage".into()));
    }
    // A server on another instance names its own people, so it counts as
    // one sender here: it can't send more by naming more.
    let elsewhere = guest.server.as_ref().map(|s| s.id.clone()).filter(|id| id.contains('@'));
    let caps = app.settings().limits.clone();
    if let Some(server_id) = &elsewhere
        && !app.federation.take_send(server_id, caps.shared_remote_sends_per_minute)
    {
        return Err(Error::ResourceExhausted("that server is sending too fast; try again in a minute".into()));
    }
    let mut send = send;
    let public_url = app.settings().public_url.clone();
    let emojis = guest_emojis_kept(
        vec![],
        std::mem::take(&mut send.emojis),
        &send.content,
        &sdb.id,
        &public_url,
        elsewhere.is_some(),
    );
    let files = take_files(app, sdb, &guest, &mut send.attachments, &send.files).await?;
    let file_bytes: i64 = send.attachments.iter().map(|file| file.size).sum();
    // AutoMod reads the poll, the embeds' words and the files' names with the text.
    let reviewed = messages::reviewed_text(&pb::Message {
        content: send.content.clone(),
        embeds: send.embeds.clone(),
        attachments: send.attachments.clone(),
        poll: poll.clone(),
        ..Default::default()
    })
    .into_owned();
    let text = automod::Text { all: &reviewed, content: &send.content };
    let asked = ask_home(app, sdb, &guest, text, &send.attachments, &send.embeds, poll.is_some()).await;
    let message = sdb
        .write(&author_id, async |conn, events| {
            let (row, user, server) = connection(conn, &guest).await?;
            if blocked(conn, &row.channel_id, &user.id).await? {
                return Ok(Err(KEPT_OUT.to_string()));
            }
            if elsewhere.is_some()
                && let Some(why) =
                    newcomer_refused(conn, &row.channel_id, &user.id, &server.id, caps.shared_remote_people).await?
            {
                return Ok(Err(why.to_string()));
            }
            let channel = load_channel(conn, &sdb.id, &row.channel_id).await?.ok_or(Error::NotFound(GONE))?;
            let access = Access::guest(&channel.id, row.allowed);
            access.require_in(&channel.id, Permission::SendMessages)?;
            if !send.attachments.is_empty() {
                access.require_in(&channel.id, Permission::AttachFiles)?;
            }
            // Checked again here, where no other message can take the room meanwhile.
            if file_bytes > 0
                && let Some(limit) = limits.attachment_bytes
                && store::usage_count(conn, "attachment_bytes").await? + file_bytes > limit
            {
                return Err(Error::ResourceExhausted(format!(
                    "this channel's home server is out of room for files ({} in all)",
                    crate::media::size_label(limit)
                )));
            }
            if !send.embeds.is_empty() {
                access.require_in(&channel.id, Permission::EmbedLinks)?;
            }
            if poll.is_some() {
                access.require_in(&channel.id, Permission::CreatePolls)?;
            }
            if !send.reply_to_id.is_empty() {
                let replied = load_message(conn, &sdb.id, &send.reply_to_id).await?;
                if replied.is_none_or(|m| m.channel_id != channel.id || m.kind != pb::MessageKind::Unspecified as i32) {
                    return Err(Error::NotFound("message being replied to"));
                }
            }
            // Starting a thread takes the guest's own server's word and this one's grant.
            let parent = match &thread {
                Some(thread) => {
                    if !thread.may_start && super::threads::load(conn, &thread.thread_id).await?.is_none() {
                        return Err(Error::denied("you can't start threads here"));
                    }
                    Some(super::threads::check_reply(conn, &sdb.id, &access, &channel, &thread.thread_id).await?)
                }
                None => None,
            };
            remember(conn, &user, &server).await?;
            let verdict =
                review_guest(conn, &sdb.id, &user, &server, &access, &channel, text, asked.as_ref(), events).await?;
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
                poll: poll.clone(),
                emojis: emojis.clone(),
                thread_id: parent.as_ref().map(|p| p.id.clone()).unwrap_or_default(),
                also_in_channel: parent.is_some() && thread.as_ref().is_some_and(|t| t.also_in_channel),
                ..Default::default()
            };
            messages::insert_message(conn, &message, now).await?;
            events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message.clone()) }));
            if let Some(parent) = parent {
                super::threads::follow_quietly(conn, &parent.id, &message.author_id).await?;
                if parent.webhook.is_none() {
                    super::threads::follow_quietly(conn, &parent.id, &parent.author_id).await?;
                }
                super::threads::refresh(conn, &channel.id, &parent.id, events).await?;
            }
            Ok(Ok(message))
        })
        .await
        .and_then(|written| written.map_err(Error::denied));
    if message.is_err() {
        // Taken for a message that wasn't written: nothing has them.
        crate::attachments::drop_soon(app, &sdb.id, files);
    }
    let mut message = message?;
    home_emojis(&*sdb.read()?, &sdb.id, std::slice::from_mut(&mut message)).await?;
    Ok(cpb::SharedReply { message: Some(message), ..Default::default() })
}

/// Takes the files a guest sends into the home, before the write, checked
/// as the home checks its own people's and within its room for files: one
/// on this instance, its own upload for its server, claimed for the home
/// (so a file goes in one message) and put with it
/// ([`crate::cluster::pictures::take_shared`]); one on another instance,
/// fetched from there with its ticket ([`take_from_elsewhere`]). The ids
/// taken, which are dropped if the message isn't written.
async fn take_files(
    app: &Arc<App>,
    sdb: &ServerDb,
    guest: &cpb::Guest,
    files: &mut [pb::Attachment],
    tickets: &[cpb::SharedFile],
) -> Result<Vec<String>> {
    if files.is_empty() {
        return Ok(vec![]);
    }
    let server_id = guest.server.as_ref().map(|s| s.id.clone()).unwrap_or_default();
    let user_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    {
        let conn = sdb.read()?;
        let (row, _, _) = connection(&conn, guest).await?;
        let access = Access::guest(&row.channel_id, row.allowed);
        access.require_in(&row.channel_id, Permission::SendMessages)?;
        access.require_in(&row.channel_id, Permission::AttachFiles)?;
    }
    if server_id.contains('@') {
        return take_from_elsewhere(app, sdb, &server_id, files, tickets).await;
    }
    messages::check_extras(files, &[])?;
    // Each one as its upload's own link, whatever server's link it came as.
    let base = app.settings().public_url.clone();
    for file in files.iter_mut() {
        file.url = format!("{base}/media/{}", file.id);
    }
    let bytes = messages::check_files(app, &user_id, &server_id, &sdb.id, files).await?;
    let per_file = app.settings().limits.attachment_upload_bytes;
    if let Some(cap) = per_file
        && files.iter().any(|file| file.size > cap)
    {
        return Err(Error::ResourceExhausted(format!("files can be at most {} here", crate::media::size_label(cap))));
    }
    let limits = sdb.limits(&app.settings().limits).await?;
    if let Some(limit) = limits.attachment_bytes
        && sdb.usage().await?.attachment_bytes + bytes > limit
    {
        return Err(Error::ResourceExhausted(format!(
            "this channel's home server is out of room for files ({} in all)",
            crate::media::size_label(limit)
        )));
    }
    let mut taken = Vec::with_capacity(files.len());
    let took = async {
        for file in files.iter() {
            app.take_attachment(&file.id, &server_id, &sdb.id, &user_id).await?;
            taken.push(file.id.clone());
            let size = u64::try_from(file.size.min(per_file.unwrap_or(i64::MAX))).unwrap_or_default();
            crate::cluster::pictures::take_shared(app, &server_id, &sdb.id, &file.id, &user_id, size).await?;
        }
        Ok::<_, Error>(())
    }
    .await;
    if let Err(err) = took {
        crate::attachments::drop_soon(app, &sdb.id, taken);
        return Err(err);
    }
    Ok(taken)
}

/// Takes the files a guest server on another instance (`server_id`) sends,
/// as [`take_files`] says: refused before anything is fetched when any is
/// over this instance's caps (each file's size as that instance said it),
/// then each fetched with its ticket, its kind read here from its bytes
/// (never as that instance said), at most the size it said, and counted
/// toward that server's bytes for the day.
async fn take_from_elsewhere(
    app: &Arc<App>,
    sdb: &ServerDb,
    server_id: &str,
    files: &mut [pb::Attachment],
    tickets: &[cpb::SharedFile],
) -> Result<Vec<String>> {
    if tickets.len() != files.len() {
        return Err(Error::invalid("each file needs its ticket"));
    }
    if files.len() > messages::MAX_ATTACHMENTS {
        return Err(Error::invalid(format!("at most {} files per message", messages::MAX_ATTACHMENTS)));
    }
    if files.len() > 1 && files.iter().any(|file| file.voice.is_some()) {
        return Err(Error::invalid("a voice message is sent on its own"));
    }
    let caps = app.settings().limits.clone();
    let mut bytes = 0i64;
    for file in files.iter() {
        if file.size <= 0 {
            return Err(Error::invalid("a file's size is off"));
        }
        if let Some(cap) = caps.attachment_upload_bytes
            && file.size > cap
        {
            return Err(Error::ResourceExhausted(format!(
                "files can be at most {} here",
                crate::media::size_label(cap)
            )));
        }
        if file.voice.is_some()
            && let Some(cap) = caps.voice_message_bytes
            && file.size > cap
        {
            return Err(Error::ResourceExhausted(format!(
                "voice messages can be at most {} here",
                crate::media::size_label(cap)
            )));
        }
        bytes = bytes.saturating_add(file.size);
    }
    let limits = sdb.limits(&caps).await?;
    if let Some(limit) = limits.attachment_bytes
        && sdb.usage().await?.attachment_bytes + bytes > limit
    {
        return Err(Error::ResourceExhausted(format!(
            "this channel's home server is out of room for files ({} in all)",
            crate::media::size_label(limit)
        )));
    }
    let mut taken = Vec::with_capacity(files.len());
    let took = async {
        for (file, ticket) in files.iter_mut().zip(tickets) {
            let (id, kind, size) = match &app.link {
                crate::app::Link::Shard(_) => {
                    crate::cluster::pictures::take_elsewhere(app, &sdb.id, server_id, &ticket.ticket, file.size).await?
                }
                _ => crate::shared_files::take(app, &sdb.id, server_id, &ticket.ticket, file.size).await?,
            };
            taken.push(id.clone());
            let voice = file.voice.take();
            if let Some(voice) = &voice {
                messages::check_voice(voice, kind, size, &caps)?;
            }
            let sized = kind.starts_with("image/") || kind.starts_with("video/");
            let pixels = |n: i32| if sized { n.clamp(0, 65_535) } else { 0 };
            *file = pb::Attachment {
                url: crate::attachments::link(app, &sdb.id, &id),
                filename: crate::attachments::clean_name(&file.filename),
                content_type: kind.to_string(),
                size,
                width: pixels(file.width),
                height: pixels(file.height),
                id,
                voice,
            };
        }
        Ok::<_, Error>(())
    }
    .await;
    if let Err(err) = took {
        crate::attachments::drop_soon(app, &sdb.id, taken);
        return Err(err);
    }
    Ok(taken)
}

/// A file a message here has, for the instance at `origin`: only one in a
/// message shown in a channel this server shares with a server there now.
pub async fn file_for(
    conn: &turso::Connection,
    server_id: &str,
    media_id: &str,
    origin: &str,
) -> Result<Option<crate::attachments::Attached>> {
    let found = query_one(
        conn,
        "SELECT a.message_id FROM attachments a
         JOIN channel_guests g ON g.channel_id = a.channel_id AND g.active = 1 AND g.instance = ?2
         WHERE a.media_id = ?1 LIMIT 1",
        (media_id, origin),
        |r| r.get::<String>(0),
    )
    .await?;
    let Some(message_id) = found else { return Ok(None) };
    match load_message(conn, server_id, &message_id).await? {
        Some(message) if message.kind == pb::MessageKind::Unspecified as i32 => {
            crate::attachments::lookup(conn, media_id).await
        }
        _ => Ok(None),
    }
}

async fn home_list(app: &App, sdb: &ServerDb, list: cpb::GuestList) -> Result<cpb::SharedReply> {
    let guest = list.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    let (mut messages, has_more) =
        messages::page(&conn, &sdb.id, &row.channel_id, "", list.limit, &list.before_id, &list.after_id, true).await?;
    super::threads::attach(&conn, &mut messages).await?;
    super::polls::mark_mine(&conn, &user.id, &mut messages).await?;
    super::reactions::attach(&conn, &user.id, &mut messages).await?;
    let home = this_server(&conn, &sdb.id).await?;
    decorate(&conn, Some(&home), &mut messages).await?;
    home_emojis(&conn, &sdb.id, &mut messages).await?;
    let mut authors = users(&conn, &messages.iter().map(|m| m.author_id.as_str()).collect::<Vec<_>>()).await?;
    if user.id.contains('@') {
        let public_url = &app.settings().public_url;
        messages.iter_mut().for_each(|m| plain_message(m, public_url));
        authors = authors.iter().map(|a| plain_user(a, public_url)).collect();
    }
    Ok(cpb::SharedReply {
        page: Some(pb::ListMessagesResponse { messages, authors, has_more, parent: None }),
        ..Default::default()
    })
}

async fn home_get(app: &App, sdb: &ServerDb, get: cpb::GuestGet) -> Result<cpb::SharedReply> {
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
    super::threads::attach(&conn, std::slice::from_mut(&mut message)).await?;
    super::polls::mark_mine(&conn, &user.id, std::slice::from_mut(&mut message)).await?;
    super::reactions::attach(&conn, &user.id, std::slice::from_mut(&mut message)).await?;
    let home = this_server(&conn, &sdb.id).await?;
    decorate(&conn, Some(&home), std::slice::from_mut(&mut message)).await?;
    home_emojis(&conn, &sdb.id, std::slice::from_mut(&mut message)).await?;
    let mut author = store::user(&conn, &message.author_id).await?;
    if user.id.contains('@') {
        let public_url = &app.settings().public_url;
        plain_message(&mut message, public_url);
        author = author.as_ref().map(|a| plain_user(a, public_url));
    }
    Ok(cpb::SharedReply { message: Some(message), author, ..Default::default() })
}

/// Asks the home's provider rule about what a guest writes, before the
/// write, as `messages` does for the home's own people. `None` when the
/// guest's connection is gone, they're kept out, or the home doesn't let
/// them send this; the write then turns them away.
async fn ask_home(
    app: &Arc<App>,
    sdb: &ServerDb,
    guest: &cpb::Guest,
    text: automod::Text<'_>,
    attachments: &[pb::Attachment],
    embeds: &[pb::Embed],
    poll: bool,
) -> Option<automod::Asked> {
    let conn = sdb.read().ok()?;
    let (row, user, _) = connection(&conn, guest).await.ok()?;
    // Someone the home kept out is turned away by the write; their text
    // never goes to the provider.
    if blocked(&conn, &row.channel_id, &user.id).await.ok()? {
        return None;
    }
    drop(conn);
    let channel_id = row.channel_id.clone();
    let access = Access::guest(&channel_id, row.allowed);
    // Nothing of a message the write will refuse goes to the provider.
    if !access.has_in(&channel_id, Permission::SendMessages)
        || (!attachments.is_empty() && !access.has_in(&channel_id, Permission::AttachFiles))
        || (!embeds.is_empty() && !access.has_in(&channel_id, Permission::EmbedLinks))
        || (poll && !access.has_in(&channel_id, Permission::CreatePolls))
    {
        return None;
    }
    let pictures = automod::picture_links(attachments, embeds, &[]);
    let member = pb::Member { user: Some(user), ..Default::default() };
    automod::ask(app, sdb, &member, &access, &channel_id, text, &pictures).await
}

/// What AutoMod reads of `message` with its text changed to `content`: the
/// embeds and file names an edit leaves as they are come too.
fn edited_text(message: &pb::Message, content: &str) -> String {
    let edited = pb::Message { content: content.to_string(), ..message.clone() };
    messages::reviewed_text(&edited).into_owned()
}

async fn home_edit(app: &Arc<App>, sdb: &ServerDb, mut edit: cpb::GuestEdit) -> Result<cpb::SharedReply> {
    let guest = edit.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    let elsewhere = guest.server.as_ref().is_some_and(|s| s.id.contains('@'));
    let public_url = app.settings().public_url.clone();
    let sent = std::mem::take(&mut edit.emojis);
    let author_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    // Only new text the author wrote goes to a provider.
    let before = load_message(&*sdb.read()?, &sdb.id, &edit.message_id).await?;
    let asked = match before {
        Some(m) if m.author_id == author_id && m.content != edit.content => {
            let all = edited_text(&m, &edit.content);
            ask_home(app, sdb, &guest, automod::Text { all: &all, content: &edit.content }, &[], &[], false).await
        }
        _ => None,
    };
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
            messages::check_content(
                &edit.content,
                !message.attachments.is_empty() || !message.embeds.is_empty() || message.poll.is_some(),
            )?;
            let channel = load_channel(conn, &sdb.id, &row.channel_id).await?.ok_or(Error::NotFound(GONE))?;
            remember(conn, &user, &server).await?;
            if message.content != edit.content {
                let access = Access::guest(&channel.id, row.allowed);
                let verdict = review_guest(
                    conn,
                    &sdb.id,
                    &user,
                    &server,
                    &access,
                    &channel,
                    automod::Text { all: &edited_text(&message, &edit.content), content: &edit.content },
                    asked.as_ref(),
                    events,
                )
                .await?;
                if let Some(why) = verdict {
                    return Ok(Err(why));
                }
            }
            let old_size = messages::stored_size(&message);
            message.content = edit.content.clone();
            message.emojis = guest_emojis_kept(
                std::mem::take(&mut message.emojis),
                sent.clone(),
                &edit.content,
                &sdb.id,
                &public_url,
                elsewhere,
            );
            message.edited_at = Some(timestamp(now_ms()));
            messages::save_edit(conn, &message, old_size).await?;
            message.shared = Some(pb::SharedAuthor { user: Some(user), server: Some(server) });
            events.push(Payload::MessageUpdated(pb::MessageUpdated { message: Some(message.clone()) }));
            Ok(Ok(message))
        })
        .await?
        .map_err(Error::denied)?;
    let mut message = message;
    home_emojis(&*sdb.read()?, &sdb.id, std::slice::from_mut(&mut message)).await?;
    Ok(cpb::SharedReply { message: Some(message), ..Default::default() })
}

/// The home server's custom emoji, for a guest's picker in the channel.
/// A server on another instance counts as one sender here, reads too; it
/// can make up any number of people, so its calls are paced per server.
/// Gives that server's id when the guest is elsewhere.
fn pace_elsewhere(app: &App, guest: &cpb::Guest) -> Result<Option<String>> {
    let elsewhere = guest.server.as_ref().map(|s| s.id.clone()).filter(|id| id.contains('@'));
    if let Some(server_id) = &elsewhere
        && !app.federation.take_send(server_id, app.settings().limits.shared_remote_sends_per_minute)
    {
        return Err(Error::ResourceExhausted("that server is sending too fast; try again in a minute".into()));
    }
    Ok(elsewhere)
}

async fn home_emoji_list(app: &App, sdb: &ServerDb, call: cpb::GuestEmojis) -> Result<cpb::SharedReply> {
    let guest = call.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    let elsewhere = pace_elsewhere(app, &guest)?;
    let public_url = &app.settings().public_url;
    let mut emojis: Vec<pb::Emoji> =
        store::load_emojis(&conn, &sdb.id).await?.into_iter().take(MAX_HOME_EMOJIS).map(shown_emoji).collect();
    if elsewhere.is_some() {
        for emoji in &mut emojis {
            emoji.url = own_picture(&emoji.url, public_url);
        }
        emojis.retain(|emoji| !emoji.url.is_empty());
    }
    Ok(cpb::SharedReply { emojis, ..Default::default() })
}

/// A page of the replies in a thread in the channel, with the message it's under.
async fn home_thread(app: &App, sdb: &ServerDb, call: cpb::GuestThread) -> Result<cpb::SharedReply> {
    let guest = call.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    let (_, mut parent, summary) = super::threads::find_in(&conn, &sdb.id, &row.channel_id, &call.thread_id).await?;
    parent.thread = Some(summary);
    let (mut messages, has_more) = messages::page(
        &conn,
        &sdb.id,
        &row.channel_id,
        &call.thread_id,
        call.limit,
        &call.before_id,
        &call.after_id,
        true,
    )
    .await?;
    messages.push(parent);
    super::polls::mark_mine(&conn, &user.id, &mut messages).await?;
    super::reactions::attach(&conn, &user.id, &mut messages).await?;
    let home = this_server(&conn, &sdb.id).await?;
    decorate(&conn, Some(&home), &mut messages).await?;
    home_emojis(&conn, &sdb.id, &mut messages).await?;
    let mut authors = users(&conn, &messages.iter().map(|m| m.author_id.as_str()).collect::<Vec<_>>()).await?;
    if user.id.contains('@') {
        let public_url = &app.settings().public_url;
        messages.iter_mut().for_each(|m| plain_message(m, public_url));
        authors = authors.iter().map(|a| plain_user(a, public_url)).collect();
    }
    let parent = messages.pop();
    Ok(cpb::SharedReply {
        page: Some(pb::ListMessagesResponse { messages, authors, has_more, parent }),
        ..Default::default()
    })
}

/// A page of the channel's threads. A search is counted for the guest here,
/// where the threads are, under their id here.
async fn home_threads(app: &App, sdb: &ServerDb, call: cpb::GuestThreads) -> Result<cpb::SharedReply> {
    let guest = call.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    pace_elsewhere(app, &guest)?;
    let channel = load_channel(&conn, &sdb.id, &row.channel_id).await?.ok_or(Error::NotFound(GONE))?;
    let query = super::threads::search_text(&call.query, &user.id)?;
    let req = pb::ListThreadsRequest {
        archived: call.archived,
        limit: call.limit,
        after_thread_id: call.after_thread_id,
        ..Default::default()
    };
    let mut page = super::threads::page(&conn, &sdb.id, &channel, &query, &req).await?;
    super::polls::mark_mine(&conn, &user.id, &mut page.threads).await?;
    let home = this_server(&conn, &sdb.id).await?;
    decorate(&conn, Some(&home), &mut page.threads).await?;
    home_emojis(&conn, &sdb.id, &mut page.threads).await?;
    if user.id.contains('@') {
        let public_url = &app.settings().public_url;
        page.threads.iter_mut().for_each(|m| plain_message(m, public_url));
        page.authors = page.authors.iter().map(|a| plain_user(a, public_url)).collect();
    }
    Ok(cpb::SharedReply { threads: Some(page), ..Default::default() })
}

async fn home_follow(app: &App, sdb: &ServerDb, call: cpb::GuestFollow) -> Result<cpb::SharedReply> {
    let guest = call.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    pace_elsewhere(app, &guest)?;
    let actor = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    // Following changes only what reaches them, so it's no event and no audit entry.
    sdb.write(&actor, async |conn, _events| {
        let (row, user, server) = connection(conn, &guest).await?;
        if blocked(conn, &row.channel_id, &user.id).await? {
            return Ok(Err(KEPT_OUT.to_string()));
        }
        super::threads::find_in(conn, &sdb.id, &row.channel_id, &call.thread_id).await?;
        remember(conn, &user, &server).await?;
        super::threads::set_follow(conn, &call.thread_id, &user.id, call.follow).await?;
        Ok(Ok(()))
    })
    .await?
    .map_err(Error::denied)?;
    Ok(cpb::SharedReply::default())
}

async fn home_followed(sdb: &ServerDb, call: cpb::GuestFollowed) -> Result<cpb::SharedReply> {
    let guest = call.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    let thread_ids = super::threads::followed(&conn, &user.id, |channel| channel == row.channel_id).await?;
    Ok(cpb::SharedReply { thread_ids, ..Default::default() })
}

async fn home_delete(app: &Arc<App>, sdb: &ServerDb, delete: cpb::GuestDelete) -> Result<cpb::SharedReply> {
    let guest = delete.guest.clone().ok_or_else(|| Error::invalid("guest is required"))?;
    let actor_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let files = sdb
        .write(&actor_id, async |conn, events| {
            let (row, user, server) = connection(conn, &guest).await?;
            let message = load_message(conn, &sdb.id, &delete.message_id)
                .await?
                .filter(|m| m.channel_id == row.channel_id)
                .ok_or(Error::NotFound("message"))?;
            // A thread others replied in goes with its message only by the home's moderators.
            if message.thread_id.is_empty()
                && super::threads::load(conn, &message.id).await?.is_some()
                && super::threads::others_replied(conn, &message.id, &message.author_id).await?
            {
                return Err(Error::denied("others replied in its thread; only this channel's home can delete it now"));
            }
            if message.author_id != user.id {
                // A guest's moderators delete their own server's people's messages.
                let theirs = guest.moderator
                    && guests_among(conn, &[message.author_id.as_str()]).await?.get(&message.author_id)
                        == Some(&server.id);
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
            let mut files = messages::remove_message(conn, &message).await?;
            events.push(Payload::MessageDeleted(pb::MessageDeleted {
                channel_id: message.channel_id.clone(),
                message_id: message.id.clone(),
            }));
            files.extend(
                super::threads::after_delete(conn, &message.channel_id, &message.id, &message.thread_id, events)
                    .await?,
            );
            Ok(files)
        })
        .await?;
    crate::attachments::drop_soon(app, &sdb.id, files);
    Ok(cpb::SharedReply::default())
}

/// A poll in the channel a guest's connection shows: one in a message
/// people wrote there, not in a thread (threads stay with the home).
async fn poll_in(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
    message_id: &str,
) -> Result<super::polls::Row> {
    let shown = load_message(conn, server_id, message_id)
        .await?
        .is_some_and(|m| m.channel_id == channel_id && m.kind == pb::MessageKind::Unspecified as i32 && in_channel(&m));
    if !shown {
        return Err(Error::NotFound("poll"));
    }
    super::polls::load(conn, message_id).await?.ok_or(Error::NotFound("poll"))
}

async fn home_vote(app: &Arc<App>, sdb: &ServerDb, vote: cpb::GuestVote) -> Result<cpb::SharedReply> {
    let guest = vote.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    // Anonymous polls never say who voted, so the write names no one then.
    let anonymous = {
        let conn = sdb.read()?;
        let (row, user, _) = connection(&conn, &guest).await?;
        if blocked(&conn, &row.channel_id, &user.id).await? {
            return Err(Error::denied(KEPT_OUT));
        }
        poll_in(&conn, &sdb.id, &row.channel_id, &vote.message_id).await?.poll.anonymous
    };
    // A server on another instance counts as one sender here, votes and all.
    let elsewhere = guest.server.as_ref().map(|s| s.id.clone()).filter(|id| id.contains('@'));
    if let Some(server_id) = &elsewhere
        && !app.federation.take_send(server_id, app.settings().limits.shared_remote_sends_per_minute)
    {
        return Err(Error::ResourceExhausted("that server is sending too fast; try again in a minute".into()));
    }
    let actor = if anonymous { String::new() } else { guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default() };
    let poll = sdb
        .write(&actor, async |conn, events| {
            let (row, user, server) = connection(conn, &guest).await?;
            if blocked(conn, &row.channel_id, &user.id).await? {
                return Ok(Err(KEPT_OUT.to_string()));
            }
            let poll = poll_in(conn, &sdb.id, &row.channel_id, &vote.message_id).await?;
            remember(conn, &user, &server).await?;
            super::polls::cast(conn, poll, &vote.message_id, &user.id, &vote.answer_ids, events).await.map(Ok)
        })
        .await?
        .map_err(Error::denied)?;
    Ok(cpb::SharedReply { poll: Some(poll), ..Default::default() })
}

async fn home_end_poll(sdb: &ServerDb, end: cpb::GuestEndPoll) -> Result<cpb::SharedReply> {
    let guest = end.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let actor_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let poll = sdb
        .write(&actor_id, async |conn, events| {
            let (row, user, server) = connection(conn, &guest).await?;
            if blocked(conn, &row.channel_id, &user.id).await? {
                return Ok(Err(KEPT_OUT.to_string()));
            }
            let poll = poll_in(conn, &sdb.id, &row.channel_id, &end.message_id).await?;
            if poll.author_id != user.id {
                // A guest's moderators end their own server's people's polls,
                // as they delete their messages.
                let theirs = guest.moderator
                    && guests_among(conn, &[poll.author_id.as_str()]).await?.get(&poll.author_id) == Some(&server.id);
                if !theirs {
                    return Err(Error::denied("you can only end your own polls here, or your server's people's"));
                }
            }
            remember(conn, &user, &server).await?;
            super::polls::close(conn, &sdb.id, poll, &end.message_id, &user.id, events).await.map(Ok)
        })
        .await?
        .map_err(Error::denied)?;
    Ok(cpb::SharedReply { poll: Some(poll), ..Default::default() })
}

async fn home_voters(app: &App, sdb: &ServerDb, voters: cpb::GuestPollVoters) -> Result<cpb::SharedReply> {
    let guest = voters.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    let poll = poll_in(&conn, &sdb.id, &row.channel_id, &voters.message_id).await?;
    let (ids, has_more) =
        super::polls::voters(&conn, &poll, &voters.message_id, voters.answer_id, voters.limit, &voters.after_id)
            .await?;
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut found = users(&conn, &ids).await?;
    found.sort_by(|a, b| a.id.cmp(&b.id));
    if user.id.contains('@') {
        let public_url = &app.settings().public_url;
        found = found.iter().map(|u| plain_user(u, public_url)).collect();
    }
    Ok(cpb::SharedReply { voters: Some(pb::ListPollVotersResponse { users: found, has_more }), ..Default::default() })
}

/// A message in the shared channel a guest may react to: one people wrote.
async fn reactable(
    conn: &turso::Connection,
    server_id: &str,
    channel_id: &str,
    message_id: &str,
) -> Result<pb::Message> {
    load_message(conn, server_id, message_id)
        .await?
        .filter(|m| m.channel_id == channel_id && m.kind == pb::MessageKind::Unspecified as i32)
        .ok_or(Error::NotFound("message"))
}

async fn home_react(app: &Arc<App>, sdb: &ServerDb, react: cpb::GuestReact) -> Result<cpb::SharedReply> {
    let guest = react.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    // A server on another instance counts as one sender here, reactions and all.
    let elsewhere = guest.server.as_ref().map(|s| s.id.clone()).filter(|id| id.contains('@'));
    if let Some(server_id) = &elsewhere
        && !app.federation.take_send(server_id, app.settings().limits.shared_remote_sends_per_minute)
    {
        return Err(Error::ResourceExhausted("that server is sending too fast; try again in a minute".into()));
    }
    let cap = app.settings().limits.reactions_per_message;
    let actor_id = guest.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let reaction = sdb
        .write(&actor_id, async |conn, events| {
            let (row, user, server) = connection(conn, &guest).await?;
            if blocked(conn, &row.channel_id, &user.id).await? {
                return Ok(Err(KEPT_OUT.to_string()));
            }
            // What the home lets the guest's people do; taking one's own off
            // needs nothing.
            if react.reacted
                && !Access::guest(&row.channel_id, row.allowed).has_in(&row.channel_id, Permission::AddReactions)
            {
                return Ok(Err("this channel's home server doesn't let your server's people react here".to_string()));
            }
            let message = reactable(conn, &sdb.id, &row.channel_id, &react.message_id).await?;
            remember(conn, &user, &server).await?;
            super::reactions::apply(conn, &message, &react.emoji, &react.emoji_id, &user.id, react.reacted, cap, events)
                .await
                .map(Ok)
        })
        .await?
        .map_err(Error::denied)?;
    let mut reaction = reaction;
    if actor_id.contains('@') {
        reaction.emoji_url = own_picture(&reaction.emoji_url, &app.settings().public_url);
    }
    Ok(cpb::SharedReply { reaction: Some(reaction), ..Default::default() })
}

async fn home_reactors(app: &App, sdb: &ServerDb, call: cpb::GuestReactors) -> Result<cpb::SharedReply> {
    let guest = call.guest.ok_or_else(|| Error::invalid("guest is required"))?;
    let conn = sdb.read()?;
    let (row, user, _) = connection(&conn, &guest).await?;
    if blocked(&conn, &row.channel_id, &user.id).await? {
        return Err(Error::denied(KEPT_OUT));
    }
    reactable(&conn, &sdb.id, &row.channel_id, &call.message_id).await?;
    let mut page = super::reactions::reactor_page(
        &conn,
        &call.message_id,
        &call.emoji,
        &call.emoji_id,
        call.limit,
        &call.after_id,
    )
    .await?;
    if user.id.contains('@') {
        let public_url = &app.settings().public_url;
        page.users = page.users.iter().map(|u| plain_user(u, public_url)).collect();
    }
    Ok(cpb::SharedReply { reactors: Some(page), ..Default::default() })
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
    // What its people followed here goes with it; their replies stay, as their messages do.
    conn.execute(
        "DELETE FROM thread_follows
         WHERE thread_id IN (SELECT id FROM threads WHERE channel_id = ?1)
           AND user_id IN (SELECT id FROM users WHERE guest_of = ?2)",
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
        link_by_id(&*sdb.read()?, &home.connection_id).await?.filter(|l| l.active).ok_or(Error::NotFound(GONE))?;
    let Some(channel_id) = link.channel_id.clone() else { return Err(Error::NotFound(GONE)) };
    let mut events = home.events;
    if link.instance.origin.is_some() {
        let conn = sdb.read()?;
        let mut checked = Vec::with_capacity(events.len());
        for mut event in events {
            if let Some(
                Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
                | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }),
            ) = event.payload.as_mut()
                && own_people(&conn, &sdb.id, &link.home.id, m).await.is_err()
            {
                continue;
            }
            checked.push(event);
        }
        events = checked;
    }
    let events: Vec<pb::Event> = events
        .into_iter()
        .filter_map(|mut event| {
            match event.payload.as_mut()? {
                Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
                | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }) => {
                    m.server_id = sdb.id.clone();
                    m.channel_id = channel_id.clone();
                    no_pings(m);
                    files_here(app, &link, m);
                }
                Payload::MessageDeleted(d) => d.channel_id = channel_id.clone(),
                Payload::PollUpdated(p) => p.channel_id = channel_id.clone(),
                Payload::ThreadUpdated(t) => t.channel_id = channel_id.clone(),
                Payload::ReactionUpdated(r) => r.channel_id = channel_id.clone(),
                Payload::ReactionsCleared(c) => c.channel_id = channel_id.clone(),
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

/// A server's connections, both ends, as its Shared channels page shows them.
/// Each side sees who reads the channel's messages: the home its own
/// providers, a guest the home's, asked of it now.
async fn connections_of(app: &Arc<App>, sdb: &ServerDb) -> Result<Vec<pb::SharedConnection>> {
    let conn = sdb.read()?;
    let channels: HashMap<String, pb::Channel> =
        store::load_channels(&conn, &sdb.id).await?.into_iter().map(|c| (c.id.clone(), c)).collect();
    let mut connections: Vec<pb::SharedConnection> = Vec::new();
    let mut readers: HashMap<String, Vec<String>> = HashMap::new();
    for row in all_guests(&conn).await? {
        let channel = channels.get(&row.channel_id);
        let mut connection = home_connection(&row, &channel.map(|c| c.name.clone()).unwrap_or_default());
        if let Some(channel) = channel {
            if !readers.contains_key(&channel.id) {
                readers.insert(channel.id.clone(), automod::readers(app, &conn, channel).await?);
            }
            connection.checked_by = readers[&channel.id].clone();
        }
        connections.push(connection);
    }
    let mut links: Vec<pb::SharedConnection> = all_links(&conn).await?.iter().map(guest_connection).collect();
    drop(conn);
    ask_readers(app, &sdb.id, &mut links).await;
    connections.extend(links);
    Ok(connections)
}

/// Ends one of a server's connections, at whichever end it is: the server
/// it's shown in goes (at the home), or its channel here goes (at a guest).
/// `by_admin` marks an instance admin's doing in the audit log.
async fn end_connection(
    app: &Arc<App>,
    sdb: &ServerDb,
    actor_id: &str,
    connection_id: &str,
    by_admin: bool,
) -> Result<()> {
    let by = |audit: Audit| if by_admin { audit.change("by", "", "an instance admin") } else { audit };
    let (ended, channel_id) = sdb
        .write(actor_id, async |conn, events| {
            if let Some(row) = guest_by_id(conn, connection_id).await? {
                let channel = load_channel(conn, &sdb.id, &row.channel_id).await?.map(|c| c.name).unwrap_or_default();
                drop_guest(conn, &sdb.id, &row.id, events).await?;
                let audit = Audit::new(pb::AuditAction::SharedChannelDisconnect, &row.server.id)
                    .channel(channel)
                    .change("server", "", &row.server.name);
                store::audit(conn, actor_id, by(audit)).await?;
                return Ok((Ended { guests: vec![(row.id, row.server.id)], links: vec![] }, None));
            }
            let link = link_by_id(conn, connection_id).await?.ok_or(Error::NotFound("connection"))?;
            let channel_id = drop_link(conn, &sdb.id, &link.id, events).await?;
            let audit = Audit::new(pb::AuditAction::SharedChannelDisconnect, &link.home.id)
                .channel(&link.home_channel_name)
                .change("server", "", &link.home.name);
            store::audit(conn, actor_id, by(audit)).await?;
            Ok((Ended { guests: vec![], links: vec![(link.id, link.home.id)] }, channel_id))
        })
        .await?;
    if let Some(channel_id) = channel_id {
        app.forget_notifications(&sdb.id, Some(&channel_id), None).await;
    }
    tell_ended(app, &sdb.id, actor_id, ended).await;
    Ok(())
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
        // What the other end said stays out of the log: it may be another
        // instance's words.
        if app
            .shared(cpb::SharedCall { server_id: guest_server_id, call: Some(call), ..Default::default() })
            .await
            .is_err()
        {
            crate::reports::server_error("shared_ended", Some("SharedChannels/ended"));
            tracing::info!("couldn't tell a server a shared channel ended");
        }
    }
    for (connection_id, home_server_id) in ended.links {
        let left =
            cpb::GuestLeft { connection_id, guest_server_id: server_id.to_string(), actor_id: actor_id.to_string() };
        if app
            .shared(cpb::SharedCall { server_id: home_server_id, call: Some(Call::Left(left)), ..Default::default() })
            .await
            .is_err()
        {
            crate::reports::server_error("shared_ended", Some("SharedChannels/ended"));
            tracing::info!("couldn't tell a server a shared channel ended");
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
        let rows = all_guests(&*sdb.read()?).await?;
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
        Err(_) => tracing::warn!("couldn't read a server's shared channels"),
    }
    targets
}

/// Whether a message shows in its channel: not a thread reply kept to its thread.
fn in_channel(m: &pb::Message) -> bool {
    m.thread_id.is_empty() || m.also_in_channel
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
        Payload::ThreadUpdated(t) => Some(&t.channel_id),
        // Checked by [`for_guests`]: only polls in messages shown in the channel.
        Payload::PollUpdated(p) => Some(&p.channel_id),
        // Checked by [`for_guests`]: only on messages people wrote.
        Payload::ReactionUpdated(r) => Some(&r.channel_id),
        Payload::ReactionsCleared(c) => Some(&c.channel_id),
        _ => None,
    }
}

/// An event as guests get it: each message says who wrote it, since its
/// author needn't be in the guest server.
async fn for_guests(app: &App, event: &pb::Event) -> Option<pb::Event> {
    let reacted_to = match &event.payload {
        Some(Payload::ReactionUpdated(r)) => Some(&r.message_id),
        Some(Payload::ReactionsCleared(c)) => Some(&c.message_id),
        _ => None,
    };
    if let Some(message_id) = reacted_to {
        let shown = async {
            let sdb = app.servers.get(&event.server_id).await?;
            let message = load_message(&*sdb.read()?, &sdb.id, message_id).await?;
            Ok::<_, Error>(message.is_some_and(|m| m.kind == pb::MessageKind::Unspecified as i32))
        }
        .await;
        return match shown {
            Ok(true) => Some(event.clone()),
            Ok(false) => None,
            Err(_) => {
                tracing::warn!("couldn't read a shared reaction's message");
                None
            }
        };
    }
    if let Some(Payload::PollUpdated(updated)) = &event.payload {
        // A poll in a thread stays with the home, as threads do.
        let shown = async {
            let sdb = app.servers.get(&event.server_id).await?;
            let message = load_message(&*sdb.read()?, &sdb.id, &updated.message_id).await?;
            Ok::<_, Error>(message.is_some_and(|m| m.kind == pb::MessageKind::Unspecified as i32 && in_channel(&m)))
        }
        .await;
        return match shown {
            Ok(true) => Some(event.clone()),
            Ok(false) => None,
            Err(_) => {
                tracing::warn!("couldn't read a shared poll's message");
                None
            }
        };
    }
    let mut event = event.clone();
    if let Some(
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }),
    ) = event.payload.as_mut()
    {
        no_pings(m);
    }
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
            decorate(&conn, Some(&home), std::slice::from_mut(m)).await?;
            super::threads::attach(&conn, std::slice::from_mut(m)).await?;
            home_emojis(&conn, &sdb.id, std::slice::from_mut(m)).await
        }
        .await;
        if decorated.is_err() {
            crate::reports::server_error("shared_decorate", Some("SharedChannels/fanout"));
            tracing::warn!("couldn't say who wrote a shared message");
            return None;
        }
    }
    Some(event)
}

/// An event as it leaves for another instance: who wrote a message, and
/// nothing more of them or their server.
fn leaving(event: &pb::Event, public_url: &str) -> pb::Event {
    let mut event = event.clone();
    if let Some(
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }),
    ) = event.payload.as_mut()
    {
        plain_message(m, public_url);
    }
    if let Some(Payload::ReactionUpdated(pb::ReactionUpdated { reaction: Some(r), .. })) = event.payload.as_mut()
        && !r.emoji_id.is_empty()
    {
        r.emoji_url = own_picture(&r.emoji_url, public_url);
    }
    event
}

/// A message as it leaves for another instance, as [`leaving`] says: its
/// files only by id (kept here, fetched for that instance's people with a
/// signed request, [`crate::shared_files`]), and only this instance's own
/// pictures, which the other fetches through its own proxy.
fn plain_message(message: &mut pb::Message, public_url: &str) {
    message.attachments.retain(|file| crate::media::parse_id(&file.id).is_some());
    for file in &mut message.attachments {
        file.url.clear();
    }
    message.emojis.retain_mut(|emoji| {
        emoji.url = own_picture(&emoji.url, public_url);
        emoji.creator_id.clear();
        !emoji.url.is_empty()
    });
    if let Some(gif) = &mut message.gif {
        gif.url = own_picture(&gif.url, public_url);
        gif.seal.clear();
    }
    if message.gif.as_ref().is_some_and(|gif| gif.url.is_empty()) {
        message.gif = None;
    }
    for embed in &mut message.embeds {
        embed.image_url = own_picture(&embed.image_url, public_url);
        embed.thumbnail_url = own_picture(&embed.thumbnail_url, public_url);
    }
    if let Some(shared) = &mut message.shared {
        shared.user = shared.user.as_ref().map(|u| plain_user(u, public_url));
        if let Some(server) = &mut shared.server {
            server.icon_url = own_picture(&server.icon_url, public_url);
        }
    }
    if let Some(webhook) = &mut message.webhook {
        webhook.avatar_url = own_picture(&webhook.avatar_url, public_url);
    }
    plain_reactions(&mut message.reactions, public_url);
}

/// Reactions as they leave for another instance: custom emoji only with a
/// picture of this instance's own.
fn plain_reactions(reactions: &mut Vec<pb::Reaction>, public_url: &str) {
    reactions.retain_mut(|r| {
        if r.emoji_id.is_empty() {
            return true;
        }
        r.emoji_url = own_picture(&r.emoji_url, public_url);
        !r.emoji_url.is_empty()
    });
}

/// How long a guest server on another instance is waited for before trying
/// again, when it can't be reached.
const REMOTE_RETRIES: [std::time::Duration; 3] =
    [std::time::Duration::from_secs(1), std::time::Duration::from_secs(4), std::time::Duration::from_secs(15)];

/// What waits for one guest server, at most: past this many, or this old,
/// what happened is dropped for it (its people see it when they next open
/// the channel), so a slow server can't make the home hold more and more.
const QUEUED: usize = 512;
const QUEUED_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// What's waiting to go to one guest server: the home, the connection, the
/// call, and when it started waiting.
type Outgoing = (String, String, cpb::SharedCall, std::time::Instant);

/// One guest server's queue: its events go in order, and a slow or missing
/// server holds up only itself.
fn spawn_queue(app: Arc<App>) -> mpsc::Sender<Outgoing> {
    let (tx, mut rx) = mpsc::channel::<Outgoing>(QUEUED);
    tokio::spawn(async move {
        while let Some((home_id, connection_id, call, queued)) = rx.recv().await {
            if queued.elapsed() > QUEUED_FOR {
                crate::reports::server_error("shared_fanout_dropped", Some("SharedChannels/fanout"));
                continue;
            }
            let started = std::time::Instant::now();
            let mut passed = app.shared(call.clone()).await;
            // A server on another instance that can't be reached gets a
            // few more tries; after that, what waited for it is dropped
            // (its people see it when they next open the channel).
            if call.server_id.contains('@') {
                for wait in REMOTE_RETRIES {
                    if !matches!(passed, Err(Error::Unavailable(_))) {
                        break;
                    }
                    tokio::select! {
                        () = tokio::time::sleep(wait) => {}
                        () = app.shutdown.cancelled() => return,
                    }
                    passed = app.shared(call.clone()).await;
                }
                if matches!(passed, Err(Error::Unavailable(_))) {
                    while rx.try_recv().is_ok() {}
                }
            }
            crate::reports::server_timing("shared.fanout", started.elapsed());
            match passed {
                Ok(_) => {}
                // The guest doesn't show the channel anymore: let it go here too.
                Err(err) if gone(&err) => {
                    let healed = async {
                        let sdb = app.servers.get(&home_id).await?;
                        sdb.write("", async |conn, events| drop_guest(conn, &sdb.id, &connection_id, events).await)
                            .await
                    }
                    .await;
                    if healed.is_err() {
                        crate::reports::server_error("shared_heal", Some("SharedChannels/fanout"));
                        tracing::warn!("couldn't end a shared channel its guest left");
                    }
                }
                // What the other end said stays out of the log: it's theirs.
                Err(_) => {
                    crate::reports::server_error("shared_fanout", Some("SharedChannels/fanout"));
                    tracing::info!("couldn't show a server what happened in a shared channel");
                }
            }
        }
    });
    tx
}

// ─────────────── Blocked instances ───────────────

/// Ends every share with an instance on the block list, on both sides: at
/// start-up, and each time a host joins the list. Runs where servers are
/// kept. The other instance is told the share ended (the only call that
/// still goes to a blocked instance); nothing from it is taken any more.
fn spawn_block_sweeper(app: Arc<App>) {
    let mut settings = app.watch_settings();
    tokio::spawn(async move {
        let mut swept: Vec<String> = Vec::new();
        loop {
            let blocked = settings.borrow_and_update().federation_blocked_hosts.clone();
            if blocked.iter().any(|host| !swept.contains(host)) {
                for sdb in app.servers.all() {
                    if end_blocked(&app, &sdb).await.is_err() {
                        crate::reports::server_error("shared_block", Some("SharedChannels/block"));
                        tracing::warn!("couldn't end a server's shares with a blocked instance");
                    }
                }
            }
            swept = blocked;
            tokio::select! {
                changed = settings.changed() => if changed.is_err() { break },
                () = app.shutdown.cancelled() => break,
            }
        }
    });
}

/// Ends one server's shares with blocked instances, as an instance admin
/// ending them would.
async fn end_blocked(app: &Arc<App>, sdb: &ServerDb) -> Result<()> {
    let conn = sdb.read()?;
    let guests = query_all(
        &conn,
        &format!("SELECT {GUEST_COLUMNS} FROM channel_guests WHERE instance IS NOT NULL"),
        (),
        guest_row,
    )
    .await?;
    let links =
        query_all(&conn, &format!("SELECT {LINK_COLUMNS} FROM channel_links WHERE instance IS NOT NULL"), (), link_row)
            .await?;
    drop(conn);
    let gone = guests
        .into_iter()
        .map(|row| (row.id, row.instance))
        .chain(links.into_iter().map(|row| (row.id, row.instance)))
        .filter(|(_, instance)| instance.origin.as_deref().is_some_and(|origin| federation::blocked(app, origin)));
    for (connection_id, _) in gone {
        Box::pin(end_connection(app, sdb, "", &connection_id, true)).await?;
    }
    Ok(())
}

/// Passes what happens in the shared channels kept here on to the servers
/// that show them, and ends shares with blocked instances. Runs where
/// servers are kept.
pub fn spawn_shared_fanout(app: Arc<App>) {
    spawn_block_sweeper(app.clone());
    let mut events = app.hub.shared_tap();
    tokio::spawn(async move {
        let mut known: HashMap<String, Arc<HashMap<String, Vec<Target>>>> = HashMap::new();
        let mut queues: HashMap<String, mpsc::Sender<Outgoing>> = HashMap::new();
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
                let shown = if target.guest_server_id.contains('@') {
                    leaving(&shown, &app.settings().public_url)
                } else {
                    shown.clone()
                };
                let call = cpb::SharedCall {
                    server_id: target.guest_server_id.clone(),
                    call: Some(Call::Events(cpb::HomeEvents {
                        connection_id: target.connection_id.clone(),
                        events: vec![shown],
                    })),
                    ..Default::default()
                };
                let queue = queues.entry(target.guest_server_id.clone()).or_insert_with(|| spawn_queue(app.clone()));
                let outgoing = (event.server_id.clone(), target.connection_id.clone(), call, std::time::Instant::now());
                match queue.try_send(outgoing) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        crate::reports::server_error("shared_fanout_dropped", Some("SharedChannels/fanout"));
                    }
                    Err(mpsc::error::TrySendError::Closed(outgoing)) => {
                        let queue = spawn_queue(app.clone());
                        let _ = queue.try_send(outgoing);
                        queues.insert(target.guest_server_id.clone(), queue);
                    }
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
            "a shared channel can let other servers' people send messages, embed links, attach files, make polls, start threads and react, nothing more",
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
                if req.other_instances && (!self.app.settings().federation || federation::named_origin(&self.app).is_none()) {
                    return Err(Error::FailedPrecondition(
                        "this instance doesn't share channels with other instances".into(),
                    ));
                }
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
                            "INSERT INTO share_codes (code, channel_id, creator_id, created_at, expires_at, other_instances) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            (
                                code.code.as_str(),
                                channel.id.as_str(),
                                account.id.as_str(),
                                now,
                                now + CODE_TTL_MS,
                                i64::from(req.other_instances),
                            ),
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
                let code = pb::ShareCode { code: shown_code(&self.app, &code.code, req.other_instances), ..code };
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
                // As copied, it may end with this instance's host.
                let code = req.code.trim().split_once('@').map_or(req.code.trim(), |(code, _)| code);
                sdb.write(&account.id, async |conn, _events| {
                    let (_, channel_id, _) = query_one(
                        conn,
                        &format!("SELECT {CODE_COLUMNS} FROM share_codes WHERE code = ?1"),
                        [code],
                        code_row,
                    )
                    .await?
                    .ok_or(Error::NotFound("share code"))?;
                    conn.execute("DELETE FROM share_codes WHERE code = ?1", [code]).await?;
                    let channel = load_channel(conn, &sdb.id, &channel_id).await?.map(|c| c.name).unwrap_or_default();
                    store::audit(conn, &account.id, Audit::new(pb::AuditAction::ShareCodeDelete, code).channel(channel))
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
                let pasted = code_home(&self.app, &req.code)?;
                if pasted.home == sdb.id {
                    return Err(Error::invalid("that code is for one of this server's own channels"));
                }
                let lookup = cpb::ShareLookup { code: pasted.code, guest_server_id: sdb.id.clone() };
                let reply = self
                    .app
                    .shared(cpb::SharedCall {
                        server_id: pasted.home,
                        call: Some(Call::Lookup(lookup)),
                        ..Default::default()
                    })
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
                let pasted = code_home(&self.app, &req.code)?;
                let home = pasted.home;
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
                    code: pasted.code,
                    connection_id: new_id(),
                    guest: Some(this_server(&conn, &sdb.id).await?),
                    asked_by_id: account.id.clone(),
                };
                drop(conn);
                let reply = self
                    .app
                    .shared(cpb::SharedCall { server_id: home.clone(), call: Some(Call::Ask(ask)), ..Default::default() })
                    .await
                    .map_err(|err| if gone(&err) { Error::NotFound("share code") } else { err })?;
                let asked = reply.connection.ok_or_else(|| Error::internal("the home server didn't say what it took"))?;
                let home_server = asked.server.clone().unwrap_or_default();
                let saved = sdb
                    .write(&account.id, async |conn, events| {
                        let now = now_ms();
                        conn.execute(
                            "INSERT INTO channel_links (id, active, home_server_id, home_server_name, home_server_icon_url, home_channel_id, home_channel_name, allowed, name, parent_id, asked_by_id, created_at, instance, instance_fingerprint)
                             VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                                pasted.origin.as_deref(),
                                pasted.origin.is_some().then_some(asked.fingerprint.as_str()),
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
                        let _ = self.app.shared(cpb::SharedCall { server_id: home, call: Some(Call::Left(left)), ..Default::default() }).await;
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
                offered.server = Some(this_server(&*sdb.read()?, &sdb.id).await?);
                let approved = cpb::HomeApproved { connection: Some(offered), actor_id: account.id.clone() };
                self.app
                    .shared(cpb::SharedCall { server_id: row.server.id.clone(), call: Some(Call::Approved(approved)), ..Default::default() })
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
                let connections = connections_of(&self.app, &sdb).await?;
                let conn = sdb.read()?;
                let names: HashMap<String, String> =
                    store::load_channels(&conn, &sdb.id).await?.into_iter().map(|c| (c.id, c.name)).collect();
                let name = |id: &str| names.get(id).cloned().unwrap_or_default();
                let codes = query_all(
                    &conn,
                    &format!("SELECT {CODE_COLUMNS} FROM share_codes WHERE expires_at > ?1 ORDER BY created_at DESC"),
                    [now_ms()],
                    code_row,
                )
                .await?
                .into_iter()
                .map(|(mut code, channel_id, other_instances)| {
                    code.channel_name = name(&channel_id);
                    code.code = shown_code(&self.app, &code.code, other_instances);
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
                let row = guest_by_id(&*sdb.read()?, &req.connection_id).await?.ok_or(Error::NotFound("connection"))?;
                access.require_in(&row.channel_id, Permission::ManageChannels)?;
                let (row, channel_name) = sdb
                    .write(&account.id, async |conn, events| {
                        let before =
                            guest_by_id(conn, &req.connection_id).await?.ok_or(Error::NotFound("connection"))?;
                        let allowed = allowed & before.instance.shareable();
                        conn.execute(
                            "UPDATE channel_guests SET allowed = ?2 WHERE id = ?1",
                            (before.id.as_str(), allowed as i64),
                        )
                        .await?;
                        let channel =
                            load_channel(conn, &sdb.id, &before.channel_id).await?.map(|c| c.name).unwrap_or_default();
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
                    if self
                        .app
                        .shared(cpb::SharedCall {
                            server_id: row.server.id.clone(),
                            call: Some(call),
                            ..Default::default()
                        })
                        .await
                        .is_err()
                    {
                        tracing::info!("couldn't tell a server what its people may do in a shared channel");
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
                end_connection(&self.app, &sdb, &account.id, &req.connection_id, false).await?;
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
        let home = |code: &str| split_code(code).map(|(_, home, _)| home);
        assert_eq!(home(&code).unwrap(), server);
        assert_eq!(home(&format!("  {code} ")).unwrap(), server);
        assert!(home("nope").is_err());
        assert!(home(&format!("{server}-short")).is_err());
        assert!(home(&format!("not-an-id-{}", "a".repeat(CODE_LENGTH))).is_err());
        // And their instance, when they name one.
        let pasted = format!("{code}@chat.example.com");
        let (bare, home, at) = split_code(&pasted).unwrap();
        assert_eq!((bare, home.as_str(), at), (code.as_str(), server.as_str(), Some("chat.example.com")));
    }

    /// This instance, in the tests of calls from another.
    const OWN: &str = "https://fuwa.example";

    /// A picture from another instance, as this one's proxy would link it.
    fn proxied(url: &str) -> String {
        format!("{OWN}/media/outside/sig?url={url}")
    }

    #[test]
    fn guests_emoji_keep_only_their_servers_pictures_the_text_uses() {
        let emoji = |id: &str, server_id: &str, url: &str| pb::Emoji {
            id: id.into(),
            server_id: server_id.into(),
            name: "blob".into(),
            url: url.into(),
            creator_id: "someone".into(),
            size: 100,
            ..Default::default()
        };
        let upload = format!("{OWN}/media/01k6q7z8a1b2c3d4e5f6g7h8j9");
        let sent = vec![
            emoji("01K6Q7Z8A1B2C3D4E5F6G7H8J1", "guest", &upload),
            emoji("01K6Q7Z8A1B2C3D4E5F6G7H8J2", "home", &upload),
            emoji("01K6Q7Z8A1B2C3D4E5F6G7H8J3", "guest", "https://elsewhere.example/media/01k6q7z8a1b2c3d4e5f6g7h8j9"),
            emoji("01K6Q7Z8A1B2C3D4E5F6G7H8J4", "guest", &format!("{OWN}/not-media/x")),
            emoji("01K6Q7Z8A1B2C3D4E5F6G7H8J5", "guest", &upload),
            emoji("not an id", "guest", &upload),
        ];
        let text = "<:blob:01K6Q7Z8A1B2C3D4E5F6G7H8J1> <:blob:01K6Q7Z8A1B2C3D4E5F6G7H8J2> <:blob:01K6Q7Z8A1B2C3D4E5F6G7H8J3> <:blob:01K6Q7Z8A1B2C3D4E5F6G7H8J4> <:blob:not an id>";
        let kept = guest_emojis_kept(vec![], sent.clone(), text, "home", OWN, false);
        assert_eq!(kept.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["01K6Q7Z8A1B2C3D4E5F6G7H8J1"]);
        assert!(kept[0].creator_id.is_empty() && kept[0].size == 0);
        // From another instance [`arrived`] has checked the pictures already.
        let kept = guest_emojis_kept(vec![], sent, text, "home", OWN, true);
        let ids = kept.iter().map(|e| e.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, ["01K6Q7Z8A1B2C3D4E5F6G7H8J1", "01K6Q7Z8A1B2C3D4E5F6G7H8J3", "01K6Q7Z8A1B2C3D4E5F6G7H8J4"]);
    }

    #[test]
    fn emoji_from_another_instance_name_their_server_there() {
        let there = Pictures { origin: "https://night-owls.example", link: &proxied };
        let sent = vec![
            pb::Emoji {
                id: "01K6Q7Z8A1B2C3D4E5F6G7H8J1".into(),
                server_id: "01K6Q7Z8A1B2C3D4E5F6G7H8J9".into(),
                name: "blob".into(),
                url: "https://night-owls.example/media/e".into(),
                creator_id: "someone".into(),
                ..Default::default()
            },
            pb::Emoji {
                id: "01K6Q7Z8A1B2C3D4E5F6G7H8J2".into(),
                server_id: "01K6Q7Z8A1B2C3D4E5F6G7H8J9@third.example".into(),
                name: "cat".into(),
                url: "https://night-owls.example/media/c".into(),
                ..Default::default()
            },
            pb::Emoji {
                id: "01K6Q7Z8A1B2C3D4E5F6G7H8J3".into(),
                server_id: "01K6Q7Z8A1B2C3D4E5F6G7H8J9".into(),
                name: "far".into(),
                url: "https://third.example/media/f".into(),
                ..Default::default()
            },
        ];
        let read = their_emojis(&sent, "night-owls.example", &there);
        assert_eq!(read.len(), 1, "{read:?}");
        assert_eq!(read[0].server_id, "01K6Q7Z8A1B2C3D4E5F6G7H8J9@night-owls.example");
        assert_eq!(read[0].url, proxied("https://night-owls.example/media/e"));
        assert!(read[0].creator_id.is_empty());
    }

    #[test]
    fn polls_from_other_instances_are_clipped_and_name_only_theirs() {
        let origin = "https://night-owls.example";
        let (there, here) = (new_id(), new_id());
        let update = |voter_id: String, poll: pb::Poll| cpb::SharedCall {
            server_id: new_id(),
            call: Some(Call::Events(cpb::HomeEvents {
                connection_id: new_id(),
                events: vec![pb::Event {
                    payload: Some(Payload::PollUpdated(pb::PollUpdated {
                        channel_id: new_id(),
                        message_id: new_id(),
                        poll: Some(poll),
                        voter_id,
                        voter_answer_ids: (1..=20).collect(),
                    })),
                    ..Default::default()
                }],
            })),
            ..Default::default()
        };
        let read = |call| {
            let Some(Call::Events(home)) = arrived(call, origin, OWN, "abcd", &proxied).unwrap().call else { panic!() };
            let Some(Payload::PollUpdated(updated)) = home.events.into_iter().next().unwrap().payload else { panic!() };
            updated
        };
        let poll = pb::Poll {
            question: "q".repeat(1000),
            answers: (1..=12)
                .map(|id| pb::PollAnswer { id, text: "a".repeat(100), emoji: "not an emoji".into(), votes: -3 })
                .collect(),
            voters: -1,
            my_answer_ids: vec![2, 2, 40],
            ended_by_id: there.clone(),
            ..Default::default()
        };
        let updated = read(update(there.clone(), poll.clone()));
        assert_eq!(updated.voter_id, format!("{there}@night-owls.example"));
        assert_eq!(updated.voter_answer_ids.len(), 10);
        let clipped = updated.poll.unwrap();
        assert_eq!((clipped.question.chars().count(), clipped.answers.len(), clipped.voters), (300, 10, 0));
        assert!(clipped.answers.iter().all(|a| a.text.chars().count() == 55 && a.emoji.is_empty() && a.votes == 0));
        assert_eq!(clipped.my_answer_ids, [2]);
        assert_eq!(clipped.ended_by_id, format!("{there}@night-owls.example"));
        // Anonymous polls name no one: an empty voter stays empty.
        let updated = read(update(String::new(), pb::Poll::default()));
        assert!(updated.voter_id.is_empty());
        // This instance's own people read back as its own; a third's are refused.
        assert_eq!(read(update(format!("{here}@fuwa.example"), pb::Poll::default())).voter_id, here);
        let third = update(format!("{there}@third.example"), pb::Poll::default());
        assert!(arrived(third, origin, OWN, "abcd", &proxied).is_err());
    }

    #[test]
    fn calls_from_other_instances_speak_only_for_their_own() {
        let origin = "https://night-owls.example";
        let (home, guest, person, connection) = (new_id(), new_id(), new_id(), new_id());
        let ask = cpb::SharedCall {
            server_id: home.clone(),
            call: Some(Call::Ask(cpb::ShareAsk {
                code: "x".into(),
                connection_id: connection.clone(),
                guest: Some(pb::SharedServer {
                    id: guest.clone(),
                    name: "Night\nOwls".into(),
                    icon_url: "https://night-owls.example/icon.png".into(),
                    ..Default::default()
                }),
                asked_by_id: person.clone(),
            })),
            // Whatever the sender says it is, the receiver says who sent it.
            from_instance: String::new(),
            from_fingerprint: "forged".into(),
        };
        let read = arrived(ask, origin, OWN, "abcd", &proxied).unwrap();
        assert_eq!(read.from_instance, origin);
        assert_eq!(read.from_fingerprint, "abcd");
        let Some(Call::Ask(ask)) = read.call else { panic!("not an ask") };
        let server = ask.guest.unwrap();
        assert_eq!(server.id, format!("{guest}@night-owls.example"));
        assert_eq!(server.name, "NightOwls");
        assert_eq!(server.icon_url, proxied("https://night-owls.example/icon.png"), "only through this instance");
        assert_eq!(ask.asked_by_id, format!("{person}@night-owls.example"));

        // Ids naming another instance's, or this one's own as theirs, are refused.
        let left = |guest_server_id: String| cpb::SharedCall {
            server_id: home.clone(),
            call: Some(Call::Left(cpb::GuestLeft {
                connection_id: connection.clone(),
                guest_server_id,
                actor_id: String::new(),
            })),
            ..Default::default()
        };
        assert!(arrived(left(format!("{guest}@third.example")), origin, OWN, "abcd", &proxied).is_err());
        let Some(Call::Left(read)) = arrived(left(guest.clone()), origin, OWN, "abcd", &proxied).unwrap().call else {
            panic!()
        };
        assert_eq!(read.actor_id, "@night-owls.example");
        let elsewhere = cpb::SharedCall { server_id: format!("{home}@night-owls.example"), ..left(guest.clone()) };
        assert!(arrived(elsewhere, origin, OWN, "abcd", &proxied).is_err());

        // Instance admins' calls don't cross instances, and every guest
        // call says who's acting.
        for call in [
            Call::Send(cpb::GuestSend::default()),
            Call::AdminList(cpb::AdminShares {}),
            Call::AdminEnd(cpb::AdminEnd::default()),
        ] {
            let call = cpb::SharedCall { server_id: home.clone(), call: Some(call), ..Default::default() };
            assert!(arrived(call, origin, OWN, "abcd", &proxied).is_err());
        }
    }

    #[test]
    fn messages_from_other_instances_name_only_their_own() {
        let origin = "https://night-owls.example";
        let (here, there, message) = (new_id(), new_id(), new_id());
        let written = |author_id: String| pb::Message {
            id: message.clone(),
            author_id: author_id.clone(),
            content: "hi @everyone".into(),
            mentions_everyone: true,
            emojis: vec![
                pb::Emoji {
                    id: new_id(),
                    name: "owl".into(),
                    url: "https://night-owls.example/media/e".into(),
                    creator_id: new_id(),
                    ..Default::default()
                },
                // Pictures from anywhere but the instance itself aren't fetched.
                pb::Emoji {
                    id: new_id(),
                    name: "pixel".into(),
                    url: "https://tracker.example/e".into(),
                    ..Default::default()
                },
                pb::Emoji {
                    id: new_id(),
                    name: "no".into(),
                    url: "https://night-owls.example.tracker.example/e".into(),
                    ..Default::default()
                },
                pb::Emoji {
                    id: new_id(),
                    name: "pw".into(),
                    url: "https://me:pw@night-owls.example/e".into(),
                    ..Default::default()
                },
            ],
            gif: Some(pb::MessageGif {
                url: "https://night-owls.example/g.gif".into(),
                seal: "theirs".into(),
                ..Default::default()
            }),
            thread_id: new_id(),
            also_in_channel: true,
            thread: Some(pb::ThreadSummary {
                reply_count: -4,
                participant_ids: (0..9).map(|_| new_id()).collect(),
                ..Default::default()
            }),
            attachments: vec![pb::Attachment::default()],
            embeds: vec![pb::Embed {
                title: "a\nlink".into(),
                url: "javascript:alert(1)".into(),
                image_url: "https://tracker.example/pixel.png".into(),
                thumbnail_url: "https://night-owls.example/media/outside/x?url=y".into(),
                ..Default::default()
            }],
            shared: Some(pb::SharedAuthor {
                user: Some(pb::User {
                    id: author_id,
                    username: "mika".into(),
                    avatar_url: "https://night-owls.example/a.png".into(),
                    ..Default::default()
                }),
                server: None,
            }),
            ..Default::default()
        };
        let events = |m: pb::Message| cpb::SharedCall {
            server_id: new_id(),
            call: Some(Call::Events(cpb::HomeEvents {
                connection_id: new_id(),
                events: vec![pb::Event {
                    actor_id: there.clone(),
                    payload: Some(Payload::MessageCreated(pb::MessageCreated { message: Some(m) })),
                    ..Default::default()
                }],
            })),
            ..Default::default()
        };
        let read = arrived(events(written(there.clone())), origin, OWN, "abcd", &proxied).unwrap();
        let Some(Call::Events(home)) = read.call else { panic!() };
        let Some(Payload::MessageCreated(created)) = &home.events[0].payload else { panic!() };
        let m = created.message.as_ref().unwrap();
        assert_eq!(m.author_id, format!("{there}@night-owls.example"));
        assert!(!m.mentions_everyone && m.attachments.is_empty());
        assert!(!m.thread_id.is_empty() && m.also_in_channel);
        let summary = m.thread.as_ref().unwrap();
        assert_eq!(summary.reply_count, 0);
        assert_eq!(summary.participant_ids.len(), 5);
        assert!(summary.participant_ids.iter().all(|id| id.ends_with("@night-owls.example")));
        let mut odd = written(there.clone());
        odd.thread_id = "elsewhere".into();
        assert!(arrived(events(odd), origin, OWN, "abcd", &proxied).is_err(), "a thread is a message's id");
        // Pictures are that instance's own, through this one's proxy.
        let emoji = &m.emojis[..];
        assert_eq!(emoji.len(), 1, "only pictures on the instance itself");
        assert_eq!(emoji[0].url, proxied("https://night-owls.example/media/e"));
        assert!(emoji[0].creator_id.is_empty());
        let gif = m.gif.as_ref().unwrap();
        assert_eq!((gif.url.as_str(), gif.seal.as_str()), (proxied("https://night-owls.example/g.gif").as_str(), ""));
        assert_eq!((m.embeds[0].title.as_str(), m.embeds[0].url.as_str()), ("alink", ""));
        assert!(m.embeds[0].image_url.is_empty(), "apps here never fetch from anywhere else");
        assert_eq!(m.embeds[0].thumbnail_url, proxied("https://night-owls.example/media/outside/x?url=y"));
        let user = m.shared.as_ref().unwrap().user.as_ref().unwrap();
        assert_eq!((user.id.as_str(), user.username.as_str()), (m.author_id.as_str(), "mika"));
        assert_eq!(user.avatar_url, proxied("https://night-owls.example/a.png"));
        assert!(home.events[0].actor_id.is_empty());
        // This instance's own people read back as its own, and how they look
        // is this instance's to say.
        let read = arrived(events(written(format!("{here}@fuwa.example"))), origin, OWN, "abcd", &proxied).unwrap();
        let Some(Call::Events(home)) = read.call else { panic!() };
        let Some(Payload::MessageCreated(created)) = &home.events[0].payload else { panic!() };
        let m = created.message.as_ref().unwrap();
        assert_eq!(m.author_id, here);
        assert_eq!(m.shared.as_ref().unwrap().user, Some(pb::User { id: here.clone(), ..Default::default() }));
        // Who wrote it is the author, not someone else they name.
        let mut posing = written(there.clone());
        posing.shared.as_mut().unwrap().user.as_mut().unwrap().id = format!("{here}@fuwa.example");
        assert!(arrived(events(posing), origin, OWN, "abcd", &proxied).is_err());
        // A third instance's people aren't theirs to name.
        assert!(arrived(events(written(format!("{there}@third.example"))), origin, OWN, "abcd", &proxied).is_err());

        // A guest there sends text as one of its own, never with files.
        let send = |user_id: String, attachments: Vec<pb::Attachment>| cpb::SharedCall {
            server_id: new_id(),
            call: Some(Call::Send(cpb::GuestSend {
                guest: Some(cpb::Guest {
                    connection_id: new_id(),
                    user: Some(pb::User { id: user_id, username: "rin".into(), ..Default::default() }),
                    server: Some(store::shared_server(new_id(), "Owls".into(), String::new())),
                    moderator: true,
                }),
                content: "hello".into(),
                attachments,
                ..Default::default()
            })),
            ..Default::default()
        };
        let Some(Call::Send(sent)) = arrived(send(there.clone(), vec![]), origin, OWN, "abcd", &proxied).unwrap().call
        else {
            panic!()
        };
        let guest = sent.guest.unwrap();
        assert_eq!(guest.user.unwrap().id, format!("{there}@night-owls.example"));
        assert_eq!(guest.server.unwrap().instance, "night-owls.example");
        assert!(arrived(send(format!("{here}@fuwa.example"), vec![]), origin, OWN, "abcd", &proxied).is_err());
        assert!(arrived(send(there.clone(), vec![pb::Attachment::default()]), origin, OWN, "abcd", &proxied).is_err());
    }

    #[test]
    fn people_leave_for_other_instances_as_just_who_they_are() {
        let user = pb::User {
            id: new_id(),
            username: "juan".into(),
            display_name: "Juan".into(),
            avatar_url: "https://fuwa.example/media/a.png".into(),
            status: "out for lunch".into(),
            ..Default::default()
        };
        let event = pb::Event {
            payload: Some(Payload::MessageCreated(pb::MessageCreated {
                message: Some(pb::Message {
                    emojis: vec![
                        pb::Emoji {
                            id: new_id(),
                            name: "sakura".into(),
                            url: "https://fuwa.example/media/e".into(),
                            creator_id: new_id(),
                            ..Default::default()
                        },
                        pb::Emoji {
                            id: new_id(),
                            name: "far".into(),
                            url: "https://far.example/e".into(),
                            ..Default::default()
                        },
                    ],
                    gif: Some(pb::MessageGif {
                        url: "https://fuwa.example/media/g.gif".into(),
                        seal: "ours".into(),
                        ..Default::default()
                    }),
                    attachments: vec![pb::Attachment {
                        url: "https://fuwa.example/media/f.png".into(),
                        ..Default::default()
                    }],
                    shared: Some(pb::SharedAuthor {
                        user: Some(user.clone()),
                        server: Some(store::shared_server(new_id(), "Home".into(), "https://far.example/i.png".into())),
                    }),
                    ..Default::default()
                }),
            })),
            ..Default::default()
        };
        let Some(Payload::MessageCreated(created)) = leaving(&event, OWN).payload else { panic!() };
        let message = created.message.unwrap();
        // Only this instance's own pictures, which the other fetches itself.
        let emoji = &message.emojis[..];
        assert_eq!((emoji.len(), emoji[0].url.as_str()), (1, "https://fuwa.example/media/e"));
        assert!(emoji[0].creator_id.is_empty());
        let gif = message.gif.unwrap();
        assert_eq!((gif.url.as_str(), gif.seal.as_str()), ("https://fuwa.example/media/g.gif", ""));
        assert!(message.attachments.is_empty(), "files stay here");
        let shared = message.shared.unwrap();
        assert_eq!(
            shared.user.unwrap(),
            pb::User {
                id: user.id,
                username: "juan".into(),
                display_name: "Juan".into(),
                avatar_url: user.avatar_url,
                ..Default::default()
            }
        );
        assert!(shared.server.unwrap().icon_url.is_empty(), "never a link to anywhere else");
    }

    #[test]
    fn answers_from_other_instances_keep_to_what_was_asked() {
        let origin = "https://night-owls.example";
        let home = new_id();
        let lookup = cpb::SharedCall { call: Some(Call::Lookup(cpb::ShareLookup::default())), ..Default::default() };
        let reply = cpb::SharedReply {
            preview: Some(pb::PreviewShareResponse {
                home_server: Some(store::shared_server(home.clone(), "Owls".into(), "x".into())),
                region: "eu".into(),
                ..Default::default()
            }),
            author: Some(pb::User::default()),
            ..Default::default()
        };
        let read = returned(&lookup, reply.clone(), origin, OWN, "abcd", &proxied).unwrap();
        let preview = read.preview.unwrap();
        assert_eq!(preview.home_server.unwrap().id, format!("{home}@night-owls.example"));
        assert_eq!((preview.instance.as_str(), preview.fingerprint.as_str()), ("night-owls.example", "abcd"));
        assert!(preview.region.is_empty());
        assert!(read.author.is_none());
        // Listed people are read like everyone else another instance names.
        let list = cpb::SharedCall { call: Some(Call::List(cpb::GuestList::default())), ..Default::default() };
        let here = new_id();
        let page = cpb::SharedReply {
            page: Some(pb::ListMessagesResponse {
                messages: vec![pb::Message { id: new_id(), author_id: here.clone(), ..Default::default() }],
                authors: vec![
                    pb::User {
                        id: format!("{here}@fuwa.example"),
                        username: "not-me".into(),
                        status: "hacked".into(),
                        ..Default::default()
                    },
                    pb::User { id: new_id(), ..Default::default() },
                ],
                has_more: false,
                parent: Some(pb::Message { id: new_id(), content: "not asked for".into(), ..Default::default() }),
            }),
            ..Default::default()
        };
        let page = returned(&list, page, origin, OWN, "abcd", &proxied).unwrap().page.unwrap();
        assert!(page.parent.is_none(), "only a thread's page has a parent");
        let authors = page.authors;
        assert_eq!(authors, vec![pb::User { id: here, ..Default::default() }], "no more people than messages");
        // An answer to another call keeps none of it.
        let left = cpb::SharedCall { call: Some(Call::Left(cpb::GuestLeft::default())), ..Default::default() };
        assert_eq!(returned(&left, reply, origin, OWN, "abcd", &proxied).unwrap(), cpb::SharedReply::default());
    }

    #[test]
    fn guests_get_no_more_than_shareable() {
        assert_eq!(allowed_from(&[Permission::SendMessages as i32]).unwrap(), bit(Permission::SendMessages));
        assert_eq!(allowed_from(&[Permission::ViewChannels as i32]).unwrap(), 0);
        assert!(allowed_from(&[Permission::ManageMessages as i32]).is_err());
        assert!(allowed_from(&[Permission::MentionEveryone as i32]).is_err());
    }
}
