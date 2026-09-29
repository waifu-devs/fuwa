//! Community servers: one Turso database file each, under `<data>/servers/`.
//!
//! A server's file holds everything about it (profile, members, channels,
//! messages, its event log, usage counters and caps), so a server can be backed
//! up or moved by copying one file. The instance keeps an in-memory index of
//! every server and who belongs where, rebuilt from the files at startup.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use prost::Message as _;
use tokio::sync::Mutex;
use turso::{Connection, Database, Row};

use crate::config;
use crate::db::{self, EncryptionKey, query_all, query_one};
use crate::error::{Error, Result};
use crate::hub::Hub;
use crate::id::{new_id, now_ms, parse_id, timestamp};
use crate::pb;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/server/0001_init.sql"),
    include_str!("../migrations/server/0002_concurrent_writes.sql"),
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
        "SELECT server.id, name, description, icon_url, owner_id, discoverable, created_at, server.updated_at, usage.members
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
            })
        },
    )
    .await?
    .ok_or_else(|| Error::internal("server row missing"))
}

/// Every community server on the instance.
pub struct Servers {
    dir: PathBuf,
    trash: PathBuf,
    key: Option<EncryptionKey>,
    hub: Arc<Hub>,
    open: Mutex<HashMap<String, Arc<ServerDb>>>,
    index: RwLock<Index>,
}

#[derive(Default)]
struct Index {
    servers: HashMap<String, pb::Server>,
    /// Account id to the ids of the servers it's a member of.
    memberships: HashMap<String, BTreeSet<String>>,
}

/// What a new server starts with.
pub struct NewServer {
    pub name: String,
    pub description: String,
    pub icon_url: String,
    pub discoverable: bool,
}

impl Servers {
    /// Opens every server under `<data>/servers/`, bringing each schema up to date.
    pub async fn open(data_path: &Path, key: Option<EncryptionKey>, hub: Arc<Hub>) -> Result<Self> {
        let dir = data_path.join("servers");
        let trash = data_path.join("deleted");
        std::fs::create_dir_all(&dir)?;
        let servers =
            Self { dir, trash, key, hub, open: Mutex::new(HashMap::new()), index: RwLock::new(Index::default()) };

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
        let conn = sdb.read()?;
        let server = load_server(&conn).await?;
        if server.id != id {
            return Err(Error::internal(format!("file is named {id} but holds server {}", server.id)));
        }
        let members = query_all(&conn, "SELECT user_id FROM members", (), |r| r.get::<String>(0)).await?;
        {
            let mut index = self.index.write().unwrap_or_else(|p| p.into_inner());
            for member in members {
                index.memberships.entry(member).or_default().insert(id.to_string());
            }
            index.servers.insert(id.to_string(), server);
        }
        self.open.lock().await.insert(id.to_string(), sdb);
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

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.db"))
    }

    /// An open server by id, as a client sent it.
    pub async fn get(&self, id: &str) -> Result<Arc<ServerDb>> {
        let id = parse_id("server_id", id)?;
        if !self.index.read().unwrap_or_else(|p| p.into_inner()).servers.contains_key(&id) {
            return Err(Error::NotFound("server"));
        }
        let mut open = self.open.lock().await;
        if let Some(sdb) = open.get(&id) {
            return Ok(sdb.clone());
        }
        let sdb = Arc::new(self.open_file(&id, &self.path(&id)).await?);
        open.insert(id, sdb.clone());
        Ok(sdb)
    }

