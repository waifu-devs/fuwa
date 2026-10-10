//! The instance's own database: accounts, sessions and settings.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use prost::Message as _;
use tokio::sync::Mutex;
use turso::{Connection, Row};

use crate::db::{self, Db, EncryptionKey, query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::media::MediaRow;
use crate::pb;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/node/0001_init.sql"),
    include_str!("../migrations/node/0002_settings.sql"),
    include_str!("../migrations/node/0003_accounts.sql"),
    include_str!("../migrations/node/0004_admin.sql"),
    include_str!("../migrations/node/0005_media.sql"),
    include_str!("../migrations/node/0006_cluster.sql"),
    include_str!("../migrations/node/0007_linked_sign_ins.sql"),
    include_str!("../migrations/node/0008_agents.sql"),
    include_str!("../migrations/node/0009_upload_days.sql"),
    include_str!("../migrations/node/0010_sso.sql"),
    include_str!("../migrations/node/0011_regions.sql"),
    include_str!("../migrations/node/0012_federation.sql"),
    include_str!("../migrations/node/0013_profile_effects.sql"),
    include_str!("../migrations/node/0014_server_arrangements.sql"),
    include_str!("../migrations/node/0018_gifs.sql"),
    include_str!("../migrations/node/0019_attachment_days.sql"),
    include_str!("../migrations/node/0020_friends.sql"),
    include_str!("../migrations/node/0021_presence.sql"),
    include_str!("../migrations/node/0022_federation_moves.sql"),
    include_str!("../migrations/node/0023_sign_in_providers.sql"),
    include_str!("../migrations/node/0024_profile_items.sql"),
    include_str!("../migrations/node/0025_featured_servers.sql"),
    include_str!("../migrations/node/0026_agent_endpoints.sql"),
];

/// Notes that the instance's profile items changed now (`Node.profile_items_at`).
async fn items_changed(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('profile_items_at', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [now_ms().to_string()],
    )
    .await?;
    Ok(())
}

/// A server being moved from one shard to another (docs/regions.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub server_id: String,
    pub from: String,
    pub to: String,
    /// The server is open on `to` and placed there: only `from` letting go is left.
    pub committed: bool,
}

/// A day, for counting uploads.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How long a session lasts after sign-in.
pub const SESSION_TTL_MS: i64 = 60 * 24 * 60 * 60 * 1000;

/// How often an account's last-seen time is written, at most.
const SEEN_RESOLUTION_MS: i64 = 60 * 60 * 1000;

/// How often a session's last-active time is written, at most.
pub const ACTIVE_RESOLUTION_MS: i64 = 5 * 60 * 1000;

/// How long a sign-in waits for its two-step code.
pub const TICKET_TTL_MS: i64 = 5 * 60 * 1000;

/// Wrong two-step codes one sign-in may try before it has to start over.
pub const TICKET_ATTEMPTS: i64 = 5;

/// The longest a User-Agent is kept.
const MAX_USER_AGENT: usize = 512;

#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub kind: pb::AccountKind,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
    pub admin: bool,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub status: String,
    pub status_expires_at: Option<i64>,
    /// Signing in takes a code from an authenticator app too.
    pub two_factor: bool,
    /// Turned off by an instance admin.
    pub disabled: bool,
    /// The decoration around their avatar: one of the instance's profile
    /// items, or empty.
    pub decoration_id: String,
}

impl Account {
    pub fn user(&self) -> pb::User {
        pb::User {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            avatar_url: self.avatar_url.clone(),
            kind: self.kind as i32,
            status: self.status.clone(),
            status_expires_at: self.status_expires_at.map(timestamp),
            decoration_id: self.decoration_id.clone(),
        }
    }

    pub fn has_password(&self) -> bool {
        self.kind == pb::AccountKind::Local
    }
}

const ACCOUNT_COLUMNS: &str = "id, kind, username, display_name, avatar_url, admin, created_at, last_seen_at, status, status_expires_at, totp_secret IS NOT NULL, disabled_at IS NOT NULL, profile_decoration";

/// [`ACCOUNT_COLUMNS`] for a query that joins accounts to another table.
fn account_columns_of(table: &str) -> String {
    ACCOUNT_COLUMNS.split(", ").map(|c| format!("{table}.{c}")).collect::<Vec<_>>().join(", ")
}

const ACCOUNT_COLUMN_COUNT: usize = 13;

fn account(row: &Row) -> turso::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        kind: pb::AccountKind::try_from(row.get::<i32>(1)?).unwrap_or(pb::AccountKind::Unspecified),
        username: row.get(2)?,
        display_name: row.get(3)?,
        avatar_url: row.get(4)?,
        admin: row.get(5)?,
        created_at: row.get(6)?,
        last_seen_at: row.get(7)?,
        status: row.get(8)?,
        status_expires_at: row.get(9)?,
        two_factor: row.get(10)?,
        disabled: row.get(11)?,
        decoration_id: row.get(12)?,
    })
}

/// A sign-in through waifu.dev waiting for its code.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedSignIn {
    pub state: String,
    pub secret_hash: String,
    pub verifier: String,
    pub issuer: String,
    pub client_id: String,
    pub return_origin: String,
}

/// Someone new from waifu.dev, as their linked account starts out.
#[derive(Debug, Clone)]
pub struct NewLinkedAccount<'a> {
    /// Linked (waifu.dev) or SSO (the instance's identity provider).
    pub kind: pb::AccountKind,
    pub issuer: &'a str,
    pub subject: &'a str,
    /// Tried as it is, then with _2, _3, … until one is free.
    pub username_base: &'a str,
    pub display_name: &'a str,
    pub avatar_url: &'a str,
}

/// Someone new from a sign-in provider, as their account starts out.
#[derive(Debug, Clone)]
pub struct NewProviderAccount<'a> {
    pub provider: &'a str,
    pub subject: &'a str,
    /// Their name there, kept with the link (shown only to them).
    pub name: &'a str,
    /// Checked, lowercase, and taken as it is.
    pub username: &'a str,
    pub display_name: &'a str,
}

/// A provider linked to an account.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedProvider {
    pub provider: String,
    pub name: String,
    pub linked_at: i64,
}

/// What a profile change sets; `None` leaves a field as it is.
#[derive(Debug, Default, Clone)]
pub struct ProfileChange {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub pronouns: Option<String>,
    pub bio: Option<String>,
    pub banner_url: Option<String>,
    /// `Some(None)` clears it.
    pub accent_color: Option<Option<i32>>,
    /// The status and when it runs out, set together.
    pub status: Option<(String, Option<i64>)>,
    /// A profile effect's id; empty for none.
    pub effect: Option<String>,
    /// One of the instance's decorations; empty for none.
    pub decoration: Option<String>,
}

/// A device signed in to an account.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_agent: String,
    pub created_at: i64,
    pub last_active_at: i64,
    pub expires_at: i64,
    pub current: bool,
}

/// What node.db keeps about an agent besides its account.
#[derive(Debug, Clone)]
pub struct AgentRow {
    pub account: Account,
    pub owner_id: String,
    pub public: bool,
    pub bio: String,
    /// When its token was last used; `None` if never.
    pub last_active_at: Option<i64>,
}

/// Two-step sign-in, as stored.
#[derive(Debug, Default, Clone)]
pub struct TotpState {
    pub secret: Option<String>,
    pub pending: Option<String>,
    pub last_step: i64,
}

/// Cuts a User-Agent down to what's kept.
pub fn clip_user_agent(user_agent: &str) -> String {
    user_agent.trim().chars().take(MAX_USER_AGENT).collect()
}

/// Another instance whose key this one pinned.
#[derive(Debug, Clone, PartialEq)]
pub struct FederationPeer {
    pub origin: String,
    pub public_key: Vec<u8>,
    pub first_seen: i64,
    pub last_heard: i64,
    /// Its key moved in a way this instance couldn't follow: an admin here
    /// checks it again before anything goes either way.
    pub needs_check: bool,
}

const PEER_COLUMNS: &str = "origin, public_key, first_seen, last_heard, needs_check";

impl FederationPeer {
    fn from_row(r: &Row) -> turso::Result<Self> {
        Ok(Self {
            origin: r.get(0)?,
            public_key: r.get(1)?,
            first_seen: r.get(2)?,
            last_heard: r.get(3)?,
            needs_check: r.get::<i64>(4)? != 0,
        })
    }
}

/// Another instance's key moving to one the old key vouched for.
pub struct FederationMove {
    pub origin: String,
    pub previous_key: Vec<u8>,
    pub key: Vec<u8>,
    pub moved_at: i64,
}

/// Rotations of this instance's own key kept for others to follow, and
/// moves of each other instance's kept here, at most.
pub const KEPT_ROTATIONS: usize = 16;

/// Forgets all but an instance's last [`KEPT_ROTATIONS`] moves.
async fn keep_last_moves(conn: &turso::Connection, origin: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM federation_moves WHERE origin = ?1 AND rowid NOT IN
           (SELECT rowid FROM federation_moves WHERE origin = ?1 ORDER BY moved_at DESC, rowid DESC LIMIT ?2)",
        (origin, KEPT_ROTATIONS as i64),
    )
    .await?;
    Ok(())
}

pub struct NodeDb {
    db: Arc<Db>,
    /// Sign-ups one at a time, so only the very first account becomes admin.
    sign_ups: Mutex<()>,
    /// Changes to who is an admin one at a time, so the last one stays.
    admin_changes: Mutex<()>,
}

/// An account as instance admins see it.
#[derive(Debug, Clone)]
pub struct AccountSummary {
    pub account: Account,
    pub disabled_at: Option<i64>,
    pub disabled_reason: String,
    /// Devices signed in now.
    pub sessions: i64,
}

/// Which accounts an admin lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountFilter {
    All,
    Admins,
    Disabled,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AccountTotals {
    pub all: i64,
    pub admins: i64,
    pub disabled: i64,
}

/// The instance's announcement banner, as kept in `meta`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredAnnouncement {
    id: String,
    text: String,
    tone: i32,
    created_at: i64,
    ends_at: Option<i64>,
}

/// Account totals for the usage signal.
#[derive(Debug, Default, Clone, Copy)]
pub struct AccountCounts {
    pub total: i64,
    pub active_1d: i64,
    pub active_30d: i64,
}

impl NodeDb {
    pub async fn open(path: &Path, key: Option<&EncryptionKey>) -> Result<Self> {
        let db = Arc::new(db::open(path, key, MIGRATIONS).await?.keep(32));
        Ok(Self { db, sign_ups: Mutex::new(()), admin_changes: Mutex::new(()) })
    }

    /// The file itself, for the replica.
    pub fn db(&self) -> &Arc<Db> {
        &self.db
    }

    fn read(&self) -> Result<db::Pooled> {
        self.db.conn()
    }

