//! Community servers: one Turso database file each, under `<data>/servers/`.
//!
//! A server's file holds everything about it (profile, members, channels,
//! messages, its event log, usage counters and caps), so a server can be backed
//! up or moved by copying one file. The directory's index of every server and
//! who belongs where (see [`crate::cluster::index`]) is built from the files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use prost::Message as _;
use tokio::sync::Mutex;
use turso::{Connection, Row};

use crate::config;
use crate::cpb;
use crate::db::{self, Db, EncryptionKey, query_all, query_one};
use crate::error::{Error, Result};
use crate::hub::Hub;
use crate::id::{millis, new_id, now_ms, parse_id, timestamp};
use crate::pb;
use crate::permissions;
use crate::replica::Replica;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/server/0001_init.sql"),
    include_str!("../migrations/server/0002_concurrent_writes.sql"),
    include_str!("../migrations/server/0003_status.sql"),
    include_str!("../migrations/server/0004_moderation.sql"),
    include_str!("../migrations/server/0005_roles.sql"),
    include_str!("../migrations/server/0006_invites.sql"),
    include_str!("../migrations/server/0007_join.sql"),
    include_str!("../migrations/server/0008_automod_emoji_welcome.sql"),
    include_str!("../migrations/server/0009_webhooks.sql"),
    include_str!("../migrations/server/0010_voice.sql"),
    include_str!("../migrations/server/0011_sso.sql"),
    include_str!("../migrations/server/0012_video.sql"),
    include_str!("../migrations/server/0013_recordings.sql"),
    include_str!("../migrations/server/0014_recording_limits.sql"),
    include_str!("../migrations/server/0015_secure_channels.sql"),
    include_str!("../migrations/server/0016_region.sql"),
    include_str!("../migrations/server/0017_shared_channels.sql"),
    include_str!("../migrations/server/0018_voice_video_off.sql"),
    include_str!("../migrations/server/0019_secure_history.sql"),
    include_str!("../migrations/server/0020_federation.sql"),
    include_str!("../migrations/server/0021_search.sql"),
];

pub type Payload = pb::event::Payload;

/// What moderators turned off for someone in a server's voice channels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceModeration {
    pub mute: bool,
    pub deaf: bool,
    /// Their camera and shared screen.
    pub video_off: bool,
}

impl VoiceModeration {
    pub fn of(state: &pb::VoiceState) -> Self {
        Self { mute: state.server_mute, deaf: state.server_deaf, video_off: state.server_video_off }
    }

    /// Puts it on someone's place, turning off what it keeps off.
    pub fn apply(self, state: &mut pb::VoiceState) {
        state.server_mute = self.mute;
        state.server_deaf = self.deaf;
        state.server_video_off = self.video_off;
        state.self_video &= !self.video_off;
        state.self_stream &= !self.video_off;
    }
}

/// One open community server database.
pub struct ServerDb {
    pub id: String,
    path: PathBuf,
    /// The file. Writes hold its gate shared, so they run side by side; the
    /// few that sweep rows other writes may be adding to (deleting a channel
    /// and its messages, deleting the server) hold it alone.
    db: Arc<Db>,
    /// The event log's last sequence, or `None` to read it again from the file.
    /// Held from handing out a write's sequences until it has committed and
    /// published them, so the log commits and goes out in sequence order.
    head: Mutex<Option<i64>>,
    /// Writes since the usage changes were last folded into the totals.
    unfolded: AtomicU32,
    folding: AtomicBool,
    hub: Arc<Hub>,
    /// Being moved to another shard: it takes no changes until the move is
    /// over (see [`Servers::freeze`]).
    frozen: AtomicBool,
}

/// Usage changes are folded into the totals after this many writes.
const FOLD_EVERY: u32 = 256;

/// What a write changes about the message totals; negative takes away.
#[derive(Debug, Default, Clone, Copy)]
pub struct UsageChange {
    pub messages: i64,
    pub messages_sent: i64,
    pub message_bytes: i64,
    pub attachments: i64,
    pub attachment_bytes: i64,
}

/// Records a change to the message totals, as a row of its own so that writes
/// running side by side never update the same row.
pub async fn add_usage(conn: &Connection, change: UsageChange) -> Result<()> {
    conn.execute(
        "INSERT INTO usage_changes (id, messages, messages_sent, message_bytes, attachments, attachment_bytes, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            new_id(),
            change.messages,
            change.messages_sent,
            change.message_bytes,
            change.attachments,
            change.attachment_bytes,
            now_ms(),
        ),
    )
    .await?;
    Ok(())
}

/// What's stored in an audit entry's `changes` column.
#[derive(Clone, PartialEq, prost::Message)]
struct AuditChanges {
    #[prost(message, repeated, tag = "1")]
    changes: Vec<pb::AuditChange>,
}

/// One thing an owner or admin did, as [`audit`] records it.
#[derive(Debug, Default, Clone)]
pub struct Audit {
    pub action: pb::AuditAction,
    pub target_id: String,
    pub channel_name: String,
    pub role_name: String,
    pub reason: String,
    pub changes: Vec<pb::AuditChange>,
}

impl Audit {
    pub fn new(action: pb::AuditAction, target_id: impl Into<String>) -> Self {
        Self { action, target_id: target_id.into(), ..Default::default() }
    }

    pub fn channel(mut self, name: impl Into<String>) -> Self {
        self.channel_name = name.into();
        self
    }

    pub fn role(mut self, name: impl Into<String>) -> Self {
        self.role_name = name.into();
        self
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    /// Notes a field's change, if it changed.
    pub fn change(mut self, field: &str, before: impl ToString, after: impl ToString) -> Self {
        let (before, after) = (before.to_string(), after.to_string());
        if before != after {
            self.changes.push(pb::AuditChange { field: field.into(), before, after });
        }
        self
    }
}

/// Adds an entry to the audit log, in the write that did it.
pub async fn audit(conn: &Connection, actor_id: &str, entry: Audit) -> Result<()> {
    let changes = (!entry.changes.is_empty()).then(|| AuditChanges { changes: entry.changes }.encode_to_vec());
    conn.execute(
        "INSERT INTO audit (id, actor_id, action, target_id, channel_name, role_name, reason, changes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
            new_id(),
            actor_id,
            entry.action as i64,
            entry.target_id.as_str(),
            entry.channel_name.as_str(),
            entry.role_name.as_str(),
            entry.reason.as_str(),
            changes,
            now_ms(),
        ),
    )
    .await?;
    Ok(())
}

/// Reads an audit entry's `changes` column.
pub fn audit_changes(bytes: Option<Vec<u8>>) -> Result<Vec<pb::AuditChange>> {
    Ok(match bytes {
        Some(bytes) => AuditChanges::decode(bytes.as_slice())?.changes,
        None => vec![],
    })
}

/// A voice channel recorded on the server, as its server's file keeps it.
#[derive(Debug, Clone)]
pub struct RecordingRow {
    pub id: String,
    pub channel_id: String,
    pub started_by: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// Its files are sealed with a key from the instance's encryption key.
    pub sealed: bool,
    /// JSON: `[{user_id, size_bytes, duration_ms}]`.
    pub tracks: String,
    pub size_bytes: i64,
}

const RECORDING_COLUMNS: &str = "id, channel_id, started_by, started_at, ended_at, sealed, tracks, size_bytes";

fn recording_row(r: &Row) -> turso::Result<RecordingRow> {
    Ok(RecordingRow {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        started_by: r.get(2)?,
        started_at: r.get(3)?,
        ended_at: r.get(4)?,
        sealed: r.get::<i64>(5)? != 0,
        tracks: r.get(6)?,
        size_bytes: r.get(7)?,
    })
}

impl ServerDb {
    /// A connection for reading.
    pub fn read(&self) -> Result<Connection> {
        db::connect(&self.db)
    }

    /// Runs a change in one concurrent transaction, alongside other writes to
    /// this server. `f` makes its changes and pushes an event payload for each;
    /// the events are appended to the server's log in the same transaction and
    /// sent to subscribers once it commits. If it clashes with another write
    /// over the same rows, `f` runs again (see [`db::transaction`]).
    ///
    /// Everything up to appending the events runs in parallel. Appending,
    /// committing and publishing go one write at a time, so the log's sequence
    /// is its commit order and subscribers see events in that order.
    pub async fn write<T>(
        &self,
        actor_id: &str,
        f: impl AsyncFnOnce(&Connection, &mut Vec<Payload>) -> Result<T> + Clone,
    ) -> Result<T> {
        self.writable()?;
        let shared = self.db.shared().await;
        self.writable()?;
        let value = self.run(actor_id, f).await;
        drop(shared);
        if value.is_ok() {
            self.fold_now_and_then().await;
        }
        value
    }

