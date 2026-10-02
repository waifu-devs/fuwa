//! Thin helpers over the `turso` crate: opening files, migrations, and the
//! write path every change goes through.
//!
//! Every database runs in Turso's concurrent-writer mode (MVCC): writes start
//! with `BEGIN CONCURRENT`, run side by side, and are retried when two of them
//! touch the same row. Anything a write must not race on (a cap, a count) has
//! to live in a row that every such write updates, so they clash instead.

use std::path::Path;

use turso::{Builder, Connection, Database, IntoParams, Row};

use crate::error::{Error, Result};

/// A database-at-rest key: 32 bytes as 64 hex characters (AEGIS-256).
#[derive(Clone)]
pub struct EncryptionKey(String);

impl EncryptionKey {
    pub fn parse(hex: &str) -> std::result::Result<Self, String> {
        let hex = hex.trim();
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("must be 64 hex characters (32 bytes), e.g. from `openssl rand -hex 32`".into());
        }
        Ok(Self(hex.to_ascii_lowercase()))
    }
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EncryptionKey(***)")
    }
}

/// Opens (or creates) a database file in Turso's concurrent-writer mode and
/// brings its schema up to date.
///
/// The mode (MVCC) lives in the file's header, so this switches a file once,
/// including one made before fuwa used it, and is a no-op after. Encryption has
/// to be set up before the switch, which the builder does.
pub async fn open(path: &Path, key: Option<&EncryptionKey>, migrations: &[&str]) -> Result<Database> {
    let db = build(path, key).await.map_err(|err| explain_key(err, path, key.is_some()))?;
    let conn = connect(&db)?;
    let mode = query_one(&conn, "PRAGMA journal_mode = 'mvcc'", (), |r| r.get::<String>(0))
        .await
        .map_err(|err| explain_key(err, path, key.is_some()))?;
    if mode.as_deref() != Some("mvcc") {
        return Err(Error::internal(format!("{} stayed in {mode:?} journal mode", path.display())));
    }
    migrate(&conn, migrations).await?;
    Ok(db)
}

/// Opening a file is where the wrong FUWA_ENCRYPTION_KEY shows up, as Turso's
/// bare "Decryption failed"; this says what to check.
fn explain_key(err: Error, path: &Path, encrypted: bool) -> Error {
    let text = err.to_string();
    let hint = if encrypted && text.contains("Decryption failed") {
        "FUWA_ENCRYPTION_KEY isn't the key this data was made with (data made without a key can't be opened with one)"
    } else if !encrypted && text.contains("not a database") {
        "It may be encrypted: set FUWA_ENCRYPTION_KEY to the key this data was made with"
    } else {
        return err;
    };
    Error::internal(format!("{}: {text}. {hint}", path.display()))
}

/// Switches a file back to plain SQLite (WAL), so other SQLite tools can open
/// it. fuwa switches it back to concurrent writes the next time it opens it.
pub async fn to_sqlite(path: &Path, key: Option<&EncryptionKey>) -> Result<()> {
    let db = build(path, key).await?;
    let conn = connect(&db)?;
    let mode = query_one(&conn, "PRAGMA journal_mode = 'wal'", (), |r| r.get::<String>(0)).await?;
    if mode.as_deref() != Some("wal") {
        return Err(Error::internal(format!("{} stayed in {mode:?} journal mode", path.display())));
    }
    Ok(())
}

async fn build(path: &Path, key: Option<&EncryptionKey>) -> Result<Database> {
    let path = path.to_str().ok_or_else(|| Error::internal(format!("{} is not valid UTF-8", path.display())))?;
    let mut builder = Builder::new_local(path);
    if let Some(key) = key {
        builder = builder
            .experimental_encryption(true)
            .with_encryption(turso::EncryptionOpts { cipher: "aegis256".into(), hexkey: key.0.clone() });
    }
    Ok(builder.build().await?)
}

/// Runs a pragma, ignoring whatever rows it answers with.
pub async fn pragma(conn: &Connection, sql: &str) -> Result<()> {
    let mut rows = conn.query(sql, ()).await?;
    while rows.next().await?.is_some() {}
    Ok(())
}

/// A new connection with the settings every connection needs.
pub fn connect(db: &Database) -> Result<Connection> {
    let conn = db.connect()?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    Ok(conn)
}

/// Applies the migrations the database hasn't seen yet, tracked in `user_version`.
/// Migration N (1-based) runs once, in its own transaction. Schema changes need
/// an exclusive transaction; this runs before anything else writes to the file.
async fn migrate(conn: &Connection, migrations: &[&str]) -> Result<()> {
    let current = query_one(conn, "PRAGMA user_version", (), |r| r.get::<i64>(0)).await?.unwrap_or(0);
    for (index, sql) in migrations.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let applied = async {
            conn.execute_batch(sql).await?;
            conn.execute_batch(&format!("PRAGMA user_version = {version}")).await?;
            Ok::<_, Error>(())
        }
        .await;
        match applied {
            Ok(()) => conn.execute("COMMIT", ()).await.map(|_| ())?,
            Err(err) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(Error::internal(format!("migration {version} failed: {err}")));
            }
        }
    }
    Ok(())
}