    /// The key links to pictures from other sites are signed with (see
    /// [`crate::outside`]), made the first time it's asked for. Never shown.
    pub async fn picture_key(&self) -> Result<crate::outside::Key> {
        db::write(&self.db, async |conn| {
            if let Some(key) =
                query_one(conn, "SELECT value FROM meta WHERE key = 'picture_key'", (), |r| r.get::<String>(0)).await?
                && let Some(key) = crate::outside::Key::from_hex(&key)
            {
                return Ok(key);
            }
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).expect("the OS random number generator failed");
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('picture_key', ?1)", [hex.as_str()]).await?;
            Ok(crate::outside::Key::from_hex(&hex).expect("hex just written"))
        })
        .await
    }

    /// This instance's Ed25519 key for talking to other instances (PKCS#8),
    /// made the first time it's asked for. Never shown; kept in node.db, which
    /// is encrypted when the instance has a key.
    pub async fn federation_key(&self) -> Result<Vec<u8>> {
        db::write(&self.db, async |conn| {
            if let Some(key) =
                query_one(conn, "SELECT value FROM meta WHERE key = 'federation_key'", (), |r| r.get::<String>(0))
                    .await?
                && let Some(key) = crate::federation::from_hex(&key)
            {
                return Ok(key);
            }
            let key = crate::federation::new_key()?;
            let hex = crate::federation::to_hex(&key);
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('federation_key', ?1)", [hex.as_str()])
                .await?;
            Ok(key)
        })
        .await
    }

    /// The key pinned for another instance.
    pub async fn federation_peer(&self, origin: &str) -> Result<Option<FederationPeer>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!("SELECT {PEER_COLUMNS} FROM federation_peers WHERE origin = ?1"),
            [origin],
            FederationPeer::from_row,
        )
        .await
    }

    /// Every instance this one has pinned a key for, most recently heard from first.
    pub async fn federation_peers(&self) -> Result<Vec<FederationPeer>> {
        let conn = self.read()?;
        query_all(
            &conn,
            &format!("SELECT {PEER_COLUMNS} FROM federation_peers ORDER BY last_heard DESC"),
            (),
            FederationPeer::from_row,
        )
        .await
    }

    /// Pins another instance's key the first time it's seen, up to
    /// [`crate::federation::MAX_PEERS`] instances. A different key
    /// for an instance already pinned is refused: the pinned one stays,
    /// unless `admin_check` and the instance needs checking, when an admin
    /// here takes the key it has now.
    pub async fn pin_federation_peer(
        &self,
        origin: &str,
        public_key: &[u8],
        admin_check: bool,
    ) -> Result<FederationPeer> {
        let (origin, public_key) = (origin.to_string(), public_key.to_vec());
        db::write(&self.db, async |conn| {
            let now = now_ms();
            if admin_check {
                let checked = query_one(
                    conn,
                    "SELECT public_key FROM federation_peers WHERE origin = ?1 AND needs_check = 1",
                    [origin.as_str()],
                    |r| r.get::<Vec<u8>>(0),
                )
                .await?;
                if let Some(previous) = checked {
                    conn.execute(
                        "UPDATE federation_peers SET public_key = ?2, needs_check = 0 WHERE origin = ?1",
                        (origin.as_str(), public_key.clone()),
                    )
                    .await?;
                    if previous != public_key {
                        conn.execute(
                            "INSERT INTO federation_moves (origin, previous_key, key, moved_at) VALUES (?1, ?2, ?3, ?4)",
                            (origin.as_str(), previous, public_key.clone(), now),
                        )
                        .await?;
                        keep_last_moves(conn, &origin).await?;
                    }
                }
            }
            let known = query_one(conn, "SELECT 1 FROM federation_peers WHERE origin = ?1", [origin.as_str()], |r| {
                r.get::<i64>(0)
            })
            .await?
            .is_some();
            let count = query_one(conn, "SELECT count(*) FROM federation_peers", (), |r| r.get::<i64>(0)).await?;
            if !known && count.unwrap_or(0) >= crate::federation::MAX_PEERS {
                return Err(Error::ResourceExhausted(format!(
                    "this instance already knows {} instances, the most it keeps",
                    crate::federation::MAX_PEERS
                )));
            }
            conn.execute(
                "INSERT INTO federation_peers (origin, public_key, first_seen, last_heard) VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT (origin) DO NOTHING",
                (origin.as_str(), public_key.clone(), now),
            )
            .await?;
            let peer = query_one(
                conn,
                &format!("SELECT {PEER_COLUMNS} FROM federation_peers WHERE origin = ?1"),
                [origin.as_str()],
                FederationPeer::from_row,
            )
            .await?
            .ok_or_else(|| Error::internal("a pinned instance went missing"))?;
            if peer.public_key != public_key {
                return Err(Error::FailedPrecondition(format!(
                    "{origin} has a different key from the one this instance pinned for it"
                )));
            }
            Ok(peer)
        })
        .await
    }

    /// Moves another instance from the key pinned for it (`from`) to the one
    /// its rotations (`path`, each (previous key, key)) lead to, unless it
    /// moved or needs checking meanwhile. Whether it moved.
    pub async fn move_federation_peer(&self, origin: &str, from: &[u8], path: &[(Vec<u8>, Vec<u8>)]) -> Result<bool> {
        let Some((_, to)) = path.last() else { return Ok(false) };
        let (origin, from, to, path) = (origin.to_string(), from.to_vec(), to.clone(), path.to_vec());
        db::write(&self.db, async |conn| {
            let moved = conn
                .execute(
                    "UPDATE federation_peers SET public_key = ?3 WHERE origin = ?1 AND public_key = ?2 AND needs_check = 0",
                    (origin.as_str(), from.clone(), to.clone()),
                )
                .await?;
            if moved == 0 {
                return Ok(false);
            }
            let now = now_ms();
            for (previous_key, key) in path {
                conn.execute(
                    "INSERT INTO federation_moves (origin, previous_key, key, moved_at) VALUES (?1, ?2, ?3, ?4)",
                    (origin.as_str(), previous_key, key, now),
                )
                .await?;
            }
            keep_last_moves(conn, &origin).await?;
            Ok(true)
        })
        .await
    }

    /// Marks another instance as needing an admin's check.
    pub async fn federation_peer_needs_check(&self, origin: &str) -> Result<()> {
        let origin = origin.to_string();
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE federation_peers SET needs_check = 1 WHERE origin = ?1", [origin.as_str()]).await?;
            Ok(())
        })
        .await
    }

    /// The keys other instances moved to, oldest first (one instance's, or
    /// everyone's).
    pub async fn federation_moves(&self, origin: Option<&str>) -> Result<Vec<FederationMove>> {
        let conn = self.read()?;
        let row = |r: &Row| {
            Ok(FederationMove { origin: r.get(0)?, previous_key: r.get(1)?, key: r.get(2)?, moved_at: r.get(3)? })
        };
        let columns = "SELECT origin, previous_key, key, moved_at FROM federation_moves";
        match origin {
            Some(origin) => {
                query_all(&conn, &format!("{columns} WHERE origin = ?1 ORDER BY moved_at, rowid"), [origin], row).await
            }
            None => query_all(&conn, &format!("{columns} ORDER BY moved_at, rowid"), (), row).await,
        }
    }

    /// Replaces this instance's federation key (`old`, PKCS#8) with `new`,
    /// keeping `rotation` (encoded) for others to follow, unless the key
    /// changed meanwhile.
    pub async fn rotate_federation_key(&self, old: &[u8], new: &[u8], rotation: &[u8]) -> Result<()> {
        let (old, new, rotation) =
            (crate::federation::to_hex(old), crate::federation::to_hex(new), crate::federation::to_hex(rotation));
        let rotated = db::write(&self.db, async |conn| {
            let current =
                query_one(conn, "SELECT value FROM meta WHERE key = 'federation_key'", (), |r| r.get::<String>(0))
                    .await?;
            if current.as_deref() != Some(old.as_str()) {
                return Err(Error::FailedPrecondition("the key was just rotated; try again".into()));
            }
            let kept = query_one(conn, "SELECT value FROM meta WHERE key = 'federation_rotations'", (), |r| {
                r.get::<String>(0)
            })
            .await?
            .unwrap_or_default();
            let mut rotations: Vec<&str> = kept.split(',').filter(|r| !r.is_empty()).collect();
            rotations.push(&rotation);
            let skip = rotations.len().saturating_sub(KEPT_ROTATIONS);
            let rotations = rotations[skip..].join(",");
            conn.execute("UPDATE meta SET value = ?1 WHERE key = 'federation_key'", [new.as_str()]).await?;
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('federation_rotations', ?1)",
                [rotations.as_str()],
            )
            .await?;
            Ok(())
        })
        .await;
        for secret in [old, new] {
            crate::federation::wipe(&mut secret.into_bytes());
        }
        rotated
    }

    /// This instance's kept key rotations (each encoded), oldest first.
    pub async fn federation_rotations(&self) -> Result<Vec<Vec<u8>>> {
        let conn = self.read()?;
        let kept =
            query_one(&conn, "SELECT value FROM meta WHERE key = 'federation_rotations'", (), |r| r.get::<String>(0))
                .await?
                .unwrap_or_default();
        Ok(kept.split(',').filter(|r| !r.is_empty()).filter_map(crate::federation::from_hex).collect())
    }

    /// Notes that a signed call from or answer by another instance checked out.
    pub async fn heard_from_peer(&self, origin: &str) -> Result<()> {
        let origin = origin.to_string();
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE federation_peers SET last_heard = ?2 WHERE origin = ?1", (origin.as_str(), now_ms()))
                .await?;
            Ok(())
        })
        .await
    }

    /// This instance's random, anonymous id, made the first time it's asked for.
    pub async fn install_id(&self) -> Result<String> {
        db::write(&self.db, async |conn| {
            if let Some(id) =
                query_one(conn, "SELECT value FROM meta WHERE key = 'install_id'", (), |r| r.get::<String>(0)).await?
            {
                return Ok(id);
            }
            let id = new_id();
            conn.execute("INSERT INTO meta (key, value) VALUES ('install_id', ?1)", [id.as_str()]).await?;
            Ok(id)
        })
        .await
    }

    /// Creates a standalone account. The first account on the instance is its admin.
    pub async fn create_local_account(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<Account> {
        let _one_at_a_time = self.sign_ups.lock().await;
        let result = db::write(&self.db, async |conn| {
            let first = query_one(conn, "SELECT count(*) FROM accounts", (), |r| r.get::<i64>(0)).await? == Some(0);
            let now = now_ms();
            let id = new_id();
            conn.execute(
                "INSERT INTO accounts (id, kind, username, display_name, password_hash, admin, created_at, updated_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7)",
                (id.as_str(), pb::AccountKind::Local as i64, username, display_name, password_hash, first, now),
            )
            .await?;
            Ok(Account {
                id,
                kind: pb::AccountKind::Local,
                username: username.to_string(),
                display_name: display_name.to_string(),
                avatar_url: String::new(),
                admin: first,
                created_at: now,
                last_seen_at: now,
                status: String::new(),
                status_expires_at: None,
                two_factor: false,
                disabled: false,
                decoration_id: String::new(),
            })
        })
        .await;
        result.map_err(|err| {
            if db::is_unique_violation(&err) { Error::AlreadyExists("that username is taken".into()) } else { err }
        })
    }

    /// The linked account for someone on an issuer, if they have one.
    pub async fn linked_account(&self, issuer: &str, subject: &str) -> Result<Option<Account>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE linked_issuer = ?1 AND linked_subject = ?2"),
            (issuer, subject),
            account,
        )
        .await
    }

    /// Creates a linked account, under the first free username from its base,
    /// and says it did. The first account on the instance is its admin. Someone
    /// who got an account meanwhile (a second sign-in racing this one) gets that one.
    pub async fn create_linked_account(&self, new: &NewLinkedAccount<'_>) -> Result<(Account, bool)> {
        let _one_at_a_time = self.sign_ups.lock().await;
        if let Some(account) = self.linked_account(new.issuer, new.subject).await? {
            return Ok((account, false));
        }
        let account = db::write(&self.db, async |conn| {
            let mut username = None;
            for attempt in 1..=99 {
                let candidate = crate::linked::username_candidate(new.username_base, attempt);
                let taken = query_one(conn, "SELECT 1 FROM accounts WHERE username = ?1", [candidate.as_str()], |r| {
                    r.get::<i64>(0)
                })
                .await?
                .is_some();
                if !taken {
                    username = Some(candidate);
                    break;
                }
            }
            let username = match username {
                Some(username) => username,
                None => format!("{}_{}", new.username_base.chars().take(20).collect::<String>(), &new_id()[16..]),
            }
            .to_lowercase();
            let first = query_one(conn, "SELECT count(*) FROM accounts", (), |r| r.get::<i64>(0)).await? == Some(0);
            let now = now_ms();
            let id = new_id();
            conn.execute(
                "INSERT INTO accounts (id, kind, username, display_name, avatar_url, linked_issuer, linked_subject, admin, created_at, updated_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?9)",
                (
                    id.as_str(),
                    new.kind as i64,
                    username.as_str(),
                    new.display_name,
                    new.avatar_url,
                    new.issuer,
                    new.subject,
                    first,
                    now,
                ),
            )
            .await?;
            Ok(Account {
                id,
                kind: new.kind,
                username,
                display_name: new.display_name.to_string(),
                avatar_url: new.avatar_url.to_string(),
                admin: first,
                created_at: now,
                last_seen_at: now,
                status: String::new(),
                status_expires_at: None,
                two_factor: false,
                disabled: false,
                decoration_id: String::new(),
            })
        })
        .await?;
        Ok((account, true))
    }

    /// The account a sign-in provider's person is linked to, if any.
    pub async fn provider_account(&self, provider: &str, subject: &str) -> Result<Option<Account>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!(
                "SELECT {} FROM account_providers JOIN accounts ON accounts.id = account_providers.account_id
                 WHERE account_providers.provider = ?1 AND account_providers.subject = ?2",
                account_columns_of("accounts")
            ),
            (provider, subject),
            account,
        )
        .await
    }

    /// The providers linked to an account, oldest first.
    pub async fn account_providers(&self, account_id: &str) -> Result<Vec<LinkedProvider>> {
        let conn = self.read()?;
        db::query_all(
            &conn,
            "SELECT provider, name, linked_at FROM account_providers WHERE account_id = ?1 ORDER BY linked_at, provider",
            [account_id],
            |r| Ok(LinkedProvider { provider: r.get(0)?, name: r.get(1)?, linked_at: r.get(2)? }),
        )
        .await
    }

    /// Links a provider's person to an account. Refused when they're linked
    /// to another account, or the account has that provider already.
    pub async fn link_provider(
        &self,
        account_id: &str,
        provider: &str,
        subject: &str,
        name: &str,
    ) -> Result<LinkedProvider> {
        let now = now_ms();
        db::write(&self.db, async |conn| {
            let linked = query_one(
                conn,
                "SELECT account_id FROM account_providers WHERE provider = ?1 AND subject = ?2",
                (provider, subject),
                |r| r.get::<String>(0),
            )
            .await?;
            match linked {
                Some(owner) if owner == account_id => {
                    return Err(Error::AlreadyExists("that account is already linked to yours".into()));
                }
                Some(_) => return Err(Error::AlreadyExists("that account is already linked to another account here".into())),
                None => {}
            }
            let has = query_one(
                conn,
                "SELECT 1 FROM account_providers WHERE account_id = ?1 AND provider = ?2",
                (account_id, provider),
                |r| r.get::<i64>(0),
            )
            .await?
            .is_some();
            if has {
                return Err(Error::AlreadyExists("you already linked one; unlink it first".into()));
            }
            conn.execute(
                "INSERT INTO account_providers (provider, subject, account_id, name, linked_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                (provider, subject, account_id, name, now),
            )
            .await?;
            Ok(())
        })
        .await?;
        Ok(LinkedProvider { provider: provider.to_string(), name: name.to_string(), linked_at: now })
    }

    /// Unlinks a provider from an account; false if it wasn't linked. Unless
    /// `others_work` (a way in that isn't a provider still works), one of the
    /// `working` providers must stay linked, or nothing changes and it says
    /// so. Every unlink writes the account's row, so two at once clash and
    /// the second counts what the first left.
    pub async fn unlink_provider(
        &self,
        account_id: &str,
        provider: &str,
        others_work: bool,
        working: &[&str],
    ) -> Result<bool> {
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE accounts SET kind = kind WHERE id = ?1", [account_id]).await?;
            let gone = conn
                .execute(
                    "DELETE FROM account_providers WHERE account_id = ?1 AND provider = ?2",
                    (account_id, provider),
                )
                .await?
                == 1;
            if gone && !others_work {
                let left = query_all(
                    conn,
                    "SELECT provider FROM account_providers WHERE account_id = ?1",
                    [account_id],
                    |r| r.get::<String>(0),
                )
                .await?;
                if !left.iter().any(|p| working.contains(&p.as_str())) {
                    return Err(Error::FailedPrecondition(
                        "that's the only way you can sign in right now; add another first".into(),
                    ));
                }
            }
            Ok(gone)
        })
        .await
    }

    /// Who vouches for a linked or single sign-on account (its issuer), if anyone.
    pub async fn linked_issuer(&self, account_id: &str) -> Result<Option<String>> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT linked_issuer FROM accounts WHERE id = ?1", [account_id], |r| {
            r.get::<Option<String>>(0)
        })
        .await?
        .flatten())
    }

    /// When a live session started.
    pub async fn session_started(&self, token_hash: &str) -> Result<Option<i64>> {
        let conn = self.read()?;
        query_one(&conn, "SELECT created_at FROM sessions WHERE token_hash = ?1", [token_hash], |r| r.get::<i64>(0))
            .await
    }

    /// Creates an account for someone who signed in with a provider, under
    /// exactly `username`, linked to them. The first account on the instance
    /// is its admin. Someone linked meanwhile (a second sign-in racing this
    /// one) gets that account.
    pub async fn create_provider_account(&self, new: &NewProviderAccount<'_>) -> Result<(Account, bool)> {
        let _one_at_a_time = self.sign_ups.lock().await;
        if let Some(account) = self.provider_account(new.provider, new.subject).await? {
            return Ok((account, false));
        }
        let result = db::write(&self.db, async |conn| {
            let first = query_one(conn, "SELECT count(*) FROM accounts", (), |r| r.get::<i64>(0)).await? == Some(0);
            let now = now_ms();
            let id = new_id();
            conn.execute(
                "INSERT INTO accounts (id, kind, username, display_name, admin, created_at, updated_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)",
                (id.as_str(), pb::AccountKind::Provider as i64, new.username, new.display_name, first, now),
            )
            .await?;
            conn.execute(
                "INSERT INTO account_providers (provider, subject, account_id, name, linked_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                (new.provider, new.subject, id.as_str(), new.name, now),
            )
            .await?;
            Ok(Account {
                id,
                kind: pb::AccountKind::Provider,
                username: new.username.to_string(),
                display_name: new.display_name.to_string(),
                avatar_url: String::new(),
                admin: first,
                created_at: now,
                last_seen_at: now,
                status: String::new(),
                status_expires_at: None,
                two_factor: false,
                disabled: false,
                decoration_id: String::new(),
            })
        })
        .await;
        let account = result.map_err(|err| {
            if db::is_unique_violation(&err) { Error::AlreadyExists("that username is taken".into()) } else { err }
        })?;
        Ok((account, true))
    }

    /// Whether a username is free.
    pub async fn username_free(&self, username: &str) -> Result<bool> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT 1 FROM accounts WHERE username = ?1", [username], |r| r.get::<i64>(0))
            .await?
            .is_none())
    }

    /// Holds a sign-in through waifu.dev until its code comes back.
    pub async fn create_linked_sign_in(&self, sign_in: &LinkedSignIn) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "INSERT INTO linked_sign_ins (state, secret_hash, verifier, issuer, client_id, return_origin, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    sign_in.state.as_str(),
                    sign_in.secret_hash.as_str(),
                    sign_in.verifier.as_str(),
                    sign_in.issuer.as_str(),
                    sign_in.client_id.as_str(),
                    sign_in.return_origin.as_str(),
                    now_ms() + crate::linked::TTL_MS,
                ),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// A sign-in through waifu.dev that hasn't run out.
    pub async fn linked_sign_in(&self, state: &str) -> Result<Option<LinkedSignIn>> {
        let conn = self.read()?;
        query_one(
            &conn,
            "SELECT state, secret_hash, verifier, issuer, client_id, return_origin FROM linked_sign_ins
             WHERE state = ?1 AND expires_at > ?2",
            (state, now_ms()),
            |r| {
                Ok(LinkedSignIn {
                    state: r.get(0)?,
                    secret_hash: r.get(1)?,
                    verifier: r.get(2)?,
                    issuer: r.get(3)?,
                    client_id: r.get(4)?,
                    return_origin: r.get(5)?,
                })
            },
        )
        .await
    }

    /// Ends a sign-in through waifu.dev, so its code is traded once. False if
    /// another request already did.
    pub async fn take_linked_sign_in(&self, state: &str) -> Result<bool> {
        db::write(&self.db, async |conn| {
            Ok(conn.execute("DELETE FROM linked_sign_ins WHERE state = ?1", [state]).await? > 0)
        })
        .await
    }

    /// Holds a single sign-on to the instance until the provider answers.
    /// A single sign-on to the instance that hasn't run out.
    pub async fn sso_sign_in(&self, state: &str) -> Result<Option<crate::sso::SignIn>> {
        crate::sso::load(&*self.read()?, state).await
    }

    /// Records who the provider signed in, once.
    pub async fn sso_answered(
        &self,
        sign_in: &crate::sso::SignIn,
        code_hash: &str,
        identity: &crate::sso::Identity,
    ) -> Result<bool> {
        db::write(&self.db, async |conn| crate::sso::answered_ticket(conn, sign_in, code_hash, identity).await).await
    }

    /// Keeps a sign-in that started signed in (linking a provider) until
    /// the provider answers.
    pub async fn save_sso_sign_in(&self, sign_in: &crate::sso::SignIn) -> Result<()> {
        db::write(&self.db, async |conn| crate::sso::save(conn, sign_in).await).await
    }

    /// Records who the provider signed in for a kept sign-in, once.
    pub async fn sso_row_answered(
        &self,
        state: &str,
        code_hash: &str,
        identity: &crate::sso::Identity,
    ) -> Result<bool> {
        db::write(&self.db, async |conn| crate::sso::answered(conn, state, code_hash, identity).await).await
    }

    /// Whether a sign-in with this state was kept at all, answered, used or
    /// not: a ticket that has a row came back once already.
    pub async fn sso_state_seen(&self, state: &str) -> Result<bool> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT 1 FROM sso_sign_ins WHERE state = ?1", [state], |r| r.get::<i64>(0))
            .await?
            .is_some())
    }

    /// Forgets sign-ins that ran out, with whatever the provider said about
    /// who signed in (a picture's address, say): nothing reads them anyway.
    pub async fn sweep_sso_sign_ins(&self, now: i64) -> Result<u64> {
        db::write(&self.db, async |conn| {
            Ok(conn.execute("DELETE FROM sso_sign_ins WHERE expires_at < ?1", [now]).await?)
        })
        .await
    }

    /// Ends a single sign-on, so its code works once. False if another request already did.
    pub async fn take_sso_sign_in(&self, state: &str) -> Result<bool> {
        db::write(&self.db, async |conn| crate::sso::take(conn, state).await).await
    }

    /// A standalone account and its password hash, by username.
    pub async fn local_account_by_username(&self, username: &str) -> Result<Option<(Account, String)>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!("SELECT {ACCOUNT_COLUMNS}, password_hash FROM accounts WHERE username = ?1 AND kind = ?2"),
            (username, pb::AccountKind::Local as i64),
            |row| Ok((account(row)?, row.get::<Option<String>>(ACCOUNT_COLUMN_COUNT)?.unwrap_or_default())),
        )
        .await
    }

    /// Any kind of account, by username (lowercase).
    pub async fn account_by_username(&self, username: &str) -> Result<Option<Account>> {
        let conn = self.read()?;
        query_one(&conn, &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE username = ?1"), [username], account)
            .await
    }

    pub async fn account(&self, id: &str) -> Result<Option<Account>> {
        let conn = self.read()?;
        query_one(&conn, &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1"), [id], account).await
    }

    pub async fn password_hash(&self, id: &str) -> Result<Option<String>> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT password_hash FROM accounts WHERE id = ?1", [id], |r| r.get::<Option<String>>(0))
            .await?
            .flatten())
    }

    pub async fn update_profile(&self, id: &str, change: &ProfileChange) -> Result<Account> {
        let accent = change.accent_color.map(|color| color.unwrap_or(-1));
        let (status, status_expires_at) = match &change.status {
            Some((status, expires)) => (Some(status.as_str()), *expires),
            None => (None, None),
        };
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE accounts SET display_name = coalesce(?2, display_name), avatar_url = coalesce(?3, avatar_url),
                   pronouns = coalesce(?4, pronouns), bio = coalesce(?5, bio), banner_url = coalesce(?6, banner_url),
                   accent_color = CASE WHEN ?7 IS NULL THEN accent_color WHEN ?7 < 0 THEN NULL ELSE ?7 END,
                   status = coalesce(?8, status),
                   status_expires_at = CASE WHEN ?8 IS NULL THEN status_expires_at ELSE ?9 END,
                   updated_at = ?10, profile_effect = coalesce(?11, profile_effect),
                   profile_decoration = coalesce(?12, profile_decoration)
                 WHERE id = ?1",
                (
                    id,
                    change.display_name.as_deref(),
                    change.avatar_url.as_deref(),
                    change.pronouns.as_deref(),
                    change.bio.as_deref(),
                    change.banner_url.as_deref(),
                    accent,
                    status,
                    status_expires_at,
                    now_ms(),
                    change.effect.as_deref(),
                    change.decoration.as_deref(),
                ),
            )
            .await?;
            Ok(())
        })
        .await?;
        self.account(id).await?.ok_or(Error::NotFound("account"))
    }

    /// Everything someone shows about themselves.
    pub async fn profile(&self, id: &str) -> Result<Option<pb::Profile>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!(
                "SELECT {ACCOUNT_COLUMNS}, pronouns, bio, banner_url, accent_color, profile_effect FROM accounts WHERE id = ?1"
            ),
            [id],
            |row| {
                let account = account(row)?;
                Ok(pb::Profile {
                    user: Some(account.user()),
                    pronouns: row.get(ACCOUNT_COLUMN_COUNT)?,
                    bio: row.get(ACCOUNT_COLUMN_COUNT + 1)?,
                    banner_url: row.get(ACCOUNT_COLUMN_COUNT + 2)?,
                    accent_color: row.get(ACCOUNT_COLUMN_COUNT + 3)?,
                    created_at: Some(timestamp(account.created_at)),
                    effect: row.get(ACCOUNT_COLUMN_COUNT + 4)?,
                })
            },
        )
        .await
    }

    /// Replaces the password and ends every other session, and any sign-in
    /// that got past the old password and is waiting on a code.
    pub async fn set_password(&self, id: &str, password_hash: &str, keep_session: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE accounts SET password_hash = ?2, updated_at = ?3 WHERE id = ?1",
                (id, password_hash, now_ms()),
            )
            .await?;
            conn.execute("DELETE FROM sessions WHERE account_id = ?1 AND token_hash != ?2", (id, keep_session)).await?;
            conn.execute("DELETE FROM sign_in_tickets WHERE account_id = ?1", [id]).await?;
            Ok(())
        })
        .await
    }

    pub async fn create_session(&self, account_id: &str, token_hash: &str, user_agent: &str) -> Result<()> {
        let user_agent = clip_user_agent(user_agent);
        db::write(&self.db, async |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO sessions (token_hash, id, account_id, user_agent, created_at, last_active_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                (token_hash, new_id(), account_id, user_agent.as_str(), now, now + SESSION_TTL_MS),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// The account a live session belongs to, marking the session as active
    /// and the account as seen. An agent whose owner is turned off has none.
    pub async fn session_account(&self, token_hash: &str) -> Result<Option<Account>> {
        let conn = self.read()?;
        let now = now_ms();
        let found = query_one(
            &conn,
            &format!(
                "SELECT {}, sessions.last_active_at FROM sessions JOIN accounts ON accounts.id = sessions.account_id
                 LEFT JOIN accounts AS owners ON owners.id = accounts.owner_id
                 WHERE sessions.token_hash = ?1 AND sessions.expires_at > ?2 AND accounts.disabled_at IS NULL
                   AND owners.disabled_at IS NULL",
                account_columns_of("accounts")
            ),
            (token_hash, now),
            |row| Ok((account(row)?, row.get::<i64>(ACCOUNT_COLUMN_COUNT)?)),
        )
        .await?;
        let Some((account, last_active)) = found else { return Ok(None) };
        let seen = now - account.last_seen_at > SEEN_RESOLUTION_MS;
        if seen || now - last_active > ACTIVE_RESOLUTION_MS {
            // Several requests at once may all try; one landing is enough.
            let marked = db::write_once(&self.db, async |conn| {
                conn.execute("UPDATE sessions SET last_active_at = ?2 WHERE token_hash = ?1", (token_hash, now))
                    .await?;
                if seen {
                    conn.execute("UPDATE accounts SET last_seen_at = ?2 WHERE id = ?1", (account.id.as_str(), now))
                        .await?;
                }
                Ok(())
            })
            .await;
            if let Err(err) = marked
                && !db::is_conflict(&err)
            {
                return Err(err);
            }
        }
        Ok(Some(account))
    }

    /// Whether a session is still signed in, for streams that outlive a request.
    pub async fn session_live(&self, token_hash: &str) -> Result<bool> {
        let conn = self.read()?;
        Ok(query_one(
            &conn,
            "SELECT 1 FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
            (token_hash, now_ms()),
            |r| r.get::<i64>(0),
        )
        .await?
        .is_some())
    }

    /// The id of the live session a token hash belongs to.
    pub async fn session_id(&self, token_hash: &str) -> Result<Option<String>> {
        let conn = self.read()?;
        query_one(
            &conn,
            "SELECT id FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
            (token_hash, now_ms()),
            |r| r.get::<String>(0),
        )
        .await
    }

    /// The ids of every live session, or only those of some accounts.
    pub async fn live_session_ids(&self, account_ids: Option<&[&str]>) -> Result<HashSet<String>> {
        let conn = self.read()?;
        let ids = match account_ids {
            None => {
                query_all(&conn, "SELECT id FROM sessions WHERE expires_at > ?1 AND id IS NOT NULL", [now_ms()], |r| {
                    r.get::<String>(0)
                })
                .await?
            }
            Some([]) => vec![],
            Some(account_ids) => {
                let placeholders = (2..account_ids.len() + 2).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
                let mut params = vec![turso::Value::from(now_ms())];
                params.extend(account_ids.iter().map(|id| turso::Value::from(*id)));
                query_all(
                    &conn,
                    &format!(
                        "SELECT id FROM sessions WHERE expires_at > ?1 AND id IS NOT NULL AND account_id IN ({placeholders})"
                    ),
                    params,
                    |r| r.get::<String>(0),
                )
                .await?
            }
        };
        Ok(ids.into_iter().collect())
    }

    /// The accounts with these ids, those that exist.
    pub async fn accounts(&self, ids: &[&str]) -> Result<Vec<Account>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.read()?;
        let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        query_all(
            &conn,
            &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id IN ({placeholders})"),
            ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
            account,
        )
        .await
    }

    /// An account's live sessions, the one with `current_hash` first, then by last use.
    pub async fn sessions(&self, account_id: &str, current_hash: &str) -> Result<Vec<Session>> {
        let conn = self.read()?;
        query_all(
            &conn,
            "SELECT id, user_agent, created_at, last_active_at, expires_at, token_hash = ?2 FROM sessions
             WHERE account_id = ?1 AND expires_at > ?3
             ORDER BY token_hash = ?2 DESC, last_active_at DESC, created_at DESC",
            (account_id, current_hash, now_ms()),
            |r| {
                Ok(Session {
                    id: r.get::<Option<String>>(0)?.unwrap_or_default(),
                    user_agent: r.get(1)?,
                    created_at: r.get(2)?,
                    last_active_at: r.get(3)?,
                    expires_at: r.get(4)?,
                    current: r.get(5)?,
                })
            },
        )
        .await
    }

    /// Ends one of an account's sessions. Whether there was one to end.
    pub async fn delete_session_by_id(&self, account_id: &str, id: &str) -> Result<bool> {
        db::write(&self.db, async |conn| {
            Ok(conn.execute("DELETE FROM sessions WHERE account_id = ?1 AND id = ?2", (account_id, id)).await? > 0)
        })
        .await
    }

    /// Ends every session of an account but one. How many ended.
    pub async fn delete_other_sessions(&self, account_id: &str, keep_hash: &str) -> Result<u64> {
        db::write(&self.db, async |conn| {
            Ok(conn
                .execute("DELETE FROM sessions WHERE account_id = ?1 AND token_hash != ?2", (account_id, keep_hash))
                .await?)
        })
        .await
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash]).await?;
            Ok(())
        })
        .await
    }

    /// Drops sessions and sign-ins waiting on a code that have run out, and
    /// upload counts from days gone by.
    pub async fn prune_sessions(&self) -> Result<u64> {
        db::write(&self.db, async |conn| {
            let now = now_ms();
            conn.execute("DELETE FROM sign_in_tickets WHERE expires_at <= ?1", [now]).await?;
            conn.execute("DELETE FROM linked_sign_ins WHERE expires_at <= ?1", [now]).await?;
            conn.execute("DELETE FROM upload_days WHERE day < ?1", [now / DAY_MS]).await?;
            conn.execute("DELETE FROM attachment_days WHERE day < ?1", [now / DAY_MS]).await?;
            Ok(conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now]).await?)
        })
        .await
    }

    // ───────────────────────── Two-step sign-in ─────────────────────────

    pub async fn totp_state(&self, account_id: &str) -> Result<TotpState> {
        let conn = self.read()?;
        Ok(query_one(
            &conn,
            "SELECT totp_secret, totp_pending, totp_last_step FROM accounts WHERE id = ?1",
            [account_id],
            |r| Ok(TotpState { secret: r.get(0)?, pending: r.get(1)?, last_step: r.get(2)? }),
        )
        .await?
        .unwrap_or_default())
    }

    /// Keeps a secret being set up until a code confirms it.
    pub async fn set_totp_pending(&self, account_id: &str, secret: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE accounts SET totp_pending = ?2 WHERE id = ?1", (account_id, secret)).await?;
            Ok(())
        })
        .await
    }

    /// Turns two-step sign-in on with the secret being set up, if it's still
    /// `secret`, and replaces the backup codes. `step` is the code's time step,
    /// used up by confirming. Whether it was turned on.
    pub async fn enable_totp(&self, account_id: &str, secret: &str, step: i64, code_hashes: &[String]) -> Result<bool> {
        db::write(&self.db, async |conn| {
            let changed = conn
                .execute(
                    "UPDATE accounts SET totp_secret = totp_pending, totp_pending = NULL, totp_last_step = ?3, updated_at = ?4
                     WHERE id = ?1 AND totp_pending = ?2",
                    (account_id, secret, step, now_ms()),
                )
                .await?;
            if changed == 0 {
                return Ok(false);
            }
            replace_backup_codes(conn, account_id, code_hashes).await?;
            Ok(true)
        })
        .await
    }

    pub async fn disable_totp(&self, account_id: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE accounts SET totp_secret = NULL, totp_pending = NULL, updated_at = ?2 WHERE id = ?1",
                (account_id, now_ms()),
            )
            .await?;
            conn.execute("DELETE FROM backup_codes WHERE account_id = ?1", [account_id]).await?;
            Ok(())
        })
        .await
    }

    /// Uses up an authenticator code's time step, so the same code can't sign
    /// in twice. False if it (or a later one) was already used.
    pub async fn use_totp_step(&self, account_id: &str, step: i64) -> Result<bool> {
        db::write(&self.db, async |conn| {
            // Every use writes this row, so two at once clash and the second sees the first.
            let last =
                query_one(conn, "SELECT totp_last_step FROM accounts WHERE id = ?1", [account_id], |r| r.get::<i64>(0))
                    .await?
                    .unwrap_or(i64::MAX);
            if step <= last {
                return Ok(false);
            }
            conn.execute("UPDATE accounts SET totp_last_step = ?2 WHERE id = ?1", (account_id, step)).await?;
            Ok(true)
        })
        .await
    }

    /// Uses up a backup code. False if it's unknown or used.
    pub async fn use_backup_code(&self, account_id: &str, code_hash: &str) -> Result<bool> {
        db::write(&self.db, async |conn| {
            Ok(conn
                .execute(
                    "UPDATE backup_codes SET used_at = ?3 WHERE account_id = ?1 AND code_hash = ?2 AND used_at IS NULL",
                    (account_id, code_hash, now_ms()),
                )
                .await?
                > 0)
        })
        .await
    }

    pub async fn set_backup_codes(&self, account_id: &str, code_hashes: &[String]) -> Result<()> {
        db::write(&self.db, async |conn| replace_backup_codes(conn, account_id, code_hashes).await).await
    }

    pub async fn backup_codes_left(&self, account_id: &str) -> Result<i64> {
        let conn = self.read()?;
        Ok(query_one(
            &conn,
            "SELECT count(*) FROM backup_codes WHERE account_id = ?1 AND used_at IS NULL",
            [account_id],
            |r| r.get::<i64>(0),
        )
        .await?
        .unwrap_or(0))
    }

    /// Holds a sign-in whose password was right until its two-step code comes.
    pub async fn create_ticket(&self, ticket_hash: &str, account_id: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "INSERT INTO sign_in_tickets (ticket_hash, account_id, expires_at) VALUES (?1, ?2, ?3)",
                (ticket_hash, account_id, now_ms() + TICKET_TTL_MS),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// Counts a try at a sign-in waiting on a code and gives the account it's
    /// for, or None when it has run out or used up its tries. The try is
    /// counted before the code is checked, in the same write that finds the
    /// sign-in, so codes sent all at once can't get past the limit.
    pub async fn try_ticket(&self, ticket_hash: &str) -> Result<Option<String>> {
        db::write(&self.db, async |conn| {
            let counted = conn
                .execute(
                    "UPDATE sign_in_tickets SET attempts = attempts + 1
                     WHERE ticket_hash = ?1 AND attempts < ?2 AND expires_at > ?3",
                    (ticket_hash, TICKET_ATTEMPTS, now_ms()),
                )
                .await?;
            if counted == 0 {
                return Ok(None);
            }
            query_one(conn, "SELECT account_id FROM sign_in_tickets WHERE ticket_hash = ?1", [ticket_hash], |r| {
                r.get::<String>(0)
            })
            .await
        })
        .await
    }

    /// Finishes a sign-in waiting on a code. False if another request already did.
    pub async fn take_ticket(&self, ticket_hash: &str) -> Result<bool> {
        db::write(&self.db, async |conn| {
            Ok(conn.execute("DELETE FROM sign_in_tickets WHERE ticket_hash = ?1", [ticket_hash]).await? > 0)
        })
        .await
    }

    // ───────────────────────── Presence settings ─────────────────────────

    /// What a person picked for their presence; the defaults (online, nothing
    /// shared) when they never changed it.
    pub async fn presence_settings(&self, account_id: &str) -> Result<pb::PresenceSettings> {
        let conn = self.read()?;
        let found = query_one(
            &conn,
            "SELECT status, show_activity, hidden_servers FROM presence_settings WHERE account_id = ?1",
            [account_id],
            |r| Ok((r.get::<i64>(0)?, r.get::<i64>(1)? != 0, r.get::<String>(2)?)),
        )
        .await?;
        Ok(match found {
            Some((status, show_activity, hidden)) => pb::PresenceSettings {
                status: status as i32,
                show_activity,
                hidden_server_ids: serde_json::from_str(&hidden).unwrap_or_default(),
            },
            None => pb::PresenceSettings { status: pb::PresenceStatus::Online as i32, ..Default::default() },
        })
    }

    /// Which of `ids` picked invisible: they show offline everywhere, to
    /// friends too.
    pub async fn invisible_of(&self, ids: &[&str]) -> Result<HashSet<String>> {
        let mut invisible = HashSet::new();
        if ids.is_empty() {
            return Ok(invisible);
        }
        let conn = self.read()?;
        // A few hundred at a time: a long friends list stays under SQLite's
        // limit on bound values.
        for chunk in ids.chunks(500) {
            let placeholders = (2..=chunk.len() + 1).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
            let mut params = vec![turso::Value::from(pb::PresenceStatus::Invisible as i64)];
            params.extend(chunk.iter().map(|id| turso::Value::from(*id)));
            let rows = query_all(
                &conn,
                &format!(
                    "SELECT account_id FROM presence_settings WHERE status = ?1 AND account_id IN ({placeholders})"
                ),
                params,
                |r| r.get::<String>(0),
            )
            .await?;
            invisible.extend(rows);
        }
        Ok(invisible)
    }

    pub async fn set_presence_settings(&self, account_id: &str, settings: &pb::PresenceSettings) -> Result<()> {
        let hidden =
            serde_json::to_string(&settings.hidden_server_ids).map_err(|err| Error::internal(err.to_string()))?;
        db::write(&self.db, async |conn| {
            conn.execute(
                "INSERT INTO presence_settings (account_id, status, show_activity, hidden_servers, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (account_id) DO UPDATE SET
                   status = excluded.status, show_activity = excluded.show_activity,
                   hidden_servers = excluded.hidden_servers, updated_at = excluded.updated_at",
                (account_id, settings.status as i64, settings.show_activity, hidden.as_str(), now_ms()),
            )
            .await?;
            Ok(())
        })
        .await
    }

    // ───────────────────────── Notification settings ─────────────────────────

    pub async fn notification_settings(&self, account_id: &str) -> Result<Vec<pb::NotificationSettings>> {
        let conn = self.read()?;
        query_all(
            &conn,
            "SELECT server_id, channel_id, level, muted, muted_until, suppress_everyone FROM notification_settings
             WHERE account_id = ?1 ORDER BY server_id, channel_id",
            [account_id],
            notification_row,
        )
        .await
    }

    /// Changes one server's or channel's settings: `change` gets the stored
    /// settings (or empty ones) and changes them. Settings left saying nothing
    /// are forgotten.
    pub async fn update_notification_settings(
        &self,
        account_id: &str,
        server_id: &str,
        channel_id: &str,
        change: impl Fn(&mut pb::NotificationSettings) + Clone,
    ) -> Result<pb::NotificationSettings> {
        db::write(&self.db, async |conn| {
            let mut settings = query_one(
                conn,
                "SELECT server_id, channel_id, level, muted, muted_until, suppress_everyone FROM notification_settings
                 WHERE account_id = ?1 AND server_id = ?2 AND channel_id = ?3",
                (account_id, server_id, channel_id),
                notification_row,
            )
            .await?
            .unwrap_or_else(|| pb::NotificationSettings {
                server_id: server_id.to_string(),
                channel_id: channel_id.to_string(),
                ..Default::default()
            });
            change(&mut settings);
            if !settings.muted {
                settings.muted_until = None;
            }
            let says_nothing = settings.level == pb::NotificationLevel::Unspecified as i32
                && !settings.muted
                && !settings.suppress_everyone;
            if says_nothing {
                conn.execute(
                    "DELETE FROM notification_settings WHERE account_id = ?1 AND server_id = ?2 AND channel_id = ?3",
                    (account_id, server_id, channel_id),
                )
                .await?;
            } else {
                conn.execute(
                    "INSERT INTO notification_settings
                       (account_id, server_id, channel_id, level, muted, muted_until, suppress_everyone, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT (account_id, server_id, channel_id) DO UPDATE SET
                       level = excluded.level, muted = excluded.muted, muted_until = excluded.muted_until,
                       suppress_everyone = excluded.suppress_everyone, updated_at = excluded.updated_at",
                    (
                        account_id,
                        server_id,
                        channel_id,
                        settings.level as i64,
                        settings.muted,
                        settings.muted_until.as_ref().map(crate::id::millis),
                        settings.suppress_everyone,
                        now_ms(),
                    ),
                )
                .await?;
            }
            Ok(settings)
        })
        .await
    }

    /// Forgets notification settings nobody can use any more: someone's for a
    /// server they left (`account_id`), or everyone's for a deleted server or
    /// channel.
    pub async fn forget_notification_settings(
        &self,
        server_id: &str,
        channel_id: Option<&str>,
        account_id: Option<&str>,
    ) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "DELETE FROM notification_settings
                 WHERE server_id = ?1 AND (?2 IS NULL OR channel_id = ?2) AND (?3 IS NULL OR account_id = ?3)",
                (server_id, channel_id, account_id),
            )
            .await?;
            Ok(())
        })
        .await
    }

    // ───────────────────────── Featured servers ─────────────────────────

    /// The servers featured in Browse, in order, as stored: some may be out
    /// of Browse now.
    pub async fn featured_servers(&self) -> Result<Vec<String>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT server_id FROM featured_servers ORDER BY position", (), |r| r.get::<String>(0)).await
    }

    /// Replaces the featured servers (already checked) with these, in order.
    pub async fn set_featured_servers(&self, server_ids: &[String]) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("DELETE FROM featured_servers", ()).await?;
            for (position, id) in server_ids.iter().enumerate() {
                conn.execute(
                    "INSERT INTO featured_servers (server_id, position) VALUES (?1, ?2)",
                    (id.as_str(), position as i64),
                )
                .await?;
            }
            Ok(())
        })
        .await
    }

    /// Stops featuring a server that's gone.
    pub async fn unfeature_server(&self, server_id: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("DELETE FROM featured_servers WHERE server_id = ?1", [server_id]).await?;
            Ok(())
        })
        .await
    }

    // ───────────────────────── Server arrangement ─────────────────────────

    /// How someone arranged their servers, and when (ms), as stored: it can
    /// still name servers they left since.
    pub async fn server_arrangement(&self, account_id: &str) -> Result<(Vec<pb::ServerRailItem>, Option<i64>)> {
        let conn = self.read()?;
        let row = query_one(
            &conn,
            "SELECT items, updated_at FROM server_arrangements WHERE account_id = ?1",
            [account_id],
            |r| Ok((r.get::<Vec<u8>>(0)?, r.get::<i64>(1)?)),
        )
        .await?;
        let Some((items, updated_at)) = row else { return Ok((Vec::new(), None)) };
        // Written by this code from a checked request, so it always decodes;
        // if it ever doesn't, the person just starts from the default order.
        let items = pb::SetServerArrangementRequest::decode(items.as_slice()).map(|r| r.items).unwrap_or_default();
        Ok((items, Some(updated_at)))
    }

    /// Replaces someone's arrangement (already checked); an empty one is
    /// forgotten. Returns when it changed (ms).
    pub async fn set_server_arrangement(&self, account_id: &str, items: Vec<pb::ServerRailItem>) -> Result<i64> {
        let now = now_ms();
        let encoded = (!items.is_empty()).then(|| pb::SetServerArrangementRequest { items }.encode_to_vec());
        db::write(&self.db, async |conn| {
            match &encoded {
                Some(bytes) => {
                    conn.execute(
                        "INSERT INTO server_arrangements (account_id, items, updated_at) VALUES (?1, ?2, ?3)
                         ON CONFLICT (account_id) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
                        (account_id, bytes.clone(), now),
                    )
                    .await?;
                }
                None => {
                    conn.execute("DELETE FROM server_arrangements WHERE account_id = ?1", [account_id]).await?;
                }
            }
            Ok(())
        })
        .await?;
        Ok(now)
    }

    // ───────────────────────── Split instances ─────────────────────────

    /// Every shard that has registered: (id, where it was, its region).
    pub async fn shards(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT id, url, region FROM shards ORDER BY id", (), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .await
    }

    /// Which shard holds each server.
    pub async fn placements(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT server_id, shard_id FROM placements", (), |r| Ok((r.get(0)?, r.get(1)?))).await
    }

    /// Records a shard's registration: where it is and every server it holds,
    /// which is all it holds.
    pub async fn register_shard(&self, shard_id: &str, url: &str, region: &str, server_ids: &[String]) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "INSERT INTO shards (id, url, region, registered_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET url = excluded.url, region = excluded.region,
                   registered_at = excluded.registered_at",
                (shard_id, url, region, now_ms()),
            )
            .await?;
            conn.execute("DELETE FROM placements WHERE shard_id = ?1", [shard_id]).await?;
            for id in server_ids {
                conn.execute(
                    "INSERT INTO placements (server_id, shard_id) VALUES (?1, ?2)
                     ON CONFLICT (server_id) DO UPDATE SET shard_id = excluded.shard_id",
                    (id.as_str(), shard_id),
                )
                .await?;
            }
            Ok(())
        })
        .await
    }

    /// Records where a new server went, or (with `None`) that it's gone.
    pub async fn place(&self, server_id: &str, shard_id: Option<&str>) -> Result<()> {
        db::write(&self.db, async |conn| {
            match shard_id {
                Some(shard_id) => {
                    conn.execute(
                        "INSERT INTO placements (server_id, shard_id) VALUES (?1, ?2)
                         ON CONFLICT (server_id) DO UPDATE SET shard_id = excluded.shard_id",
                        (server_id, shard_id),
                    )
                    .await?
                }
                None => conn.execute("DELETE FROM placements WHERE server_id = ?1", [server_id]).await?,
            };
            Ok(())
        })
        .await
    }

    /// Servers being moved between shards.
    pub async fn moves(&self) -> Result<Vec<Move>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT server_id, from_shard, to_shard, committed FROM moves", (), |r| {
            Ok(Move { server_id: r.get(0)?, from: r.get(1)?, to: r.get(2)?, committed: r.get(3)? })
        })
        .await
    }

    /// Records a move starting, or (`committed`) the server open on its new
    /// shard and placed there, in one transaction with the placement.
    pub async fn save_move(&self, moved: &Move) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "INSERT INTO moves (server_id, from_shard, to_shard, committed, started_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (server_id) DO UPDATE SET from_shard = excluded.from_shard,
                   to_shard = excluded.to_shard, committed = excluded.committed",
                (moved.server_id.as_str(), moved.from.as_str(), moved.to.as_str(), moved.committed, now_ms()),
            )
            .await?;
            if moved.committed {
                conn.execute(
                    "INSERT INTO placements (server_id, shard_id) VALUES (?1, ?2)
                     ON CONFLICT (server_id) DO UPDATE SET shard_id = excluded.shard_id",
                    (moved.server_id.as_str(), moved.to.as_str()),
                )
                .await?;
            }
            Ok(())
        })
        .await
    }

    /// A move is over, one way or the other.
    pub async fn end_move(&self, server_id: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("DELETE FROM moves WHERE server_id = ?1", [server_id]).await?;
            Ok(())
        })
        .await
    }

    // ───────────────────────── Instance admins ─────────────────────────

    /// Accounts newest first (ids are ULIDs), matching `query` in the username
    /// or display name, a page of `limit` after `before_id`. Also says whether
    /// there are more.
    pub async fn list_accounts(
        &self,
        query: &str,
        filter: AccountFilter,
        before_id: &str,
        limit: i64,
    ) -> Result<(Vec<AccountSummary>, bool)> {
        let conn = self.read()?;
        let query = query.trim().to_lowercase();
        let filter_sql = match filter {
            AccountFilter::All => "",
            AccountFilter::Admins => " AND admin",
            AccountFilter::Disabled => " AND disabled_at IS NOT NULL",
        };
        let mut found = query_all(
            &conn,
            &format!(
                "SELECT {ACCOUNT_COLUMNS}, disabled_at, disabled_reason,
                        (SELECT count(*) FROM sessions WHERE sessions.account_id = accounts.id AND sessions.expires_at > ?4)
                 FROM accounts
                 WHERE (?1 = '' OR instr(username, ?1) > 0 OR instr(lower(display_name), ?1) > 0)
                   AND (?2 = '' OR id < ?2){filter_sql}
                 ORDER BY id DESC LIMIT ?3"
            ),
            (query.as_str(), before_id, limit + 1, now_ms()),
            summary_row,
        )
        .await?;
        let more = found.len() as i64 > limit;
        found.truncate(limit as usize);
        Ok((found, more))
    }

    pub async fn account_summary(&self, id: &str) -> Result<Option<AccountSummary>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!(
                "SELECT {ACCOUNT_COLUMNS}, disabled_at, disabled_reason,
                        (SELECT count(*) FROM sessions WHERE sessions.account_id = accounts.id AND sessions.expires_at > ?2)
                 FROM accounts WHERE id = ?1"
            ),
            (id, now_ms()),
            summary_row,
        )
        .await
    }

    pub async fn account_totals(&self) -> Result<AccountTotals> {
        let conn = self.read()?;
        Ok(query_one(
            &conn,
            "SELECT count(*), coalesce(sum(admin), 0), coalesce(sum(CASE WHEN disabled_at IS NULL THEN 0 ELSE 1 END), 0)
             FROM accounts",
            (),
            |r| Ok(AccountTotals { all: r.get(0)?, admins: r.get(1)?, disabled: r.get(2)? }),
        )
        .await?
        .unwrap_or_default())
    }

    /// Makes an account an instance admin, or takes that away. The last admin
    /// can't stop being one, and a turned-off account can't become one.
    pub async fn set_admin(&self, id: &str, admin: bool) -> Result<()> {
        let _one_at_a_time = self.admin_changes.lock().await;
        let account = self.account(id).await?.ok_or(Error::NotFound("account"))?;
        if account.admin == admin {
            return Ok(());
        }
        if admin && account.disabled {
            return Err(Error::FailedPrecondition("turn the account back on first".into()));
        }
        if !admin && self.admin_count().await? <= 1 {
            return Err(Error::FailedPrecondition("an instance needs at least one admin".into()));
        }
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE accounts SET admin = ?2, updated_at = ?3 WHERE id = ?1", (id, admin, now_ms()))
                .await?;
            Ok(())
        })
        .await
    }

    /// Turns an account off, signing out its devices, any sign-in half done
    /// and its agents, or back on. Admins have to stop being admins first.
    pub async fn set_disabled(&self, id: &str, disabled: bool, reason: &str) -> Result<()> {
        let _one_at_a_time = self.admin_changes.lock().await;
        let account = self.account(id).await?.ok_or(Error::NotFound("account"))?;
        if disabled && account.admin {
            return Err(Error::FailedPrecondition("take away their admin rights first".into()));
        }
        db::write(&self.db, async |conn| {
            let now = now_ms();
            if disabled {
                conn.execute(
                    "UPDATE accounts SET disabled_at = coalesce(disabled_at, ?2), disabled_reason = ?3, updated_at = ?2
                     WHERE id = ?1",
                    (id, now, reason),
                )
                .await?;
                conn.execute("DELETE FROM sessions WHERE account_id = ?1", [id]).await?;
                conn.execute("DELETE FROM sign_in_tickets WHERE account_id = ?1", [id]).await?;
                // Their agents stop too: they act for someone who can't.
                conn.execute(
                    "DELETE FROM sessions WHERE account_id IN (SELECT id FROM accounts WHERE owner_id = ?1)",
                    [id],
                )
                .await?;
            } else {
                conn.execute(
                    "UPDATE accounts SET disabled_at = NULL, disabled_reason = '', updated_at = ?2 WHERE id = ?1",
                    (id, now),
                )
                .await?;
            }
            Ok(())
        })
        .await
    }

    /// Replaces a standalone account's password, signs out every device and
    /// sign-in in progress, and optionally turns off two-step sign-in.
    pub async fn reset_password(&self, id: &str, password_hash: &str, turn_off_two_factor: bool) -> Result<()> {
        db::write(&self.db, async |conn| {
            let now = now_ms();
            conn.execute(
                "UPDATE accounts SET password_hash = ?2, updated_at = ?3 WHERE id = ?1",
                (id, password_hash, now),
            )
            .await?;
            if turn_off_two_factor {
                conn.execute("UPDATE accounts SET totp_secret = NULL, totp_pending = NULL WHERE id = ?1", [id]).await?;
                conn.execute("DELETE FROM backup_codes WHERE account_id = ?1", [id]).await?;
            }
            conn.execute("DELETE FROM sessions WHERE account_id = ?1", [id]).await?;
            conn.execute("DELETE FROM sign_in_tickets WHERE account_id = ?1", [id]).await?;
            Ok(())
        })
        .await
    }

    /// The announcement banner as last set, even if it has run out.
    pub async fn announcement(&self) -> Result<Option<pb::Announcement>> {
        let conn = self.read()?;
        let Some(json) =
            query_one(&conn, "SELECT value FROM meta WHERE key = 'announcement'", (), |r| r.get::<String>(0)).await?
        else {
            return Ok(None);
        };
        let stored: StoredAnnouncement = serde_json::from_str(&json)
            .map_err(|err| Error::internal(format!("the stored announcement doesn't read: {err}")))?;
        Ok(Some(pb::Announcement {
            id: stored.id,
            text: stored.text,
            tone: stored.tone,
            created_at: Some(timestamp(stored.created_at)),
            ends_at: stored.ends_at.map(timestamp),
        }))
    }

    /// Stores the announcement banner, or forgets it.
    pub async fn set_announcement(&self, announcement: Option<&pb::Announcement>) -> Result<()> {
        let json = match announcement {
            Some(a) => Some(
                serde_json::to_string(&StoredAnnouncement {
                    id: a.id.clone(),
                    text: a.text.clone(),
                    tone: a.tone,
                    created_at: a.created_at.as_ref().map(crate::id::millis).unwrap_or_else(now_ms),
                    ends_at: a.ends_at.as_ref().map(crate::id::millis),
                })
                .map_err(|err| Error::internal(err.to_string()))?,
            ),
            None => None,
        };
        db::write(&self.db, async |conn| {
            match &json {
                Some(json) => {
                    conn.execute(
                        "INSERT INTO meta (key, value) VALUES ('announcement', ?1)
                         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                        [json.as_str()],
                    )
                    .await?;
                }
                None => {
                    conn.execute("DELETE FROM meta WHERE key = 'announcement'", ()).await?;
                }
            }
            Ok(())
        })
        .await
    }

    // ───────────────────────── Profile items ─────────────────────────

    /// The instance's profile effects and decorations, oldest first.
    pub async fn profile_items(&self) -> Result<Vec<pb::ProfileItem>> {
        crate::profile_items::list(&*self.read()?, "").await
    }

    pub async fn profile_item(&self, id: &str) -> Result<Option<pb::ProfileItem>> {
        crate::profile_items::get(&*self.read()?, "", id).await
    }

    /// Whether the instance offers an item of this kind with this id.
    pub async fn has_profile_item(&self, id: &str, kind: pb::ProfileItemKind) -> Result<bool> {
        crate::profile_items::has(&*self.read()?, id, kind).await
    }

    /// When the instance's profile items last changed, if they ever did.
    pub async fn profile_items_at(&self) -> Result<Option<i64>> {
        let conn = self.read()?;
        let at = query_one(&conn, "SELECT value FROM meta WHERE key = 'profile_items_at'", (), |r| r.get::<String>(0))
            .await?;
        Ok(at.and_then(|at| at.parse().ok()))
    }

    pub async fn add_profile_item(&self, item: &pb::ProfileItem) -> Result<()> {
        db::write(&self.db, async |conn| {
            crate::profile_items::insert(conn, item).await?;
            items_changed(conn).await
        })
        .await
    }

    pub async fn save_profile_item(&self, item: &pb::ProfileItem) -> Result<()> {
        db::write(&self.db, async |conn| {
            crate::profile_items::save(conn, item).await?;
            items_changed(conn).await
        })
        .await
    }

    /// Deletes one of the instance's profile items and takes it off everyone
    /// wearing it. What it was, if it was there.
    pub async fn delete_profile_item(&self, id: &str) -> Result<Option<pb::ProfileItem>> {
        db::write(&self.db, async |conn| {
            let Some(item) = crate::profile_items::get(conn, "", id).await? else { return Ok(None) };
            crate::profile_items::delete(conn, id).await?;
            conn.execute("UPDATE accounts SET profile_effect = '' WHERE profile_effect = ?1", [id]).await?;
            conn.execute("UPDATE accounts SET profile_decoration = '' WHERE profile_decoration = ?1", [id]).await?;
            items_changed(conn).await?;
            Ok(Some(item))
        })
        .await
    }

    // ───────────────────────── Uploaded pictures ─────────────────────────

    /// Reserves an upload, unless the account has too many going already or
    /// has used up `bytes_per_day` today (of pictures, or of attachments for
    /// an attachment or a sealed file, which are counted apart).
    pub async fn reserve_media(
        &self,
        row: &MediaRow,
        upload_hash: &str,
        expires_at: i64,
        bytes_per_day: Option<i64>,
    ) -> Result<()> {
        let (days, what) = match row.purpose {
            // A sealed file may be anything (the instance can't open it), so
            // it counts with attachments, whatever the app says it is.
            pb::MediaPurpose::Attachment | pb::MediaPurpose::Sealed => ("attachment_days", "files"),
            _ => ("upload_days", "pictures"),
        };
        db::write(&self.db, async |conn| {
            let now = now_ms();
            // Every reservation writes the account's row for the day, so ones
            // made at once clash here and the counts below hold.
            let day = now / DAY_MS;
            conn.execute(
                &format!(
                    "INSERT INTO {days} (account_id, day, bytes) VALUES (?1, ?2, ?3)
                     ON CONFLICT (account_id, day) DO UPDATE SET bytes = bytes + excluded.bytes"
                ),
                (row.account_id.as_str(), day, row.size),
            )
            .await?;
            if let Some(cap) = bytes_per_day {
                let today = query_one(
                    conn,
                    &format!("SELECT bytes FROM {days} WHERE account_id = ?1 AND day = ?2"),
                    (row.account_id.as_str(), day),
                    |r| r.get::<i64>(0),
                )
                .await?
                .unwrap_or(0);
                if today > cap {
                    return Err(Error::ResourceExhausted(format!(
                        "you can upload {} of {what} a day here; try again tomorrow",
                        crate::media::size_label(cap)
                    )));
                }
            }
            let pending = query_one(
                conn,
                "SELECT count(*) FROM media WHERE account_id = ?1 AND stored_at IS NULL AND expires_at > ?2",
                (row.account_id.as_str(), now),
                |r| r.get::<i64>(0),
            )
            .await?
            .unwrap_or(0);
            if pending >= crate::media::MAX_PENDING_UPLOADS {
                return Err(Error::ResourceExhausted("finish the uploads you started first".into()));
            }
            conn.execute(
                "INSERT INTO media (id, account_id, purpose, content_type, size, upload_hash, created_at, expires_at,
                                    server_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                (
                    row.id.as_str(),
                    row.account_id.as_str(),
                    row.purpose as i64,
                    row.content_type.as_str(),
                    row.size,
                    upload_hash,
                    now,
                    expires_at,
                    row.server_id.as_deref(),
                ),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// Reserves the row for a home server's copy of a file a guest server on
    /// another instance sends into a shared channel (see
    /// [`crate::shared_files`]): counted for the day under `account_id`
    /// (that server's "shared:<id>@<instance>"), refused past
    /// `bytes_per_day`. It's the home's and used, by the message being
    /// written; until its bytes are stored it's never served, and swept
    /// if they don't come.
    pub async fn reserve_shared_media(
        &self,
        id: &str,
        account_id: &str,
        home_id: &str,
        size: i64,
        bytes_per_day: Option<i64>,
    ) -> Result<()> {
        db::write(&self.db, async |conn| {
            let now = now_ms();
            let day = now / DAY_MS;
            conn.execute(
                "INSERT INTO attachment_days (account_id, day, bytes) VALUES (?1, ?2, ?3)
                 ON CONFLICT (account_id, day) DO UPDATE SET bytes = bytes + excluded.bytes",
                (account_id, day, size),
            )
            .await?;
            if let Some(cap) = bytes_per_day {
                let today = query_one(
                    conn,
                    "SELECT bytes FROM attachment_days WHERE account_id = ?1 AND day = ?2",
                    (account_id, day),
                    |r| r.get::<i64>(0),
                )
                .await?
                .unwrap_or(0);
                if today > cap {
                    return Err(Error::ResourceExhausted(format!(
                        "that server can send {} of files a day here; try again tomorrow",
                        crate::media::size_label(cap)
                    )));
                }
            }
            conn.execute(
                "INSERT INTO media (id, account_id, purpose, content_type, size, created_at, expires_at, server_id,
                                    used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?6)",
                (
                    id,
                    account_id,
                    pb::MediaPurpose::Attachment as i64,
                    crate::media::OCTET_STREAM,
                    size,
                    now,
                    now + crate::media::RECEIVE_TTL_MS,
                    home_id,
                ),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// Uses up an upload link: the reserved upload it's for, if it still
    /// works. It then has until `receive_by` to arrive.
    pub async fn start_upload(&self, upload_hash: &str, now: i64, receive_by: i64) -> Result<Option<MediaRow>> {
        db::write(&self.db, async |conn| {
            let Some(id) = query_one(
                conn,
                "SELECT id FROM media WHERE upload_hash = ?1 AND stored_at IS NULL AND expires_at > ?2",
                (upload_hash, now),
                |r| r.get::<String>(0),
            )
            .await?
            else {
                return Ok(None);
            };
            conn.execute(
                "UPDATE media SET upload_hash = NULL, expires_at = ?2 WHERE id = ?1",
                (id.as_str(), receive_by),
            )
            .await?;
            media_by_id(conn, &id).await
        })
        .await
    }

    /// Records that an upload's bytes arrived, and what they turned out to be.
    /// Marks an upload stored, with its type and its size as kept (smaller
    /// than it came when its metadata was taken out).
    pub async fn finish_upload(&self, id: &str, content_type: &str, size: i64, now: i64) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE media SET stored_at = ?2, content_type = ?3, size = ?4 WHERE id = ?1",
                (id, now, content_type, size),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// Every stored picture here, and what it is.
    pub async fn stored_media(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT id, content_type FROM media WHERE stored_at IS NOT NULL ORDER BY id", (), |r| {
            Ok((r.get::<String>(0)?, r.get::<String>(1)?))
        })
        .await
    }

    /// Records a stored picture's new size, once its metadata was taken out.
    pub async fn set_media_size(&self, id: &str, size: i64) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE media SET size = ?2 WHERE id = ?1 AND stored_at IS NOT NULL", (id, size)).await?;
            Ok(())
        })
        .await
    }

    pub async fn media(&self, id: &str) -> Result<Option<MediaRow>> {
        media_by_id(&*self.read()?, id).await
    }

    /// Marks a picture as in use, so it isn't swept; a server's pictures
    /// also note their server. One uploaded for a server is that server's;
    /// otherwise whatever used it first owns it: an account's avatar set as a
    /// webhook's picture stays the account's, and an emoji added to a second
    /// server stays the first's (and where it's kept).
    pub async fn use_media(&self, id: &str, server_id: Option<&str>) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE media SET server_id = CASE WHEN used_at IS NULL THEN coalesce(server_id, ?3) ELSE server_id END,
                                  used_at = coalesce(used_at, ?2) WHERE id = ?1",
                (id, now_ms(), server_id),
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// Moves a file `account_id` uploaded for the guest server `from` to the
    /// shared channel's home `to` it's sent in, marking it used: only a
    /// stored attachment no message has yet. Whether it moved; a second try
    /// for the same file finds it taken.
    pub async fn take_media(&self, id: &str, from: &str, to: &str, account_id: &str) -> Result<bool> {
        db::write(&self.db, async |conn| {
            let changed = conn
                .execute(
                    "UPDATE media SET server_id = ?3, used_at = ?4
                     WHERE id = ?1 AND server_id = ?2 AND account_id = ?5 AND purpose = ?6
                       AND stored_at IS NOT NULL AND used_at IS NULL",
                    (id, from, to, now_ms(), account_id, pb::MediaPurpose::Attachment as i64),
                )
                .await?;
            Ok(changed == 1)
        })
        .await
    }

    /// A server's files in use: its icon, banner, emoji, decorations,
    /// webhooks' pictures and messages' attachments.
    pub async fn server_media(&self, server_id: &str) -> Result<Vec<String>> {
        let conn = self.read()?;
        query_all(
            &conn,
            "SELECT id FROM media WHERE server_id = ?1 AND stored_at IS NOT NULL AND used_at IS NOT NULL
             AND purpose IN (?2, ?3, ?4, ?5, ?6, ?7) ORDER BY id",
            (
                server_id,
                pb::MediaPurpose::ServerIcon as i64,
                pb::MediaPurpose::Emoji as i64,
                pb::MediaPurpose::Avatar as i64,
                pb::MediaPurpose::Attachment as i64,
                pb::MediaPurpose::Banner as i64,
                pb::MediaPurpose::Decoration as i64,
            ),
            |r| r.get::<String>(0),
        )
        .await
    }

    /// Every picture uploaded for or used by a server, whatever its state:
    /// they go when the server is deleted.
    pub async fn media_of_server(&self, server_id: &str) -> Result<Vec<String>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT id FROM media WHERE server_id = ?1 ORDER BY id", [server_id], |r| r.get::<String>(0))
            .await
    }

    pub async fn delete_media(&self, ids: &[String]) -> Result<()> {
        db::write(&self.db, async |conn| {
            for id in ids {
                conn.execute("DELETE FROM media WHERE id = ?1", [id.as_str()]).await?;
            }
            Ok(())
        })
        .await
    }

    /// Uploads to sweep at `now`: ones whose bytes never came, and ones
    /// stored more than a day ago that nothing uses.
    pub async fn sweepable_media(&self, now: i64) -> Result<Vec<String>> {
        let conn = self.read()?;
        query_all(
            &conn,
            "SELECT id FROM media WHERE (stored_at IS NULL AND expires_at <= ?1)
                                     OR (stored_at IS NOT NULL AND used_at IS NULL AND stored_at <= ?2)",
            (now, now - crate::media::UNUSED_TTL_MS),
            |r| r.get::<String>(0),
        )
        .await
    }

    /// Everything an account uploaded that goes with it when it's deleted:
    /// all but the server icons and emoji in use, which belong to their servers.
    pub async fn account_media(&self, account_id: &str) -> Result<Vec<String>> {
        let conn = self.read()?;
        query_all(
            &conn,
            // Pictures in use by a server (its icon, emoji and webhooks) stay
            // with it, and the instance's decorations with the instance.
            "SELECT id FROM media WHERE account_id = ?1
             AND NOT ((purpose IN (?2, ?3, ?4) OR server_id IS NOT NULL) AND used_at IS NOT NULL)",
            (
                account_id,
                pb::MediaPurpose::ServerIcon as i64,
                pb::MediaPurpose::Emoji as i64,
                pb::MediaPurpose::Decoration as i64,
            ),
            |r| r.get::<String>(0),
        )
        .await
    }

    /// An account's kept backgrounds, newest first.
    pub async fn backgrounds(&self, account_id: &str) -> Result<Vec<MediaRow>> {
        let conn = self.read()?;
        let ids = query_all(
            &conn,
            "SELECT id FROM media WHERE account_id = ?1 AND purpose = ?2 AND used_at IS NOT NULL
             ORDER BY used_at DESC, id DESC",
            (account_id, pb::MediaPurpose::Background as i64),
            |r| r.get::<String>(0),
        )
        .await?;
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(row) = media_by_id(&conn, &id).await? {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    pub async fn media_ids(&self) -> Result<HashSet<String>> {
        let conn = self.read()?;
        Ok(query_all(&conn, "SELECT id FROM media", (), |r| r.get::<String>(0)).await?.into_iter().collect())
    }

    /// How many pictures are in use, and their bytes.
    pub async fn picture_totals(&self) -> Result<(i64, i64)> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT count(*), coalesce(sum(size), 0) FROM media WHERE used_at IS NOT NULL", (), |r| {
            Ok((r.get::<i64>(0)?, r.get::<i64>(1)?))
        })
        .await?
        .unwrap_or_default())
    }

    // ───────────────────────── Deleting ─────────────────────────

    /// How many instance admins there are.
    pub async fn admin_count(&self) -> Result<i64> {
        let conn = self.read()?;
        Ok(query_one(&conn, "SELECT count(*) FROM accounts WHERE admin", (), |r| r.get::<i64>(0)).await?.unwrap_or(0))
    }

    /// Deletes an account and everything node.db keeps about it.
    pub async fn delete_account(&self, account_id: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            for table in [
                "sessions",
                "backup_codes",
                "sign_in_tickets",
                "notification_settings",
                "upload_days",
                "server_arrangements",
                "attachment_days",
                "saved_gifs",
                "presence_settings",
                "account_providers",
                "agent_endpoints",
            ] {
                conn.execute(&format!("DELETE FROM {table} WHERE account_id = ?1"), [account_id]).await?;
            }
            conn.execute("DELETE FROM accounts WHERE id = ?1", [account_id]).await?;
            Ok(())
        })
        .await
    }

    // ───────────────────────── Agents ─────────────────────────

    /// Makes an agent and its token's session, which never runs out.
    pub async fn create_agent(
        &self,
        owner_id: &str,
        username: &str,
        display_name: &str,
        token_hash: &str,
    ) -> Result<Account> {
        let result = db::write(&self.db, async |conn| {
            let now = now_ms();
            let id = new_id();
            conn.execute(
                "INSERT INTO accounts (id, kind, username, display_name, owner_id, admin, created_at, updated_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6, ?6)",
                (id.as_str(), pb::AccountKind::Agent as i64, username, display_name, owner_id, now),
            )
            .await?;
            insert_agent_session(conn, &id, token_hash, now).await?;
            Ok(Account {
                id,
                kind: pb::AccountKind::Agent,
                username: username.to_string(),
                display_name: display_name.to_string(),
                avatar_url: String::new(),
                admin: false,
                created_at: now,
                last_seen_at: now,
                status: String::new(),
                status_expires_at: None,
                two_factor: false,
                disabled: false,
                decoration_id: String::new(),
            })
        })
        .await;
        result.map_err(|err| {
            if db::is_unique_violation(&err) { Error::AlreadyExists("that username is taken".into()) } else { err }
        })
    }

    /// The agents someone made, oldest first.
    pub async fn agents(&self, owner_id: &str) -> Result<Vec<AgentRow>> {
        let conn = self.read()?;
        query_all(
            &conn,
            &format!("{AGENT_SELECT} WHERE accounts.kind = ?1 AND accounts.owner_id = ?2 ORDER BY accounts.created_at, accounts.id"),
            (pb::AccountKind::Agent as i64, owner_id),
            agent_row,
        )
        .await
    }

    /// An agent, by id or by username.
    pub async fn agent(&self, id: Option<&str>, username: Option<&str>) -> Result<Option<AgentRow>> {
        let conn = self.read()?;
        let (column, value) = match (id, username) {
            (Some(id), _) => ("id", id),
            (None, Some(username)) => ("username", username),
            (None, None) => return Ok(None),
        };
        query_one(
            &conn,
            &format!("{AGENT_SELECT} WHERE accounts.kind = ?1 AND accounts.{column} = ?2"),
            (pb::AccountKind::Agent as i64, value),
            agent_row,
        )
        .await
    }

    /// How many agents someone has made.
    pub async fn agent_count(&self, owner_id: &str) -> Result<i64> {
        let conn = self.read()?;
        Ok(query_one(
            &conn,
            "SELECT count(*) FROM accounts WHERE kind = ?1 AND owner_id = ?2",
            (pb::AccountKind::Agent as i64, owner_id),
            |r| r.get::<i64>(0),
        )
        .await?
        .unwrap_or(0))
    }

    pub async fn set_agent_public(&self, id: &str, public: bool) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE accounts SET public = ?2, updated_at = ?3 WHERE id = ?1", (id, public, now_ms()))
                .await?;
            Ok(())
        })
        .await
    }

    /// Swaps an agent's token: its old sessions end and the new one starts.
    pub async fn reset_agent_token(&self, id: &str, token_hash: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("DELETE FROM sessions WHERE account_id = ?1", [id]).await?;
            insert_agent_session(conn, id, token_hash, now_ms()).await
        })
        .await
    }

    /// An agent's endpoint, made with `secret` if it has none yet, and
    /// whether it was made just now.
    pub async fn agent_endpoint(&self, agent_id: &str, secret: &str) -> Result<(EndpointRow, bool)> {
        db::write(&self.db, async |conn| {
            let made = conn
                .execute(
                    "INSERT INTO agent_endpoints (account_id, secret, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (account_id) DO NOTHING",
                    (agent_id, secret, now_ms()),
                )
                .await?;
            Ok((endpoint_of(conn, agent_id).await?, made > 0))
        })
        .await
    }

    /// Sets an agent's endpoint (an empty `url` turns it off) and starts its
    /// deliveries again from now, as a new epoch.
    pub async fn set_agent_endpoint(&self, agent_id: &str, url: &str, events: &[String]) -> Result<EndpointRow> {
        let events = events.join(",");
        db::write(&self.db, async |conn| {
            conn.execute(
                "UPDATE agent_endpoints SET url = ?2, events = ?3, epoch = epoch + 1, updated_at = ?4,
                 failing_since = NULL, last_error = '', disabled_at = NULL WHERE account_id = ?1",
                (agent_id, url, events.as_str(), now_ms()),
            )
            .await?;
            endpoint_of(conn, agent_id).await
        })
        .await
    }

    pub async fn reset_agent_endpoint_secret(&self, agent_id: &str, secret: &str) -> Result<EndpointRow> {
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE agent_endpoints SET secret = ?2 WHERE account_id = ?1", (agent_id, secret)).await?;
            endpoint_of(conn, agent_id).await
        })
        .await
    }

    /// The endpoints of these agents that are on, with the agents. An agent
    /// turned off, or whose owner is, has none.
    pub async fn active_agent_endpoints(&self, agent_ids: &[String]) -> Result<Vec<(Account, EndpointRow)>> {
        let conn = self.read()?;
        let mut found = Vec::new();
        for agent_id in agent_ids {
            let Some(row) = query_one(
                &conn,
                &format!("{ENDPOINT_SELECT} WHERE account_id = ?1 AND url != '' AND disabled_at IS NULL"),
                [agent_id.as_str()],
                endpoint_row,
            )
            .await?
            else {
                continue;
            };
            let Some(account) = self.account(agent_id).await?.filter(|account| !account.disabled) else { continue };
            let owner_off = query_one(
                &conn,
                "SELECT 1 FROM accounts AS agents JOIN accounts AS owners ON owners.id = agents.owner_id
                 WHERE agents.id = ?1 AND owners.disabled_at IS NOT NULL",
                [agent_id.as_str()],
                |r| r.get::<i64>(0),
            )
            .await?
            .is_some();
            if !owner_off {
                found.push((account, row));
            }
        }
        Ok(found)
    }

    /// Notes how a delivery to an endpoint went, if it's still at `epoch`:
    /// the first failure starts the clock, and failing for `give_up_after`
    /// ms turns the endpoint off. Whether it's off now.
    pub async fn report_agent_delivery(
        &self,
        agent_id: &str,
        epoch: i64,
        error: &str,
        give_up_after: i64,
    ) -> Result<bool> {
        let now = now_ms();
        db::write(&self.db, async |conn| {
            if error.is_empty() {
                conn.execute(
                    "UPDATE agent_endpoints SET last_delivered_at = ?3, failing_since = NULL, last_error = ''
                     WHERE account_id = ?1 AND epoch = ?2",
                    (agent_id, epoch, now),
                )
                .await?;
            } else {
                conn.execute(
                    "UPDATE agent_endpoints SET failing_since = coalesce(failing_since, ?3), last_error = ?4,
                     disabled_at = CASE WHEN ?3 - coalesce(failing_since, ?3) >= ?5 THEN ?3 ELSE disabled_at END
                     WHERE account_id = ?1 AND epoch = ?2",
                    (agent_id, epoch, now, error, give_up_after),
                )
                .await?;
            }
            let row = endpoint_of(conn, agent_id).await?;
            Ok(row.epoch != epoch || row.url.is_empty() || row.disabled_at.is_some())
        })
        .await
    }

    /// Hands an upload to another account, such as an agent's picture its
    /// owner uploaded, so it goes with that account.
    pub async fn give_media(&self, id: &str, account_id: &str) -> Result<()> {
        db::write(&self.db, async |conn| {
            conn.execute("UPDATE media SET account_id = ?2 WHERE id = ?1", (id, account_id)).await?;
            Ok(())
        })
        .await
    }

    /// Settings changed from a client, by field path, as stored JSON.
    pub async fn settings(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT key, value FROM settings ORDER BY key", (), |r| Ok((r.get(0)?, r.get(1)?))).await
    }

    /// Stores changed settings and forgets reset ones, all at once.
    pub async fn save_settings(&self, set: &[(String, String)], reset: &[String]) -> Result<()> {
        db::write(&self.db, async |conn| {
            let now = now_ms();
            for (key, value) in set {
                conn.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    (key.as_str(), value.as_str(), now),
                )
                .await?;
            }
            for key in reset {
                conn.execute("DELETE FROM settings WHERE key = ?1", [key.as_str()]).await?;
            }
            Ok(())
        })
        .await
    }

    pub async fn account_counts(&self) -> Result<AccountCounts> {
        let conn = self.read()?;
        let now = now_ms();
        let day = 24 * 60 * 60 * 1000;
        Ok(query_one(
            &conn,
            "SELECT count(*),
                    coalesce(sum(CASE WHEN last_seen_at > ?1 THEN 1 ELSE 0 END), 0),
                    coalesce(sum(CASE WHEN last_seen_at > ?2 THEN 1 ELSE 0 END), 0)
             FROM accounts",
            (now - day, now - 30 * day),
            |r| Ok(AccountCounts { total: r.get(0)?, active_1d: r.get(1)?, active_30d: r.get(2)? }),
        )
        .await?
        .unwrap_or_default())
    }
}