    /// Like [`write`](Self::write), but with no other write to this server
    /// running at the same time: for changes that sweep rows other writes may
    /// be adding to, such as a channel's messages.
    pub async fn write_alone<T>(
        &self,
        actor_id: &str,
        f: impl AsyncFnOnce(&Connection, &mut Vec<Payload>) -> Result<T> + Clone,
    ) -> Result<T> {
        self.writable()?;
        let _alone = self.db.alone().await;
        self.writable()?;
        self.run(actor_id, f).await
    }

    /// Runs a change that sends no events, alongside other writes, in its own
    /// transaction: for what the server keeps about itself, like the search
    /// index. Refused while the server is being moved, like any write.
    pub async fn write_quiet<T>(&self, f: impl AsyncFnOnce(&Connection) -> Result<T> + Clone) -> Result<T> {
        self.writable()?;
        let _shared = self.db.shared().await;
        self.writable()?;
        db::transaction(&db::connect(&self.db)?, f).await
    }

    /// Refuses changes while the server is being moved to another shard: the
    /// gateway holds them and tries again, on the new shard once it's there.
    pub fn writable(&self) -> Result<()> {
        if self.frozen.load(Ordering::Acquire) { Err(Error::Moving) } else { Ok(()) }
    }

    /// The database, for copying its files.
    pub fn db(&self) -> &Arc<Db> {
        &self.db
    }

    async fn run<T>(
        &self,
        actor_id: &str,
        f: impl AsyncFnOnce(&Connection, &mut Vec<Payload>) -> Result<T> + Clone,
    ) -> Result<T> {
        let conn = db::connect(&self.db)?;
        let mut attempt = 0;
        loop {
            db::begin(&conn).await?;
            let mut payloads = Vec::new();
            let result = match f.clone()(&conn, &mut payloads).await {
                Ok(value) => self.append_and_commit(&conn, actor_id, payloads).await.map(|()| value),
                Err(err) => Err(err),
            };
            match result {
                Err(err) if db::is_conflict(&err) => {
                    db::abort(&conn).await;
                    db::retry_after(&err, &mut attempt).await?;
                }
                Err(err) => {
                    db::abort(&conn).await;
                    return Err(err);
                }
                Ok(value) => return Ok(value),
            }
        }
    }

    /// Appends the events under the next sequences, commits and publishes.
    async fn append_and_commit(&self, conn: &Connection, actor_id: &str, payloads: Vec<Payload>) -> Result<()> {
        let mut head = self.head.lock().await;
        let last = match *head {
            Some(last) => last,
            None => self.stored_head().await?,
        };
        let now = now_ms();
        let mut events = Vec::with_capacity(payloads.len());
        for payload in payloads {
            let event = pb::Event {
                id: new_id(),
                server_id: self.id.clone(),
                sequence: last + 1 + events.len() as i64,
                actor_id: actor_id.to_string(),
                created_at: Some(timestamp(now)),
                payload: Some(payload),
            };
            conn.execute(
                "INSERT INTO events (sequence, id, actor_id, created_at, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                (event.sequence, event.id.as_str(), actor_id, now, event.encode_to_vec()),
            )
            .await?;
            events.push(event);
        }
        // Unknown until COMMIT returns: if this request is dropped mid-commit,
        // the next write reads the head from the file instead of guessing.
        *head = None;
        conn.execute("COMMIT", ()).await.inspect_err(|_| *head = Some(last))?;
        *head = Some(last + events.len() as i64);
        self.hub.publish(events);
        Ok(())
    }

