//! Live tiles (docs/live-tiles.md): small cards at the top of a server's
//! channel list for things happening there right now (people talking in a
//! voice room, a poll about to close, a thread you follow moving, a busy
//! shared channel, an app's scoreboard). A port of the web app's
//! `lib/live-tiles.ts` and `lib/live-tiles-store.ts`.
//!
//! Which kinds show is the server's setting (`Server.live_tiles`); each
//! person can still turn them off here (`Prefs::live_tiles_*`). Every tile
//! but an app's is built from what this app already holds and already shows
//! somewhere else, so a tile never tells anyone more than the sidebar or the
//! channel would. App tiles come from the server (`LiveTileService`), only
//! for channels you can see.

use std::collections::{HashMap, HashSet};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::store::{ChannelMessages, InstanceState};
use crate::pb;
use crate::rpc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileKind {
    Voice,
    Poll,
    Thread,
    Shared,
    App,
}

/// Every kind, in the order the server menu lists them.
pub const TILE_KINDS: [TileKind; 5] =
    [TileKind::Voice, TileKind::Poll, TileKind::Thread, TileKind::Shared, TileKind::App];

impl TileKind {
    /// Its `LiveTileKind` number.
    pub fn number(self) -> i32 {
        match self {
            TileKind::Voice => 1,
            TileKind::Poll => 2,
            TileKind::Thread => 3,
            TileKind::Shared => 4,
            TileKind::App => 5,
        }
    }

    pub fn from_number(n: i32) -> Option<Self> {
        TILE_KINDS.into_iter().find(|k| k.number() == n)
    }

    /// Its name in the translations (`tiles.kind.<name>`).
    pub fn name(self) -> &'static str {
        match self {
            TileKind::Voice => "voice",
            TileKind::Poll => "poll",
            TileKind::Thread => "thread",
            TileKind::Shared => "shared",
            TileKind::App => "app",
        }
    }
}

/// A tile an app (an agent or a webhook) keeps up to date in a server, such
/// as a match's scoreboard. Apps fill a fixed template (the server holds them
/// to its limits, and `fit_app` again here) and never send markup or links;
/// the app's name always shows on it.
#[derive(Debug, Clone, PartialEq)]
pub struct AppTile {
    /// The app's name, shown with its badge so nobody mistakes the tile for fuwa's own or a person's.
    pub app: String,
    pub avatar_url: String,
    pub webhook: bool,
    /// The agent's or webhook's id and the app's own id for the tile, which a moderator's removal names.
    pub source_id: String,
    pub tile_id: String,
    pub title: String,
    /// A short state, such as "67'" or "Half time".
    pub status: String,
    /// Going on right now: the tile gets the live dot.
    pub live: bool,
    /// Up to four label and value pairs.
    pub rows: Vec<(String, String)>,
    /// How far along, 0 to 1, for the bar along the bottom.
    pub progress: Option<f32>,
    /// The button's words, empty for "Open"; it always opens the tile's channel.
    pub action: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Voice { channel_name: String, user_ids: Vec<String>, since: i64, video: bool, screen: bool },
    Poll { channel_name: String, message_id: String, question: String, voters: i64, ends_at: Option<i64> },
    Thread { thread_id: String, title: String, replies: i32, unread: u32, user_ids: Vec<String> },
    Shared { channel_name: String, unread: u32, servers: usize },
    App(AppTile),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    /// Stable while the activity lasts, and new when it starts again: hiding a tile hides this one only.
    pub id: String,
    pub channel_id: String,
    pub body: Body,
}

impl Tile {
    pub fn kind(&self) -> TileKind {
        match self.body {
            Body::Voice { .. } => TileKind::Voice,
            Body::Poll { .. } => TileKind::Poll,
            Body::Thread { .. } => TileKind::Thread,
            Body::Shared { .. } => TileKind::Shared,
            Body::App(_) => TileKind::App,
        }
    }
}

/// The template's limits: what an app sends is cut to these before anyone sees it.
pub const LIMIT_TITLE: usize = 40;
pub const LIMIT_STATUS: usize = 16;
pub const LIMIT_ROWS: usize = 4;
pub const LIMIT_LABEL: usize = 24;
pub const LIMIT_VALUE: usize = 8;
pub const LIMIT_ACTION: usize = 12;