/// How many times a write that lost a clash is run again before giving up.
const ATTEMPTS: u32 = 50;

/// Runs `f` once in a concurrent write transaction on a connection of its
/// own: for writes that may simply give way when they clash.
pub async fn write_once<T>(db: &Database, f: impl AsyncFnOnce(&Connection) -> Result<T>) -> Result<T> {
    let conn = connect(db)?;
    begin(&conn).await?;
    let result = match f(&conn).await {
        Ok(value) => conn.execute("COMMIT", ()).await.map(|_| value).map_err(Error::from),
        Err(err) => Err(err),
    };
    if result.is_err() {
        abort(&conn).await;
    }
    result
}

/// Runs `f` in a concurrent write transaction on a connection of its own,
/// committing if it succeeds. See [`transaction`].
pub async fn write<T>(db: &Database, f: impl AsyncFnOnce(&Connection) -> Result<T> + Clone) -> Result<T> {
    transaction(&connect(db)?, f).await
}

/// Runs `f` in a concurrent write transaction (`BEGIN CONCURRENT`) on `conn`,
/// committing if it succeeds. Writers run side by side, and only two touching
/// the same row clash: the later one fails, at that statement or at COMMIT,
/// and Turso has already rolled it back. A copy of `f` then runs again from
/// the start on a fresh snapshot, after a short random pause, so `f` must not
/// have effects outside the transaction (closures that borrow what they use
/// are `Clone`).
pub async fn transaction<T>(conn: &Connection, f: impl AsyncFnOnce(&Connection) -> Result<T> + Clone) -> Result<T> {
    let mut attempt = 0;
    loop {
        begin(conn).await?;
        let result = match f.clone()(conn).await {
            Ok(value) => conn.execute("COMMIT", ()).await.map(|_| value).map_err(Error::from),
            Err(err) => Err(err),
        };
        match result {
            Err(err) if is_conflict(&err) => {
                abort(conn).await;
                retry_after(&err, &mut attempt).await?;
            }
            Err(err) => {
                abort(conn).await;
                return Err(err);
            }
            Ok(value) => return Ok(value),
        }
    }
}

/// Waits before running a write that lost a clash again, or gives up after
/// [`ATTEMPTS`] of them.
pub async fn retry_after(err: &Error, attempt: &mut u32) -> Result<()> {
    *attempt += 1;
    if *attempt >= ATTEMPTS {
        tracing::warn!(error = %err, attempts = ATTEMPTS, "gave up on a write that kept clashing");
        return Err(Error::Busy);
    }
    tracing::debug!(error = %err, attempt = *attempt, "a write clashed with another; running it again");
    tokio::time::sleep(backoff(*attempt)).await;
    Ok(())
}

/// A random pause that grows with each attempt (up to 32 ms), so writers that
/// clashed don't clash again in step.
fn backoff(attempt: u32) -> std::time::Duration {
    let ceiling = 250u64 << attempt.min(7);
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    std::time::Duration::from_micros(u64::from_le_bytes(bytes) % ceiling)
}

/// Starts a concurrent write transaction, clearing one a cancelled request left open.
pub async fn begin(conn: &Connection) -> Result<()> {
    abort(conn).await;
    conn.execute("BEGIN CONCURRENT", ()).await?;
    Ok(())
}

/// Rolls back the open transaction, if there is one: after a clash Turso has
/// rolled it back already.
pub async fn abort(conn: &Connection) {
    if !conn.is_autocommit().unwrap_or(true) {
        let _ = conn.execute("ROLLBACK", ()).await;
    }
}

/// Whether a write failed because another writer got to the same rows first.
pub fn is_conflict(err: &Error) -> bool {
    match err {
        Error::Database(turso::Error::Busy(_) | turso::Error::BusySnapshot(_)) => true,
        Error::Database(turso::Error::Error(message)) => {
            ["Write-write conflict", "Commit dependency aborted", "Database schema conflict"]
                .iter()
                .any(|clash| message.contains(clash))
        }
        _ => false,
    }
}

pub async fn query_one<T>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
    map: impl Fn(&Row) -> turso::Result<T>,
) -> Result<Option<T>> {
    let mut rows = conn.query(sql, params).await?;
    let row = rows.next().await?;
    Ok(row.as_ref().map(map).transpose()?)
}

pub async fn query_all<T>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
    map: impl Fn(&Row) -> turso::Result<T>,
) -> Result<Vec<T>> {
    let mut rows = conn.query(sql, params).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(map(&row)?);
    }
    Ok(out)
}

/// Whether a database error is a UNIQUE constraint failing.
pub fn is_unique_violation(err: &Error) -> bool {
    matches!(err, Error::Database(e) if e.to_string().contains("UNIQUE constraint failed"))
}