    /// The last committed sequence, read from the file.
    async fn stored_head(&self) -> Result<i64> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT coalesce(max(sequence), 0) FROM events", (), |r| r.get::<i64>(0))
            .await?
            .unwrap_or(0))
    }

    /// Folds the usage changes into the totals every [`FOLD_EVERY`] writes,
    /// so reading them stays cheap. One fold at a time; a failed one is left
    /// for the next.
    async fn fold_now_and_then(&self) {
        if self.unfolded.fetch_add(1, Ordering::Relaxed) + 1 < FOLD_EVERY || self.folding.swap(true, Ordering::Acquire)
        {
            return;
        }
        self.unfolded.store(0, Ordering::Relaxed);
        if let Err(err) = self.fold_usage().await {
            tracing::warn!(server = %self.id, error = %err, "couldn't fold usage changes into the totals");
        }
        self.folding.store(false, Ordering::Release);
    }

    /// Adds the usage changes recorded so far into `usage` and removes them.
    /// Changes still being written aren't seen, so they're left for next time.
    pub async fn fold_usage(&self) -> Result<()> {
        db::write(&self.db, async |conn| {
            let Some((last, sums)) = query_one(
                conn,
                "SELECT max(id), coalesce(sum(messages), 0), coalesce(sum(messages_sent), 0), coalesce(sum(message_bytes), 0),
                        coalesce(sum(attachments), 0), coalesce(sum(attachment_bytes), 0)
                 FROM usage_changes",
                (),
                |r| Ok((r.get::<Option<String>>(0)?, [r.get::<i64>(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?])),
            )
            .await?
            .and_then(|(last, sums)| last.map(|last| (last, sums))) else {
                return Ok(());
            };
            conn.execute(
                "UPDATE usage SET messages = messages + ?1, messages_sent = messages_sent + ?2, message_bytes = message_bytes + ?3,
                 attachments = attachments + ?4, attachment_bytes = attachment_bytes + ?5, updated_at = ?6 WHERE id = 1",
                (sums[0], sums[1], sums[2], sums[3], sums[4], now_ms()),
            )
            .await?;
            conn.execute("DELETE FROM usage_changes WHERE id <= ?1", [last.as_str()]).await?;
            Ok(())
        })
        .await
    }

    /// Bytes this server takes on disk: its database, the log of commits not
    /// yet folded into it, and the write-ahead log.
    pub fn storage_bytes(&self) -> i64 {
        storage_bytes(&self.path)
    }

    /// The sequence of the last event committed, or 0 for none.
    pub async fn head_sequence(&self) -> Result<i64> {
        self.stored_head().await
    }

    /// Events after `after`, oldest first.
    pub async fn events_after(&self, after: i64, limit: i64) -> Result<Vec<pb::Event>> {
        let conn = self.read()?;
        let rows = query_all(
            &conn,
            "SELECT sequence, payload FROM events WHERE sequence > ?1 ORDER BY sequence LIMIT ?2",
            (after, limit),
            |r| Ok((r.get::<i64>(0)?, r.get::<Vec<u8>>(1)?)),
        )
        .await?;
        rows.into_iter()
            .map(|(sequence, payload)| {
                let mut event = pb::Event::decode(payload.as_slice())?;
                event.sequence = sequence;
                Ok(event)
            })
            .collect()
    }

    /// Writes a copy of this server's database to `dest` as a plain SQLite
    /// file: not encrypted, and not in concurrent-writer mode, so any SQLite
    /// tool opens it. Writes to the server wait while it's copied, so the copy
    /// is one moment.
    pub async fn export_to(&self, dest: &Path) -> Result<()> {
        let path = dest
            .to_str()
            .filter(|p| !p.contains('\''))
            .ok_or_else(|| Error::internal(format!("can't export to {}", dest.display())))?;
        {
            let _alone = self.db.alone().await;
            let conn = self.read()?;
            conn.execute(&format!("VACUUM INTO '{path}'"), ()).await?;
        }
        db::to_sqlite(dest, None).await?;
        {
            // Secrets stay on the instance: the provider's client secret and
            // sign-ins under way.
            let (_db, conn) = db::open_plain(dest).await?;
            let mut sso = load_sso(&conn).await?.provider;
            if !sso.oidc_client_secret.is_empty() {
                sso.oidc_client_secret.clear();
                conn.execute("UPDATE server SET sso = ?1", [sso.stored()]).await?;
            }
            conn.execute("DELETE FROM sso_sign_ins", ()).await?;
            db::pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)").await?;
        }
        for suffix in ["-wal", "-log"] {
            let mut side = dest.as_os_str().to_owned();
            side.push(suffix);
            let _ = std::fs::remove_file(side);
        }
        Ok(())
    }

    pub async fn server(&self) -> Result<pb::Server> {
        let conn = self.read()?;
        load_server(&conn).await
    }

    /// The totals: members and channels as kept, message totals with the
    /// changes not yet folded in, and the event log's length.
    pub async fn usage(&self) -> Result<pb::ServerUsage> {
        let conn = self.read()?;
        let mut usage = query_one(
            &conn,
            "SELECT u.members, u.channels, u.messages + c.messages, u.messages_sent + c.messages_sent,
                    u.message_bytes + c.message_bytes, u.attachments + c.attachments,
                    u.attachment_bytes + c.attachment_bytes, (SELECT coalesce(max(sequence), 0) FROM events),
                    max(u.updated_at, c.at), u.emojis
             FROM usage u,
                  (SELECT coalesce(sum(messages), 0) messages, coalesce(sum(messages_sent), 0) messages_sent,
                          coalesce(sum(message_bytes), 0) message_bytes, coalesce(sum(attachments), 0) attachments,
                          coalesce(sum(attachment_bytes), 0) attachment_bytes, coalesce(max(at), 0) at
                   FROM usage_changes) c
             WHERE u.id = 1",
            (),
            usage_row,
        )
        .await?
        .ok_or_else(|| Error::internal("usage row missing"))?;
        usage.server_id = self.id.clone();
        usage.storage_bytes = self.storage_bytes();
        Ok(usage)
    }

    /// This server's own caps (unset where it uses the instance defaults).
    pub async fn own_limits(&self) -> Result<pb::ServerLimits> {
        let conn = self.read()?;
        query_one(
            &conn,
            "SELECT members, channels, storage_bytes, attachment_bytes, emojis, recording_bytes FROM limits WHERE id = 1",
            (),
            |r| {
                Ok(pb::ServerLimits {
                    members: r.get(0)?,
                    channels: r.get(1)?,
                    storage_bytes: r.get(2)?,
                    attachment_bytes: r.get(3)?,
                    emojis: r.get(4)?,
                    recording_bytes: r.get(5)?,
                    // Instance-wide only.
                    automod_checks_per_day: None,
                })
            },
        )
        .await?
        .ok_or_else(|| Error::internal("limits row missing"))
    }

    /// What moderators turned off for someone, which outlasts their calls.
    pub async fn voice_moderation(&self, user_id: &str) -> Result<VoiceModeration> {
        let conn = self.read()?;
        let row =
            query_one(&conn, "SELECT mute, deaf, video_off FROM voice_moderation WHERE user_id = ?1", [user_id], |r| {
                Ok(VoiceModeration {
                    mute: r.get::<i64>(0)? != 0,
                    deaf: r.get::<i64>(1)? != 0,
                    video_off: r.get::<i64>(2)? != 0,
                })
            })
            .await?;
        Ok(row.unwrap_or_default())
    }

    pub async fn set_voice_moderation(&self, user_id: &str, moderation: VoiceModeration) -> Result<()> {
        self.writable()?;
        let user_id = user_id.to_owned();
        let VoiceModeration { mute, deaf, video_off } = moderation;
        db::write(&self.db, async |conn| {
            if mute || deaf || video_off {
                conn.execute(
                    "INSERT INTO voice_moderation (user_id, mute, deaf, video_off) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT (user_id) DO UPDATE SET mute = excluded.mute, deaf = excluded.deaf, \
                     video_off = excluded.video_off",
                    (user_id.as_str(), mute as i64, deaf as i64, video_off as i64),
                )
                .await?;
            } else {
                conn.execute("DELETE FROM voice_moderation WHERE user_id = ?1", [user_id.as_str()]).await?;
            }
            Ok(())
        })
        .await
    }

    /// Starts a recording of a voice channel ([`crate::recordings`]).
    pub async fn add_recording(&self, row: &RecordingRow) -> Result<()> {
        self.writable()?;
        let row = row.clone();
        db::write(&self.db, async |conn| {
            conn.execute(
                "INSERT INTO recordings (id, channel_id, started_by, started_at, sealed) VALUES (?1, ?2, ?3, ?4, ?5)",
                (row.id.as_str(), row.channel_id.as_str(), row.started_by.as_str(), row.started_at, row.sealed as i64),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// Says a recording ended, and what its tracks came to.
    pub async fn end_recording(&self, id: &str, ended_at: i64, tracks: &str, size_bytes: i64) -> Result<()> {
        let (id, tracks) = (id.to_owned(), tracks.to_owned());
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE recordings SET ended_at = ?2, tracks = ?3, size_bytes = ?4 WHERE id = ?1",
                (id.as_str(), ended_at, tracks.as_str(), size_bytes),
            )
            .await?;
            Ok(())
        })
        .await
    }

    pub async fn delete_recording(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        db::write(&self.db, async |conn| {
            conn.execute("DELETE FROM recordings WHERE id = ?1", [id.as_str()]).await?;
            Ok(())
        })
        .await
    }

    pub async fn recording(&self, id: &str) -> Result<Option<RecordingRow>> {
        let conn = self.read()?;
        query_one(&conn, &format!("SELECT {RECORDING_COLUMNS} FROM recordings WHERE id = ?1"), [id], recording_row)
            .await
    }

    /// A channel's recordings, newest first.
    pub async fn recordings(&self, channel_id: &str, limit: i64) -> Result<Vec<RecordingRow>> {
        let conn = self.read()?;
        query_all(
            &conn,
            &format!(
                "SELECT {RECORDING_COLUMNS} FROM recordings WHERE channel_id = ?1 ORDER BY started_at DESC, id DESC LIMIT ?2"
            ),
            (channel_id, limit),
            recording_row,
        )
        .await
    }

    /// What the server's finished recordings come to, in bytes.
    pub async fn recording_bytes(&self) -> Result<i64> {
        let conn = self.read()?;
        let total =
            query_one(&conn, "SELECT coalesce(sum(size_bytes), 0) FROM recordings", (), |r| r.get::<i64>(0)).await?;
        Ok(total.unwrap_or(0))
    }

    /// Finished recordings that ended before `before` (ms), oldest first.
    pub async fn recordings_ended_before(&self, before: i64, limit: i64) -> Result<Vec<RecordingRow>> {
        let conn = self.read()?;
        query_all(
            &conn,
            &format!(
                "SELECT {RECORDING_COLUMNS} FROM recordings WHERE ended_at IS NOT NULL AND ended_at < ?1 \
                 ORDER BY ended_at LIMIT ?2"
            ),
            (before, limit),
            recording_row,
        )
        .await
    }

    /// The caps in force: this server's own, else the instance defaults.
    pub async fn limits(&self, defaults: &config::Limits) -> Result<pb::ServerLimits> {
        Ok(effective_limits(self.own_limits().await?, defaults))
    }

    pub async fn set_limits(&self, limits: &pb::ServerLimits) -> Result<()> {
        self.writable()?;
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE limits SET members = ?1, channels = ?2, storage_bytes = ?3, attachment_bytes = ?4, emojis = ?5, \
                 recording_bytes = ?6 WHERE id = 1",
                (
                    limits.members,
                    limits.channels,
                    limits.storage_bytes,
                    limits.attachment_bytes,
                    limits.emojis,
                    limits.recording_bytes,
                ),
            )
            .await?;
            Ok(())
        })
        .await
    }
}

pub fn effective_limits(own: pb::ServerLimits, defaults: &config::Limits) -> pb::ServerLimits {
    pb::ServerLimits {
        members: own.members.or(defaults.members),
        channels: own.channels.or(defaults.channels),
        storage_bytes: own.storage_bytes.or(defaults.storage_bytes),
        attachment_bytes: own.attachment_bytes.or(defaults.attachment_bytes),
        emojis: own.emojis.or(defaults.emojis),
        recording_bytes: own.recording_bytes.or(defaults.recording_bytes),
        automod_checks_per_day: defaults.automod_checks_per_day,
    }
}

/// The files a server's database is made of: the database itself, the log of
/// concurrent commits not yet folded into it, and the write-ahead log.
pub(crate) const SIDECARS: [&str; 4] = ["", "-log", "-wal", "-shm"];

fn storage_bytes(path: &Path) -> i64 {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len() as i64).unwrap_or(0);
    size(path) + size(&sidecar(path, "-log")) + size(&sidecar(path, "-wal"))
}

