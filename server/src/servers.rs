//! Community servers: one Turso database file each, under `<data>/servers/`.
//!
//! A server's file holds everything about it (profile, members, channels,
//! messages, its event log, usage counters and caps), so a server can be backed
//! up or moved by copying one file. The instance keeps an in-memory index of
//! every server and who belongs where, rebuilt from the files at startup.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
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

const MIGRATIONS: &[&str] = &[include_str!("../migrations/server/0001_init.sql")];

pub type Payload = pb::event::Payload;

/// One open community server database.
pub struct ServerDb {
    pub id: String,
    path: PathBuf,
    db: Database,
    writer: Mutex<Connection>,
    hub: Arc<Hub>,
}

impl ServerDb {
    /// A connection for reading.
    pub fn read(&self) -> Result<Connection> {
        db::connect(&self.db)
    }

    /// Runs a change in one transaction. `f` makes its changes and pushes an
    /// event payload for each; the events are appended to the server's log in the
    /// same transaction and sent to subscribers once it commits. Writes to one
    /// server run one at a time, so events go out in sequence order.
    pub async fn write<T>(
        &self,
        actor_id: &str,
        f: impl AsyncFnOnce(&Connection, &mut Vec<Payload>) -> Result<T>,
    ) -> Result<T> {
        let conn = self.writer.lock().await;
        let mut events = Vec::new();
        let recorded = &mut events;
        let server_id = self.id.as_str();
        let value = db::transaction(&conn, async move |conn| {
            let mut payloads = Vec::new();
            let value = f(conn, &mut payloads).await?;
            let now = now_ms();
            for payload in payloads {
                let mut event = pb::Event {
                    id: new_id(),
                    server_id: server_id.to_string(),
                    sequence: 0,
                    actor_id: actor_id.to_string(),
                    created_at: Some(timestamp(now)),
                    payload: Some(payload),
                };
                event.sequence = query_one(
                    conn,
                    "INSERT INTO events (id, actor_id, created_at, payload) VALUES (?1, ?2, ?3, ?4) RETURNING sequence",
                    (event.id.as_str(), actor_id, now, event.encode_to_vec()),
                    |r| r.get::<i64>(0),
                )
                .await?
                .ok_or_else(|| Error::internal("appending an event returned no sequence"))?;
                recorded.push(event);
            }
            if !recorded.is_empty() {
                conn.execute(
                    "UPDATE usage SET events = events + ?1, updated_at = ?2 WHERE id = 1",
                    (recorded.len() as i64, now),
                )
                .await?;
            }
            Ok(value)
        })
        .await?;
        self.hub.publish(events);
        Ok(value)
    }

    /// Bytes this server takes on disk: its database and write-ahead log.
    pub fn storage_bytes(&self) -> i64 {
        storage_bytes(&self.path)
    }

    /// The sequence of the last event committed, or 0 for none.
    pub async fn head_sequence(&self) -> Result<i64> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT coalesce(max(sequence), 0) FROM events", (), |r| r.get::<i64>(0))
            .await?
            .unwrap_or(0))
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

    pub async fn usage(&self) -> Result<pb::ServerUsage> {
        let conn = self.read()?;
        let mut usage = query_one(&conn, &format!("SELECT {USAGE_COLUMNS} FROM usage WHERE id = 1"), (), usage_row)
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
        let conn = self.writer.lock().await;
        conn.execute(
            "UPDATE limits SET members = ?1, channels = ?2, storage_bytes = ?3, attachment_bytes = ?4 WHERE id = 1",
            (limits.members, limits.channels, limits.storage_bytes, limits.attachment_bytes),
        )
        .await?;
        Ok(())
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

fn storage_bytes(path: &Path) -> i64 {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len() as i64).unwrap_or(0);
    size(path) + size(&sidecar(path, "-wal"))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

const USAGE_COLUMNS: &str =
    "members, channels, messages, messages_sent, message_bytes, attachments, attachment_bytes, events, updated_at";

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
        let writer = Mutex::new(db::connect(&db)?);
        Ok(ServerDb { id: id.to_string(), path: path.to_path_buf(), db, writer, hub: self.hub.clone() })
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
                for file in [path.clone(), sidecar(&path, "-wal"), sidecar(&path, "-shm")] {
                    let _ = std::fs::remove_file(file);
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
            // Wait out any write in flight, then settle the log into the main file.
            let conn = sdb.writer.lock().await;
            let _ = db::pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)").await;
            std::fs::create_dir_all(&self.trash)?;
            let stamp = now_ms();
            for suffix in ["", "-wal", "-shm"] {
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
