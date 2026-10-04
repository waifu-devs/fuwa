//! Thin helpers over the `turso` crate: opening files, migrations, and the
//! write path every change goes through.
//!
//! Every database runs in Turso's concurrent-writer mode (MVCC): writes start
//! with `BEGIN CONCURRENT`, run side by side, and are retried when two of them
//! touch the same row. Anything a write must not race on (a cap, a count) has
//! to live in a row that every such write updates, so they clash instead.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, Semaphore, SemaphorePermit};
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

    /// The key's 32 bytes, for sealing files that aren't databases (call
    /// recordings), each under a key derived from it.
    pub fn bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.0[2 * i..2 * i + 2], 16).unwrap_or_default();
        }
        out
    }
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EncryptionKey(***)")
    }
}

/// An open database file, and the gate its writes go through.
///
/// Every write holds the gate shared, so writes still run side by side. The
/// few that must not overlap any other hold it alone, and so does the replica
/// when it folds the file's log into it (`replica/`), so no commit lands
/// between the last bytes it reads from the log and the log starting over.
pub struct Db {
    database: Database,
    path: PathBuf,
    gate: Arc<RwLock<()>>,
    /// Writes running at once (see [`WRITE_LANES`]).
    lanes: Semaphore,
    /// Writes running or waiting for a lane (see [`WRITE_QUEUE`]).
    queued: AtomicUsize,
    /// Writes let in since the file opened, for telling busy servers apart.
    writes: AtomicU64,
    /// The replica folds this file's log itself (`replica::Replica::track`).
    replicated: AtomicBool,
    /// Folding the log in: whether a fold is running, and when to try again
    /// after one failed.
    fold: Arc<Fold>,
    /// Tasks holding a turn at writing, to catch a write started inside
    /// another on the same file (it would wait on itself for a lane).
    #[cfg(debug_assertions)]
    writers: Mutex<std::collections::HashSet<tokio::task::Id>>,
    /// Connections that finished their work, kept to be used again: opening
    /// one and parsing every statement afresh was most of what a write cost.
    idle: Arc<Mutex<Vec<(Connection, u32)>>>,
    /// How many idle connections this file keeps.
    keep: usize,
}

/// Writes to one file that run at once; the rest wait their turn. Commits
/// go one at a time anyway, and many open transactions at once only clash
/// and spin: with every write let in, 100 messages arriving together in one
/// server took 260 ms each and half the CPU; four at a time, 55 ms and a
/// quarter (docs/capacity.md).
pub const WRITE_LANES: usize = 4;

/// Writes to one file that may wait at once, unless FUWA_WRITE_QUEUE says
/// otherwise. Past this the file is busy: new writes are turned away
/// straight away, rather than piling up until they time out and taking
/// memory from every other server. A protective default, the agreed
/// exception to caps being unlimited by default.
pub const WRITE_QUEUE: usize = 512;

/// The queue in force for every file in this process ([`set_write_queue`]).
static QUEUE: AtomicUsize = AtomicUsize::new(WRITE_QUEUE);

/// Sets how many writes each file may have waiting (`None` for no limit).
pub fn set_write_queue(queue: Option<usize>) {
    QUEUE.store(queue.unwrap_or(usize::MAX), Ordering::Relaxed);
}

/// What a write is told when its file's queue is full.
pub const BUSY: &str = "this server is busy right now; try again in a moment";

/// How big a file's log may grow before fuwa folds it into the file (the
/// size at which Turso would fold it by itself). Turso's own fold is off:
/// it ran inside whichever commit crossed the size, waiting for every other
/// open transaction, while those waited on that commit, so the file's writes
/// stalled until Turso gave up (seen under load in `examples/load.rs`).
pub const FOLD_BYTES: u64 = 4_120_000;

/// A write's turn at its file: its lane and its place in the queue.
pub struct Writing<'a> {
    _lane: SemaphorePermit<'a>,
    _gate: RwLockReadGuard<'a, ()>,
    _queued: Queued<'a>,
    #[cfg(debug_assertions)]
    _writer: Writer<'a>,
}

/// The task holding a turn, forgotten when the turn ends.
#[cfg(debug_assertions)]
struct Writer<'a>(&'a Mutex<std::collections::HashSet<tokio::task::Id>>, Option<tokio::task::Id>);