/// What a server's file is called in the replica, wherever it's kept.
pub fn replica_name(id: &str) -> String {
    format!("servers/{id}")
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn usage_row(r: &Row) -> turso::Result<pb::ServerUsage> {
    Ok(pb::ServerUsage {
        server_id: String::new(),
        members: r.get(0)?,
        channels: r.get(1)?,
        messages: r.get(2)?,
        messages_sent: r.get(3)?,
        message_bytes: r.get(4)?,
        attachments: r.get(5)?,
        attachment_bytes: r.get(6)?,
        events: r.get(7)?,
        storage_bytes: 0,
        updated_at: Some(timestamp(r.get(8)?)),
        emojis: r.get(9)?,
        automod_checks_today: 0,
    })
}

pub async fn load_server(conn: &Connection) -> Result<pb::Server> {
    query_one(
        conn,
        "SELECT server.id, name, description, icon_url, owner_id, discoverable, created_at, server.updated_at, usage.members,
                default_notifications, system_channel_id, min_account_age_seconds, applications, linked_only, rules <> '[]', welcome,
                sso, sso_required, sso_recheck_days, region
         FROM server, usage WHERE usage.id = 1",
        (),
        |r| {
            Ok(pb::Server {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                icon_url: r.get(3)?,
                owner_id: r.get(4)?,
                discoverable: r.get(5)?,
                created_at: Some(timestamp(r.get(6)?)),
                updated_at: Some(timestamp(r.get(7)?)),
                member_count: r.get(8)?,
                default_notifications: r.get(9)?,
                system_channel_id: r.get::<Option<String>>(10)?.unwrap_or_default(),
                min_account_age_seconds: r.get(11)?,
                applications: r.get(12)?,
                linked_only: r.get(13)?,
                has_rules: r.get(14)?,
                has_welcome_screen: from_json::<StoredWelcome>(&r.get::<String>(15)?, "welcome screen").enabled,
                sso_required: r.get(17)?,
                sso_name: if r.get::<bool>(17)? { crate::sso::Provider::parse(&r.get::<String>(16)?).name } else { String::new() },
                sso_host: if r.get::<bool>(17)? { crate::sso::Provider::parse(&r.get::<String>(16)?).host() } else { String::new() },
                sso_recheck_days: r.get(18)?,
                region: r.get(19)?,
            })
        },
    )
    .await?
    .ok_or_else(|| Error::internal("server row missing"))
}

/// A server's single sign-on, as its file has it.
pub struct ServerSso {
    pub provider: crate::sso::Provider,
    pub required: bool,
    pub recheck_days: i32,
}

impl ServerSso {
    /// Whether a sign-in made at `signed_in_at` still counts at `now`.
    pub fn fresh(&self, signed_in_at: Option<i64>, now: i64) -> bool {
        match signed_in_at {
            None => false,
            Some(_) if self.recheck_days <= 0 => true,
            Some(at) => now < at + i64::from(self.recheck_days) * 24 * 60 * 60 * 1000,
        }
    }
}

pub async fn load_sso(conn: &Connection) -> Result<ServerSso> {
    query_one(conn, "SELECT sso, sso_required, sso_recheck_days FROM server", (), |r| {
        Ok(ServerSso {
            provider: crate::sso::Provider::parse(&r.get::<String>(0)?),
            required: r.get(1)?,
            recheck_days: r.get(2)?,
        })
    })
    .await?
    .ok_or_else(|| Error::internal("server row missing"))
}

/// When someone last signed in through the server's single sign-on.
/// Leaves out when a member last signed in through the server's provider,
/// unless `viewer` is that member or a manager: it's nobody else's business
/// when they're online with their organization.
pub fn scrub_sso(member: &mut pb::Member, viewer: &str, manager: bool) {
    if !manager && member.user.as_ref().is_none_or(|u| u.id != viewer) {
        member.sso_signed_in_at = None;
    }
}

pub async fn sso_signed_in_at(conn: &Connection, user_id: &str) -> Result<Option<i64>> {
    query_one(conn, "SELECT signed_in_at FROM sso_identities WHERE user_id = ?1", [user_id], |r| r.get::<i64>(0)).await
}

/// The community servers whose files this process keeps: all of them, or a
/// shard's share in a split instance.
pub struct Servers {
    dir: PathBuf,
    trash: PathBuf,
    key: Option<EncryptionKey>,
    hub: Arc<Hub>,
    open: RwLock<HashMap<String, Arc<ServerDb>>>,
    /// Part of a split instance, where a server not here may be on another shard.
    split: bool,
    /// Where every server file here is continuously copied, if anywhere.
    replica: Option<Arc<Replica>>,
    /// The region this process is in (FUWA_REGION), which every server here
    /// is in too.
    region: String,
    /// Servers moved here that aren't replicated yet: they will be once the
    /// shard they came from has let go of its copies (see cluster/moves.rs).
    unreplicated: RwLock<std::collections::HashSet<String>>,
}

/// What a new server starts with.
pub struct NewServer {
    pub name: String,
    pub description: String,
    pub icon_url: String,
    pub discoverable: bool,
}

impl Servers {
    /// No servers, for a process that keeps none.
    pub fn none(hub: Arc<Hub>) -> Self {
        Self {
            dir: PathBuf::new(),
            trash: PathBuf::new(),
            key: None,
            hub,
            open: RwLock::new(HashMap::new()),
            split: true,
            replica: None,
            region: String::new(),
            unreplicated: RwLock::default(),
        }
    }

    /// Opens every server under `<data>/servers/`, bringing each schema up to
    /// date, and replicates each to `replica`.
    pub async fn open(
        data_path: &Path,
        key: Option<EncryptionKey>,
        hub: Arc<Hub>,
        split: bool,
        replica: Option<Arc<Replica>>,
        region: &str,
    ) -> Result<Self> {
        let dir = data_path.join("servers");
        let trash = data_path.join("deleted");
        std::fs::create_dir_all(&dir)?;
        let region = region.to_string();
        let servers = Self {
            dir,
            trash,
            key,
            hub,
            open: RwLock::new(HashMap::new()),
            split,
            replica,
            region,
            unreplicated: RwLock::default(),
        };

        let mut entries: Vec<PathBuf> = std::fs::read_dir(&servers.dir)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "db"))
            .collect();
        entries.sort();
        for path in entries {
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| parse_id("server", s).ok()) else {
                tracing::warn!(path = %path.display(), "skipping a file in servers/ that isn't named after a server id");
                continue;
            };
            if let Err(err) = servers.load(&id, &path, true).await {
                tracing::error!(server = %id, error = %err, "couldn't open server database; skipping it");
            }
        }
        Ok(servers)
    }

    async fn load(&self, id: &str, path: &Path, replicate: bool) -> Result<()> {
        let sdb = Arc::new(self.open_file(id, path).await?);
        if replicate {
            self.replicate(&sdb).await?;
        } else {
            self.unreplicated.write().unwrap_or_else(|p| p.into_inner()).insert(id.to_string());
        }
        sdb.fold_usage().await?;
        let server = sdb.server().await?;
        if server.id != id {
            return Err(Error::internal(format!("file is named {id} but holds server {}", server.id)));
        }
        // Servers from before roles get theirs the first time they open.
        db::write(&sdb.db, async |conn| permissions::seed(conn, id, now_ms()).await.map(drop)).await?;
        // A file brought here from another region (moved, or copied by hand)
        // is in this one now.
        if server.region != self.region {
            let region = self.region.clone();
            db::write(&sdb.db, async |conn| {
                conn.execute("UPDATE server SET region = ?1", [region.as_str()]).await?;
                Ok(())
            })
            .await?;
        }
        self.write().insert(id.to_string(), sdb);
        Ok(())
    }

    /// Opens a server whose files were just put in servers/ (moved here from
    /// another shard), as at start but not replicated yet: see
    /// [`replicate_adopted`](Self::replicate_adopted).
    pub async fn adopt(&self, id: &str) -> Result<Arc<ServerDb>> {
        if self.holds(id) {
            return Err(Error::AlreadyExists(format!("server {id} is already here")));
        }
        if let Err(err) = self.load(id, &self.path(id), false).await {
            self.unreplicated.write().unwrap_or_else(|p| p.into_inner()).remove(id);
            return Err(err);
        }
        self.get(id).await
    }

    /// Servers moved here not replicated yet.
    pub fn unreplicated(&self) -> Vec<String> {
        self.unreplicated.read().unwrap_or_else(|p| p.into_inner()).iter().cloned().collect()
    }

    /// Starts replicating a server moved here, now that the shard it came
    /// from has let go of its replica. True if it wasn't yet.
    pub async fn replicate_adopted(&self, id: &str) -> Result<bool> {
        if !self.unreplicated.write().unwrap_or_else(|p| p.into_inner()).remove(id) {
            return Ok(false);
        }
        let Some(sdb) = self.read().get(id).cloned() else { return Ok(false) };
        if let Some(replica) = &self.replica {
            self.replicate(&sdb).await?;
            replica.sync_now(&replica_name(id)).await?;
        }
        Ok(true)
    }

    /// Every server here and who's in it, for one server.
    pub async fn entry(&self, id: &str) -> Result<cpb::ServerEntry> {
        let sdb = self.get(id).await?;
        let conn = sdb.read()?;
        let server = load_server(&conn).await?;
        let member_ids = query_all(&conn, "SELECT user_id FROM members", (), |r| r.get::<String>(0)).await?;
        let invite_codes = query_all(&conn, "SELECT code FROM invites", (), |r| r.get::<String>(0)).await?;
        Ok(cpb::ServerEntry { server: Some(server), member_ids, invite_codes })
    }

    pub fn replica(&self) -> Option<&Arc<Replica>> {
        self.replica.as_ref()
    }

    /// Lets go of a server that moved to another shard: it's no longer
    /// served here and its files are deleted, along with this process's
    /// replica of it (see [`Replica::release`]). Unlike [`delete`](Self::delete)
    /// nothing is kept here and nobody is told it's gone: it lives on elsewhere.
    pub async fn release(&self, id: &str) -> Result<()> {
        let Some(sdb) = self.write().remove(id) else { return Ok(()) };
        let replicated = !self.unreplicated.write().unwrap_or_else(|p| p.into_inner()).remove(id);
        if let Some(replica) = self.replica.as_ref().filter(|_| replicated) {
            replica.release(&replica_name(id)).await;
        }
        let _alone = sdb.db.alone().await;
        for suffix in SIDECARS {
            match std::fs::remove_file(sidecar(&sdb.path, suffix)) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
                _ => {}
            }
        }
        // Live streams following it here end, and follow it on its new shard.
        self.hub.publish([crate::hub::moved_event(id)]);
        Ok(())
    }

    /// Stops a server taking changes, for moving it (or lets it take them
    /// again). Returns whether it was frozen before.
    pub fn freeze(&self, id: &str, frozen: bool) -> bool {
        match self.read().get(id) {
            Some(sdb) => sdb.frozen.swap(frozen, Ordering::AcqRel),
            None => false,
        }
    }

    /// The servers here that are frozen for a move.
    pub fn frozen(&self) -> Vec<String> {
        self.read().values().filter(|sdb| sdb.frozen.load(Ordering::Acquire)).map(|sdb| sdb.id.clone()).collect()
    }

    /// Where a server's file is, and the files kept beside it, as they are
    /// now (with the log folded in, for a server that's taking no changes).
    pub fn files(&self, id: &str) -> Vec<PathBuf> {
        let path = self.path(id);
        SIDECARS.iter().map(|suffix| sidecar(&path, suffix)).filter(|p| p.exists()).collect()
    }

    async fn open_file(&self, id: &str, path: &Path) -> Result<ServerDb> {
        let db = Arc::new(db::open(path, self.key.as_ref(), MIGRATIONS).await?);
        Ok(ServerDb {
            id: id.to_string(),
            path: path.to_path_buf(),
            db,
            head: Mutex::new(None),
            unfolded: AtomicU32::new(0),
            folding: AtomicBool::new(false),
            hub: self.hub.clone(),
            frozen: AtomicBool::new(false),
        })
    }

    /// Starts copying a server's file to the replica, if there is one.
    async fn replicate(&self, sdb: &ServerDb) -> Result<()> {
        match &self.replica {
            Some(replica) => replica.track(&replica_name(&sdb.id), sdb.db.clone()).await,
            None => Ok(()),
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Arc<ServerDb>>> {
        self.open.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Arc<ServerDb>>> {
        self.open.write().unwrap_or_else(|p| p.into_inner())
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.db"))
    }

    /// A server by id, as a client sent it.
    pub async fn get(&self, id: &str) -> Result<Arc<ServerDb>> {
        let id = parse_id("server_id", id)?;
        match self.read().get(&id) {
            Some(sdb) => Ok(sdb.clone()),
            None if self.split => Err(Error::Misrouted),
            None => Err(Error::NotFound("server")),
        }
    }

    /// Whether this process keeps the server's file.
    pub fn holds(&self, id: &str) -> bool {
        self.read().contains_key(id)
    }

    /// Every server here and who's in it, as the directory's index takes them.
    pub async fn entries(&self) -> Result<Vec<cpb::ServerEntry>> {
        let mut entries = Vec::new();
        for sdb in self.all() {
            let conn = sdb.read()?;
            let server = load_server(&conn).await?;
            let member_ids = query_all(&conn, "SELECT user_id FROM members", (), |r| r.get::<String>(0)).await?;
            let invite_codes = query_all(&conn, "SELECT code FROM invites", (), |r| r.get::<String>(0)).await?;
            entries.push(cpb::ServerEntry { server: Some(server), member_ids, invite_codes });
        }
        Ok(entries)
    }

    /// Every server here, in id order.
    pub fn all(&self) -> Vec<Arc<ServerDb>> {
        let mut all: Vec<Arc<ServerDb>> = self.read().values().cloned().collect();
        all.sort_by(|a, b| a.id.cmp(&b.id));
        all
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Creates a server owned by `owner`, with a #general channel.
    pub async fn create(&self, owner: &pb::User, new: NewServer) -> Result<pb::Server> {
        let id = new_id();
        let path = self.path(&id);
        let sdb = Arc::new(self.open_file(&id, &path).await?);
        self.replicate(&sdb).await?;
        let region = self.region.clone();
        let created = sdb
            .write(&owner.id, async |conn, events| {
                let now = now_ms();
                let general = new_id();
                conn.execute(
                    "INSERT INTO server (id, name, description, icon_url, owner_id, discoverable, system_channel_id, created_at, updated_at, region)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
                    (id.as_str(), new.name.as_str(), new.description.as_str(), new.icon_url.as_str(), owner.id.as_str(), new.discoverable, general.as_str(), now, region.as_str()),
                )
                .await?;
                for role in permissions::seed(conn, &id, now).await? {
                    events.push(Payload::RoleCreated(pb::RoleCreated { role: Some(role) }));
                }
                let member = add_member(conn, owner, &id, now, false).await?;
                events.push(Payload::MemberJoined(pb::MemberJoined { member: Some(member) }));
                let channel = pb::Channel {
                    id: general.clone(),
                    server_id: id.clone(),
                    name: "general".into(),
                    r#type: pb::ChannelType::Text as i32,
                    parent_id: String::new(),
                    topic: String::new(),
                    position: 0,
                    created_at: Some(timestamp(now)),
                    updated_at: Some(timestamp(now)),
                    slowmode_seconds: 0,
                    permission_overwrites: vec![],
                    shared: None,
                };
                conn.execute(
                    "INSERT INTO channels (id, name, type, position, created_at, updated_at) VALUES (?1, ?2, ?3, 0, ?4, ?4)",
                    (channel.id.as_str(), channel.name.as_str(), channel.r#type as i64, now),
                )
                .await?;
                conn.execute("UPDATE usage SET channels = channels + 1 WHERE id = 1", ()).await?;
                events.push(Payload::ChannelCreated(pb::ChannelCreated { channel: Some(channel) }));
                load_server(conn).await
            })
            .await;
        let server = match created {
            Ok(server) => server,
            Err(err) => {
                if let Some(replica) = &self.replica {
                    replica.forget(&replica_name(&id), false).await;
                }
                drop(sdb);
                for suffix in SIDECARS {
                    let _ = std::fs::remove_file(sidecar(&path, suffix));
                }
                return Err(err);
            }
        };
        self.write().insert(id, sdb);
        Ok(server)
    }

    /// Deletes a server. Its file moves to `<data>/deleted/` rather than being
    /// erased, so an operator can bring it back by moving it into servers/.
    pub async fn delete(&self, id: &str, actor_id: &str) -> Result<()> {
        let sdb = self.get(id).await?;
        let id = sdb.id.clone();
        self.write().remove(&id);
        if let Some(replica) = &self.replica {
            replica.forget(&replica_name(&id), true).await;
        }
        {
            // Wait out any write in flight, then settle the logs into the main file.
            let _alone = sdb.db.alone().await;
            let _ = db::pragma(&sdb.read()?, "PRAGMA wal_checkpoint(TRUNCATE)").await;
            std::fs::create_dir_all(&self.trash)?;
            let stamp = now_ms();
            for suffix in SIDECARS {
                let from = sidecar(&sdb.path, suffix);
                if from.exists() {
                    std::fs::rename(&from, self.trash.join(format!("{id}-{stamp}.db{suffix}")))?;
                }
            }
        }
        self.hub.publish([pb::Event {
            id: new_id(),
            server_id: id,
            sequence: 0,
            actor_id: actor_id.to_string(),
            created_at: Some(timestamp(now_ms())),
            payload: Some(Payload::ServerDeleted(pb::ServerDeleted {})),
        }]);
        Ok(())
    }
}

/// Adds (or re-adds) someone to a server, keeping their profile for display.
/// They start with no roles but @everyone, and `pending` until they agree to
/// the rules.
pub async fn add_member(
    conn: &Connection,
    user: &pb::User,
    server_id: &str,
    now: i64,
    pending: bool,
) -> Result<pb::Member> {
    upsert_user(conn, user).await?;
    // `role` is from before roles and no longer read.
    conn.execute(
        "INSERT INTO members (user_id, role, joined_at, pending) VALUES (?1, 0, ?2, ?3)",
        (user.id.as_str(), now, pending),
    )
    .await?;
    conn.execute("UPDATE usage SET members = members + 1, updated_at = ?1 WHERE id = 1", [now]).await?;
    Ok(pb::Member {
        server_id: server_id.to_string(),
        user: Some(user.clone()),
        nickname: String::new(),
        joined_at: Some(timestamp(now)),
        timed_out_until: None,
        role_ids: vec![],
        pending,
        sso_signed_in_at: sso_signed_in_at(conn, &user.id).await?.map(timestamp),
    })
}

/// Takes someone out of a server, inside a write: their membership, their
/// roles, and the channel overwrites that named them, each such channel's
/// change going out as an event. False if they weren't a member.
pub async fn remove_member(
    conn: &Connection,
    server_id: &str,
    user_id: &str,
    events: &mut Vec<Payload>,
) -> Result<bool> {
    if conn.execute("DELETE FROM members WHERE user_id = ?1", [user_id]).await? == 0 {
        return Ok(false);
    }
    conn.execute("UPDATE usage SET members = members - 1, updated_at = ?1 WHERE id = 1", [now_ms()]).await?;
    conn.execute("DELETE FROM member_roles WHERE user_id = ?1", [user_id]).await?;
    conn.execute("DELETE FROM sso_identities WHERE user_id = ?1", [user_id]).await?;
    let channels = query_all(
        conn,
        "SELECT channel_id FROM channel_overwrites WHERE target_id = ?1 AND target = ?2",
        (user_id, pb::OverwriteTarget::Member as i64),
        |r| r.get::<String>(0),
    )
    .await?;
    if !channels.is_empty() {
        conn.execute(
            "DELETE FROM channel_overwrites WHERE target_id = ?1 AND target = ?2",
            (user_id, pb::OverwriteTarget::Member as i64),
        )
        .await?;
        for id in channels {
            if let Some(channel) = load_channel(conn, server_id, &id).await? {
                events.push(Payload::ChannelUpdated(pb::ChannelUpdated { channel: Some(channel) }));
            }
        }
    }
    Ok(true)
}

pub const CHANNEL_COLUMNS: &str =
    "id, name, type, parent_id, topic, position, created_at, updated_at, slowmode_seconds";

/// Reads a channel row; its overwrites come from [`permissions::attach_overwrites`].
pub fn channel_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::Channel> + '_ {
    move |r| {
        Ok(pb::Channel {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            name: r.get(1)?,
            r#type: r.get(2)?,
            parent_id: r.get::<Option<String>>(3)?.unwrap_or_default(),
            topic: r.get(4)?,
            position: r.get(5)?,
            created_at: Some(timestamp(r.get(6)?)),
            updated_at: Some(timestamp(r.get(7)?)),
            slowmode_seconds: r.get(8)?,
            permission_overwrites: vec![],
            shared: None,
        })
    }
}

/// A channel with its overwrites.
pub async fn load_channel(conn: &Connection, server_id: &str, channel_id: &str) -> Result<Option<pb::Channel>> {
    let Some(mut channel) = query_one(
        conn,
        &format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE id = ?1"),
        [channel_id],
        channel_row(server_id),
    )
    .await?
    else {
        return Ok(None);
    };
    permissions::attach_overwrites(conn, std::slice::from_mut(&mut channel)).await?;
    attach_shared(conn, std::slice::from_mut(&mut channel)).await?;
    Ok(Some(channel))
}

/// Says which channels are shared with other servers, and which show
/// another server's (docs/shared-channels.md).
pub async fn attach_shared(conn: &Connection, channels: &mut [pb::Channel]) -> Result<()> {
    let guests = query_all(
        conn,
        "SELECT channel_id, guest_server_id, guest_name, guest_icon_url FROM channel_guests WHERE active = 1 ORDER BY created_at",
        (),
        |r| Ok((r.get::<String>(0)?, pb::SharedServer { id: r.get(1)?, name: r.get(2)?, icon_url: r.get(3)? })),
    )
    .await?;
    let links = query_all(
        conn,
        "SELECT channel_id, home_server_id, home_server_name, home_server_icon_url, home_channel_name
         FROM channel_links WHERE active = 1 AND channel_id IS NOT NULL",
        (),
        |r| {
            Ok((
                r.get::<String>(0)?,
                pb::SharedServer { id: r.get(1)?, name: r.get(2)?, icon_url: r.get(3)? },
                r.get::<String>(4)?,
            ))
        },
    )
    .await?;
    if guests.is_empty() && links.is_empty() {
        return Ok(());
    }
    let this = if guests.is_empty() {
        None
    } else {
        let server = load_server(conn).await?;
        Some(pb::SharedServer { id: server.id, name: server.name, icon_url: server.icon_url })
    };
    for channel in channels.iter_mut() {
        if let Some((_, home, home_channel_name)) = links.iter().find(|(id, ..)| *id == channel.id) {
            channel.shared = Some(pb::SharedChannel {
                home: false,
                home_server: Some(home.clone()),
                home_channel_name: home_channel_name.clone(),
                guests: vec![],
            });
            continue;
        }
        let shown_in: Vec<pb::SharedServer> =
            guests.iter().filter(|(id, _)| *id == channel.id).map(|(_, server)| server.clone()).collect();
        if !shown_in.is_empty() {
            channel.shared = Some(pb::SharedChannel {
                home: true,
                home_server: this.clone(),
                home_channel_name: channel.name.clone(),
                guests: shown_in,
            });
        }
    }
    Ok(())
}

/// Every channel with its overwrites, in display order.
pub async fn load_channels(conn: &Connection, server_id: &str) -> Result<Vec<pb::Channel>> {
    let mut channels = query_all(
        conn,
        &format!("SELECT {CHANNEL_COLUMNS} FROM channels ORDER BY position, id"),
        (),
        channel_row(server_id),
    )
    .await?;
    permissions::attach_overwrites(conn, &mut channels).await?;
    attach_shared(conn, &mut channels).await?;
    Ok(channels)
}

/// Stores the latest look of a user who is or was a member.
pub async fn upsert_user(conn: &Connection, user: &pb::User) -> Result<()> {
    let status_expires_at = user.status_expires_at.as_ref().map(millis);
    let updated = conn
        .execute(
            "UPDATE users SET username = ?2, display_name = ?3, avatar_url = ?4, kind = ?5, status = ?6, status_expires_at = ?7
             WHERE id = ?1",
            (
                user.id.as_str(),
                user.username.as_str(),
                user.display_name.as_str(),
                user.avatar_url.as_str(),
                user.kind as i64,
                user.status.as_str(),
                status_expires_at,
            ),
        )
        .await?;
    if updated == 0 {
        conn.execute(
            "INSERT INTO users (id, username, display_name, avatar_url, kind, status, status_expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                user.id.as_str(),
                user.username.as_str(),
                user.display_name.as_str(),
                user.avatar_url.as_str(),
                user.kind as i64,
                user.status.as_str(),
                status_expires_at,
            ),
        )
        .await?;
    }
    Ok(())
}

/// The columns of `users` that make a `pb::User`, in the order [`user_row`] reads them.
pub const USER_COLUMNS: &str =
    "users.id, users.username, users.display_name, users.avatar_url, users.kind, users.status, users.status_expires_at";

pub fn user_row(r: &Row) -> turso::Result<pb::User> {
    Ok(pb::User {
        id: r.get(0)?,
        username: r.get(1)?,
        display_name: r.get(2)?,
        avatar_url: r.get(3)?,
        kind: r.get(4)?,
        status: r.get(5)?,
        status_expires_at: r.get::<Option<i64>>(6)?.map(timestamp),
    })
}

/// A user as they looked, if they were ever a member here.
pub async fn user(conn: &Connection, user_id: &str) -> Result<Option<pb::User>> {
    query_one(conn, &format!("SELECT {USER_COLUMNS} FROM users WHERE users.id = ?1"), [user_id], user_row).await
}

pub const MEMBER_COLUMNS: &str = "users.id, users.username, users.display_name, users.avatar_url, users.kind, users.status, users.status_expires_at, members.nickname, members.joined_at, members.timed_out_until, members.pending,
     (SELECT signed_in_at FROM sso_identities WHERE sso_identities.user_id = members.user_id)";

/// Reads a member row; their roles come from [`permissions::attach_roles`].
pub fn member_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::Member> + '_ {
    move |r| {
        Ok(pb::Member {
            server_id: server_id.to_string(),
            user: Some(user_row(r)?),
            nickname: r.get(7)?,
            joined_at: Some(timestamp(r.get(8)?)),
            timed_out_until: r.get::<Option<i64>>(9)?.map(timestamp),
            role_ids: vec![],
            pending: r.get(10)?,
            sso_signed_in_at: r.get::<Option<i64>>(11)?.map(timestamp),
        })
    }
}

/// A member with their roles.
pub async fn member(conn: &Connection, server_id: &str, user_id: &str) -> Result<Option<pb::Member>> {
    let Some(mut member) = query_one(
        conn,
        &format!(
            "SELECT {MEMBER_COLUMNS} FROM members JOIN users ON users.id = members.user_id WHERE members.user_id = ?1"
        ),
        [user_id],
        member_row(server_id),
    )
    .await?
    else {
        return Ok(None);
    };
    member.role_ids = permissions::member_role_ids(conn, user_id).await?;
    Ok(Some(member))
}

/// A member and what they can do, as the server's file has it now.
pub async fn member_access(
    conn: &Connection,
    server_id: &str,
    user_id: &str,
) -> Result<Option<(pb::Member, permissions::Access)>> {
    let Some(member) = member(conn, server_id, user_id).await? else {
        return Ok(None);
    };
    let mut access = permissions::load(conn, server_id).await?.access(user_id, &member.role_ids);
    if member.pending {
        access.hold_back();
    }
    if member.timed_out_until.as_ref().is_some_and(|until| millis(until) > now_ms()) {
        access.time_out();
    }
    let sso = load_sso(conn).await?;
    let agent = member.user.as_ref().is_some_and(|u| u.kind == pb::AccountKind::Agent as i32);
    if sso.required && !agent && !sso.fresh(member.sso_signed_in_at.as_ref().map(millis), now_ms()) {
        access.lock_out();
    }
    Ok(Some((member, access)))
}

/// One of the usage counters, read inside a write so limits hold under concurrency.
pub async fn usage_count(conn: &Connection, counter: &'static str) -> Result<i64> {
    query_one(conn, &format!("SELECT {counter} FROM usage WHERE id = 1"), (), |r| r.get::<i64>(0))
        .await?
        .ok_or_else(|| Error::internal("usage row missing"))
}

// ───────────────────────── Invites ─────────────────────────

const INVITE_COLUMNS: &str = "code, channel_id, inviter_id, max_uses, uses, expires_at, created_at";

pub fn invite_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::Invite> + '_ {
    move |r| {
        Ok(pb::Invite {
            code: r.get(0)?,
            server_id: server_id.to_string(),
            channel_id: r.get::<Option<String>>(1)?.unwrap_or_default(),
            inviter_id: r.get(2)?,
            max_uses: r.get(3)?,
            uses: r.get(4)?,
            expires_at: r.get::<Option<i64>>(5)?.map(timestamp),
            created_at: Some(timestamp(r.get(6)?)),
        })
    }
}

