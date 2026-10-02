//! Direct messages' store, `dms.db`: the devices taking part, their key
//! packages, who is in each conversation, and each conversation's records.
//!
//! Records are MLS messages the instance can't open (see `fuwa_e2ee`). What
//! it does is keep them in one order per conversation and take a record only
//! for the conversation's current epoch, which a commit moves on: two devices
//! committing at once can't both win, so every device sees the same group.
//! The conversation's row carries the epoch and the last sequence, and every
//! record updates it, so two writes to a conversation clash and one runs
//! again (see [`db::transaction`]) instead of both taking the same place.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex as SyncMutex};

use tokio::sync::{Mutex, broadcast};
use turso::{Connection, Database, Row, Value};

use crate::db::{self, EncryptionKey, query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb;
use crate::pb::direct_message_event::Payload;

const MIGRATIONS: &[&str] = &[include_str!("../migrations/dms/0001_init.sql")];

/// The most single-use key packages kept for one device.
pub const MAX_KEY_PACKAGES: i64 = 100;

/// How many events a watcher may fall behind before it's cut off and has to
/// catch up from the records.
const BUFFER: usize = 256;

/// A device as stored.
#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub id: String,
    pub account_id: String,
    pub session_id: String,
    pub signature_key: Vec<u8>,
    pub label: String,
    pub created_at: i64,
}

impl DeviceRow {
    pub fn to_pb(&self) -> pb::Device {
        pb::Device {
            id: self.id.clone(),
            user_id: self.account_id.clone(),
            signature_key: self.signature_key.clone(),
            label: self.label.clone(),
            created_at: Some(timestamp(self.created_at)),
        }
    }
}

const DEVICE_COLUMNS: &str = "id, account_id, session_id, signature_key, label, created_at";

fn device_row(r: &Row) -> turso::Result<DeviceRow> {
    Ok(DeviceRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        session_id: r.get(2)?,
        signature_key: r.get(3)?,
        label: r.get(4)?,
        created_at: r.get(5)?,
    })
}