#[cfg(debug_assertions)]
impl Drop for Writer<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.1 {
            self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
        }
    }
}

/// A place in a file's queue, given back however the write ends.
struct Queued<'a>(&'a AtomicUsize);

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Idle connections a file keeps by default (node.db keeps more, see [`Db::keep`]).
const KEEP_IDLE: usize = 4;
/// Uses after which a connection is closed rather than kept, so its cache of
/// prepared statements can't grow without end.
const USES: u32 = 10_000;

/// A connection from a file's pool, back in the pool when dropped (unless a
/// transaction was left open on it, by a request cancelled halfway: then it
/// is closed, so nobody reads through a stale snapshot).
pub struct Pooled {
    conn: Option<(Connection, u32)>,
    idle: Arc<Mutex<Vec<(Connection, u32)>>>,
    keep: usize,
}

impl std::ops::Deref for Pooled {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn.as_ref().expect("a pooled connection is there until dropped").0
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        let Some((conn, uses)) = self.conn.take() else { return };
        if uses >= USES || !conn.is_autocommit().unwrap_or(false) {
            return;
        }
        let mut idle = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        if idle.len() < self.keep {
            idle.push((conn, uses));
        }
    }
}

impl std::ops::Deref for Db {
    type Target = Database;

    fn deref(&self) -> &Database {
        &self.database
    }
}

impl Db {
    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file of commits not yet folded into the database (Turso's logical log).
    pub fn log_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push("-log");
        PathBuf::from(name)
    }

    /// A connection: an idle one if there is one, else a new one.
    pub fn conn(&self) -> Result<Pooled> {
        let reused = self.idle.lock().unwrap_or_else(|p| p.into_inner()).pop();
        let (conn, uses) = match reused {
            Some(found) => found,
            None => (connect(&self.database)?, 0),
        };
        Ok(Pooled { conn: Some((conn, uses + 1)), idle: self.idle.clone(), keep: self.keep })
    }

    /// Keeps up to `keep` idle connections instead of [`KEEP_IDLE`], for a
    /// file nearly every call reads (node.db).
    pub fn keep(mut self, keep: usize) -> Self {
        self.keep = keep;
        self
    }

    /// Held by every write while it runs.
    pub async fn shared(&self) -> RwLockReadGuard<'_, ()> {
        self.gate.read().await
    }

    /// A write's turn: a place in the queue (or [`BUSY`] when it's full), then
    /// one of the lanes, then the gate, shared.
    pub async fn writing(&self) -> Result<Writing<'_>> {
        if self.queued.fetch_add(1, Ordering::AcqRel) >= QUEUE.load(Ordering::Relaxed) {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            crate::reports::server_error("server_busy", None);
            return Err(Error::ResourceExhausted(BUSY.into()));
        }
        let queued = Queued(&self.queued);
        #[cfg(debug_assertions)]
        let writer = {
            let id = tokio::task::try_id();
            if let Some(id) = id {
                let fresh = self.writers.lock().unwrap_or_else(|p| p.into_inner()).insert(id);
                debug_assert!(fresh, "a write started inside another write on the same file");
            }
            Writer(&self.writers, id)
        };
        self.writes.fetch_add(1, Ordering::Relaxed);
        let lane = self.lanes.acquire().await.map_err(|_| Error::internal("the write lanes closed"))?;
        let gate = self.gate.read().await;
        Ok(Writing {
            _lane: lane,
            _gate: gate,
            _queued: queued,
            #[cfg(debug_assertions)]
            _writer: writer,
        })
    }

    /// Writes running or waiting on this file right now.
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }

    /// Writes let in since the file opened.
    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }

    /// From now on the replica folds this file's log, after shipping it.
    pub fn replicated(&self) {
        self.replicated.store(true, Ordering::Release);
    }

    /// Folds the log into the file once it has passed [`FOLD_BYTES`], unless
    /// the replica does that for this file. Called after writes, with none of
    /// the write's own guards held. The fold runs on a task of its own, so no
    /// request waits for it or can stop it halfway by going away; writes wait
    /// while it runs. After a fold fails the file carries on with a longer
    /// log, and the next try waits [`FOLD_RETRY`] or another [`FOLD_BYTES`].
    pub fn fold_if_big(&self) {
        if self.replicated.load(Ordering::Acquire) {
            return;
        }
        let log = std::fs::metadata(self.log_path()).map(|m| m.len()).unwrap_or(0);
        if log < FOLD_BYTES || !self.fold.may_start(log) {
            return;
        }
        let (database, gate, fold) = (self.database.clone(), self.gate.clone(), self.fold.clone());
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let folded = {
                let _alone = gate.write_owned().await;
                checkpoint_with(&database, FOLD_ATTEMPTS).await
            };
            crate::reports::server_timing("db.fold", started.elapsed());
            if folded.is_err() {
                tracing::warn!("couldn't fold a database's log into it; trying again later");
            }
            fold.finished(folded.is_ok(), log);
        });
    }

    /// Waits for the writes running now to finish and keeps new ones waiting.
    pub async fn alone(&self) -> RwLockWriteGuard<'_, ()> {
        self.gate.write().await
    }

    /// Folds the log into the database file and empties it. Readers and
    /// writers wait while it runs; it answers busy if a transaction is open,
    /// so it tries again a few times.
    pub async fn checkpoint(&self) -> Result<()> {
        checkpoint_with(self, ATTEMPTS).await
    }
}