/// Whether an invite still lets people in at `now`.
pub fn invite_works(invite: &pb::Invite, now: i64) -> bool {
    (invite.max_uses == 0 || invite.uses < invite.max_uses)
        && invite.expires_at.as_ref().is_none_or(|t| millis(t) > now)
}

pub async fn load_invite(conn: &Connection, server_id: &str, code: &str) -> Result<Option<pb::Invite>> {
    query_one(conn, &format!("SELECT {INVITE_COLUMNS} FROM invites WHERE code = ?1"), [code], invite_row(server_id))
        .await
}

/// Every invite, newest first, including ones that no longer work.
pub async fn load_invites(conn: &Connection, server_id: &str) -> Result<Vec<pb::Invite>> {
    query_all(
        conn,
        &format!("SELECT {INVITE_COLUMNS} FROM invites ORDER BY created_at DESC, code"),
        (),
        invite_row(server_id),
    )
    .await
}

/// Deletes invites that expired, inside a write; returns their codes.
pub async fn sweep_invites(conn: &Connection, now: i64) -> Result<Vec<String>> {
    let expired =
        query_all(conn, "SELECT code FROM invites WHERE expires_at <= ?1", [now], |r| r.get::<String>(0)).await?;
    if !expired.is_empty() {
        conn.execute("DELETE FROM invites WHERE expires_at <= ?1", [now]).await?;
    }
    Ok(expired)
}