const AGENT_SELECT: &str =
    "SELECT accounts.id, accounts.kind, accounts.username, accounts.display_name, accounts.avatar_url,
    accounts.admin, accounts.created_at, accounts.last_seen_at, accounts.status, accounts.status_expires_at,
    accounts.totp_secret IS NOT NULL, accounts.disabled_at IS NOT NULL, accounts.profile_decoration,
    coalesce(accounts.owner_id, ''), accounts.public, accounts.bio, sessions.last_active_at
    FROM accounts LEFT JOIN sessions ON sessions.account_id = accounts.id";

fn agent_row(row: &Row) -> turso::Result<AgentRow> {
    Ok(AgentRow {
        account: account(row)?,
        owner_id: row.get(ACCOUNT_COLUMN_COUNT)?,
        public: row.get(ACCOUNT_COLUMN_COUNT + 1)?,
        bio: row.get(ACCOUNT_COLUMN_COUNT + 2)?,
        last_active_at: row.get::<Option<i64>>(ACCOUNT_COLUMN_COUNT + 3)?.filter(|at| *at > 0),
    })
}

/// An agent's endpoint as node.db keeps it (`agent_endpoints`).
pub struct EndpointRow {
    pub url: String,
    pub events: Vec<String>,
    pub secret: String,
    pub epoch: i64,
    pub updated_at: i64,
    pub last_delivered_at: Option<i64>,
    pub failing_since: Option<i64>,
    pub last_error: String,
    pub disabled_at: Option<i64>,
}

