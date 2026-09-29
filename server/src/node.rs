//! The instance's own database: accounts, sessions and settings.

use std::path::Path;

use tokio::sync::Mutex;
use turso::{Connection, Database, Row};

use crate::db::{self, EncryptionKey, query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms};
use crate::pb;

const MIGRATIONS: &[&str] =
    &[include_str!("../migrations/node/0001_init.sql"), include_str!("../migrations/node/0002_settings.sql")];

/// How long a session lasts after sign-in.
pub const SESSION_TTL_MS: i64 = 60 * 24 * 60 * 60 * 1000;

/// How often an account's last-seen time is written, at most.
const SEEN_RESOLUTION_MS: i64 = 60 * 60 * 1000;

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
}

impl Account {
    pub fn user(&self) -> pb::User {
        pb::User {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            avatar_url: self.avatar_url.clone(),
            kind: self.kind as i32,
        }
    }
}

const ACCOUNT_COLUMNS: &str = "id, kind, username, display_name, avatar_url, admin, created_at, last_seen_at";

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
    })
}

pub struct NodeDb {
    db: Database,
    writer: Mutex<Connection>,
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
        let db = db::open(path, key, MIGRATIONS).await?;
        let writer = Mutex::new(db::connect(&db)?);
        Ok(Self { db, writer })
    }

    fn read(&self) -> Result<Connection> {
        db::connect(&self.db)
    }

    /// This instance's random, anonymous id, made the first time it's asked for.
    pub async fn install_id(&self) -> Result<String> {
        let conn = self.writer.lock().await;
        db::transaction(&conn, async |conn| {
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
        let conn = self.writer.lock().await;
        let result = db::transaction(&conn, async |conn| {
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
            })
        })
        .await;
        result.map_err(|err| {
            if db::is_unique_violation(&err) { Error::AlreadyExists("that username is taken".into()) } else { err }
        })
    }

    /// A standalone account and its password hash, by username.
    pub async fn local_account_by_username(&self, username: &str) -> Result<Option<(Account, String)>> {
        let conn = self.read()?;
        query_one(
            &conn,
            &format!("SELECT {ACCOUNT_COLUMNS}, password_hash FROM accounts WHERE username = ?1 AND kind = ?2"),
            (username, pb::AccountKind::Local as i64),
            |row| Ok((account(row)?, row.get::<Option<String>>(8)?.unwrap_or_default())),
        )
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

    pub async fn update_profile(
        &self,
        id: &str,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
    ) -> Result<Account> {
        let conn = self.writer.lock().await;
        conn.execute(
            "UPDATE accounts SET display_name = coalesce(?2, display_name), avatar_url = coalesce(?3, avatar_url), updated_at = ?4
             WHERE id = ?1",
            (id, display_name, avatar_url, now_ms()),
        )
        .await?;
        drop(conn);
        self.account(id).await?.ok_or(Error::NotFound("account"))
    }

    /// Replaces the password and ends every other session.
    pub async fn set_password(&self, id: &str, password_hash: &str, keep_session: &str) -> Result<()> {
        let conn = self.writer.lock().await;
        db::transaction(&conn, async |conn| {
            conn.execute(
                "UPDATE accounts SET password_hash = ?2, updated_at = ?3 WHERE id = ?1",
                (id, password_hash, now_ms()),
            )
            .await?;
            conn.execute("DELETE FROM sessions WHERE account_id = ?1 AND token_hash != ?2", (id, keep_session)).await?;
            Ok(())
        })
        .await
    }

    pub async fn create_session(&self, account_id: &str, token_hash: &str) -> Result<()> {
        let conn = self.writer.lock().await;
        let now = now_ms();
        conn.execute(
            "INSERT INTO sessions (token_hash, account_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            (token_hash, account_id, now, now + SESSION_TTL_MS),
        )
        .await?;
        Ok(())
    }

    /// The account a live session belongs to, marking the account as seen.
    pub async fn session_account(&self, token_hash: &str) -> Result<Option<Account>> {
        let conn = self.read()?;
        let now = now_ms();
        let found = query_one(
            &conn,
            &format!(
                "SELECT {} FROM sessions JOIN accounts ON accounts.id = sessions.account_id
                 WHERE sessions.token_hash = ?1 AND sessions.expires_at > ?2",
                ACCOUNT_COLUMNS.split(", ").map(|c| format!("accounts.{c}")).collect::<Vec<_>>().join(", ")
            ),
            (token_hash, now),
            account,
        )
        .await?;
        if let Some(account) = &found
            && now - account.last_seen_at > SEEN_RESOLUTION_MS
        {
            let conn = self.writer.lock().await;
            conn.execute("UPDATE accounts SET last_seen_at = ?2 WHERE id = ?1", (account.id.as_str(), now)).await?;
        }
        Ok(found)
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<()> {
        let conn = self.writer.lock().await;
        conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash]).await?;
        Ok(())
    }

    /// Drops sessions that have run out.
    pub async fn prune_sessions(&self) -> Result<u64> {
        let conn = self.writer.lock().await;
        Ok(conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now_ms()]).await?)
    }

    /// Settings changed from a client, by field path, as stored JSON.
    pub async fn settings(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read()?;
        query_all(&conn, "SELECT key, value FROM settings ORDER BY key", (), |r| Ok((r.get(0)?, r.get(1)?))).await
    }

    /// Stores changed settings and forgets reset ones, all at once.
    pub async fn save_settings(&self, set: &[(String, String)], reset: &[String]) -> Result<()> {
        let conn = self.writer.lock().await;
        db::transaction(&conn, async |conn| {
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
