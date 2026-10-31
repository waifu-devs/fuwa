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
///
/// It may carry the keys it replaces (FUWA_ENCRYPTION_KEY_PREVIOUS): a file
/// that doesn't open under it but does under one of them is encrypted again
/// under it as it's opened ([`open`]), and sealed files that aren't
/// databases still unseal with them ([`EncryptionKey::all_bytes`]).
#[derive(Clone)]
pub struct EncryptionKey {
    hex: String,
    /// The keys it replaces, newest first; `None` for data made without a key.
    previous: Arc<[Option<String>]>,
}

impl EncryptionKey {
    pub fn parse(hex: &str) -> std::result::Result<Self, String> {
        Ok(Self { hex: parse_hex(hex)?, previous: Arc::new([]) })
    }

    /// The same key, replacing these: each 64 hex characters, or `none` for
    /// data made without a key.
    pub fn replacing(self, previous: &[&str]) -> std::result::Result<Self, String> {
        let previous = previous
            .iter()
            .map(|key| match key.trim() {
                "none" => Ok(None),
                key => parse_hex(key).map(Some),
            })
            .collect::<std::result::Result<Vec<_>, String>>()?;
        if previous.iter().any(|key| key.as_deref() == Some(self.hex.as_str())) {
            return Err("lists the key in use; list only the ones it replaces".into());
        }
        Ok(Self { previous: previous.into(), ..self })
    }

    /// The keys it replaces, each alone (`None` for no key).
    pub fn previous(&self) -> impl Iterator<Item = Option<EncryptionKey>> + '_ {
        self.previous.iter().map(|key| key.clone().map(|hex| Self { hex, previous: Arc::new([]) }))
    }

    pub fn replaces_any(&self) -> bool {
        !self.previous.is_empty()
    }

    /// The key alone, without the ones it replaces.
    pub fn alone(&self) -> Self {
        Self { hex: self.hex.clone(), previous: Arc::new([]) }
    }

    /// The key's 32 bytes, for sealing files that aren't databases (call
    /// recordings), each under a key derived from it.
    pub fn bytes(&self) -> [u8; 32] {
        hex_bytes(&self.hex)
    }

    /// Its bytes, then those of each key it replaces: what a sealed file is
    /// tried with.
    pub fn all_bytes(&self) -> Vec<[u8; 32]> {
        std::iter::once(self.bytes()).chain(self.previous.iter().flatten().map(|hex| hex_bytes(hex))).collect()
    }
}

fn parse_hex(hex: &str) -> std::result::Result<String, String> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("must be 64 hex characters (32 bytes), e.g. from `openssl rand -hex 32`".into());
    }
    Ok(hex.to_ascii_lowercase())
}

fn hex_bytes(hex: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap_or_default();
    }
    out
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EncryptionKey(***, replacing {})", self.previous.len())
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
            // Counts as failed unless it gets to the end: a panic, or the task
            // dropped at shutdown, still frees the fold for a later try.
            let mut running = Running { fold, log, ok: false };
            let started = std::time::Instant::now();
            let folded = {
                let _alone = gate.write_owned().await;
                checkpoint_with(&database, FOLD_ATTEMPTS, Some(FOLD_WAIT)).await
            };
            crate::reports::server_timing("db.fold", started.elapsed());
            if folded.is_err() {
                tracing::warn!("couldn't fold a database's log into it; trying again later");
            }
            running.ok = folded.is_ok();
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
        checkpoint_with(self, ATTEMPTS, None).await
    }
}

/// [`Db::checkpoint`], giving up after `attempts`, each waiting up to `wait`
/// for a lock (the connection's usual 10 s when `None`).
async fn checkpoint_with(database: &Database, attempts: u32, wait: Option<std::time::Duration>) -> Result<()> {
    let conn = connect(database)?;
    if let Some(wait) = wait {
        conn.busy_timeout(wait)?;
    }
    let mut attempt = 0;
    loop {
        match pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)").await {
            Err(err) if is_conflict(&err) && attempt + 1 >= attempts => return Err(Error::Busy),
            Err(err) if is_conflict(&err) => retry_after(&mut attempt).await?,
            other => return other,
        }
    }
}