const ENDPOINT_SELECT: &str = "SELECT url, events, secret, epoch, updated_at, last_delivered_at, failing_since,
    last_error, disabled_at FROM agent_endpoints";

fn endpoint_row(row: &Row) -> turso::Result<EndpointRow> {
    let events: String = row.get(1)?;
    Ok(EndpointRow {
        url: row.get(0)?,
        events: events.split(',').filter(|name| !name.is_empty()).map(str::to_string).collect(),
        secret: row.get(2)?,
        epoch: row.get(3)?,
        updated_at: row.get(4)?,
        last_delivered_at: row.get(5)?,
        failing_since: row.get(6)?,
        last_error: row.get(7)?,
        disabled_at: row.get(8)?,
    })
}

async fn endpoint_of(conn: &Connection, agent_id: &str) -> Result<EndpointRow> {
    query_one(conn, &format!("{ENDPOINT_SELECT} WHERE account_id = ?1"), [agent_id], endpoint_row)
        .await?
        .ok_or(Error::NotFound("agent endpoint"))
}

/// An agent's token as a session (an agent has one at most, so joining
/// sessions gives one row per agent): it never runs out, and its last use starts
/// at zero so the first one is recorded at once.
async fn insert_agent_session(conn: &Connection, account_id: &str, token_hash: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (token_hash, id, account_id, user_agent, created_at, last_active_at, expires_at)
         VALUES (?1, ?2, ?3, 'Agent token', ?4, 0, ?5)",
        (token_hash, new_id(), account_id, now, i64::MAX),
    )
    .await?;
    Ok(())
}