/// A conversation as stored, with both people in it.
#[derive(Debug, Clone)]
pub struct ConversationRow {
    pub id: String,
    pub participants: Vec<String>,
    pub epoch: i64,
    pub last_seq: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ConversationRow {
    pub fn has(&self, account_id: &str) -> bool {
        self.participants.iter().any(|id| id == account_id)
    }

    /// The other person in it.
    pub fn partner_of(&self, account_id: &str) -> Option<&str> {
        self.participants.iter().map(String::as_str).find(|id| *id != account_id)
    }
}

/// The two people a conversation is between, as its unique `pair` column says.
fn pair(a: &str, b: &str) -> String {
    if a <= b { format!("{a}:{b}") } else { format!("{b}:{a}") }
}

fn participants_of(pair: &str) -> Vec<String> {
    pair.split(':').map(str::to_string).collect()
}

const CONVERSATION_COLUMNS: &str = "id, pair, epoch, last_seq, created_at, updated_at";

fn conversation_row(r: &Row) -> turso::Result<ConversationRow> {
    Ok(ConversationRow {
        id: r.get(0)?,
        participants: participants_of(&r.get::<String>(1)?),
        epoch: r.get(2)?,
        last_seq: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

const RECORD_COLUMNS: &str =
    "conversation_id, seq, kind, epoch, sender_id, sender_device_id, data, created_at, deleted_at";

fn record_row(r: &Row) -> turso::Result<pb::ConversationRecord> {
    Ok(pb::ConversationRecord {
        conversation_id: r.get(0)?,
        sequence: r.get(1)?,
        kind: r.get(2)?,
        epoch: r.get(3)?,
        sender_id: r.get(4)?,
        sender_device_id: r.get(5)?,
        data: r.get::<Option<Vec<u8>>>(6)?.unwrap_or_default(),
        created_at: Some(timestamp(r.get(7)?)),
        deleted_at: r.get::<Option<i64>>(8)?.map(timestamp),
    })
}

/// A key package to keep, with when it runs out (unix ms).
pub struct KeyPackage {
    pub data: Vec<u8>,
    pub expires_at: i64,
}

/// A record to add to a conversation.
pub struct NewRecord<'a> {
    pub conversation_id: &'a str,
    pub kind: pb::ConversationRecordKind,
    /// The epoch it was made in: it must be the conversation's current one.
    pub epoch: i64,
    pub sender_id: &'a str,
    pub sender_device_id: &'a str,
    pub data: &'a [u8],
    /// For a commit: the group as it leaves it.
    pub group_info: Option<&'a [u8]>,
    /// For a commit adding devices: what they join with, and which they are.
    pub welcome: Option<(&'a [u8], &'a [String])>,
}

/// What a write sends to watchers once it commits: each event, and whose it is.
type Outbox = Vec<(Vec<String>, pb::DirectMessageEvent)>;

pub struct DmDb {
    db: Database,
    /// Commits and publishes one write at a time, so watchers get each
    /// conversation's records in sequence.
    publishing: Mutex<()>,
    watchers: SyncMutex<HashMap<String, broadcast::Sender<Arc<pb::DirectMessageEvent>>>>,
}

impl DmDb {
    pub async fn open(path: &Path, key: Option<&EncryptionKey>) -> Result<Self> {
        let db = db::open(path, key, MIGRATIONS).await?;
        Ok(Self { db, publishing: Mutex::new(()), watchers: SyncMutex::default() })
    }

    fn read(&self) -> Result<Connection> {
        db::connect(&self.db)
    }

    // ───────────────────────── Devices ─────────────────────────

    /// Registers a session's device. A session has one: a new key replaces the
    /// device it had, with its key packages and welcomes. The account's devices
    /// whose sessions ended (not in `live_sessions`) go too.
    pub async fn register_device(
        &self,
        device: &DeviceRow,
        key_packages: &[KeyPackage],
        last_resort: Option<&[u8]>,
        live_sessions: &HashSet<String>,
    ) -> Result<i64> {
        db::write(&self.db, async |conn| {
            let existing = query_one(
                conn,
                &format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE id = ?1"),
                [device.id.as_str()],
                device_row,
            )
            .await?;
            if let Some(existing) = &existing
                && (existing.account_id != device.account_id || existing.session_id != device.session_id)
            {
                return Err(Error::AlreadyExists("another session registered that key; make a new one".into()));
            }
            let mut gone: Vec<String> = query_all(
                conn,
                "SELECT id, session_id FROM devices WHERE account_id = ?1",
                [device.account_id.as_str()],
                |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?)),
            )
            .await?
            .into_iter()
            .filter(|(id, session)| {
                *id != device.id && (*session == device.session_id || !live_sessions.contains(session))
            })
            .map(|(id, _)| id)
            .collect();
            gone.sort();
            delete_devices(conn, &gone).await?;
            if existing.is_some() {
                conn.execute(
                    "UPDATE devices SET label = ?2, last_resort = coalesce(?3, last_resort) WHERE id = ?1",
                    (device.id.as_str(), device.label.as_str(), last_resort.map(<[u8]>::to_vec)),
                )
                .await?;
            } else {
                conn.execute(
                    &format!("INSERT INTO devices ({DEVICE_COLUMNS}, last_resort) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
                    (
                        device.id.as_str(),
                        device.account_id.as_str(),
                        device.session_id.as_str(),
                        device.signature_key.clone(),
                        device.label.as_str(),
                        device.created_at,
                        last_resort.map(<[u8]>::to_vec),
                    ),
                )
                .await?;
            }
            add_key_packages(conn, &device.id, key_packages).await
        })
        .await
    }

    /// Adds single-use key packages for a device, up to [`MAX_KEY_PACKAGES`]
    /// in all. How many it has now.
    pub async fn add_key_packages(&self, device_id: &str, key_packages: &[KeyPackage]) -> Result<i64> {
        db::write(&self.db, async |conn| add_key_packages(conn, device_id, key_packages).await).await
    }

    /// The device a session registered, if any.
    pub async fn session_device(&self, session_id: &str) -> Result<Option<DeviceRow>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE session_id = ?1"),
            [session_id],
            device_row,
        )
        .await
    }

