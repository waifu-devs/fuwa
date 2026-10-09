//! Live tiles that apps keep up to date (`LiveTileService`), set by agents
//! here and by webhooks through `crate::webhooks` (docs/live-tiles.md). They
//! live in the server's file (`live_tiles`); changing one isn't an event in
//! the log: it goes out, not stored, to whoever can see its channel, at most
//! once per `live_tile_publish_ms` per tile.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, Seat, automod, respond, users};
use crate::app::App;
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, LiveTileKind, LiveTileSource, Permission, live_tile_service_server::LiveTileService};
use crate::servers::{self as store, Payload, ServerDb, load_channel};

/// The shortest time between two updates of one tile going out, unless the
/// instance says otherwise (`live_tile_publish_ms`).
pub const LIVE_TILE_PUBLISH_MS: usize = 1000;

/// How many times a minute one agent or webhook may set or end tiles in a
/// server, unless the instance says otherwise (`live_tile_updates_per_minute`).
pub const LIVE_TILE_UPDATES_PER_MINUTE: usize = 120;
/// How long a tile stays after its last change when its app doesn't say.
const DEFAULT_TTL_MS: i64 = 2 * 60 * 60 * 1000;
const MAX_TILE_ID: usize = 64;
const MAX_TITLE: usize = 40;
const MAX_STATUS: usize = 16;
const MAX_ROWS: usize = 4;
const MAX_LABEL: usize = 24;
const MAX_VALUE: usize = 8;
const MAX_ACTION: usize = 12;

/// Who is setting a tile.
pub(super) struct Source {
    pub id: String,
    pub kind: LiveTileSource,
    pub name: String,
    pub avatar_url: String,
    /// The agent's roles, for AutoMod's exemptions; none for a webhook.
    pub role_ids: Vec<String>,
    /// MANAGE_SERVER: AutoMod leaves them alone, as with messages.
    pub manager: bool,
}

/// Characters a tile never shows: controls, zero-width ones and the ones
/// that turn text around, so a tile can't hide words or pass as another.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// One field of a tile: hidden characters out, runs of spaces folded, at
/// most `max` characters.
fn field(name: &str, value: &str, max: usize) -> Result<String> {
    let cleaned: String = value.chars().filter(|c| !hidden(*c) || c.is_whitespace()).collect();
    let folded = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let folded: String = folded.chars().filter(|c| !hidden(*c)).collect();
    if folded.chars().count() > max {
        return Err(Error::invalid(format!("a tile's {name} can be at most {max} characters")));
    }
    Ok(folded)
}