async fn media_by_id(conn: &Connection, id: &str) -> Result<Option<MediaRow>> {
    query_one(
        conn,
        "SELECT id, account_id, purpose, content_type, size, stored_at IS NOT NULL, used_at IS NOT NULL, server_id
         FROM media WHERE id = ?1",
        [id],
        |r| {
            Ok(MediaRow {
                id: r.get(0)?,
                account_id: r.get(1)?,
                purpose: pb::MediaPurpose::try_from(r.get::<i32>(2)?).unwrap_or(pb::MediaPurpose::Unspecified),
                content_type: r.get(3)?,
                size: r.get(4)?,
                stored: r.get(5)?,
                used: r.get(6)?,
                server_id: r.get(7)?,
            })
        },
    )
    .await
}

fn summary_row(row: &Row) -> turso::Result<AccountSummary> {
    Ok(AccountSummary {
        account: account(row)?,
        disabled_at: row.get(ACCOUNT_COLUMN_COUNT)?,
        disabled_reason: row.get(ACCOUNT_COLUMN_COUNT + 1)?,
        sessions: row.get(ACCOUNT_COLUMN_COUNT + 2)?,
    })
}

async fn replace_backup_codes(conn: &Connection, account_id: &str, code_hashes: &[String]) -> Result<()> {
    conn.execute("DELETE FROM backup_codes WHERE account_id = ?1", [account_id]).await?;
    for hash in code_hashes {
        conn.execute("INSERT INTO backup_codes (account_id, code_hash) VALUES (?1, ?2)", (account_id, hash.as_str()))
            .await?;
    }
    Ok(())
}