    /// The devices of these accounts, oldest first, signed in or not.
    pub async fn devices_of(&self, account_ids: &[&str]) -> Result<Vec<DeviceRow>> {
        if account_ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.read()?;
        query_all(
            &conn,
            &format!(
                "SELECT {DEVICE_COLUMNS} FROM devices WHERE account_id IN ({}) ORDER BY created_at, id",
                placeholders(account_ids.len())
            ),
            values(account_ids),
            device_row,
        )
        .await
    }

    /// Devices by id, those that exist.
    pub async fn devices(&self, ids: &[&str]) -> Result<Vec<DeviceRow>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.read()?;
        query_all(
            &conn,
            &format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE id IN ({})", placeholders(ids.len())),
            values(ids),
            device_row,
        )
        .await
    }

    /// Hands out one key package per device: the oldest single-use one still
    /// good, which goes, or else the device's last-resort one.
    pub async fn claim_key_packages(&self, device_ids: &[&str]) -> Result<Vec<(String, Vec<u8>)>> {
        db::write(&self.db, async |conn| {
            let now = now_ms();
            let mut claimed = Vec::with_capacity(device_ids.len());
            for &device_id in device_ids {
                let single = query_one(
                    conn,
                    "SELECT id, data FROM key_packages WHERE device_id = ?1 AND expires_at > ?2
                     ORDER BY expires_at, id LIMIT 1",
                    (device_id, now),
                    |r| Ok((r.get::<String>(0)?, r.get::<Vec<u8>>(1)?)),
                )
                .await?;
                if let Some((id, data)) = single {
                    conn.execute("DELETE FROM key_packages WHERE id = ?1", [id.as_str()]).await?;
                    claimed.push((device_id.to_string(), data));
                } else if let Some(Some(data)) =
                    query_one(conn, "SELECT last_resort FROM devices WHERE id = ?1", [device_id], |r| {
                        r.get::<Option<Vec<u8>>>(0)
                    })
                    .await?
                {
                    claimed.push((device_id.to_string(), data));
                }
            }
            Ok(claimed)
        })
        .await
    }

    /// Forgets the devices whose sessions ended (those not in `live_sessions`)
    /// and key packages that ran out. How many devices went.
    pub async fn sweep(&self, live_sessions: &HashSet<String>) -> Result<usize> {
        let conn = self.read()?;
        let gone: Vec<String> = query_all(&conn, "SELECT id, session_id FROM devices", (), |r| {
            Ok((r.get::<String>(0)?, r.get::<String>(1)?))
        })
        .await?
        .into_iter()
        .filter(|(_, session)| !live_sessions.contains(session))
        .map(|(id, _)| id)
        .collect();
        db::write(&self.db, async |conn| {
            delete_devices(conn, &gone).await?;
            conn.execute("DELETE FROM key_packages WHERE expires_at <= ?1", [now_ms()]).await?;
            Ok(())
        })
        .await?;
        Ok(gone.len())
    }

    /// Forgets a deleted account's devices. Its conversations stay, for the
    /// other person in each.
    pub async fn forget_account(&self, account_id: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            let ids =
                query_all(conn, "SELECT id FROM devices WHERE account_id = ?1", [account_id], |r| r.get(0)).await?;
            delete_devices(conn, &ids).await
        })
        .await
    }

    // ───────────────────────── Conversations ─────────────────────────

    /// The conversation between two people, started if there's none yet.
    /// Whether this call started it.
    pub async fn open_conversation(&self, by: &str, with: &str) -> Result<(ConversationRow, bool)> {
        let pair = pair(by, with);
        let found = async |conn: &Connection| {
            query_one(
                conn,
                &format!("SELECT {CONVERSATION_COLUMNS} FROM conversations WHERE pair = ?1"),
                [pair.as_str()],
                conversation_row,
            )
            .await
        };
        if let Some(conversation) = found(&self.read()?).await? {
            return Ok((conversation, false));
        }
        let now = now_ms();
        let id = new_id();
        let started = db::write(&self.db, async |conn| {
            if let Some(conversation) = found(conn).await? {
                return Ok((conversation, false));
            }
            conn.execute(
                "INSERT INTO conversations (id, pair, created_by, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                (id.as_str(), pair.as_str(), by, now),
            )
            .await?;
            for account_id in participants_of(&pair) {
                conn.execute(
                    "INSERT INTO participants (conversation_id, account_id) VALUES (?1, ?2)",
                    (id.as_str(), account_id.as_str()),
                )
                .await?;
            }
            let conversation = ConversationRow {
                id: id.clone(),
                participants: participants_of(&pair),
                epoch: 0,
                last_seq: 0,
                created_at: now,
                updated_at: now,
            };
            Ok((conversation, true))
        })
        .await;
        match started {
            // Started by someone else just now.
            Err(err) if db::is_unique_violation(&err) => match found(&self.read()?).await? {
                Some(conversation) => Ok((conversation, false)),
                None => Err(err),
            },
            started => started,
        }
    }

    pub async fn conversation(&self, id: &str) -> Result<Option<ConversationRow>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!("SELECT {CONVERSATION_COLUMNS} FROM conversations WHERE id = ?1"),
            [id],
            conversation_row,
        )
        .await
    }

    /// The conversation, if `account_id` is in it.
    pub async fn conversation_of(&self, account_id: &str, id: &str) -> Result<ConversationRow> {
        self.conversation(id)
            .await?
            .filter(|conversation| conversation.has(account_id))
            .ok_or(Error::NotFound("conversation"))
    }

    /// Every conversation someone's in, the latest first.
    pub async fn conversations(&self, account_id: &str) -> Result<Vec<ConversationRow>> {
        let conn = self.read()?;
        query_all(
            &conn,
            &format!(
                "SELECT {} FROM participants p JOIN conversations c ON c.id = p.conversation_id
                 WHERE p.account_id = ?1 ORDER BY c.updated_at DESC, c.id DESC",
                CONVERSATION_COLUMNS.split(", ").map(|column| format!("c.{column}")).collect::<Vec<_>>().join(", ")
            ),
            [account_id],
            conversation_row,
        )
        .await
    }

    /// Everyone someone has a conversation with.
    pub async fn partners(&self, account_id: &str) -> Result<HashSet<String>> {
        Ok(self
            .conversations(account_id)
            .await?
            .iter()
            .filter_map(|conversation| conversation.partner_of(account_id).map(str::to_string))
            .collect())
    }

    /// The group as the last commit left it: its epoch and group info.
    pub async fn group_info(&self, conversation_id: &str) -> Result<(i64, Option<Vec<u8>>)> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT epoch, group_info FROM conversations WHERE id = ?1", [conversation_id], |r| {
            Ok((r.get::<i64>(0)?, r.get::<Option<Vec<u8>>>(1)?))
        })
        .await?
        .unwrap_or((0, None)))
    }

    /// Up to `limit` records after `after`, oldest first, and whether there are more.
    pub async fn records(
        &self,
        conversation_id: &str,
        after: i64,
        limit: i64,
    ) -> Result<(Vec<pb::ConversationRecord>, bool)> {
        let conn = self.read()?;
        let mut records = query_all(
            &conn,
            &format!(
                "SELECT {RECORD_COLUMNS} FROM records WHERE conversation_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3"
            ),
            (conversation_id, after, limit + 1),
            record_row,
        )
        .await?;
        let more = records.len() as i64 > limit;
        records.truncate(limit as usize);
        Ok((records, more))
    }

    /// The welcomes waiting for a device.
    pub async fn welcomes(&self, device_id: &str) -> Result<Vec<pb::ConversationWelcome>> {
        let conn = self.read()?;
        query_all(
            &conn,
            "SELECT conversation_id, seq, data FROM welcomes WHERE device_id = ?1 ORDER BY created_at, conversation_id",
            [device_id],
            |r| Ok(pb::ConversationWelcome { conversation_id: r.get(0)?, sequence: r.get(1)?, data: r.get(2)? }),
        )
        .await
    }

    /// Adds a record, if it's for the conversation's current epoch; a commit
    /// moves the conversation to the next one. Both people's watchers get it.
    pub async fn append(&self, record: &NewRecord<'_>) -> Result<pb::ConversationRecord> {
        let commit = record.kind == pb::ConversationRecordKind::Commit;
        self.write(async |conn, outbox| {
            let now = now_ms();
            let moved = conn
                .execute(
                    "UPDATE conversations SET epoch = epoch + ?3, last_seq = last_seq + 1, updated_at = ?4,
                     group_info = coalesce(?5, group_info) WHERE id = ?1 AND epoch = ?2",
                    (
                        record.conversation_id,
                        record.epoch,
                        i64::from(commit),
                        now,
                        record.group_info.filter(|_| commit).map(<[u8]>::to_vec),
                    ),
                )
                .await?;
            let conversation = query_one(
                conn,
                &format!("SELECT {CONVERSATION_COLUMNS} FROM conversations WHERE id = ?1"),
                [record.conversation_id],
                conversation_row,
            )
            .await?
            .ok_or(Error::NotFound("conversation"))?;
            if moved == 0 {
                return Err(Error::FailedPrecondition(format!(
                    "the conversation is at epoch {}, not {}: catch up and try again",
                    conversation.epoch, record.epoch
                )));
            }
            let stored = pb::ConversationRecord {
                conversation_id: record.conversation_id.to_string(),
                sequence: conversation.last_seq,
                kind: record.kind as i32,
                epoch: record.epoch,
                sender_id: record.sender_id.to_string(),
                sender_device_id: record.sender_device_id.to_string(),
                data: record.data.to_vec(),
                created_at: Some(timestamp(now)),
                deleted_at: None,
            };
            conn.execute(
                &format!("INSERT INTO records ({RECORD_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)"),
                (
                    record.conversation_id,
                    stored.sequence,
                    stored.kind,
                    stored.epoch,
                    record.sender_id,
                    record.sender_device_id,
                    record.data.to_vec(),
                    now,
                ),
            )
            .await?;
            // The sender is in: whatever welcome it had here is used up.
            conn.execute(
                "DELETE FROM welcomes WHERE conversation_id = ?1 AND device_id = ?2",
                (record.conversation_id, record.sender_device_id),
            )
            .await?;
            if let Some((welcome, device_ids)) = record.welcome.filter(|_| commit) {
                for device_id in device_ids {
                    conn.execute(
                        "INSERT OR REPLACE INTO welcomes (conversation_id, device_id, seq, data, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        (record.conversation_id, device_id.as_str(), stored.sequence, welcome.to_vec(), now),
                    )
                    .await?;
                }
            }
            outbox.push((
                conversation.participants.clone(),
                pb::DirectMessageEvent { payload: Some(Payload::RecordAdded(stored.clone())) },
            ));
            Ok(stored)
        })
        .await
    }

    /// Deletes a message its sender sent: the ciphertext goes, the record stays
    /// as a gap. Deleting one already deleted does nothing.
    pub async fn delete_record(&self, account_id: &str, conversation_id: &str, sequence: i64) -> Result<()> {
        self.write(async |conn, outbox| {
            let record = query_one(
                conn,
                &format!("SELECT {RECORD_COLUMNS} FROM records WHERE conversation_id = ?1 AND seq = ?2"),
                (conversation_id, sequence),
                record_row,
            )
            .await?
            .ok_or(Error::NotFound("record"))?;
            if record.sender_id != account_id {
                return Err(Error::denied("you can only delete what you sent"));
            }
            if record.kind != pb::ConversationRecordKind::Message as i32 {
                return Err(Error::invalid("only messages can be deleted"));
            }
            if record.deleted_at.is_some() {
                return Ok(());
            }
            let now = now_ms();
            conn.execute(
                "UPDATE records SET data = NULL, deleted_at = ?3 WHERE conversation_id = ?1 AND seq = ?2",
                (conversation_id, sequence, now),
            )
            .await?;
            let participants = query_all(
                conn,
                "SELECT account_id FROM participants WHERE conversation_id = ?1",
                [conversation_id],
                |r| r.get::<String>(0),
            )
            .await?;
            let deleted =
                pb::ConversationRecord { data: Vec::new(), deleted_at: Some(timestamp(now)), ..record.clone() };
            outbox.push((participants, pb::DirectMessageEvent { payload: Some(Payload::RecordDeleted(deleted)) }));
            Ok(())
        })
        .await
    }

    // ───────────────────────── Watching ─────────────────────────

    /// Everything that happens in someone's conversations from now on.
    pub fn watch(&self, account_id: &str) -> broadcast::Receiver<Arc<pb::DirectMessageEvent>> {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        watchers.entry(account_id.to_string()).or_insert_with(|| broadcast::channel(BUFFER).0).subscribe()
    }

    /// Sends an event to everyone watching these accounts.
    pub fn publish(&self, account_ids: &[String], event: pb::DirectMessageEvent) {
        let event = Arc::new(event);
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        for account_id in account_ids {
            let idle = match watchers.get(account_id) {
                None => continue,
                Some(sender) if sender.receiver_count() == 0 => true,
                Some(sender) => sender.send(event.clone()).is_err(),
            };
            if idle {
                watchers.remove(account_id);
            }
        }
    }

    /// Runs `f` in a write transaction, then commits and sends what it put in
    /// the outbox, one write at a time so watchers get events in commit order.
    /// A clash runs `f` again (see [`db::transaction`]).
    async fn write<T>(&self, f: impl AsyncFnOnce(&Connection, &mut Outbox) -> Result<T> + Clone) -> Result<T> {
        let conn = db::connect(&self.db)?;
        let mut attempt = 0;
        loop {
            db::begin(&conn).await?;
            let mut outbox = Vec::new();
            let result = match f.clone()(&conn, &mut outbox).await {
                Ok(value) => {
                    let _one_at_a_time = self.publishing.lock().await;
                    conn.execute("COMMIT", ()).await.map_err(Error::from).map(|_| {
                        for (account_ids, event) in outbox {
                            self.publish(&account_ids, event);
                        }
                        value
                    })
                }
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
}

async fn add_key_packages(conn: &Connection, device_id: &str, key_packages: &[KeyPackage]) -> Result<i64> {
    let count = async || {
        Ok::<_, Error>(
            query_one(conn, "SELECT count(*) FROM key_packages WHERE device_id = ?1", [device_id], |r| r.get::<i64>(0))
                .await?
                .unwrap_or(0),
        )
    };
    let room = (MAX_KEY_PACKAGES - count().await?).max(0) as usize;
    for package in key_packages.iter().take(room) {
        conn.execute(
            "INSERT INTO key_packages (id, device_id, data, expires_at) VALUES (?1, ?2, ?3, ?4)",
            (new_id(), device_id, package.data.clone(), package.expires_at),
        )
        .await?;
    }
    count().await
}

async fn delete_devices(conn: &Connection, ids: &[String]) -> Result<()> {
    for id in ids {
        for sql in [
            "DELETE FROM key_packages WHERE device_id = ?1",
            "DELETE FROM welcomes WHERE device_id = ?1",
            "DELETE FROM devices WHERE id = ?1",
        ] {
            conn.execute(sql, [id.as_str()]).await?;
        }
    }
    Ok(())
}

fn placeholders(count: usize) -> String {
    (1..=count).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ")
}

fn values(items: &[&str]) -> Vec<Value> {
    items.iter().map(|item| Value::from(*item)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open() -> (tempfile::TempDir, DmDb) {
        let dir = tempfile::tempdir().unwrap();
        let dms = DmDb::open(&dir.path().join("dms.db"), None).await.unwrap();
        (dir, dms)
    }

    fn device(id: &str, account: &str, session: &str) -> DeviceRow {
        DeviceRow {
            id: id.into(),
            account_id: account.into(),
            session_id: session.into(),
            signature_key: id.as_bytes().to_vec(),
            label: String::new(),
            created_at: now_ms(),
        }
    }

    fn package(byte: u8, expires_at: i64) -> KeyPackage {
        KeyPackage { data: vec![byte], expires_at }
    }

    fn message<'a>(conversation_id: &'a str, epoch: i64) -> NewRecord<'a> {
        NewRecord {
            conversation_id,
            kind: pb::ConversationRecordKind::Message,
            epoch,
            sender_id: "a",
            sender_device_id: "da",
            data: b"ciphertext",
            group_info: None,
            welcome: None,
        }
    }

    #[tokio::test]
    async fn opening_twice_finds_the_same_conversation() {
        let (_dir, dms) = open().await;
        let (first, created) = dms.open_conversation("b", "a").await.unwrap();
        assert!(created);
        assert_eq!(first.participants, ["a", "b"]);
        let (again, created) = dms.open_conversation("a", "b").await.unwrap();
        assert!(!created);
        assert_eq!(again.id, first.id);
        assert_eq!(dms.partners("a").await.unwrap(), HashSet::from(["b".to_string()]));
    }

    #[tokio::test]
    async fn records_only_land_in_the_current_epoch() {
        let (_dir, dms) = open().await;
        let (conversation, _) = dms.open_conversation("a", "b").await.unwrap();
        let ids = ["db".to_string()];
        let commit = NewRecord {
            kind: pb::ConversationRecordKind::Commit,
            group_info: Some(b"group info"),
            welcome: Some((b"welcome", &ids)),
            ..message(&conversation.id, 0)
        };
        let first = dms.append(&commit).await.unwrap();
        assert_eq!((first.sequence, first.epoch), (1, 0));
        // Someone else's commit for the same epoch lost the race.
        let err = dms.append(&commit).await.unwrap_err();
        assert!(matches!(err, Error::FailedPrecondition(_)), "{err}");
        assert_eq!(dms.group_info(&conversation.id).await.unwrap(), (1, Some(b"group info".to_vec())));
        assert_eq!(dms.welcomes("db").await.unwrap()[0].sequence, 1);

        let err = dms.append(&message(&conversation.id, 0)).await.unwrap_err();
        assert!(matches!(err, Error::FailedPrecondition(_)), "{err}");
        let sent = dms.append(&message(&conversation.id, 1)).await.unwrap();
        assert_eq!(sent.sequence, 2);

        // Posting from the welcomed device uses its welcome up.
        dms.append(&NewRecord { sender_device_id: "db", sender_id: "b", ..message(&conversation.id, 1) })
            .await
            .unwrap();
        assert!(dms.welcomes("db").await.unwrap().is_empty());

        let (records, more) = dms.records(&conversation.id, 1, 1).await.unwrap();
        assert_eq!((records.len(), records[0].sequence, more), (1, 2, true));
    }

    #[tokio::test]
    async fn concurrent_messages_take_one_place_each() {
        let (_dir, dms) = open().await;
        let dms = Arc::new(dms);
        let (conversation, _) = dms.open_conversation("a", "b").await.unwrap();
        dms.append(&NewRecord { kind: pb::ConversationRecordKind::Commit, ..message(&conversation.id, 0) })
            .await
            .unwrap();
        let mut watcher = dms.watch("b");
        let sends = (0..8).map(|_| {
            let dms = dms.clone();
            let id = conversation.id.clone();
            tokio::spawn(async move { dms.append(&message(&id, 1)).await.unwrap().sequence })
        });
        let mut sequences: Vec<i64> = futures::future::join_all(sends).await.into_iter().map(Result::unwrap).collect();
        sequences.sort();
        assert_eq!(sequences, (2..=9).collect::<Vec<_>>());
        // Watchers get them in order.
        for expected in 2..=9 {
            let event = watcher.recv().await.unwrap();
            let Some(Payload::RecordAdded(record)) = &event.payload else { panic!("{event:?}") };
            assert_eq!(record.sequence, expected);
        }
    }

    #[tokio::test]
    async fn key_packages_go_once_then_the_last_resort() {
        let (_dir, dms) = open().await;
        let now = now_ms();
        let live = HashSet::from(["s1".to_string()]);
        let count = dms
            .register_device(
                &device("d1", "a", "s1"),
                &[package(1, now + 1000), package(2, now - 1)],
                Some(&[9]),
                &live,
            )
            .await
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(dms.claim_key_packages(&["d1", "nope"]).await.unwrap(), [("d1".to_string(), vec![1])]);
        // The expired one isn't handed out.
        assert_eq!(dms.claim_key_packages(&["d1"]).await.unwrap(), [("d1".to_string(), vec![9])]);
        assert_eq!(dms.sweep(&live).await.unwrap(), 0);
        assert_eq!(dms.add_key_packages("d1", &[]).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_session_has_one_device() {
        let (_dir, dms) = open().await;
        let live = HashSet::from(["s1".to_string(), "s2".to_string()]);
        dms.register_device(&device("d1", "a", "s1"), &[], None, &live).await.unwrap();
        dms.register_device(&device("d2", "a", "s2"), &[], None, &live).await.unwrap();
        // A new key for s1 replaces d1.
        dms.register_device(&device("d3", "a", "s1"), &[], None, &live).await.unwrap();
        let ids: Vec<String> = dms.devices_of(&["a"]).await.unwrap().into_iter().map(|d| d.id).collect();
        assert_eq!(ids, ["d2", "d3"]);
        // Another session can't take a key that's registered.
        let err = dms.register_device(&device("d3", "a", "s2"), &[], None, &live).await.unwrap_err();
        assert!(matches!(err, Error::AlreadyExists(_)), "{err}");
        // s2 ended.
        assert_eq!(dms.sweep(&HashSet::from(["s1".to_string()])).await.unwrap(), 1);
        assert_eq!(dms.session_device("s1").await.unwrap().unwrap().id, "d3");
        dms.forget_account("a").await.unwrap();
        assert!(dms.devices_of(&["a"]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn only_the_sender_deletes_a_message() {
        let (_dir, dms) = open().await;
        let (conversation, _) = dms.open_conversation("a", "b").await.unwrap();
        dms.append(&NewRecord { kind: pb::ConversationRecordKind::Commit, ..message(&conversation.id, 0) })
            .await
            .unwrap();
        let sent = dms.append(&message(&conversation.id, 1)).await.unwrap();
        let err = dms.delete_record("b", &conversation.id, sent.sequence).await.unwrap_err();
        assert!(matches!(err, Error::PermissionDenied(_)), "{err}");
        let err = dms.delete_record("a", &conversation.id, 1).await.unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)), "{err}");
        dms.delete_record("a", &conversation.id, sent.sequence).await.unwrap();
        let (records, _) = dms.records(&conversation.id, 1, 10).await.unwrap();
        assert!(records[0].data.is_empty() && records[0].deleted_at.is_some());
    }
}