    /// Creates a server owned by `owner`, with a #general channel.
    pub async fn create(&self, owner: &pb::User, new: NewServer) -> Result<pb::Server> {
        let id = new_id();
        let path = self.path(&id);
        let sdb = Arc::new(self.open_file(&id, &path).await?);
        let created = sdb
            .write(&owner.id, async |conn, events| {
                let now = now_ms();
                conn.execute(
                    "INSERT INTO server (id, name, description, icon_url, owner_id, discoverable, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    (id.as_str(), new.name.as_str(), new.description.as_str(), new.icon_url.as_str(), owner.id.as_str(), new.discoverable, now),
                )
                .await?;
                let member = add_member(conn, owner, pb::MemberRole::Owner, &id, now).await?;
                events.push(Payload::MemberJoined(pb::MemberJoined { member: Some(member) }));
                let channel = pb::Channel {
                    id: new_id(),
                    server_id: id.clone(),
                    name: "general".into(),
                    r#type: pb::ChannelType::Text as i32,
                    parent_id: String::new(),
                    topic: String::new(),
                    position: 0,
                    created_at: Some(timestamp(now)),
                    updated_at: Some(timestamp(now)),
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
        {
            let mut index = self.index.write().unwrap_or_else(|p| p.into_inner());
            index.servers.insert(id.clone(), server.clone());
            index.memberships.entry(owner.id.clone()).or_default().insert(id.clone());
        }
        self.open.lock().await.insert(id, sdb);
        Ok(server)
    }

    /// Deletes a server. Its file moves to `<data>/deleted/` rather than being
    /// erased, so an operator can bring it back by moving it into servers/.
    pub async fn delete(&self, id: &str, actor_id: &str) -> Result<()> {
        let sdb = self.get(id).await?;
        let id = sdb.id.clone();
        {
            let mut index = self.index.write().unwrap_or_else(|p| p.into_inner());
            index.servers.remove(&id);
            for servers in index.memberships.values_mut() {
                servers.remove(&id);
            }
        }
        self.open.lock().await.remove(&id);
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

    pub fn summary(&self, id: &str) -> Option<pb::Server> {
        self.index.read().unwrap_or_else(|p| p.into_inner()).servers.get(id).cloned()
    }

    /// Records a server's new profile after a committed change.
    pub fn index_server(&self, server: pb::Server) {
        let mut index = self.index.write().unwrap_or_else(|p| p.into_inner());
        if index.servers.contains_key(&server.id) {
            index.servers.insert(server.id.clone(), server);
        }
    }

    pub fn index_join(&self, account_id: &str, server_id: &str) {
        let mut index = self.index.write().unwrap_or_else(|p| p.into_inner());
        index.memberships.entry(account_id.to_string()).or_default().insert(server_id.to_string());
        if let Some(server) = index.servers.get_mut(server_id) {
            server.member_count += 1;
        }
    }

    pub fn index_leave(&self, account_id: &str, server_id: &str) {
        let mut index = self.index.write().unwrap_or_else(|p| p.into_inner());
        if let Some(servers) = index.memberships.get_mut(account_id) {
            servers.remove(server_id);
        }
        if let Some(server) = index.servers.get_mut(server_id) {
            server.member_count = (server.member_count - 1).max(0);
        }
    }

    /// Ids of the servers an account is a member of.
    pub fn joined_ids(&self, account_id: &str) -> Vec<String> {
        let index = self.index.read().unwrap_or_else(|p| p.into_inner());
        index.memberships.get(account_id).map(|ids| ids.iter().cloned().collect()).unwrap_or_default()
    }

    /// The servers an account is a member of, oldest first.
    pub fn joined(&self, account_id: &str) -> Vec<pb::Server> {
        let index = self.index.read().unwrap_or_else(|p| p.into_inner());
        let Some(ids) = index.memberships.get(account_id) else { return vec![] };
        ids.iter().filter_map(|id| index.servers.get(id).cloned()).collect()
    }

    pub fn is_member(&self, account_id: &str, server_id: &str) -> bool {
        let index = self.index.read().unwrap_or_else(|p| p.into_inner());
        index.memberships.get(account_id).is_some_and(|ids| ids.contains(server_id))
    }

    /// Discoverable servers, busiest first.
    pub fn discoverable(&self) -> Vec<pb::Server> {
        let index = self.index.read().unwrap_or_else(|p| p.into_inner());
        let mut servers: Vec<pb::Server> = index.servers.values().filter(|s| s.discoverable).cloned().collect();
        servers.sort_by(|a, b| b.member_count.cmp(&a.member_count).then_with(|| a.id.cmp(&b.id)));
        servers
    }

    pub fn owned_count(&self, account_id: &str) -> i64 {
        let index = self.index.read().unwrap_or_else(|p| p.into_inner());
        index.servers.values().filter(|s| s.owner_id == account_id).count() as i64
    }

    pub fn ids(&self) -> Vec<String> {
        let index = self.index.read().unwrap_or_else(|p| p.into_inner());
        let mut ids: Vec<String> = index.servers.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn count(&self) -> (i64, i64) {
        let index = self.index.read().unwrap_or_else(|p| p.into_inner());
        (index.servers.len() as i64, index.servers.values().filter(|s| s.discoverable).count() as i64)
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
    })
}

/// Stores the latest look of a user who is or was a member.
pub async fn upsert_user(conn: &Connection, user: &pb::User) -> Result<()> {
    let updated = conn
        .execute(
            "UPDATE users SET username = ?2, display_name = ?3, avatar_url = ?4, kind = ?5 WHERE id = ?1",
            (
                user.id.as_str(),
                user.username.as_str(),
                user.display_name.as_str(),
                user.avatar_url.as_str(),
                user.kind as i64,
            ),
        )
        .await?;
    if updated == 0 {
        conn.execute(
            "INSERT INTO users (id, username, display_name, avatar_url, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                user.id.as_str(),
                user.username.as_str(),
                user.display_name.as_str(),
                user.avatar_url.as_str(),
                user.kind as i64,
            ),
        )
        .await?;
    }
    Ok(())
}

pub const MEMBER_COLUMNS: &str = "users.id, users.username, users.display_name, users.avatar_url, users.kind, members.nickname, members.role, members.joined_at";

pub fn member_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::Member> + '_ {
    move |r| {
        Ok(pb::Member {
            server_id: server_id.to_string(),
            user: Some(pb::User {
                id: r.get(0)?,
                username: r.get(1)?,
                display_name: r.get(2)?,
                avatar_url: r.get(3)?,
                kind: r.get(4)?,
            }),
            nickname: r.get(5)?,
            role: r.get(6)?,
            joined_at: Some(timestamp(r.get(7)?)),
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