/// Control, zero-width and direction-override characters: an app could use
/// them to make its name or a row read as something else.
fn hidden_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// On one line, without hidden characters, cut to `max` characters with an ellipsis.
fn cut(text: &str, max: usize) -> String {
    let flat: String =
        text.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !hidden_char(*c)).collect();
    let flat = flat.trim();
    if flat.chars().count() > max {
        let mut out: String = flat.chars().take(max - 1).collect();
        out.push('…');
        out
    } else {
        flat.to_owned()
    }
}

/// An app's tile as it may show: text on one line, without hidden
/// characters, cut to the limits, at most four rows, progress kept between 0 and 1.
pub fn fit_app(tile: AppTile) -> AppTile {
    AppTile {
        app: cut(&tile.app, LIMIT_TITLE),
        title: cut(&tile.title, LIMIT_TITLE),
        status: cut(&tile.status, LIMIT_STATUS),
        rows: tile.rows.iter().take(LIMIT_ROWS).map(|(l, v)| (cut(l, LIMIT_LABEL), cut(v, LIMIT_VALUE))).collect(),
        progress: tile.progress.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0)),
        action: cut(&tile.action, LIMIT_ACTION),
        ..tile
    }
}

/// At most this many tiles show in a server; the rest wait their turn.
pub const MAX_TILES: usize = 3;
/// A voice room is a tile from this many people: one person alone isn't a gathering to point at.
pub const VOICE_MIN: usize = 2;
/// A poll is a tile while it closes within this long (or runs with no end and is new).
pub const POLL_SOON_MS: i64 = 6 * 60 * 60_000;
/// A shared channel is busy from this many unread messages.
pub const SHARED_MIN: u32 = 10;
/// From this many members voice room tiles start off, unless the server's managers chose.
pub const BIG_SERVER: i64 = 500;

/// What tiles are made from: what this app already knows about one server.
pub struct Sources<'a> {
    /// The channels the sidebar lists (the server already limited them to what you can see).
    pub channels: &'a [pb::Channel],
    pub voice: &'a [pb::VoiceState],
    /// Loaded messages, per channel.
    pub messages: &'a HashMap<String, ChannelMessages>,
    pub thread_parents: &'a HashMap<String, pb::Message>,
    pub followed: Option<&'a HashSet<String>>,
    pub thread_unread: &'a HashMap<String, u32>,
    pub unread: &'a HashMap<String, u32>,
    /// The voice channel you're in, if any: no tile asks you to join where you are.
    pub in_channel: Option<&'a str>,
    /// Channels whose notifications you muted: they don't get tiles either.
    pub muted: &'a dyn Fn(&str) -> bool,
    /// Apps' tiles, as the server listed and sent them.
    pub apps: &'a [pb::LiveTile],
}

fn ms(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map_or(0, |t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

/// The first line with something on it, cut to `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if line.chars().count() > max {
        let mut out: String = line.chars().take(max - 1).collect();
        out.push('…');
        out
    } else {
        line.to_owned()
    }
}