// ───────────────────────── Joining ─────────────────────────

/// A question as the server's file keeps it.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredQuestion {
    prompt: String,
    #[serde(default)]
    paragraph: bool,
    #[serde(default)]
    required: bool,
}

/// The welcome screen as the server's file keeps it.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct StoredWelcome {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    description: String,
    #[serde(default)]
    channels: Vec<StoredWelcomeChannel>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredWelcomeChannel {
    channel_id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    emoji: String,
}

/// The server's welcome screen, whole.
pub async fn load_welcome(conn: &Connection) -> Result<pb::WelcomeScreen> {
    let text = query_one(conn, "SELECT welcome FROM server", (), |r| r.get::<String>(0))
        .await?
        .ok_or_else(|| Error::internal("server row missing"))?;
    let stored: StoredWelcome = from_json(&text, "welcome screen");
    Ok(pb::WelcomeScreen {
        enabled: stored.enabled,
        description: stored.description,
        channels: stored
            .channels
            .into_iter()
            .map(|c| pb::WelcomeChannel { channel_id: c.channel_id, description: c.description, emoji: c.emoji })
            .collect(),
    })
}

/// Replaces the welcome screen, inside a write.
pub async fn save_welcome(conn: &Connection, welcome: &pb::WelcomeScreen) -> Result<()> {
    let stored = StoredWelcome {
        enabled: welcome.enabled,
        description: welcome.description.clone(),
        channels: welcome
            .channels
            .iter()
            .map(|c| StoredWelcomeChannel {
                channel_id: c.channel_id.clone(),
                description: c.description.clone(),
                emoji: c.emoji.clone(),
            })
            .collect(),
    };
    conn.execute("UPDATE server SET welcome = ?1, updated_at = ?2", (to_json(&stored)?, now_ms())).await?;
    Ok(())
}