/// [`Db::checkpoint`], giving up after `attempts`.
async fn checkpoint_with(database: &Database, attempts: u32) -> Result<()> {
    let conn = connect(database)?;
    let mut attempt = 0;
    loop {
        match pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)").await {
            Err(err) if is_conflict(&err) && attempt + 1 < attempts => retry_after(&err, &mut attempt).await?,
            other => return other,
        }
    }
}

/// Tries a fold of its own makes before giving up until later: writes wait
/// while it runs, so it doesn't keep them waiting through many.
const FOLD_ATTEMPTS: u32 = 3;

/// How long a file waits after a failed fold before trying again (unless its
/// log grows by another [`FOLD_BYTES`] first).
pub const FOLD_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether a fold is running, and when the next may start after one failed.
#[derive(Default)]
struct Fold {
    running: AtomicBool,
    /// After a failed fold: when, and how long the log was then.
    failed: Mutex<Option<(std::time::Instant, u64)>>,
}

impl Fold {
    /// Claims the fold, unless one is running or a failed one is too recent.
    fn may_start(&self, log: u64) -> bool {
        let failed = *self.failed.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, then)) = failed
            && at.elapsed() < FOLD_RETRY
            && log < then.saturating_add(FOLD_BYTES)
        {
            return false;
        }
        !self.running.swap(true, Ordering::AcqRel)
    }

    fn finished(&self, ok: bool, log: u64) {
        *self.failed.lock().unwrap_or_else(|p| p.into_inner()) = (!ok).then(|| (std::time::Instant::now(), log));
        self.running.store(false, Ordering::Release);
    }
}