/// How long each of a fold's own attempts waits for a lock: writes wait
/// while it runs, so it gives up soon and tries again later.
const FOLD_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// A fold under way, marked finished when dropped.
struct Running {
    fold: Arc<Fold>,
    log: u64,
    ok: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.fold.finished(self.ok, self.log);
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
///
/// A file that doesn't open under `key` but does under one of the keys it
/// replaces is encrypted again under it first ([`rekey`]).
pub async fn open(path: &Path, key: Option<&EncryptionKey>, migrations: &[&str]) -> Result<Db> {
    match open_as_is(path, key, migrations).await {
        Ok(db) => Ok(db),
        Err(err) => {
            let Some(key) = key.filter(|key| key.replaces_any() && path.exists()) else { return Err(err) };
            if !rekey_from_previous(path, key).await? {
                return Err(err);
            }
            open_as_is(path, Some(key), migrations).await
        }
    }
}

/// Encrypts a file under `key` if it's under one of the keys `key` replaces:
/// whether it was. A file that opens under none of them is left as it is.
pub async fn rekey_from_previous(path: &Path, key: &EncryptionKey) -> Result<bool> {
    for previous in key.previous() {
        // Only the right key opens a file, so trying the others changes nothing.
        if !opens(path, previous.as_ref()).await {
            continue;
        }
        rekey(path, previous.as_ref(), &key.alone()).await?;
        tracing::info!(file = %path.file_name().unwrap_or_default().to_string_lossy(), "encrypted a database under the new key");
        return Ok(true);
    }
    Ok(false)
}

/// The key a file is under: `key` itself (alone) or one of those it
/// replaces, `key` alone when none opens it (so the usual error says so).
pub async fn key_of(path: &Path, key: Option<&EncryptionKey>) -> Option<EncryptionKey> {
    let key = key?;
    for candidate in std::iter::once(Some(key.alone())).chain(key.previous()) {
        if opens(path, candidate.as_ref()).await {
            return candidate;
        }
    }
    Some(key.alone())
}

/// Whether a file opens and reads under `key`.
async fn opens(path: &Path, key: Option<&EncryptionKey>) -> bool {
    let Ok(db) = build(path, key).await else { return false };
    let Ok(conn) = connect(&db) else { return false };
    query_one(&conn, "SELECT count(*) FROM sqlite_schema", (), |r| r.get::<i64>(0)).await.is_ok()
}

/// Writes a database file again under another key (or none), keeping
/// everything in it: its log folded in first, then every table, row, index
/// and trigger copied into a new file under `to`, which takes its place.
///
/// Turso can't change a file's key in place, so this is a copy. Nothing
/// else may have the file open. A crash midway leaves the file as it was
/// (with its log folded in) and a `.rekey` copy that the next try replaces.
pub async fn rekey(path: &Path, from: Option<&EncryptionKey>, to: &EncryptionKey) -> Result<()> {
    let copy = with_suffix(path, ".rekey");
    for leftover in [copy.clone(), with_suffix(&copy, "-wal"), with_suffix(&copy, "-log")] {
        remove_if_there(&leftover)?;
    }
    {
        let source = build(path, from).await?;
        // The log under the old key goes into the file first, so nothing is
        // left in it once the file's replaced.
        checkpoint_with(&source, ATTEMPTS, None).await?;
        let source_conn = connect(&source)?;
        let target = build(&copy, Some(to)).await?;
        let target_conn = connect(&target)?;
        copy_into(&source_conn, &target_conn).await?;
        checkpoint_with(&target, ATTEMPTS, None).await?;
    }
    for side in ["-wal", "-log"] {
        let leftover = with_suffix(&copy, side);
        if std::fs::metadata(&leftover).is_ok_and(|meta| meta.len() > 0) {
            return Err(Error::internal(format!("{} kept a log after its copy was folded", path.display())));
        }
        remove_if_there(&leftover)?;
    }
    std::fs::File::open(&copy)?.sync_all()?;
    std::fs::rename(&copy, path)?;
    // The old file's logs, empty since it was folded.
    for side in ["-wal", "-log"] {
        remove_if_there(&with_suffix(path, side))?;
    }
    if let Some(dir) = path.parent()
        && let Ok(dir) = std::fs::File::open(dir)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Copies a whole database into an empty one: tables, then their rows, then
/// indexes, views and triggers, then `user_version` (which migrations go by).
async fn copy_into(from: &Connection, to: &Connection) -> Result<()> {
    let mut schema =
        query_all(from, "SELECT type, name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY rowid", (), |r| {
            Ok((r.get::<String>(0)?, r.get::<String>(1)?, r.get::<String>(2)?))
        })
        .await?;
    // The engine's own tables come with the file.
    schema.retain(|(_, name, _)| !name.starts_with("sqlite_") && !name.starts_with("__turso_internal_"));
    for (_, _, sql) in schema.iter().filter(|(kind, _, _)| kind == "table") {
        to.execute(sql, ()).await?;
    }
    for (_, name, _) in schema.iter().filter(|(kind, _, _)| kind == "table") {
        copy_rows(from, to, name).await?;
    }
    for (_, _, sql) in schema.iter().filter(|(kind, _, _)| kind != "table") {
        to.execute(sql, ()).await?;
    }
    let version = query_one(from, "PRAGMA user_version", (), |r| r.get::<i64>(0)).await?.unwrap_or(0);
    pragma(to, &format!("PRAGMA user_version = {version}")).await?;
    Ok(())
}

/// Rows copied in each transaction.
const COPY_BATCH: usize = 1_000;

async fn copy_rows(from: &Connection, to: &Connection, table: &str) -> Result<()> {
    let quoted = format!("\"{}\"", table.replace('"', "\"\""));
    let mut rows = from.query(&format!("SELECT * FROM {quoted}"), ()).await?;
    let columns = rows.column_count();
    if columns == 0 {
        return Ok(());
    }
    let marks = (1..=columns).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let insert = format!("INSERT INTO {quoted} VALUES ({marks})");
    let mut batch = 0;
    to.execute("BEGIN", ()).await?;
    while let Some(row) = rows.next().await? {
        let values = (0..columns).map(|i| row.get_value(i)).collect::<std::result::Result<Vec<_>, _>>()?;
        to.execute(&insert, values).await?;
        batch += 1;
        if batch == COPY_BATCH {
            to.execute("COMMIT", ()).await?;
            to.execute("BEGIN", ()).await?;
            batch = 0;
        }
    }
    to.execute("COMMIT", ()).await?;
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_if_there(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
        _ => Ok(()),
    }
}

async fn open_as_is(path: &Path, key: Option<&EncryptionKey>, migrations: &[&str]) -> Result<Db> {
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
            .with_encryption(turso::EncryptionOpts { cipher: "aegis256".into(), hexkey: key.hex.clone() });
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
                retry_after(&mut attempt).await?;
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
pub async fn retry_after(attempt: &mut u32) -> Result<()> {
    *attempt += 1;
    if *attempt >= ATTEMPTS {
        tracing::warn!(attempts = ATTEMPTS, "gave up on a write that kept clashing");
        return Err(Error::Busy);
    }
    tracing::debug!(attempt = *attempt, "a write clashed with another; running it again");
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

    fn key(byte: &str) -> EncryptionKey {
        EncryptionKey::parse(&byte.repeat(32)).unwrap()
    }

    const KEYED: &[&str] = &[
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL, b BLOB, r REAL); CREATE INDEX t_v ON t (v);",
        "CREATE TABLE u (k TEXT PRIMARY KEY, n INTEGER);",
    ];

    async fn fill(db: &Db, count: i64) {
        for id in 0..count {
            write(db, async move |conn| {
                conn.execute(
                    "INSERT INTO t (id, v, b, r) VALUES (?1, ?2, ?3, ?4)",
                    (id, format!("secret row {id}"), vec![id as u8, 0, 255], id as f64 / 2.0),
                )
                .await?;
                conn.execute("INSERT INTO u (k, n) VALUES (?1, ?2)", (format!("k{id}"), id)).await?;
                Ok(())
            })
            .await
            .unwrap();
        }
    }

    async fn rows(db: &Db) -> Vec<(i64, String, Option<Vec<u8>>, Option<f64>)> {
        query_all(&connect(db).unwrap(), "SELECT id, v, b, r FROM t ORDER BY id", (), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_new_key_takes_over_files_under_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let (old, new) = (key("5a"), key("a5"));
        let db = open(&path, Some(&old), KEYED).await.unwrap();
        // More than one copy batch, with some still in the log.
        fill(&db, 2_500).await;
        let before = rows(&db).await;
        drop(db);

        // The new key alone can't open it, as before.
        assert!(open(&path, Some(&new), KEYED).await.is_err());
        let replacing = new.clone().replacing(&["a5".repeat(32).as_str()]);
        assert!(replacing.is_err(), "the key in use isn't one it replaces");
        let replacing = new.clone().replacing(&["5a".repeat(32).as_str()]).unwrap();
        let db = open(&path, Some(&replacing), KEYED).await.unwrap();
        assert_eq!(rows(&db).await, before);
        let conn = connect(&db).unwrap();
        let version = query_one(&conn, "PRAGMA user_version", (), |r| r.get::<i64>(0)).await.unwrap();
        assert_eq!(version, Some(KEYED.len() as i64), "migrations don't run again");
        let indexes =
            query_one(&conn, "SELECT count(*) FROM sqlite_schema WHERE name = 't_v'", (), |r| r.get::<i64>(0));
        assert_eq!(indexes.await.unwrap(), Some(1));
        let n = query_one(&conn, "SELECT n FROM u WHERE k = 'k2499'", (), |r| r.get::<i64>(0)).await.unwrap();
        assert_eq!(n, Some(2499));
        // Still writable, in concurrent mode.
        fill_more(&db).await;
        drop(conn);
        drop(db);

        // Under the new key now, alone; the old one no longer opens it.
        assert_eq!(rows(&open(&path, Some(&new), &[]).await.unwrap()).await.len(), 2_501);
        assert!(open(&path, Some(&old), &[]).await.is_err());
        // Nothing of it in plain text, and no copy left behind.
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(10).any(|w| w == b"secret row"));
        assert!(!with_suffix(&path, ".rekey").exists());
    }

    async fn fill_more(db: &Db) {
        write(db, async |conn| {
            conn.execute("INSERT INTO t (id, v) VALUES (5000, 'after')", ()).await?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn plain_files_are_encrypted_when_the_previous_key_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = open(&path, None, KEYED).await.unwrap();
        fill(&db, 10).await;
        drop(db);
        let key = key("5a").replacing(&["none"]).unwrap();
        assert_eq!(rows(&open(&path, Some(&key), KEYED).await.unwrap()).await.len(), 10);
        assert!(open(&path, None, &[]).await.is_err(), "no longer plain");
    }

    #[tokio::test]
    async fn a_file_under_no_key_it_knows_stays_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = open(&path, Some(&key("11")), KEYED).await.unwrap();
        fill(&db, 3).await;
        drop(db);
        let other = key("5a").replacing(&["a5".repeat(32).as_str()]).unwrap();
        assert!(open(&path, Some(&other), KEYED).await.is_err());
        assert_eq!(rows(&open(&path, Some(&key("11")), &[]).await.unwrap()).await.len(), 3);
    }

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