// ───────────────────────── Emoji and AutoMod ─────────────────────────

const EMOJI_COLUMNS: &str = "id, name, url, animated, creator_id, size, created_at";

fn emoji_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::Emoji> + '_ {
    move |r| {
        Ok(pb::Emoji {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            name: r.get(1)?,
            url: r.get(2)?,
            animated: r.get(3)?,
            creator_id: r.get(4)?,
            size: r.get(5)?,
            created_at: Some(timestamp(r.get(6)?)),
        })
    }
}

/// The server's emoji, oldest first.
pub async fn load_emojis(conn: &Connection, server_id: &str) -> Result<Vec<pb::Emoji>> {
    query_all(conn, &format!("SELECT {EMOJI_COLUMNS} FROM emojis ORDER BY created_at, id"), (), emoji_row(server_id))
        .await
}

/// The server's AutoMod rules, oldest first.
/// A new, empty server file at `path`, for tests that work in one.
#[cfg(test)]
pub(crate) async fn scratch(path: &std::path::Path) -> Connection {
    let db = db::open(path, None, MIGRATIONS).await.unwrap();
    db::connect(&db).unwrap()
}

pub async fn load_automod(conn: &Connection) -> Result<Vec<pb::AutoModRule>> {
    query_all(conn, "SELECT rule FROM automod_rules ORDER BY created_at, id", (), |r| r.get::<Vec<u8>>(0))
        .await?
        .into_iter()
        .map(|bytes| Ok(pb::AutoModRule::decode(bytes.as_slice())?))
        .collect()
}