/// Opens (or creates) a database file in Turso's concurrent-writer mode and
/// brings its schema up to date.
///
/// The mode (MVCC) lives in the file's header, so this switches a file once,
/// including one made before fuwa used it, and is a no-op after. Encryption has
/// to be set up before the switch, which the builder does.
pub async fn open(path: &Path, key: Option<&EncryptionKey>, migrations: &[&str]) -> Result<Db> {
    let db = build(path, key).await.map_err(|err| explain_key(err, path, key.is_some()))?;
    let conn = connect(&db)?;
    let mode = query_one(&conn, "PRAGMA journal_mode = 'mvcc'", (), |r| r.get::<String>(0))
        .await
        .map_err(|err| explain_key(err, path, key.is_some()))?;
    if mode.as_deref() != Some("mvcc") {
        return Err(Error::internal(format!("{} stayed in {mode:?} journal mode", path.display())));
    }
    migrate(&conn, migrations).await?;
    // fuwa folds the log itself, between writes (see FOLD_BYTES).
    pragma(&conn, "PRAGMA mvcc_checkpoint_threshold = -1").await?;
    Ok(Db {
        database: db,
        path: path.to_path_buf(),
        gate: Arc::new(RwLock::new(())),
        lanes: Semaphore::new(WRITE_LANES),
        queued: AtomicUsize::new(0),
        writes: AtomicU64::new(0),
        replicated: AtomicBool::new(false),
        fold: Arc::default(),
        #[cfg(debug_assertions)]
        writers: Mutex::default(),
        idle: Arc::new(Mutex::new(Vec::new())),
        keep: KEEP_IDLE,
    })
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

/// Opens a plain (unencrypted, WAL) file, for touching up a copy before it
/// leaves the instance.
pub async fn open_plain(path: &Path) -> Result<(Database, Connection)> {
    let db = build(path, None).await?;
    let conn = connect(&db)?;
    Ok((db, conn))
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
pub async fn write_once<T>(db: &Db, f: impl AsyncFnOnce(&Connection) -> Result<T>) -> Result<T> {
    let result = {
        let _writing = db.writing().await?;
        let conn = db.conn()?;
        begin(&conn).await?;
        let result = match f(&conn).await {
            Ok(value) => conn.execute("COMMIT", ()).await.map(|_| value).map_err(Error::from),
            Err(err) => Err(err),
        };
        if result.is_err() {
            abort(&conn).await;
        }
        result
    };
    db.fold_if_big();
    result
}

/// Runs `f` in a concurrent write transaction on a connection of its own,
/// committing if it succeeds. See [`transaction`].
pub async fn write<T>(db: &Db, f: impl AsyncFnOnce(&Connection) -> Result<T> + Clone) -> Result<T> {
    let result = {
        let _writing = db.writing().await?;
        transaction(&*db.conn()?, f).await
    };
    db.fold_if_big();
    result
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
    let mut rows = conn.prepare_cached(sql).await?.query(params).await?;
    let row = rows.next().await?;
    Ok(row.as_ref().map(map).transpose()?)
}

pub async fn query_all<T>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
    map: impl Fn(&Row) -> turso::Result<T>,
) -> Result<Vec<T>> {
    let mut rows = conn.prepare_cached(sql).await?.query(params).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn scratch(migrations: &[&str]) -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("t.db"), None, migrations).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn folds_its_own_log_once_it_is_big() {
        let (_dir, db) = scratch(&["CREATE TABLE t (id INTEGER PRIMARY KEY, body TEXT NOT NULL)"]).await;
        let body = "x".repeat(64 * 1024);
        for _ in 0..80 {
            let body = body.clone();
            write(&db, async move |conn| {
                conn.execute("INSERT INTO t (body) VALUES (?1)", [body]).await?;
                Ok(())
            })
            .await
            .unwrap();
        }
        // The fold runs on a task of its own.
        let log = || std::fs::metadata(db.log_path()).map(|m| m.len()).unwrap_or(0);
        for _ in 0..200 {
            if log() < FOLD_BYTES {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(log() < FOLD_BYTES, "the log was folded in, now {} bytes", log());
        let count = query_one(&db.conn().unwrap(), "SELECT count(*) FROM t", (), |r| r.get::<i64>(0)).await.unwrap();
        assert_eq!(count, Some(80));
    }

    #[tokio::test]
    async fn a_full_queue_says_busy_and_gives_places_back() {
        let (_dir, db) = scratch(&[]).await;
        let db = Arc::new(db);
        let (release, _) = tokio::sync::broadcast::channel::<()>(1);
        // Writes from tasks of their own: 4 get lanes and hold them, the rest
        // wait for one, holding places in the queue.
        let held: Vec<_> = (0..WRITE_QUEUE)
            .map(|_| {
                let (db, mut released) = (db.clone(), release.subscribe());
                tokio::spawn(async move {
                    let _turn = db.writing().await.unwrap();
                    let _ = released.recv().await;
                })
            })
            .collect();
        while db.queued() < WRITE_QUEUE {
            tokio::task::yield_now().await;
        }
        match db.writing().await {
            Err(Error::ResourceExhausted(message)) => assert_eq!(message, BUSY),
            other => panic!("expected busy, got {:?}", other.map(|_| ())),
        }
        release.send(()).unwrap();
        for task in held {
            task.await.unwrap();
        }
        assert_eq!(db.queued(), 0);
        db.writing().await.unwrap();
    }

    #[tokio::test]
    async fn connections_come_back_unless_a_transaction_was_left_open() {
        let (_dir, db) = scratch(&[]).await;
        drop(db.conn().unwrap());
        assert_eq!(db.idle.lock().unwrap().len(), 1);
        let conn = db.conn().unwrap();
        assert_eq!(db.idle.lock().unwrap().len(), 0);
        conn.execute("BEGIN CONCURRENT", ()).await.unwrap();
        drop(conn);
        assert_eq!(db.idle.lock().unwrap().len(), 0, "a connection mid-transaction is closed");
    }
}
