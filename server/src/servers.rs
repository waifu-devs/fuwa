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
use turso::{Connection, Database, Row};

use crate::config;
use crate::cpb;
use crate::db::{self, EncryptionKey, query_all, query_one};
use crate::error::{Error, Result};
use crate::hub::Hub;
use crate::id::{millis, new_id, now_ms, parse_id, timestamp};
use crate::pb;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/server/0001_init.sql"),
    include_str!("../migrations/server/0002_concurrent_writes.sql"),
    include_str!("../migrations/server/0003_status.sql"),
    include_str!("../migrations/server/0004_moderation.sql"),
];

pub type Payload = pb::event::Payload;

/// One open community server database.
pub struct ServerDb {
    pub id: String,
    path: PathBuf,
    db: Database,
    /// Writes hold this shared, so they run side by side. The few that sweep
    /// rows other writes may be adding to (deleting a channel and its
    /// messages, deleting the server) hold it alone.
    gate: tokio::sync::RwLock<()>,
    /// The event log's last sequence, or `None` to read it again from the file.
    /// Held from handing out a write's sequences until it has committed and
    /// published them, so the log commits and goes out in sequence order.
    head: Mutex<Option<i64>>,
    /// Writes since the usage changes were last folded into the totals.
    unfolded: AtomicU32,
    folding: AtomicBool,
    hub: Arc<Hub>,
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
        "INSERT INTO audit (id, actor_id, action, target_id, channel_name, reason, changes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            new_id(),
            actor_id,
            entry.action as i64,
            entry.target_id.as_str(),
            entry.channel_name.as_str(),
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
        let shared = self.gate.read().await;
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
        let _alone = self.gate.write().await;
        self.run(actor_id, f).await
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
            let _alone = self.gate.write().await;
            let conn = self.read()?;
            conn.execute(&format!("VACUUM INTO '{path}'"), ()).await?;
        }
        db::to_sqlite(dest, None).await?;
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
                    max(u.updated_at, c.at)
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
            "SELECT members, channels, storage_bytes, attachment_bytes FROM limits WHERE id = 1",
            (),
            |r| {
                Ok(pb::ServerLimits {
                    members: r.get(0)?,
                    channels: r.get(1)?,
                    storage_bytes: r.get(2)?,
                    attachment_bytes: r.get(3)?,
                })
            },
        )
        .await?
        .ok_or_else(|| Error::internal("limits row missing"))
    }

    /// The caps in force: this server's own, else the instance defaults.
    pub async fn limits(&self, defaults: &config::Limits) -> Result<pb::ServerLimits> {
        Ok(effective_limits(self.own_limits().await?, defaults))
    }

    pub async fn set_limits(&self, limits: &pb::ServerLimits) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE limits SET members = ?1, channels = ?2, storage_bytes = ?3, attachment_bytes = ?4 WHERE id = 1",
                (limits.members, limits.channels, limits.storage_bytes, limits.attachment_bytes),
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
    }
}

/// The files a server's database is made of: the database itself, the log of
/// concurrent commits not yet folded into it, and the write-ahead log.
const SIDECARS: [&str; 4] = ["", "-log", "-wal", "-shm"];

fn storage_bytes(path: &Path) -> i64 {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len() as i64).unwrap_or(0);
    size(path) + size(&sidecar(path, "-log")) + size(&sidecar(path, "-wal"))
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
    })
}