fn notification_row(r: &Row) -> turso::Result<pb::NotificationSettings> {
    Ok(pb::NotificationSettings {
        server_id: r.get(0)?,
        channel_id: r.get(1)?,
        level: r.get(2)?,
        muted: r.get(3)?,
        muted_until: r.get::<Option<i64>>(4)?.map(timestamp),
        suppress_everyone: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node.db from before devices were listed: its sessions get ids and a
    /// last-active time, and still sign in.
    #[tokio::test]
    async fn older_sessions_get_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.db");
        {
            let db = db::open(&path, None, &MIGRATIONS[..2]).await.unwrap();
            let conn = db::connect(&db).unwrap();
            conn.execute_batch(
                "BEGIN CONCURRENT;
                 INSERT INTO accounts (id, kind, username, display_name, admin, created_at, updated_at, last_seen_at)
                   VALUES ('a1', 1, 'juan', 'Juan', 1, 5, 5, 5);
                 INSERT INTO sessions (token_hash, account_id, created_at, expires_at) VALUES ('t1', 'a1', 7, 9999999999999);
                 INSERT INTO sessions (token_hash, account_id, created_at, expires_at) VALUES ('t2', 'a1', 8, 9999999999999);
                 COMMIT;",
            )
            .await
            .unwrap();
        }

        let node = NodeDb::open(&path, None).await.unwrap();
        let sessions = node.sessions("a1", "t1").await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].current && !sessions[1].current);
        assert_eq!((sessions[0].last_active_at, sessions[1].last_active_at), (7, 8));
        assert!(sessions.iter().all(|s| s.id.len() == 32));
        assert_ne!(sessions[0].id, sessions[1].id);
        let account = node.session_account("t2").await.unwrap().unwrap();
        assert_eq!(account.username, "juan");
        assert!(!account.two_factor);
        assert!(node.delete_session_by_id("a1", &sessions[1].id).await.unwrap());
        assert!(node.session_account("t2").await.unwrap().is_none());
    }
}