/// An answer as an application keeps it, with the question as it was asked.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredAnswer {
    question: String,
    answer: String,
}

/// JSON from the server's file; a value that doesn't parse reads as empty.
fn from_json<T: serde::de::DeserializeOwned + Default>(text: &str, what: &str) -> T {
    serde_json::from_str(text).unwrap_or_else(|err| {
        tracing::warn!(error = %err, "couldn't read the {what} in a server's file");
        T::default()
    })
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|err| Error::internal(err.to_string()))
}

/// The server's rules and questions.
pub async fn load_join_form(conn: &Connection) -> Result<pb::JoinForm> {
    let (rules, questions) =
        query_one(conn, "SELECT rules, questions FROM server", (), |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?)))
            .await?
            .ok_or_else(|| Error::internal("server row missing"))?;
    let questions: Vec<StoredQuestion> = from_json(&questions, "questions");
    Ok(pb::JoinForm {
        rules: from_json(&rules, "rules"),
        questions: questions
            .into_iter()
            .map(|q| pb::JoinQuestion { prompt: q.prompt, paragraph: q.paragraph, required: q.required })
            .collect(),
    })
}

/// Replaces the rules and questions, inside a write.
pub async fn save_join_form(conn: &Connection, form: &pb::JoinForm) -> Result<()> {
    let questions: Vec<StoredQuestion> = form
        .questions
        .iter()
        .map(|q| StoredQuestion { prompt: q.prompt.clone(), paragraph: q.paragraph, required: q.required })
        .collect();
    conn.execute(
        "UPDATE server SET rules = ?1, questions = ?2, updated_at = ?3",
        (to_json(&form.rules)?, to_json(&questions)?, now_ms()),
    )
    .await?;
    Ok(())
}

const APPLICATION_COLUMNS: &str = "users.id, users.username, users.display_name, users.avatar_url, users.kind, users.status, users.status_expires_at, applications.answers, applications.status, applications.reason, applications.account_created_at, applications.created_at, applications.reviewed_by, applications.reviewed_at";

fn application_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::Application> + '_ {
    move |r| {
        let answers: Vec<StoredAnswer> = from_json(&r.get::<String>(7)?, "answers");
        Ok(pb::Application {
            server_id: server_id.to_string(),
            user: Some(user_row(r)?),
            answers: answers
                .into_iter()
                .map(|a| pb::ApplicationAnswer { question: a.question, answer: a.answer })
                .collect(),
            status: r.get(8)?,
            reason: r.get(9)?,
            account_created_at: Some(timestamp(r.get(10)?)),
            created_at: Some(timestamp(r.get(11)?)),
            reviewed_by_id: r.get::<Option<String>>(12)?.unwrap_or_default(),
            reviewed_at: r.get::<Option<i64>>(13)?.map(timestamp),
        })
    }
}

pub async fn load_application(conn: &Connection, server_id: &str, user_id: &str) -> Result<Option<pb::Application>> {
    query_one(
        conn,
        &format!(
            "SELECT {APPLICATION_COLUMNS} FROM applications JOIN users ON users.id = applications.user_id
             WHERE applications.user_id = ?1"
        ),
        [user_id],
        application_row(server_id),
    )
    .await
}

/// Applications with this status, oldest first.
pub async fn load_applications(
    conn: &Connection,
    server_id: &str,
    status: pb::ApplicationStatus,
) -> Result<Vec<pb::Application>> {
    query_all(
        conn,
        &format!(
            "SELECT {APPLICATION_COLUMNS} FROM applications JOIN users ON users.id = applications.user_id
             WHERE applications.status = ?1 ORDER BY applications.created_at, applications.user_id"
        ),
        [status as i64],
        application_row(server_id),
    )
    .await
}

/// Files someone's application, inside a write, replacing any turned-down one.
pub async fn save_application(conn: &Connection, application: &pb::Application) -> Result<()> {
    let user = application.user.as_ref().ok_or_else(|| Error::internal("application without its applicant"))?;
    upsert_user(conn, user).await?;
    let answers: Vec<StoredAnswer> = application
        .answers
        .iter()
        .map(|a| StoredAnswer { question: a.question.clone(), answer: a.answer.clone() })
        .collect();
    conn.execute("DELETE FROM applications WHERE user_id = ?1", [user.id.as_str()]).await?;
    conn.execute(
        "INSERT INTO applications (user_id, answers, status, account_created_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            user.id.as_str(),
            to_json(&answers)?,
            application.status as i64,
            application.account_created_at.as_ref().map_or(0, millis),
            application.created_at.as_ref().map_or_else(now_ms, millis),
        ),
    )
    .await?;
    Ok(())
}

/// What reviewers hear when an application closes: who, and how, without the answers.
pub fn closed_application(server_id: &str, user: pb::User, status: pb::ApplicationStatus, actor_id: &str) -> Payload {
    let now = now_ms();
    Payload::ApplicationUpdated(pb::ApplicationUpdated {
        application: Some(pb::Application {
            server_id: server_id.to_string(),
            user: Some(user),
            status: status as i32,
            reviewed_by_id: actor_id.to_string(),
            reviewed_at: Some(timestamp(now)),
            ..Default::default()
        }),
    })
}

/// Drops someone's application, inside a write, telling reviewers it went:
/// for a ban, or an account that's gone. False if they had none.
pub async fn drop_application(
    conn: &Connection,
    server_id: &str,
    user_id: &str,
    actor_id: &str,
    events: &mut Vec<Payload>,
) -> Result<bool> {
    let Some(application) = load_application(conn, server_id, user_id).await? else { return Ok(false) };
    conn.execute("DELETE FROM applications WHERE user_id = ?1", [user_id]).await?;
    if application.status == pb::ApplicationStatus::Pending as i32 {
        let user = application.user.unwrap_or_default();
        events.push(closed_application(server_id, user, pb::ApplicationStatus::Withdrawn, actor_id));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server file as fuwa made them before concurrent writes: WAL mode,
    /// AUTOINCREMENT sequences, and a message its deleted channel left behind.
    #[tokio::test]
    async fn older_files_move_to_concurrent_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let db = turso::Builder::new_local(path.to_str().unwrap()).build().await.unwrap();
            let conn = db.connect().unwrap();
            db::pragma(&conn, "PRAGMA journal_mode = wal").await.unwrap();
            conn.execute_batch(MIGRATIONS[0]).await.unwrap();
            conn.execute_batch(
                "PRAGMA user_version = 1;
                 INSERT INTO channels (id, name, type, created_at, updated_at) VALUES ('c1', 'general', 1, 0, 0);
                 INSERT INTO messages (id, channel_id, author_id, content, size, created_at) VALUES ('m1', 'c1', 'u', 'hi', 2, 0);
                 INSERT INTO messages (id, channel_id, author_id, content, size, created_at) VALUES ('m2', 'gone', 'u', 'lost', 4, 0);
                 INSERT INTO events (id, actor_id, created_at, payload) VALUES ('e1', 'u', 0, x'');
                 INSERT INTO events (id, actor_id, created_at, payload) VALUES ('e2', 'u', 0, x'');",
            )
            .await
            .unwrap();
        }

        let db = db::open(&path, None, MIGRATIONS).await.unwrap();
        let conn = db::connect(&db).unwrap();
        let mode = query_one(&conn, "PRAGMA journal_mode", (), |r| r.get::<String>(0)).await.unwrap();
        assert_eq!(mode.as_deref(), Some("mvcc"));
        let events = query_all(&conn, "SELECT sequence, id FROM events ORDER BY sequence", (), |r| {
            Ok((r.get::<i64>(0)?, r.get::<String>(1)?))
        })
        .await
        .unwrap();
        assert_eq!(events, [(1, "e1".to_string()), (2, "e2".to_string())]);
        let messages = query_all(&conn, "SELECT id FROM messages", (), |r| r.get::<String>(0)).await.unwrap();
        assert_eq!(messages, ["m1"]);

        // The next write continues the log where it left off.
        let sdb = ServerDb {
            id: "s".into(),
            path: path.clone(),
            db: Arc::new(db),
            head: Mutex::new(None),
            unfolded: AtomicU32::new(0),
            folding: AtomicBool::new(false),
            hub: Arc::new(Hub::default()),
            frozen: AtomicBool::new(false),
        };
        sdb.write("u", async |_, events| {
            events.push(Payload::ChannelDeleted(pb::ChannelDeleted { channel_id: "c1".into() }));
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(sdb.head_sequence().await.unwrap(), 3);
    }
}