/// The tiles a server has now, from what this app already knows. Only
/// channels in `channels` count, and secure channels never do: their text is
/// end-to-end encrypted and stays inside the channel.
pub fn collect_tiles(s: &Sources, now: i64) -> Vec<Tile> {
    let mut tiles = Vec::new();
    let by_id: HashMap<&str, &pb::Channel> = s.channels.iter().map(|c| (c.id.as_str(), c)).collect();
    let shown = |id: &str| by_id.get(id).is_some_and(|c| c.r#type != pb::ChannelType::Secure as i32) && !(s.muted)(id);
    let name = |id: &str| by_id.get(id).map(|c| c.name.clone()).unwrap_or_default();

    // Voice rooms with people in them, in the order they started.
    let mut rooms: Vec<(&str, Vec<&pb::VoiceState>)> = Vec::new();
    for v in s.voice.iter().filter(|v| !v.channel_id.is_empty()) {
        match rooms.iter_mut().find(|(c, _)| *c == v.channel_id) {
            Some((_, list)) => list.push(v),
            None => rooms.push((&v.channel_id, vec![v])),
        }
    }
    for (channel_id, states) in rooms {
        if states.len() < VOICE_MIN || s.in_channel == Some(channel_id) || !shown(channel_id) {
            continue;
        }
        let since = states.iter().map(|v| ms(v.joined_at.as_ref())).min().unwrap_or(0);
        tiles.push(Tile {
            id: format!("voice:{channel_id}:{since}"),
            channel_id: channel_id.to_owned(),
            body: Body::Voice {
                channel_name: name(channel_id),
                user_ids: states.iter().map(|v| v.user_id.clone()).collect(),
                since,
                video: states.iter().any(|v| v.self_video),
                screen: states.iter().any(|v| v.self_stream),
            },
        });
    }

    // Open polls you haven't answered, in channels this app loaded.
    for c in s.channels {
        let Some(loaded) = s.messages.get(&c.id) else { continue };
        if !shown(&c.id) {
            continue;
        }
        for m in &loaded.items {
            let Some(poll) = &m.poll else { continue };
            if poll.ended_at.is_some() || !poll.my_answer_ids.is_empty() {
                continue;
            }
            let ends_at = poll.ends_at.as_ref().map(|t| ms(Some(t)));
            match ends_at {
                Some(end) if end <= now || end - now > POLL_SOON_MS => continue,
                None if now - ms(m.created_at.as_ref()) > POLL_SOON_MS => continue,
                _ => {}
            }
            tiles.push(Tile {
                id: format!("poll:{}", m.id),
                channel_id: c.id.clone(),
                body: Body::Poll {
                    channel_name: c.name.clone(),
                    message_id: m.id.clone(),
                    question: first_line(&poll.question, 80),
                    voters: poll.voters,
                    ends_at,
                },
            });
        }
    }

    // Threads you follow with replies you haven't read.
    let mut threads: Vec<(&String, &u32)> = s.thread_unread.iter().collect();
    threads.sort();
    for (thread_id, &unread) in threads {
        let parent = s.thread_parents.get(thread_id).or_else(|| {
            s.channels
                .iter()
                .filter_map(|c| s.messages.get(&c.id))
                .find_map(|l| l.items.iter().find(|m| m.id == *thread_id))
        });
        let Some(parent) = parent else { continue };
        if unread == 0 || !s.followed.is_some_and(|f| f.contains(thread_id)) || !shown(&parent.channel_id) {
            continue;
        }
        let title = first_line(&parent.content, 80);
        tiles.push(Tile {
            id: format!("thread:{thread_id}"),
            channel_id: parent.channel_id.clone(),
            body: Body::Thread {
                thread_id: thread_id.clone(),
                title: if title.is_empty() { name(&parent.channel_id) } else { title },
                replies: parent.thread.as_ref().map_or(0, |t| t.reply_count),
                unread,
                user_ids: parent.thread.as_ref().map(|t| t.participant_ids.clone()).unwrap_or_default(),
            },
        });
    }

    // Shared channels with a lot going on since you last looked.
    for c in s.channels {
        let unread = s.unread.get(&c.id).copied().unwrap_or(0);
        let Some(shared) = &c.shared else { continue };
        if unread < SHARED_MIN || !shown(&c.id) {
            continue;
        }
        tiles.push(Tile {
            id: format!("shared:{}", c.id),
            channel_id: c.id.clone(),
            body: Body::Shared { channel_name: c.name.clone(), unread, servers: 1 + shared.guests.len() },
        });
    }

    // Apps' tiles that haven't run out. Hiding one hides it for as long as its app keeps it.
    for t in s.apps {
        let expires_at = ms(t.expires_at.as_ref());
        if expires_at <= now || !shown(&t.channel_id) {
            continue;
        }
        let c = t.content.clone().unwrap_or_default();
        tiles.push(Tile {
            id: format!("app:{}:{}:{}", t.source_id, t.channel_id, t.id),
            channel_id: t.channel_id.clone(),
            body: Body::App(fit_app(AppTile {
                app: t.source_name.clone(),
                avatar_url: t.source_avatar_url.clone(),
                webhook: t.source_kind == pb::LiveTileSource::Webhook as i32,
                source_id: t.source_id.clone(),
                tile_id: t.id.clone(),
                title: c.title,
                status: c.status,
                live: c.live,
                rows: c.rows.into_iter().map(|r| (r.label, r.value)).collect(),
                progress: c.progress,
                action: c.action,
                expires_at,
            })),
        });
    }
    tiles
}

/// How much a tile matters now, higher first. A poll closing rises as the
/// moment comes; rooms rise with the people in them; the rest sit below.
pub fn urgency(tile: &Tile, now: i64) -> f64 {
    match &tile.body {
        Body::Poll { ends_at: None, .. } => 30.0,
        Body::Poll { ends_at: Some(end), .. } => {
            50.0 + 25.0 * (1.0 - (end - now).min(POLL_SOON_MS) as f64 / POLL_SOON_MS as f64)
        }
        Body::Voice { user_ids, .. } => 40.0 + user_ids.len().min(20) as f64,
        Body::Thread { unread, .. } => 25.0 + (*unread).min(10) as f64,
        Body::Shared { unread, .. } => 20.0 + (*unread as f64 / 10.0).min(10.0),
        // An app can't buy its way to the top: a live tile sits with busy rooms, a quiet one below.
        Body::App(app) => {
            if app.live {
                55.0
            } else {
                35.0
            }
        }
    }
}

/// The kinds a server shows, from its setting (LiveTileKind numbers).
pub fn server_kinds(kinds: &[i32]) -> HashSet<TileKind> {
    kinds.iter().filter_map(|n| TileKind::from_number(*n)).collect()
}

/// The kinds a server shows now. A server that never chose follows its
/// size, which changes without a ServerUpdated, so the app works the default
/// out the same way the server does.
pub fn shown_kinds(setting: Option<&pb::LiveTileSettings>, members: i64) -> HashSet<TileKind> {
    match setting {
        None => HashSet::new(),
        Some(s) if s.customized => server_kinds(&s.kinds),
        Some(_) => TILE_KINDS.into_iter().filter(|k| *k != TileKind::Voice || members < BIG_SERVER).collect(),
    }
}

/// A server's kinds as its setting takes them back: one number each, in order.
pub fn kind_numbers(kinds: &HashSet<TileKind>) -> Vec<i32> {
    TILE_KINDS.into_iter().filter(|k| kinds.contains(k)).map(TileKind::number).collect()
}

/// Whether someone may take a tile down for everyone: an app's, in a channel where they manage messages.
pub fn removable(tile: &Tile, manages_messages: impl Fn(&str) -> bool) -> bool {
    matches!(tile.body, Body::App(_)) && manages_messages(&tile.channel_id)
}

/// The tiles to show: hidden ones and kinds the server turned off out, the
/// most urgent first, at most `max`. Ties keep their order.
pub fn pick_tiles(tiles: Vec<Tile>, hidden: &[String], now: i64, kinds: &HashSet<TileKind>, max: usize) -> Vec<Tile> {
    let mut kept: Vec<(f64, usize, Tile)> = tiles
        .into_iter()
        .filter(|t| kinds.contains(&t.kind()) && !hidden.contains(&t.id))
        .enumerate()
        .map(|(n, t)| (urgency(&t, now), n, t))
        .collect();
    kept.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    kept.into_iter().take(max).map(|(_, _, t)| t).collect()
}

/// The tiles a server shows you now: nothing while you're on Do not disturb
/// or have the server muted, the kinds it shows, without the ones you hid.
pub fn tiles_for(
    i: &InstanceState,
    server_id: &str,
    in_channel: Option<&str>,
    hidden: &[String],
    now: i64,
) -> Vec<Tile> {
    let Some(server) = i.server(server_id) else { return Vec::new() };
    let kinds = shown_kinds(server.live_tiles.as_ref(), server.member_count);
    if kinds.is_empty()
        || i.status() == pb::PresenceStatus::DoNotDisturb
        || crate::core::notifications::is_muted(i.notification_settings(server_id, ""), now)
    {
        return Vec::new();
    }
    let Some(channels) = i.channels.get(server_id) else { return Vec::new() };
    let muted = |id: &str| crate::core::notifications::is_muted(i.notification_settings(server_id, id), now);
    let sources = Sources {
        channels,
        voice: i.voice.get(server_id).map(Vec::as_slice).unwrap_or_default(),
        messages: &i.messages,
        thread_parents: &i.thread_parents,
        followed: i.followed.get(server_id),
        thread_unread: &i.thread_unread,
        unread: &i.unread,
        in_channel,
        muted: &muted,
        apps: i.live_tiles.get(server_id).map(Vec::as_slice).unwrap_or_default(),
    };
    pick_tiles(collect_tiles(&sources, now), hidden, now, &kinds, MAX_TILES)
}

// ───────────────────────── The store's side ─────────────────────────

/// An app set or changed a tile: one per app, channel and tile id.
pub fn with_live_tile(i: &mut InstanceState, server_id: &str, tile: pb::LiveTile) {
    let list = i.live_tiles.entry(server_id.to_owned()).or_default();
    match list.iter_mut().find(|t| t.id == tile.id && t.channel_id == tile.channel_id && t.source_id == tile.source_id)
    {
        Some(t) => *t = tile,
        None => list.push(tile),
    }
}

/// A tile ended.
pub fn without_live_tile(i: &mut InstanceState, server_id: &str, channel_id: &str, source_id: &str, tile_id: &str) {
    if let Some(list) = i.live_tiles.get_mut(server_id) {
        list.retain(|t| !(t.channel_id == channel_id && t.id == tile_id && t.source_id == source_id));
    }
}

// ───────────────────────── Each person's switches ─────────────────────────

/// The most hidden tiles kept; the newest stay.
const MAX_HIDDEN: usize = 200;

/// Hides one tile on this computer.
pub fn hide(hidden: &mut Vec<String>, id: &str) {
    hidden.retain(|h| h != id);
    hidden.push(id.to_owned());
    if hidden.len() > MAX_HIDDEN {
        let extra = hidden.len() - MAX_HIDDEN;
        hidden.drain(..extra);
    }
}

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

impl Core {
    /// Takes an app's live tile down for everyone (Manage Messages in its channel).
    pub async fn end_live_tile(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        source_id: &str,
        tile_id: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.live_tiles(),
            end_live_tile(pb::EndLiveTileRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                tile_id: tile_id.into(),
                source_id: source_id.into(),
            })
        )
        .await?;
        self.shared.instance(key, |i| without_live_tile(i, server_id, channel_id, source_id, tile_id));
        Ok(())
    }

    /// Which kinds of live tiles everyone in a server sees; `None` goes back to the default.
    pub async fn set_server_live_tiles(
        &self,
        key: &str,
        server_id: &str,
        kinds: Option<Vec<i32>>,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let setting = pb::LiveTileSettings { customized: kinds.is_some(), kinds: kinds.unwrap_or_default() };
        let res = rpc!(
            api.servers(),
            update_server(pb::UpdateServerRequest {
                server_id: server_id.into(),
                live_tiles: Some(setting),
                ..Default::default()
            })
        )
        .await?;
        if let Some(server) = res.server {
            self.shared.instance(key, |i| crate::core::store::add_server(i, server));
        }
        Ok(())
    }

    /// Apps' tiles in a server, listed again (after the stream was away).
    pub(crate) async fn list_live_tiles(&self, key: &str, server_id: &str) -> Vec<pb::LiveTile> {
        let Some(api) = self.api(key) else { return Vec::new() };
        if !self.shared.read(|s| s.instance(key).is_some_and(|i| i.has("live-tiles"))) {
            return Vec::new();
        }
        rpc!(api.live_tiles(), list_live_tiles(pb::ListLiveTilesRequest { server_id: server_id.into() }))
            .await
            .map(|r| r.tiles)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn ts(ms: i64) -> Option<prost_types::Timestamp> {
        Some(prost_types::Timestamp { seconds: ms.div_euclid(1000), nanos: (ms.rem_euclid(1000) * 1_000_000) as i32 })
    }

    fn channel(id: &str, kind: pb::ChannelType) -> pb::Channel {
        pb::Channel { id: id.into(), name: id.into(), r#type: kind as i32, ..Default::default() }
    }

    fn voice(user: &str, channel: &str, joined: i64) -> pb::VoiceState {
        pb::VoiceState { user_id: user.into(), channel_id: channel.into(), joined_at: ts(joined), ..Default::default() }
    }

    fn poll(id: &str, f: impl FnOnce(&mut pb::Poll)) -> pb::Message {
        let mut p = pb::Poll { question: "Pizza?".into(), voters: 3, ..Default::default() };
        f(&mut p);
        pb::Message {
            id: id.into(),
            channel_id: "general".into(),
            created_at: ts(NOW - 60_000),
            poll: Some(p),
            ..Default::default()
        }
    }

    fn app(id: &str, channel: &str, expires: i64) -> pb::LiveTile {
        pb::LiveTile {
            id: id.into(),
            channel_id: channel.into(),
            source_id: "bot".into(),
            source_kind: pb::LiveTileSource::Agent as i32,
            source_name: "Scorebot".into(),
            expires_at: ts(expires),
            content: Some(pb::LiveTileContent {
                title: "Cup final".into(),
                status: "67'".into(),
                live: true,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// What `Sources` borrows, owned.
    struct Held {
        channels: Vec<pb::Channel>,
        voice: Vec<pb::VoiceState>,
        messages: HashMap<String, ChannelMessages>,
        thread_parents: HashMap<String, pb::Message>,
        followed: HashSet<String>,
        thread_unread: HashMap<String, u32>,
        unread: HashMap<String, u32>,
        in_channel: Option<String>,
        muted: Vec<String>,
        apps: Vec<pb::LiveTile>,
    }

    impl Held {
        fn new() -> Self {
            Self {
                channels: vec![
                    channel("general", pb::ChannelType::Text),
                    channel("lounge", pb::ChannelType::Voice),
                    channel("vault", pb::ChannelType::Secure),
                ],
                voice: vec![],
                messages: HashMap::new(),
                thread_parents: HashMap::new(),
                followed: HashSet::new(),
                thread_unread: HashMap::new(),
                unread: HashMap::new(),
                in_channel: None,
                muted: vec![],
                apps: vec![],
            }
        }

        fn tiles(&self, now: i64) -> Vec<Tile> {
            let muted = |id: &str| self.muted.iter().any(|m| m == id);
            collect_tiles(
                &Sources {
                    channels: &self.channels,
                    voice: &self.voice,
                    messages: &self.messages,
                    thread_parents: &self.thread_parents,
                    followed: Some(&self.followed),
                    thread_unread: &self.thread_unread,
                    unread: &self.unread,
                    in_channel: self.in_channel.as_deref(),
                    muted: &muted,
                    apps: &self.apps,
                },
                now,
            )
        }
    }

    fn kinds(tiles: &[Tile]) -> Vec<String> {
        tiles.iter().map(|t| format!("{}:{}", t.kind().name(), t.channel_id)).collect()
    }

    fn ids(tiles: &[Tile]) -> Vec<&str> {
        tiles.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn a_voice_room_is_a_tile_once_two_people_are_in_it_and_never_the_room_youre_in() {
        let mut h = Held::new();
        h.voice = vec![voice("a", "lounge", NOW - 60_000)];
        assert!(h.tiles(NOW).is_empty());
        h.voice = vec![voice("a", "lounge", NOW - 90_000), voice("b", "lounge", NOW - 30_000)];
        let tiles = h.tiles(NOW);
        assert_eq!(kinds(&tiles), ["voice:lounge"]);
        assert!(matches!(tiles[0].body, Body::Voice { since, .. } if since == NOW - 90_000));
        h.in_channel = Some("lounge".into());
        assert!(h.tiles(NOW).is_empty());
    }

    #[test]
    fn no_tile_tells_of_a_channel_the_sidebar_doesnt_list_a_secure_one_or_a_muted_one() {
        let mut h = Held::new();
        h.voice = vec![voice("a", "staff", NOW), voice("b", "staff", NOW)];
        assert!(h.tiles(NOW).is_empty());
        h.voice = vec![voice("a", "vault", NOW), voice("b", "vault", NOW)];
        assert!(h.tiles(NOW).is_empty());
        h.voice = vec![voice("a", "lounge", NOW), voice("b", "lounge", NOW)];
        h.muted = vec!["lounge".into()];
        assert!(h.tiles(NOW).is_empty());
        h.voice.clear();
        let parent = pb::Message {
            channel_id: "vault".into(),
            content: "secret plans".into(),
            thread: Some(pb::ThreadSummary { reply_count: 4, ..Default::default() }),
            ..Default::default()
        };
        h.thread_parents.insert("t1".into(), parent);
        h.followed.insert("t1".into());
        h.thread_unread.insert("t1".into(), 2);
        assert!(h.tiles(NOW).is_empty());
        let mut h = Held::new();
        h.apps =
            vec![app("a", "staff", NOW + 60_000), app("b", "vault", NOW + 60_000), app("c", "lounge", NOW + 60_000)];
        h.muted = vec!["lounge".into()];
        assert!(h.tiles(NOW).is_empty());
    }

    #[test]
    fn an_apps_tile_shows_until_it_runs_out_under_the_apps_name() {
        let mut h = Held::new();
        h.apps = vec![app("final", "general", NOW + 60_000), app("old", "general", NOW)];
        let tiles = h.tiles(NOW);
        assert_eq!(ids(&tiles), ["app:bot:general:final"]);
        assert!(matches!(&tiles[0].body, Body::App(a) if a.app == "Scorebot"));
        assert!(h.tiles(NOW + 60_000).is_empty());
    }

    #[test]
    fn a_poll_is_a_tile_while_its_open_closing_soon_and_you_havent_voted() {
        let mut h = Held::new();
        let items = vec![
            poll("soon", |p| p.ends_at = ts(NOW + 30 * 60_000)),
            poll("voted", |p| {
                p.ends_at = ts(NOW + 30 * 60_000);
                p.my_answer_ids = vec![1];
            }),
            poll("ended", |p| p.ended_at = ts(NOW - 1000)),
            poll("past", |p| p.ends_at = ts(NOW - 1000)),
            poll("far", |p| p.ends_at = ts(NOW + 7 * 24 * 3_600_000)),
        ];
        h.messages.insert("general".into(), ChannelMessages { items, ..Default::default() });
        assert_eq!(ids(&h.tiles(NOW)), ["poll:soon"]);
    }

    #[test]
    fn a_thread_is_a_tile_only_when_you_follow_it_and_it_has_replies_you_havent_read() {
        let mut h = Held::new();
        let parent = pb::Message {
            channel_id: "general".into(),
            content: "\n  Release notes for v2\nmore".into(),
            thread: Some(pb::ThreadSummary { reply_count: 9, participant_ids: vec!["a".into()], ..Default::default() }),
            ..Default::default()
        };
        h.thread_parents.insert("t1".into(), parent);
        h.thread_unread.insert("t1".into(), 3);
        assert!(h.tiles(NOW).is_empty());
        h.followed.insert("t1".into());
        let tiles = h.tiles(NOW);
        assert!(matches!(&tiles[0].body, Body::Thread { title, .. } if title == "Release notes for v2"));
    }

    #[test]
    fn a_shared_channel_is_a_tile_once_enough_has_happened_in_it() {
        let mut h = Held::new();
        let mut shared = channel("bridge", pb::ChannelType::Text);
        shared.shared =
            Some(pb::SharedChannel { guests: vec![Default::default(), Default::default()], ..Default::default() });
        h.channels = vec![shared];
        h.unread.insert("bridge".into(), 9);
        assert!(h.tiles(NOW).is_empty());
        h.unread.insert("bridge".into(), 10);
        assert!(matches!(h.tiles(NOW)[0].body, Body::Shared { servers: 3, .. }));
    }

    fn scoreboard(id: &str, live: bool) -> Tile {
        Tile {
            id: id.into(),
            channel_id: "c".into(),
            body: Body::App(AppTile {
                app: "Scorebot".into(),
                avatar_url: String::new(),
                webhook: false,
                source_id: "bot".into(),
                tile_id: "s".into(),
                title: "Cup final".into(),
                status: "67'".into(),
                live,
                rows: vec![],
                progress: None,
                action: String::new(),
                expires_at: NOW + 60_000,
            }),
        }
    }

    fn voice_tile() -> Tile {
        Tile {
            id: "voice".into(),
            channel_id: "c".into(),
            body: Body::Voice {
                channel_name: String::new(),
                user_ids: vec!["a".into(), "b".into()],
                since: 0,
                video: false,
                screen: false,
            },
        }
    }

    fn thread_tile() -> Tile {
        Tile {
            id: "thread".into(),
            channel_id: "c".into(),
            body: Body::Thread { thread_id: "t".into(), title: String::new(), replies: 1, unread: 1, user_ids: vec![] },
        }
    }

    #[test]
    fn the_most_urgent_tiles_show_up_to_the_cap_without_the_hidden_ones() {
        let tiles = vec![
            thread_tile(),
            voice_tile(),
            Tile {
                id: "poll".into(),
                channel_id: "c".into(),
                body: Body::Poll {
                    channel_name: String::new(),
                    message_id: "m".into(),
                    question: String::new(),
                    voters: 0,
                    ends_at: Some(NOW + 10 * 60_000),
                },
            },
            scoreboard("quiet", false),
            scoreboard("live", true),
        ];
        let all: HashSet<TileKind> = TILE_KINDS.into_iter().collect();
        // A closing poll first, then a live app tile beside busy rooms, a quiet one below.
        assert_eq!(ids(&pick_tiles(tiles.clone(), &[], NOW, &all, MAX_TILES)), ["poll", "live", "voice"]);
        let hidden = ["poll".to_owned(), "live".to_owned()];
        assert_eq!(ids(&pick_tiles(tiles, &hidden, NOW, &all, MAX_TILES)), ["voice", "quiet", "thread"]);
    }

    #[test]
    fn a_followed_threads_parent_is_found_in_a_loaded_channel_when_its_thread_was_never_opened() {
        let mut h = Held::new();
        let parent = pb::Message {
            id: "t1".into(),
            channel_id: "general".into(),
            content: "Plans".into(),
            thread: Some(pb::ThreadSummary { reply_count: 2, ..Default::default() }),
            ..Default::default()
        };
        h.messages.insert("general".into(), ChannelMessages { items: vec![parent], ..Default::default() });
        h.followed.insert("t1".into());
        h.thread_unread.insert("t1".into(), 2);
        assert_eq!(h.tiles(NOW)[0].channel_id, "general");
    }

    #[test]
    fn a_servers_kinds_come_from_its_setting_and_only_those_show() {
        let kinds = server_kinds(&[3, 5, 99]);
        assert_eq!(kinds, HashSet::from([TileKind::Thread, TileKind::App]));
        assert!(server_kinds(&[]).is_empty());
        assert_eq!(kind_numbers(&HashSet::from([TileKind::App, TileKind::Voice])), [1, 5]);
        let tiles = vec![voice_tile(), thread_tile()];
        assert_eq!(ids(&pick_tiles(tiles, &[], NOW, &server_kinds(&[2, 3, 4, 5]), MAX_TILES)), ["thread"]);
    }

    #[test]
    fn only_an_apps_tile_and_only_where_you_manage_messages_can_be_removed_for_everyone() {
        let manages_here = |c: &str| c == "c";
        assert!(removable(&scoreboard("s", true), manages_here));
        let elsewhere = Tile { channel_id: "elsewhere".into(), ..scoreboard("s", true) };
        assert!(!removable(&elsewhere, manages_here));
        assert!(!removable(&voice_tile(), manages_here), "fuwa's own tiles aren't anyone's to remove");
    }

    #[test]
    fn a_server_that_never_chose_drops_voice_rooms_once_it_grows_past_the_big_server_line() {
        let unchosen = pb::LiveTileSettings { customized: false, kinds: vec![1, 2, 3, 4, 5] };
        assert!(shown_kinds(Some(&unchosen), 499).contains(&TileKind::Voice));
        assert_eq!(
            shown_kinds(Some(&unchosen), 500),
            HashSet::from([TileKind::Poll, TileKind::Thread, TileKind::Shared, TileKind::App])
        );
        let chosen = pb::LiveTileSettings { customized: true, kinds: vec![1] };
        assert!(shown_kinds(Some(&chosen), 5000).contains(&TileKind::Voice), "a chosen setting stays as chosen");
        assert!(shown_kinds(None, 3).is_empty(), "an instance without tiles shows none");
    }

    #[test]
    fn an_apps_tile_is_cut_to_the_template_before_anyone_sees_it() {
        let Body::App(base) = scoreboard("s", true).body else { unreachable!() };
        let fitted = fit_app(AppTile {
            title: "A very\nlong title that keeps going well past forty characters".into(),
            rows: (1..=5).map(|n| (format!("Team {n}"), "123456789".to_owned())).collect(),
            progress: Some(7.0),
            ..base.clone()
        });
        assert_eq!(fitted.title.chars().count(), 40);
        assert!(!fitted.title.contains('\n'));
        assert_eq!(fitted.rows.len(), 4);
        assert_eq!(fitted.rows[0].1, "1234567…");
        assert_eq!(fitted.progress, Some(1.0));
        assert_eq!(fit_app(AppTile { progress: Some(f32::NAN), ..fitted.clone() }).progress, None);
        let sneaky = fit_app(AppTile {
            app: "Score\u{202E}bot\u{200B}".into(),
            rows: vec![("Home\u{2066}\u{0007}".into(), "1".into())],
            ..fitted
        });
        assert_eq!(sneaky.app, "Scorebot");
        assert_eq!(sneaky.rows[0].0, "Home");
    }

    #[test]
    fn hidden_tiles_keep_the_newest() {
        let mut hidden: Vec<String> = (0..MAX_HIDDEN).map(|n| n.to_string()).collect();
        hide(&mut hidden, "new");
        assert_eq!(hidden.len(), MAX_HIDDEN);
        assert_eq!(hidden.first().map(String::as_str), Some("1"));
        assert_eq!(hidden.last().map(String::as_str), Some("new"));
    }
}