/// A tile's content as it will be kept and shown.
pub(super) fn checked_content(content: &pb::LiveTileContent) -> Result<pb::LiveTileContent> {
    let title = field("title", &content.title, MAX_TITLE)?;
    if title.is_empty() {
        return Err(Error::invalid("a tile needs a title"));
    }
    if content.rows.len() > MAX_ROWS {
        return Err(Error::invalid(format!("a tile has at most {MAX_ROWS} rows")));
    }
    let rows = content
        .rows
        .iter()
        .map(|row| {
            Ok(pb::LiveTileRow {
                label: field("row label", &row.label, MAX_LABEL)?,
                value: field("row value", &row.value, MAX_VALUE)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let progress = match content.progress {
        Some(p) if !(0.0..=1.0).contains(&p) => return Err(Error::invalid("a tile's progress is 0 to 1")),
        other => other,
    };
    Ok(pb::LiveTileContent {
        title,
        status: field("status", &content.status, MAX_STATUS)?,
        live: content.live,
        rows,
        progress,
        action: field("action", &content.action, MAX_ACTION)?,
    })
}

/// An app's id for a tile: 1 to 64 letters, digits, `-` and `_`.
pub(super) fn tile_id(value: &str) -> Result<String> {
    let shaped = !value.is_empty()
        && value.len() <= MAX_TILE_ID
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !shaped {
        return Err(Error::invalid("a tile's id is 1 to 64 letters, digits, - and _"));
    }
    Ok(value.to_string())
}

/// How long a tile stays after this change.
fn ttl_ms(ttl_seconds: Option<i64>) -> Result<i64> {
    match ttl_seconds {
        None => Ok(DEFAULT_TTL_MS),
        Some(s) if s >= 1 => Ok(s.saturating_mul(1000)),
        Some(_) => Err(Error::invalid("a tile's ttl_seconds is 1 or more")),
    }
}

/// Everything AutoMod reads of a tile, one field a line.
fn reviewed(content: &pb::LiveTileContent) -> String {
    let mut all = vec![content.title.as_str(), content.status.as_str(), content.action.as_str()];
    for row in &content.rows {
        all.push(&row.label);
        all.push(&row.value);
    }
    all.retain(|s| !s.is_empty());
    all.join("\n")
}

/// A channel apps may put tiles in: one of the server's own text or
/// announcement channels, shared with nobody (so never secure: those are
/// a type of their own).
fn tile_channel(channel: &pb::Channel) -> Result<()> {
    if channel.shared.is_some() {
        return Err(Error::invalid("live tiles stay out of channels shared with other servers"));
    }
    match pb::ChannelType::try_from(channel.r#type) {
        Ok(pb::ChannelType::Text | pb::ChannelType::Announcement) => Ok(()),
        _ => Err(Error::invalid("live tiles go in text and announcement channels")),
    }
}

/// Whether the server shows app tiles.
fn apps_shown(server: &pb::Server) -> bool {
    server.live_tiles.as_ref().is_some_and(|t| t.kinds.contains(&(LiveTileKind::App as i32)))
}

const TILE_COLUMNS: &str = "channel_id, source_id, tile_id, source_kind, content, updated_at, expires_at";

/// A row as a tile; its source's name and picture are filled in after.
fn tile_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<pb::LiveTile> + '_ {
    move |r| {
        let content = pb::LiveTileContent::decode(r.get::<Vec<u8>>(4)?.as_slice()).unwrap_or_default();
        Ok(pb::LiveTile {
            channel_id: r.get(0)?,
            source_id: r.get(1)?,
            id: r.get(2)?,
            source_kind: r.get(3)?,
            server_id: server_id.to_string(),
            source_name: String::new(),
            source_avatar_url: String::new(),
            content: Some(content),
            updated_at: Some(timestamp(r.get(5)?)),
            expires_at: Some(timestamp(r.get(6)?)),
        })
    }
}

/// Fills in who each tile is from: agents as the server last saw them,
/// webhooks under their own name. Tiles whose agent or webhook is gone are
/// dropped.
async fn named(conn: &turso::Connection, mut tiles: Vec<pb::LiveTile>) -> Result<Vec<pb::LiveTile>> {
    let agents: Vec<&str> =
        tiles.iter().filter(|t| t.source_kind == LiveTileSource::Agent as i32).map(|t| t.source_id.as_str()).collect();
    let people = users(conn, &agents).await?;
    let hooks: HashMap<String, (String, String, String)> =
        query_all(conn, "SELECT id, name, avatar_url, channel_id FROM webhooks", (), |r| {
            Ok((r.get::<String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
        })
        .await?
        .into_iter()
        .collect();
    tiles.retain_mut(|tile| {
        if tile.source_kind == LiveTileSource::Webhook as i32 {
            let Some((name, avatar, channel)) = hooks.get(&tile.source_id) else { return false };
            if *channel != tile.channel_id {
                return false;
            }
            tile.source_name = name.clone();
            tile.source_avatar_url = avatar.clone();
            true
        } else {
            let Some(user) = people.iter().find(|u| u.id == tile.source_id) else { return false };
            tile.source_name =
                if user.display_name.is_empty() { user.username.clone() } else { user.display_name.clone() };
            tile.source_avatar_url = user.avatar_url.clone();
            true
        }
    });
    Ok(tiles)
}

/// Sets a tile for `source` inside one write: checks the server shows app
/// tiles, the channel takes them, AutoMod and the instance's cap.
pub(super) async fn set_tile(
    app: &Arc<App>,
    sdb: &Arc<ServerDb>,
    source: &Source,
    channel_id: &str,
    tile_id: &str,
    content: &pb::LiveTileContent,
    ttl_seconds: Option<i64>,
) -> Result<pb::LiveTile> {
    let tile_id = self::tile_id(tile_id)?;
    let content = checked_content(content)?;
    let ttl = ttl_ms(ttl_seconds)?;
    let settings = app.settings();
    pace(&sdb.id, &source.id, now_ms(), settings.live_tile_updates_per_minute)?;
    let per_channel = settings.limits.live_tiles_per_channel;
    let text = reviewed(&content);
    let tile = sdb
        .write(&source.id, async |conn, _events| {
            if !apps_shown(&store::load_server(conn).await?) {
                return Err(Error::FailedPrecondition("this server doesn't show tiles from apps".into()));
            }
            let channel = load_channel(conn, &sdb.id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
            tile_channel(&channel)?;
            if let Some(why) = automod::tile_blocked(conn, &source.role_ids, source.manager, &channel, &text).await? {
                return Ok(Err(why));
            }
            let now = now_ms();
            conn.execute("DELETE FROM live_tiles WHERE expires_at <= ?1", [now]).await?;
            if let Some(cap) = per_channel {
                // Every capped write here touches this row, so two racing ones clash.
                conn.execute(
                    "INSERT INTO live_tile_channels (channel_id, changed_at) VALUES (?1, ?2)
                     ON CONFLICT (channel_id) DO UPDATE SET changed_at = excluded.changed_at",
                    (channel.id.as_str(), now),
                )
                .await?;
                let others = query_one(
                    conn,
                    "SELECT count(*) FROM live_tiles WHERE channel_id = ?1 AND NOT (source_id = ?2 AND tile_id = ?3)",
                    (channel.id.as_str(), source.id.as_str(), tile_id.as_str()),
                    |r| r.get::<i64>(0),
                )
                .await?
                .unwrap_or_default();
                if others >= cap {
                    return Err(Error::ResourceExhausted(format!("a channel can have at most {cap} live tiles")));
                }
            }
            let expires = now.saturating_add(ttl);
            conn.execute(
                "INSERT INTO live_tiles (channel_id, source_id, tile_id, source_kind, content, updated_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (channel_id, source_id, tile_id) DO UPDATE SET
                   content = excluded.content, updated_at = excluded.updated_at, expires_at = excluded.expires_at",
                (
                    channel.id.as_str(),
                    source.id.as_str(),
                    tile_id.as_str(),
                    source.kind as i64,
                    content.encode_to_vec(),
                    now,
                    expires,
                ),
            )
            .await?;
            Ok(Ok(pb::LiveTile {
                id: tile_id.clone(),
                server_id: sdb.id.clone(),
                channel_id: channel.id.clone(),
                source_id: source.id.clone(),
                source_kind: source.kind as i32,
                source_name: source.name.clone(),
                source_avatar_url: source.avatar_url.clone(),
                content: Some(content.clone()),
                updated_at: Some(timestamp(now)),
                expires_at: Some(timestamp(expires)),
            }))
        })
        .await?
        .map_err(Error::denied)?;
    let event = tile_event(
        &tile.server_id,
        &tile.source_id,
        Payload::LiveTileUpdated(pb::LiveTileUpdated { tile: Some(tile.clone()) }),
    );
    publish_update(
        app,
        key(&tile.server_id, &tile.channel_id, &tile.source_id, &tile.id),
        event,
        settings.live_tile_publish_ms,
    );
    Ok(tile)
}

/// Ends one tile and tells its channel, if it was there. A moderator's end
/// (`audit`, with the channel's name) is in the audit log, in the same write.
pub(super) async fn end_tile(
    app: &Arc<App>,
    sdb: &Arc<ServerDb>,
    actor_id: &str,
    channel_id: &str,
    source_id: &str,
    tile_id: &str,
    audit: Option<&str>,
) -> Result<bool> {
    let ended = sdb
        .write(actor_id, async |conn, _events| {
            let ended = conn
                .execute(
                    "DELETE FROM live_tiles WHERE channel_id = ?1 AND source_id = ?2 AND tile_id = ?3",
                    (channel_id, source_id, tile_id),
                )
                .await?
                > 0;
            if let (true, Some(channel_name)) = (ended, audit) {
                let entry = store::Audit::new(pb::AuditAction::LiveTileEnd, source_id)
                    .channel(channel_name.to_string())
                    .change("tile", tile_id, "");
                store::audit(conn, actor_id, entry).await?;
            }
            Ok(ended)
        })
        .await?;
    if ended {
        announce_end(app, &sdb.id, channel_id, source_id, tile_id);
    }
    Ok(ended)
}

/// Tells a channel a tile went, dropping any update of it still waiting.
pub(super) fn announce_end(app: &App, server_id: &str, channel_id: &str, source_id: &str, tile_id: &str) {
    forget(&key(server_id, channel_id, source_id, tile_id));
    app.hub.publish([tile_event(
        server_id,
        source_id,
        Payload::LiveTileEnded(pb::LiveTileEnded {
            channel_id: channel_id.to_string(),
            tile_id: tile_id.to_string(),
            source_id: source_id.to_string(),
        }),
    )]);
}

/// Ends every tile a webhook keeps, inside a write (it was deleted or moved);
/// returns them as (channel, tile) for `announce_end` once the write is done.
pub(super) async fn drop_webhook_tiles(conn: &turso::Connection, webhook_id: &str) -> Result<Vec<(String, String)>> {
    let tiles = query_all(conn, "SELECT channel_id, tile_id FROM live_tiles WHERE source_id = ?1", [webhook_id], |r| {
        Ok((r.get::<String>(0)?, r.get::<String>(1)?))
    })
    .await?;
    conn.execute("DELETE FROM live_tiles WHERE source_id = ?1", [webhook_id]).await?;
    Ok(tiles)
}

fn tile_event(server_id: &str, actor_id: &str, payload: Payload) -> pb::Event {
    pb::Event {
        id: new_id(),
        server_id: server_id.to_string(),
        sequence: 0,
        actor_id: actor_id.to_string(),
        created_at: Some(timestamp(now_ms())),
        payload: Some(payload),
    }
}

fn key(server_id: &str, channel_id: &str, source_id: &str, tile_id: &str) -> String {
    format!("{server_id}/{channel_id}/{source_id}/{tile_id}")
}

/// Each tile's last update sent and the one waiting to go, kept in memory.
#[derive(Default)]
struct Outgoing {
    sent_at: i64,
    waiting: Option<pb::Event>,
}

static OUTGOING: Mutex<Option<HashMap<String, Outgoing>>> = Mutex::new(None);

fn outgoing() -> std::sync::MutexGuard<'static, Option<HashMap<String, Outgoing>>> {
    OUTGOING.lock().unwrap_or_else(|e| e.into_inner())
}

/// What to do with a tile's update at `now`, `gap` milliseconds apart at
/// least: send it now, or hold it as the latest and, for the first one held,
/// send it after the returned wait.
fn hold(map: &mut HashMap<String, Outgoing>, key: &str, event: pb::Event, now: i64, gap: i64) -> Hold {
    if map.len() > 10_000 {
        map.retain(|_, o| o.waiting.is_some() || now - o.sent_at < gap);
    }
    let entry = map.entry(key.to_string()).or_default();
    if entry.waiting.is_some() {
        entry.waiting = Some(event);
        return Hold::Held;
    }
    if now - entry.sent_at >= gap {
        entry.sent_at = now;
        return Hold::Send(Box::new(event));
    }
    entry.waiting = Some(event);
    Hold::Later(entry.sent_at + gap - now)
}

#[derive(Debug)]
enum Hold {
    Send(Box<pb::Event>),
    Later(i64),
    Held,
}

/// The update held for `key`, if it's still wanted, marked as sent at `now`.
fn take_held(map: &mut HashMap<String, Outgoing>, key: &str, now: i64) -> Option<pb::Event> {
    let entry = map.get_mut(key)?;
    let event = entry.waiting.take()?;
    entry.sent_at = now;
    Some(event)
}

fn forget(key: &str) {
    if let Some(map) = outgoing().as_mut() {
        map.remove(key);
    }
}

/// Sends a tile's update now, or the latest one once `gap_ms` has passed
/// since the last went out.
fn publish_update(app: &Arc<App>, key: String, event: pb::Event, gap_ms: Option<i64>) {
    let Some(gap) = gap_ms else {
        app.hub.publish([event]);
        return;
    };
    let decided = hold(outgoing().get_or_insert_with(HashMap::new), &key, event, now_ms(), gap);
    match decided {
        Hold::Send(event) => app.hub.publish([*event]),
        Hold::Held => {}
        Hold::Later(wait) => {
            let app = app.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(wait.max(1) as u64)).await;
                let event = outgoing().as_mut().and_then(|map| take_held(map, &key, now_ms()));
                if let Some(event) = event {
                    app.hub.publish([event]);
                }
            });
        }
    }
}

/// Sets and ends per agent or webhook per server per minute (the instance's
/// `live_tile_updates_per_minute`, 120 unless set), kept in memory.
static PACE: Mutex<Option<HashMap<String, (i64, i64)>>> = Mutex::new(None);

pub(super) fn pace(server_id: &str, source_id: &str, now: i64, per_minute: Option<i64>) -> Result<()> {
    let Some(per_minute) = per_minute else { return Ok(()) };
    let mut guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
    count_in(guard.get_or_insert_with(HashMap::new), server_id, source_id, now, per_minute)
}

/// [`pace`] against the counts given.
fn count_in(
    counts: &mut HashMap<String, (i64, i64)>,
    server_id: &str,
    source_id: &str,
    now: i64,
    per_minute: i64,
) -> Result<()> {
    let minute = now / 60_000;
    if counts.len() > 10_000 {
        counts.retain(|_, (at, _)| *at == minute);
    }
    let entry = counts.entry(format!("{server_id}/{source_id}")).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    if entry.1 >= per_minute {
        let wait = (minute + 1) * 60_000 - now;
        return Err(Error::Limited("this app is changing its tiles too fast; slow down".into(), wait));
    }
    entry.1 += 1;
    Ok(())
}

#[tonic::async_trait]
impl LiveTileService for Api {
    async fn set_live_tile(
        &self,
        request: Request<pb::SetLiveTileRequest>,
    ) -> Result<Response<pb::SetLiveTileResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                if account.kind != pb::AccountKind::Agent {
                    return Err(Error::denied("only agents and webhooks set live tiles"));
                }
                let Seat { sdb, member, access } = self.membership(&account, &req.server_id).await?;
                if !access.can_see(&req.channel_id) {
                    return Err(Error::NotFound("channel"));
                }
                access.require_in(&req.channel_id, Permission::SendMessages)?;
                let source = Source {
                    id: account.id.clone(),
                    kind: LiveTileSource::Agent,
                    name: if account.display_name.is_empty() {
                        account.username.clone()
                    } else {
                        account.display_name.clone()
                    },
                    avatar_url: account.avatar_url.clone(),
                    role_ids: member.role_ids.clone(),
                    manager: access.has(Permission::ManageServer),
                };
                let content = req.content.unwrap_or_default();
                let tile = set_tile(&self.app, &sdb, &source, &req.channel_id, &req.tile_id, &content, req.ttl_seconds)
                    .await?;
                Ok(pb::SetLiveTileResponse { tile: Some(tile) })
            }
            .await,
        )
    }

    async fn end_live_tile(
        &self,
        request: Request<pb::EndLiveTileRequest>,
    ) -> Result<Response<pb::EndLiveTileResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                if !access.can_see(&req.channel_id) {
                    return Err(Error::NotFound("channel"));
                }
                let tile_id = tile_id(&req.tile_id)?;
                let source_id = if req.source_id.is_empty() { account.id.clone() } else { req.source_id.clone() };
                if source_id == account.id {
                    pace(&sdb.id, &account.id, now_ms(), self.app.settings().live_tile_updates_per_minute)?;
                    end_tile(&self.app, &sdb, &account.id, &req.channel_id, &source_id, &tile_id, None).await?;
                } else {
                    access.require_in(&req.channel_id, Permission::ManageMessages)?;
                    let channel = load_channel(&*sdb.read()?, &sdb.id, &req.channel_id).await?;
                    let name = channel.map(|c| c.name).unwrap_or_default();
                    end_tile(&self.app, &sdb, &account.id, &req.channel_id, &source_id, &tile_id, Some(&name)).await?;
                }
                Ok(pb::EndLiveTileResponse {})
            }
            .await,
        )
    }

    async fn list_live_tiles(
        &self,
        request: Request<pb::ListLiveTilesRequest>,
    ) -> Result<Response<pb::ListLiveTilesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let Seat { sdb, access, .. } = self.membership(&account, &request.get_ref().server_id).await?;
                if !apps_shown(&sdb.server().await?) {
                    return Ok(pb::ListLiveTilesResponse { tiles: vec![] });
                }
                let conn = sdb.read()?;
                let mut tiles = query_all(
                    &conn,
                    &format!("SELECT {TILE_COLUMNS} FROM live_tiles WHERE expires_at > ?1 ORDER BY updated_at DESC"),
                    [now_ms()],
                    tile_row(&sdb.id),
                )
                .await?;
                tiles.retain(|t| access.can_see(&t.channel_id));
                let channels = store::load_channels(&conn, &sdb.id).await?;
                tiles.retain(|t| channels.iter().any(|c| c.id == t.channel_id && tile_channel(c).is_ok()));
                // An agent that may no longer send in a channel no longer shows tiles there.
                let mut agents = HashMap::new();
                for tile in &tiles {
                    if tile.source_kind == LiveTileSource::Agent as i32 && !agents.contains_key(&tile.source_id) {
                        let source = sdb.member_access(&tile.source_id).await?.map(|(_, access)| access);
                        agents.insert(tile.source_id.clone(), source);
                    }
                }
                tiles.retain(|t| match agents.get(&t.source_id) {
                    Some(source) => source.as_ref().is_some_and(|a| a.has_in(&t.channel_id, Permission::SendMessages)),
                    None => true,
                });
                Ok(pb::ListLiveTilesResponse { tiles: named(&conn, tiles).await? })
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(n: &str) -> pb::Event {
        pb::Event { id: n.into(), ..Default::default() }
    }

    #[test]
    fn tile_text_loses_hidden_characters_and_extra_spaces() {
        let content = pb::LiveTileContent {
            title: "  Cup\u{202E}  final\u{200B}\n".into(),
            status: "67'\u{0007}".into(),
            rows: vec![pb::LiveTileRow { label: "Red\tFoxes".into(), value: "2".into() }],
            action: "Watch\u{2066}".into(),
            ..Default::default()
        };
        let checked = checked_content(&content).unwrap();
        assert_eq!(checked.title, "Cup final");
        assert_eq!(checked.status, "67'");
        assert_eq!(checked.rows[0].label, "Red Foxes");
        assert_eq!(checked.action, "Watch");
    }

    #[test]
    fn tile_content_keeps_to_its_limits() {
        let ok = pb::LiveTileContent { title: "a".repeat(MAX_TITLE), ..Default::default() };
        assert!(checked_content(&ok).is_ok());
        let long = pb::LiveTileContent { title: "a".repeat(MAX_TITLE + 1), ..Default::default() };
        assert!(checked_content(&long).is_err());
        let empty = pb::LiveTileContent { title: "\u{200B} ".into(), ..Default::default() };
        assert!(checked_content(&empty).is_err());
        let rows = pb::LiveTileContent {
            title: "t".into(),
            rows: vec![pb::LiveTileRow::default(); MAX_ROWS + 1],
            ..Default::default()
        };
        assert!(checked_content(&rows).is_err());
        let value = pb::LiveTileContent {
            title: "t".into(),
            rows: vec![pb::LiveTileRow { label: "x".into(), value: "123456789".into() }],
            ..Default::default()
        };
        assert!(checked_content(&value).is_err());
        for bad in [f32::NAN, -0.1, 1.5] {
            let progress = pb::LiveTileContent { title: "t".into(), progress: Some(bad), ..Default::default() };
            assert!(checked_content(&progress).is_err(), "{bad}");
        }
    }

    #[test]
    fn tile_ids_are_short_plain_words() {
        assert!(tile_id("cup-final_2").is_ok());
        for bad in ["", "has space", "slash/", &"a".repeat(MAX_TILE_ID + 1), "ünï"] {
            assert!(tile_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn updates_go_out_at_most_once_a_gap_with_the_latest_held() {
        let mut map = HashMap::new();
        assert!(matches!(hold(&mut map, "t", event("1"), 10_000, 1000), Hold::Send(e) if e.id == "1"));
        // Too soon: held, and only the first held one sets a timer.
        assert!(matches!(hold(&mut map, "t", event("2"), 10_300, 1000), Hold::Later(700)));
        assert!(matches!(hold(&mut map, "t", event("3"), 10_600, 1000), Hold::Held));
        // Other tiles aren't held back.
        assert!(matches!(hold(&mut map, "u", event("x"), 10_600, 1000), Hold::Send(_)));
        // The timer sends the latest, once.
        assert_eq!(take_held(&mut map, "t", 11_000).map(|e| e.id), Some("3".into()));
        assert!(take_held(&mut map, "t", 11_000).is_none());
        assert!(matches!(hold(&mut map, "t", event("4"), 11_500, 1000), Hold::Later(500)));
        // An ended tile's held update never goes out.
        map.remove("t");
        assert!(take_held(&mut map, "t", 12_000).is_none());
    }

    #[test]
    fn pace_counts_per_app_per_server() {
        let mut counts = HashMap::new();
        let mut pace = |server: &str, now: i64| count_in(&mut counts, server, "app", now, 2);
        let now = 7 * 60_000;
        assert!(pace("s1", now).is_ok());
        assert!(pace("s1", now + 1).is_ok());
        assert!(matches!(pace("s1", now + 2), Err(Error::Limited(_, 59_998))));
        assert!(pace("s2", now + 2).is_ok());
        assert!(pace("s1", now + 60_000).is_ok());
        assert!(
            count_in(&mut counts, "s1", "other", now + 2, 2).is_ok(),
            "another app in the same server counts apart"
        );
    }

    #[test]
    fn only_a_servers_own_text_channels_take_tiles() {
        let text = pb::Channel { r#type: pb::ChannelType::Text as i32, ..Default::default() };
        assert!(tile_channel(&text).is_ok());
        let secure = pb::Channel { r#type: pb::ChannelType::Secure as i32, ..Default::default() };
        assert!(tile_channel(&secure).is_err());
        let voice = pb::Channel { r#type: pb::ChannelType::Voice as i32, ..Default::default() };
        assert!(tile_channel(&voice).is_err());
        let shared = pb::Channel { shared: Some(pb::SharedChannel { home: true, ..Default::default() }), ..text };
        assert!(tile_channel(&shared).is_err());
    }
}
