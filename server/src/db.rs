//! Thin helpers over the `turso` crate: opening files, migrations, and the
//! write path every change goes through.

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

/// Opens (or creates) a database file and brings its schema up to date.
pub async fn open(path: &Path, key: Option<&EncryptionKey>, migrations: &[&str]) -> Result<Database> {
    let path = path.to_str().ok_or_else(|| Error::internal(format!("{} is not valid UTF-8", path.display())))?;
    let mut builder = Builder::new_local(path);
    if let Some(key) = key {
        builder = builder
            .experimental_encryption(true)
            .with_encryption(turso::EncryptionOpts { cipher: "aegis256".into(), hexkey: key.0.clone() });
    }
    let db = builder.build().await?;
    let conn = connect(&db)?;
    pragma(&conn, "PRAGMA journal_mode = wal").await?;
    migrate(&conn, migrations).await?;
    Ok(db)
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
/// Migration N (1-based) runs once, in its own transaction.
async fn migrate(conn: &Connection, migrations: &[&str]) -> Result<()> {
    let current = query_one(conn, "PRAGMA user_version", (), |row| row.get::<i64>(0)).await?.unwrap_or(0);
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

/// Runs `f` inside a write transaction on `conn`, committing if it succeeds.
/// A transaction left open by a cancelled request is rolled back first.
pub async fn transaction<T>(conn: &Connection, f: impl AsyncFnOnce(&Connection) -> Result<T>) -> Result<T> {
    if !conn.is_autocommit()? {
        conn.execute("ROLLBACK", ()).await?;
    }
    conn.execute("BEGIN IMMEDIATE", ()).await?;
    match f(conn).await {
        Ok(value) => match conn.execute("COMMIT", ()).await {
            Ok(_) => Ok(value),
            Err(err) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(err.into())
            }
        },
        Err(err) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(err)
        }
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