pub async fn load_server(conn: &Connection) -> Result<pb::Server> {
    query_one(
        conn,
        "SELECT server.id, name, description, icon_url, owner_id, discoverable, created_at, server.updated_at, usage.members,
                default_notifications, system_channel_id
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
            })
        },
    )
    .await?
    .ok_or_else(|| Error::internal("server row missing"))
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
        }
    }

    /// Opens every server under `<data>/servers/`, bringing each schema up to date.
    pub async fn open(data_path: &Path, key: Option<EncryptionKey>, hub: Arc<Hub>, split: bool) -> Result<Self> {
        let dir = data_path.join("servers");
        let trash = data_path.join("deleted");
        std::fs::create_dir_all(&dir)?;
        let servers = Self { dir, trash, key, hub, open: RwLock::new(HashMap::new()), split };

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
            if let Err(err) = servers.load(&id, &path).await {
                tracing::error!(server = %id, error = %err, "couldn't open server database; skipping it");
            }
        }
        Ok(servers)
    }

    async fn load(&self, id: &str, path: &Path) -> Result<()> {
        let sdb = Arc::new(self.open_file(id, path).await?);
        sdb.fold_usage().await?;
        let server = sdb.server().await?;
        if server.id != id {
            return Err(Error::internal(format!("file is named {id} but holds server {}", server.id)));
        }
        self.write().insert(id.to_string(), sdb);
        Ok(())
    }

    async fn open_file(&self, id: &str, path: &Path) -> Result<ServerDb> {
        let db = db::open(path, self.key.as_ref(), MIGRATIONS).await?;
        Ok(ServerDb {
            id: id.to_string(),
            path: path.to_path_buf(),
            db,
            gate: tokio::sync::RwLock::new(()),
            head: Mutex::new(None),
            unfolded: AtomicU32::new(0),
            folding: AtomicBool::new(false),
            hub: self.hub.clone(),
        })
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
            entries.push(cpb::ServerEntry { server: Some(server), member_ids });
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
        let created = sdb
            .write(&owner.id, async |conn, events| {
                let now = now_ms();
                let general = new_id();
                conn.execute(
                    "INSERT INTO server (id, name, description, icon_url, owner_id, discoverable, system_channel_id, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                    (id.as_str(), new.name.as_str(), new.description.as_str(), new.icon_url.as_str(), owner.id.as_str(), new.discoverable, general.as_str(), now),
                )
                .await?;
                let member = add_member(conn, owner, pb::MemberRole::Owner, &id, now).await?;
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
        {
            // Wait out any write in flight, then settle the logs into the main file.
            let _alone = sdb.gate.write().await;
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
pub async fn add_member(
    conn: &Connection,
    user: &pb::User,
    role: pb::MemberRole,
    server_id: &str,
    now: i64,
) -> Result<pb::Member> {
    upsert_user(conn, user).await?;
    conn.execute(
        "INSERT INTO members (user_id, role, joined_at) VALUES (?1, ?2, ?3)",
        (user.id.as_str(), role as i64, now),
    )
    .await?;
    conn.execute("UPDATE usage SET members = members + 1, updated_at = ?1 WHERE id = 1", [now]).await?;
    Ok(pb::Member {
        server_id: server_id.to_string(),
        user: Some(user.clone()),
        nickname: String::new(),
        role: role as i32,
        joined_at: Some(timestamp(now)),
        timed_out_until: None,
    })
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

pub const MEMBER_COLUMNS: &str = "users.id, users.username, users.display_name, users.avatar_url, users.kind, users.status, users.status_expires_at, members.nickname, members.role, members.joined_at, members.timed_out_until";

pub fn member_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::Member> + '_ {
    move |r| {
        Ok(pb::Member {
            server_id: server_id.to_string(),
            user: Some(user_row(r)?),
            nickname: r.get(7)?,
            role: r.get(8)?,
            joined_at: Some(timestamp(r.get(9)?)),
            timed_out_until: r.get::<Option<i64>>(10)?.map(timestamp),
        })
    }
}

pub async fn member(conn: &Connection, server_id: &str, user_id: &str) -> Result<Option<pb::Member>> {
    query_one(
        conn,
        &format!(
            "SELECT {MEMBER_COLUMNS} FROM members JOIN users ON users.id = members.user_id WHERE members.user_id = ?1"
        ),
        [user_id],
        member_row(server_id),
    )
    .await
}

/// One of the usage counters, read inside a write so limits hold under concurrency.
pub async fn usage_count(conn: &Connection, counter: &'static str) -> Result<i64> {
    query_one(conn, &format!("SELECT {counter} FROM usage WHERE id = 1"), (), |r| r.get::<i64>(0))
        .await?
        .ok_or_else(|| Error::internal("usage row missing"))
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
            db,
            gate: tokio::sync::RwLock::new(()),
            head: Mutex::new(None),
            unfolded: AtomicU32::new(0),
            folding: AtomicBool::new(false),
            hub: Arc::new(Hub::default()),
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
