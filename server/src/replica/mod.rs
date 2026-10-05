//! Continuous backup of a split instance's databases and pictures to a bucket
//! (or a directory), and restoring from it: the directory's node.db and
//! pictures, and each shard's servers. A single process doesn't replicate.
//! See docs/storage.md for the design.
//!
//! Every database runs in Turso's concurrent-writer mode, where a commit is
//! appended to the file's log (`<file>.db-log`) and the file itself only
//! changes when the log is folded into it (a checkpoint). So the replica
//! keeps, per file:
//!
//! - a *generation*: a copy of the file (`snapshot`), then every byte the log
//!   gets after it, shipped every `FUWA_REPLICA_INTERVAL` as *segments*;
//! - *epochs* within it: the replica folds the log into the file itself once
//!   it reaches [`CHECKPOINT_BYTES`] (Turso's own threshold is turned off), so
//!   it can ship the log's last bytes first. The log then starts over as the
//!   next epoch.
//!
//! Restoring a file downloads its snapshot and replays each epoch's log in
//! order, letting Turso fold each in. A new generation (a fresh snapshot)
//! starts when the logs shipped since the last one outgrow the file, when the
//! files on disk no longer match what was shipped (a crash between folding
//! the log and shipping its tail, a file swapped by hand), and on the first
//! run. Only the current generation and the one before it are kept.
//!
//! Keys, under the replica's prefix: `node/…` for node.db, `dms/…` for
//! dms.db (direct messages, already end-to-end encrypted), `servers/<id>/…`
//! for each community server (not per shard, so a server keeps its history
//! wherever it moves), `media/<id>` for each uploaded picture. A file's
//! `current` names its generation in use, the data directory that set it and,
//! for a server, the shard holding it; `deleted` marks a deleted server.

pub mod s3;
pub mod store;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use self::s3::S3Config;
pub use self::store::Store;
use crate::db::{self, Db, EncryptionKey};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms};

/// Log size at which the replica folds the log into its file: Turso's own
/// default, so memory use stays what it is without a replica.
pub const CHECKPOINT_BYTES: u64 = 4_120_000;

/// A new generation starts once the logs shipped since its snapshot are
/// bigger than the file, and at least this much.
pub const REBASE_BYTES: u64 = 64 * 1024 * 1024;

/// How many times [`CHECKPOINT_BYTES`] a file's log may grow while the
/// replica can't take it (the bucket down, another process writing its
/// replica) before it's folded in anyway, so the instance keeps working. Its
/// replica then starts a new generation once it can.
const BEHIND_FACTOR: u64 = 16;

/// The most log one segment carries.
const SEGMENT_BYTES: u64 = 16 * 1024 * 1024;

/// How often each file checks that it's still this process writing its replica.
const FENCE_EVERY: Duration = Duration::from_secs(60);

/// How often new pictures are copied up.
const MEDIA_EVERY: Duration = Duration::from_secs(60);
/// How often releases that failed (the bucket was unreachable) are tried
/// again, so a server that moved away leaves nothing in the old bucket.
const RETRY_RELEASES: Duration = Duration::from_secs(10 * 60);
/// Where, under `<data>/replica/`, owed releases are noted.
const RELEASES: &str = "released";

/// Files synced at once.
const PARALLEL: usize = 8;

/// The last bytes of shipped log remembered, to notice the log being rewritten.
const TAIL: usize = 16;

/// Whether to restore an empty data directory from the replica at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restore {
    Off,
    /// When node.db isn't there yet (a new or wiped volume).
    IfEmpty,
}

/// Where the replica goes, from `FUWA_S3_*` or `FUWA_REPLICA_PATH`.
#[derive(Debug, Clone)]
pub enum Target {
    Bucket { s3: S3Config, prefix: String },
    Dir(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ReplicaConfig {
    pub target: Target,
    /// FUWA_REPLICA_INTERVAL, default 1s: how often new commits are shipped.
    pub interval: Duration,
    /// FUWA_RESTORE: off (default) | if-empty.
    pub restore: Restore,
}

impl ReplicaConfig {
    pub fn store(&self) -> Result<Store> {
        match &self.target {
            Target::Bucket { s3, prefix } => Store::bucket(s3.clone(), prefix),
            Target::Dir(dir) => Ok(Store::dir(dir)),
        }
    }
}

/// Where a file's replica stands, kept in `<data>/replica/<name>.json` so a
/// restart carries on where it was instead of starting a new generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Position {
    generation: String,
    epoch: u32,
    /// The log's salt (bytes 8 to 16 of its header, new each time it starts
    /// over), once this epoch's log has a header.
    salt: Option<String>,
    /// Bytes of this epoch's log shipped.
    shipped: u64,
    /// The last of them, hex.
    tail: String,
    /// The database file as this epoch began, which only a checkpoint changes.
    db_len: u64,
    db_modified: u128,
    /// The snapshot's size, and the log shipped since it, every epoch.
    base: u64,
    logged: u64,
}

struct Tracked {
    /// `node` or `servers/<id>`.
    name: String,
    db: Arc<Db>,
    state: tokio::sync::Mutex<State>,
}

#[derive(Default)]
struct State {
    position: Option<Position>,
    /// Another process writes this file's replica now; this one stopped.
    fenced: bool,
    /// No longer replicated (a round that started before may still hold it).
    forgotten: bool,
    fence_checked: Option<Instant>,
    failures: u32,
}

/// The running replica of this process's files.
pub struct Replica {
    store: Store,
    interval: Duration,
    /// `<data>/replica/`, where positions are kept.
    positions: PathBuf,
    /// This data directory's random id, written into each `current` it sets,
    /// so a second process writing the same replica shows up.
    writer: String,
    /// The shard this process is, written into each `current` too, so a shard
    /// that lost its volume finds its servers.
    holder: Option<String>,
    files: Mutex<BTreeMap<String, Arc<Tracked>>>,
    media: Mutex<Option<PathBuf>>,
    /// The pictures the replica has, once listed.
    media_copied: tokio::sync::Mutex<Option<BTreeSet<String>>>,
    checkpoint_bytes: u64,
    rebase_bytes: u64,
    /// [`FENCE_EVERY`], in milliseconds (shorter in tests).
    fence_every_ms: std::sync::atomic::AtomicU64,
    stop: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// One lock per file, held while a release deletes and by
    /// [`track`](Self::track), so a server coming back here never has its
    /// replica deleted under it, and a slow bucket holds up no other file.
    releasing: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Replica {
    /// The replica of a directory's files (`holder` None) or a shard's (its id).
    pub fn new(store: Store, interval: Duration, data_path: &Path, holder: Option<String>) -> Result<Arc<Self>> {
        Self::with_sizes(store, interval, data_path, holder, CHECKPOINT_BYTES, REBASE_BYTES)
    }

    /// With other thresholds than the defaults, for tests.
    pub fn with_sizes(
        store: Store,
        interval: Duration,
        data_path: &Path,
        holder: Option<String>,
        checkpoint_bytes: u64,
        rebase_bytes: u64,
    ) -> Result<Arc<Self>> {
        let positions = data_path.join("replica");
        std::fs::create_dir_all(&positions)?;
        let writer = match std::fs::read_to_string(positions.join("writer")) {
            Ok(writer) if !writer.trim().is_empty() => writer.trim().to_string(),
            _ => {
                let writer = new_id();
                std::fs::write(positions.join("writer"), &writer)?;
                writer
            }
        };
        Ok(Arc::new(Self {
            store,
            interval,
            positions,
            writer,
            holder,
            files: Mutex::new(BTreeMap::new()),
            media: Mutex::new(None),
            media_copied: tokio::sync::Mutex::new(None),
            releasing: Mutex::new(BTreeMap::new()),
            checkpoint_bytes,
            rebase_bytes,
            fence_every_ms: (FENCE_EVERY.as_millis() as u64).into(),
            stop: CancellationToken::new(),
            task: Mutex::new(None),
        }))
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Starts replicating a database file. From now on only the replica folds
    /// its log into it.
    pub async fn track(&self, name: &str, db: Arc<Db>) -> Result<()> {
        db::pragma(&db::connect(&db)?, "PRAGMA mvcc_checkpoint_threshold = -1").await?;
        db.replicated();
        // Waits out a release of it that's under way.
        let _released = self.release_lock(name).lock_owned().await;
        let position = self.load_position(name);
        let tracked = Arc::new(Tracked {
            name: name.to_string(),
            db,
            state: tokio::sync::Mutex::new(State { position, ..State::default() }),
        });
        self.lock_files().insert(name.to_string(), tracked);
        // It's back here: a release still owed from when it left is void.
        let _ = std::fs::remove_file(self.release_path(name));
        Ok(())
    }

    /// Stops replicating a file, after shipping what's left of its log. A
    /// deleted server is marked so restores leave it out.
    pub async fn forget(&self, name: &str, deleted: bool) {
        let tracked = self.lock_files().remove(name);
        let mut ours = true;
        if let Some(tracked) = tracked {
            let mut state = tracked.state.lock().await;
            state.forgotten = true;
            ours = !state.fenced;
            if ours
                && state.position.is_some()
                && let Err(_) = self.ship(&tracked, &mut state).await
            {
                tracing::warn!("couldn't ship the last commits to the replica");
            }
        }
        if deleted
            && ours
            && let Err(_) = self.store.put(&format!("{name}/deleted"), now_ms().to_string().into()).await
        {
            tracing::warn!("couldn't mark the file deleted in the replica");
        }
        let _ = std::fs::remove_file(self.position_path(name));
    }

    /// Brings a file's replica up to date now rather than at the next round:
    /// for a server that just moved here, so it's backed up before the move
    /// is over.
    pub async fn sync_now(&self, name: &str) -> Result<()> {
        let tracked = self.lock_files().get(name).cloned();
        let Some(tracked) = tracked else { return Ok(()) };
        let mut state = tracked.state.lock().await;
        self.sync(&tracked, &mut state).await
    }

    /// Stops replicating a server that moved to another shard, and deletes
    /// this process's copies of it: everything under its name (and its
    /// recordings and pictures) when nobody else has written it since, as when the new
    /// shard replicates to another region's bucket; else only the
    /// generations not in use, leaving the new shard's.
    pub async fn release(&self, name: &str) {
        self.forget(name, false).await;
        // Noted first, so a failure (or a restart) is tried again later.
        let owed = self.release_path(name);
        if let Some(dir) = owed.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&owed, b"").is_err() {
            tracing::warn!("couldn't note a release to try again");
        }
        self.try_release(name).await;
    }

    /// Deletes what's in the replica of a file this process let go of, and
    /// clears the note that it's owed. On failure the note stays.
    async fn try_release(&self, name: &str) -> bool {
        let _alone = self.release_lock(name).lock_owned().await;
        if self.lock_files().contains_key(name) {
            // It came back here: what's in the replica is its own again.
            let _ = std::fs::remove_file(self.release_path(name));
            return false;
        }
        let released = async {
            let current = current(&self.store, name).await?;
            let ours = current.as_ref().and_then(|c| c.writer.as_deref()).is_none_or(|writer| writer == self.writer);
            if !ours {
                let keep: BTreeSet<String> = current.into_iter().map(|c| c.generation).collect();
                return self.prune(name, &keep).await;
            }
            let mut prefixes = vec![format!("{name}/")];
            if let Some(id) = name.strip_prefix("servers/") {
                prefixes.push(format!("recordings/{id}/"));
                prefixes.push(format!("{}/{id}/", crate::cluster::pictures::DIR));
            }
            for prefix in prefixes {
                for object in self.store.list(&prefix).await? {
                    self.store.delete(&object.key).await?;
                }
            }
            Ok::<_, Error>(())
        }
        .await;
        if released.is_err() {
            tracing::warn!("couldn't delete the replica of a server that moved away; trying again later");
            crate::reports::server_error("replica_release", Some("replica"));
            return false;
        }
        let _ = std::fs::remove_file(self.release_path(name));
        true
    }

    /// Tries again the releases that failed, leaving out any file that came
    /// back here since. Returns how many went through.
    pub async fn retry_releases(&self) -> usize {
        let Ok(entries) = std::fs::read_dir(self.positions.join(RELEASES)) else { return 0 };
        let mut done = 0;
        for entry in entries.flatten() {
            let Ok(id) = entry.file_name().into_string() else { continue };
            if !crate::id::parse_id("server_id", &id).is_ok_and(|parsed| parsed == id) {
                continue;
            }
            let name = format!("servers/{id}");
            done += usize::from(self.try_release(&name).await);
        }
        done
    }

    /// The lock a file's release and tracking take.
    fn release_lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.releasing.lock().unwrap_or_else(|p| p.into_inner());
        locks.entry(name.to_string()).or_default().clone()
    }

    /// Where the note that a file's release is owed is kept.
    fn release_path(&self, name: &str) -> PathBuf {
        self.positions.join(RELEASES).join(name.strip_prefix("servers/").unwrap_or(name).replace('/', "_"))
    }

    /// Copies pictures from `dir` (`<data>/media`) up as they arrive.
    pub fn track_media(&self, dir: PathBuf) {
        *self.media.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir);
    }

    /// Deletes pictures from the replica.
    pub async fn drop_media(&self, ids: &[String]) {
        let mut copied = self.media_copied.lock().await;
        for id in ids {
            if let Some(copied) = copied.as_mut() {
                copied.remove(id);
            }
            if self.store.delete(&format!("media/{id}")).await.is_err() {
                tracing::warn!("couldn't delete a picture from the replica");
            }
        }
    }

    /// Deletes pictures from the replica that node.db no longer knows.
    pub async fn prune_media(&self, known: &std::collections::HashSet<String>) -> Result<usize> {
        let mut removed = 0;
        for object in self.store.list("media/").await? {
            let id = object.key.trim_start_matches("media/");
            if !known.contains(id) {
                self.store.delete(&object.key).await?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Ships new commits every interval until [`close`](Self::close).
    pub fn start(self: &Arc<Self>) {
        let replica = self.clone();
        let task = tokio::spawn(async move {
            let mut every = tokio::time::interval(replica.interval);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut media_synced: Option<Instant> = None;
            let mut released: Option<Instant> = None;
            loop {
                tokio::select! {
                    _ = replica.stop.cancelled() => return,
                    _ = every.tick() => {}
                }
                replica.sync_files().await;
                if media_synced.is_none_or(|at| at.elapsed() >= MEDIA_EVERY) {
                    media_synced = Some(Instant::now());
                    if replica.sync_media().await.is_err() {
                        tracing::warn!("couldn't copy new pictures to the replica");
                    }
                }
                if released.is_none_or(|at| at.elapsed() >= RETRY_RELEASES) {
                    released = Some(Instant::now());
                    replica.retry_releases().await;
                }
            }
        });
        *self.task.lock().unwrap_or_else(|p| p.into_inner()) = Some(task);
    }

    /// Stops shipping on a timer and ships what's left, for shutting down.
    pub async fn close(&self) {
        self.stop.cancel();
        let task = self.task.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(task) = task {
            let _ = task.await;
        }
        self.sync_files().await;
        if self.sync_media().await.is_err() {
            tracing::warn!("couldn't copy new pictures to the replica");
        }
    }

    /// One round: ships every file's new commits.
    pub async fn sync_files(&self) {
        futures::stream::iter(self.tracked())
            .for_each_concurrent(PARALLEL, |tracked| async move {
                let mut state = tracked.state.lock().await;
                match self.sync(&tracked, &mut state).await {
                    Ok(()) => {
                        if state.failures > 0 {
                            tracing::info!("replicating again");
                        }
                        state.failures = 0;
                    }
                    Err(_) => {
                        state.failures += 1;
                        // The first failure, then about once a minute at the default interval.
                        if state.failures % 60 == 1 {
                            tracing::warn!(failures = state.failures, "couldn't replicate");
                        }
                        if self.too_far_behind(&tracked) {
                            tracing::warn!("the replica is too far behind; folding the log into the file anyway, and starting a new generation once the replica is back");
                            state.position = None;
                            let _ = std::fs::remove_file(self.position_path(&tracked.name));
                            if fold(&tracked.db).await.is_err() {
                                tracing::warn!("couldn't fold the log into the file");
                            }
                        }
                    }
                }
            })
            .await;
    }

    /// Copies pictures the replica doesn't have yet.
    pub async fn sync_media(&self) -> Result<usize> {
        let Some(dir) = self.media.lock().unwrap_or_else(|p| p.into_inner()).clone() else { return Ok(0) };
        let mut copied = self.media_copied.lock().await;
        if copied.is_none() {
            let listed = self.store.list("media/").await?;
            *copied = Some(listed.iter().map(|o| o.key.trim_start_matches("media/").to_string()).collect());
        }
        let have = copied.as_mut().expect("listed just now");
        let mut count = 0;
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if have.contains(&name)
                || !entry.file_type()?.is_file()
                || crate::media::parse_id(&name).as_deref() != Some(name.as_str())
            {
                continue;
            }
            self.store.put_file(&format!("media/{name}"), &entry.path()).await?;
            have.insert(name);
            count += 1;
        }
        Ok(count)
    }

    fn too_far_behind(&self, file: &Tracked) -> bool {
        size(&file.db.log_path()) >= self.checkpoint_bytes.saturating_mul(BEHIND_FACTOR)
    }

    fn tracked(&self) -> Vec<Arc<Tracked>> {
        self.lock_files().values().cloned().collect()
    }

    fn lock_files(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<Tracked>>> {
        self.files.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Brings one file's replica up to date.
    async fn sync(&self, file: &Tracked, state: &mut State) -> Result<()> {
        if state.forgotten {
            return Ok(());
        }
        if !state.fenced {
            self.check_fence(file, state).await?;
        }
        if state.fenced {
            // Not replicated from here any more, but the log still needs folding in.
            if size(&file.db.log_path()) >= self.checkpoint_bytes {
                fold(&file.db).await?;
            }
            return Ok(());
        }
        let continues = match &state.position {
            Some(position) => continues(position, &file.db)?,
            None => false,
        };
        if !continues {
            if state.position.is_some() {
                tracing::info!("the files on disk moved on from the replica; starting a new generation");
            }
            state.position = None;
            self.start_generation(file, state).await?;
        }
        self.ship(file, state).await?;
        if size(&file.db.log_path()) >= self.checkpoint_bytes {
            self.next_epoch(file, state).await?;
        }
        Ok(())
    }

    /// Ships the log bytes written since the last time.
    async fn ship(&self, file: &Tracked, state: &mut State) -> Result<()> {
        let log = file.db.log_path();
        loop {
            let Some(position) = state.position.as_mut() else { return Ok(()) };
            let len = size(&log);
            if len <= position.shipped {
                return Ok(());
            }
            let bytes = read_range(&log, position.shipped, (len - position.shipped).min(SEGMENT_BYTES))?;
            let key = segment_key(&file.name, &position.generation, position.epoch, position.shipped);
            self.store.put(&key, bytes.clone()).await?;
            advance(position, &bytes);
            self.save_position(&file.name, position)?;
        }
    }

    /// Folds the log into the file and starts the next epoch, shipping the
    /// log's last bytes first: with writes held for that moment, nothing
    /// commits between reading them and the log starting over.
    async fn next_epoch(&self, file: &Tracked, state: &mut State) -> Result<()> {
        let Some(mut position) = state.position.clone() else { return Ok(()) };
        let log = file.db.log_path();
        let tail = {
            let _alone = file.db.alone().await;
            let len = size(&log);
            let tail = if len > position.shipped {
                read_range(&log, position.shipped, len - position.shipped)?
            } else {
                Bytes::new()
            };
            file.db.checkpoint().await?;
            tail
        };
        // The log has started over: these bytes are only in memory now.
        if !tail.is_empty() {
            let key = segment_key(&file.name, &position.generation, position.epoch, position.shipped);
            let mut attempt = 0;
            while let Err(err) = self.store.put(&key, tail.clone()).await {
                attempt += 1;
                if attempt == 3 {
                    // This generation can't go on; the next round starts a new one.
                    state.position = None;
                    let _ = std::fs::remove_file(self.position_path(&file.name));
                    return Err(err);
                }
                tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
            }
            position.logged += tail.len() as u64;
        }
        let (db_len, db_modified) = metadata(file.db.path())?;
        position.epoch += 1;
        position.salt = None;
        position.shipped = 0;
        position.tail.clear();
        position.db_len = db_len;
        position.db_modified = db_modified;
        self.save_position(&file.name, &position)?;
        let rebase = position.logged >= position.base.max(self.rebase_bytes);
        state.position = Some(position);
        if rebase {
            self.start_generation(file, state).await?;
        }
        Ok(())
    }

    /// Uploads a fresh copy of the file and makes it the current generation.
    async fn start_generation(&self, file: &Tracked, state: &mut State) -> Result<()> {
        {
            let _alone = file.db.alone().await;
            file.db.checkpoint().await?;
        }
        // From here the file only changes when this replica folds the log in
        // again, so it can be copied while writes go on into the log.
        let (db_len, db_modified) = metadata(file.db.path())?;
        let generation = new_id();
        let base = self.store.put_file(&snapshot_key(&file.name, &generation), file.db.path()).await?;
        if metadata(file.db.path())? != (db_len, db_modified) {
            return Err(Error::internal(format!("{} changed while it was copied", file.db.path().display())));
        }
        let previous = current(&self.store, &file.name).await?.map(|current| current.generation);
        let current = match &self.holder {
            Some(holder) => format!("{generation} {} {holder}", self.writer),
            None => format!("{generation} {}", self.writer),
        };
        self.store.put(&format!("{}/current", file.name), current.into()).await?;
        let position = Position {
            generation: generation.clone(),
            epoch: 0,
            salt: None,
            shipped: 0,
            tail: String::new(),
            db_len,
            db_modified,
            base,
            logged: 0,
        };
        self.save_position(&file.name, &position)?;
        state.position = Some(position);
        state.fence_checked = Some(Instant::now());
        tracing::debug!(%generation, bytes = base, "started a new replica generation");

        let keep: BTreeSet<String> = [Some(generation), previous].into_iter().flatten().collect();
        if self.prune(&file.name, &keep).await.is_err() {
            tracing::warn!("couldn't delete old replica generations");
        }
        Ok(())
    }

    /// Stops replicating a file another process has taken over: its
    /// `current` was set from another data directory. (One this directory set
    /// that isn't the position's, after a crash between the two, just starts
    /// a new generation.)
    async fn check_fence(&self, file: &Tracked, state: &mut State) -> Result<()> {
        let Some(position) = &state.position else { return Ok(()) };
        let every = Duration::from_millis(self.fence_every_ms.load(std::sync::atomic::Ordering::Relaxed));
        if state.fence_checked.is_some_and(|at| at.elapsed() < every) {
            return Ok(());
        }
        let generation = position.generation.clone();
        let current = current(&self.store, &file.name).await?;
        state.fence_checked = Some(Instant::now());
        match current {
            Some(current) if current.writer.as_deref().is_some_and(|writer| writer != self.writer) => {
                state.fenced = true;
                tracing::error!(replica = %self.store.describe(), "another fuwa process is replicating this file to the same place; this one stopped replicating it");
            }
            Some(current) if current.generation != generation => state.position = None,
            _ => {}
        }
        Ok(())
    }

    /// Deletes a file's generations other than `keep`.
    async fn prune(&self, name: &str, keep: &BTreeSet<String>) -> Result<()> {
        for object in self.store.list(&format!("{name}/")).await? {
            let rest = &object.key[name.len() + 1..];
            if let Some((generation, _)) = rest.split_once('/')
                && !keep.contains(generation)
            {
                self.store.delete(&object.key).await?;
            }
        }
        Ok(())
    }

    fn position_path(&self, name: &str) -> PathBuf {
        let mut path = self.positions.clone();
        for part in name.split('/') {
            path.push(part);
        }
        path.set_extension("json");
        path
    }

    fn load_position(&self, name: &str) -> Option<Position> {
        let bytes = std::fs::read(self.position_path(name)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_position(&self, name: &str, position: &Position) -> Result<()> {
        let path = self.position_path(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let partial = path.with_extension("json.partial");
        std::fs::write(&partial, serde_json::to_vec(position).map_err(|e| Error::internal(e.to_string()))?)?;
        std::fs::rename(&partial, &path)?;
        Ok(())
    }
}

/// Folds a file's log into it, with writes held meanwhile.
async fn fold(db: &Db) -> Result<()> {
    let _alone = db.alone().await;
    db.checkpoint().await
}

/// Whether the files on disk are still where `position` left them: the
/// database file unchanged, and the log the same one, grown or not.
fn continues(position: &Position, db: &Db) -> Result<bool> {
    if metadata(db.path())? != (position.db_len, position.db_modified) {
        return Ok(false);
    }
    let log = db.log_path();
    let len = size(&log);
    if len < position.shipped {
        return Ok(false);
    }
    if position.shipped == 0 {
        return Ok(true);
    }
    if let Some(salt) = &position.salt
        && len >= 16
        && hex(&read_range(&log, 8, 8)?) != *salt
    {
        return Ok(false);
    }
    let tail_len = position.tail.len() as u64 / 2;
    Ok(hex(&read_range(&log, position.shipped - tail_len, tail_len)?) == position.tail)
}

/// Moves a position past bytes just shipped.
fn advance(position: &mut Position, bytes: &[u8]) {
    if position.shipped == 0 && bytes.len() >= 16 {
        position.salt = Some(hex(&bytes[8..16]));
    }
    position.shipped += bytes.len() as u64;
    position.logged += bytes.len() as u64;
    let mut tail = unhex(&position.tail);
    tail.extend_from_slice(bytes);
    let from = tail.len().saturating_sub(TAIL);
    position.tail = hex(&tail[from..]);
}

fn snapshot_key(name: &str, generation: &str) -> String {
    format!("{name}/{generation}/snapshot")
}

fn segment_key(name: &str, generation: &str, epoch: u32, offset: u64) -> String {
    format!("{name}/{generation}/{epoch:08x}-{offset:016x}")
}

/// The epoch and offset a segment key ends in.
fn parse_segment(last: &str) -> Option<(u32, u64)> {
    let (epoch, offset) = last.split_once('-')?;
    if epoch.len() != 8 || offset.len() != 16 {
        return None;
    }
    Some((u32::from_str_radix(epoch, 16).ok()?, u64::from_str_radix(offset, 16).ok()?))
}

/// What a file's `current` says: its generation in use, which data
/// directory set it, and for a server, the shard holding it.
struct Current {
    generation: String,
    writer: Option<String>,
    holder: Option<String>,
}

async fn current(store: &Store, name: &str) -> Result<Option<Current>> {
    let Some(bytes) = store.get(&format!("{name}/current")).await? else { return Ok(None) };
    let text = String::from_utf8_lossy(&bytes);
    let mut words = text.split_whitespace().map(String::from);
    Ok(words.next().map(|generation| Current { generation, writer: words.next(), holder: words.next() }))
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn metadata(path: &Path) -> Result<(u64, u128)> {
    let meta = std::fs::metadata(path)?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    Ok((meta.len(), modified))
}

fn read_range(path: &Path, offset: u64, len: u64) -> Result<Bytes> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(Error::internal(format!("{} got shorter while it was read", path.display())));
    }
    Ok(bytes.into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).filter_map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()).collect()
}

// ─────────────────────────────── Restoring ───────────────────────────────

/// What a restore brought back.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Restored {
    pub node: bool,
    pub dms: bool,
    pub servers: usize,
    pub media: usize,
    /// Servers whose replica stopped short (a segment missing): restored up
    /// to there.
    pub incomplete: Vec<String>,
}

/// Whether the replica holds an instance: a node.db.
pub async fn has_instance(store: &Store) -> Result<bool> {
    Ok(current(store, "node").await?.is_some() || !store.list("node/").await?.is_empty())
}

/// Restores a directory into `data`: node.db, dms.db and the pictures.
/// Files already there are left alone.
pub async fn restore_directory(store: &Store, data: &Path, key: Option<&EncryptionKey>) -> Result<Restored> {
    let mut restored = Restored::default();
    let node = data.join("node.db");
    if !node.exists() && has_instance(store).await? {
        if !restore_file(store, "node", &node, key).await? {
            restored.incomplete.push("node".into());
        }
        restored.node = true;
    }
    // Instances replicated before direct messages have no dms.db to restore.
    let dms = data.join("dms.db");
    if !dms.exists() && (current(store, "dms").await?.is_some() || !store.list("dms/").await?.is_empty()) {
        if !restore_file(store, "dms", &dms, key).await? {
            restored.incomplete.push("dms".into());
        }
        restored.dms = true;
    }

    let media = data.join("media");
    // Where uploads arrive too: anything a crash leaves there is cleared at start.
    let incoming = media.join(".incoming");
    std::fs::create_dir_all(&incoming)?;
    for object in store.list("media/").await? {
        let id = object.key.trim_start_matches("media/");
        if crate::media::parse_id(id).as_deref() != Some(id) || media.join(id).exists() {
            continue;
        }
        let partial = incoming.join(format!("restoring-{id}"));
        if store.get_to_file(&object.key, &partial).await? {
            std::fs::rename(&partial, media.join(id))?;
            restored.media += 1;
        }
    }
    Ok(restored)
}

/// The servers the replica has from a shard: the ones whose replica it
/// wrote last, leaving out deleted ones.
pub async fn held_by(store: &Store, shard: &str) -> Result<Vec<String>> {
    let mut found: BTreeMap<String, bool> = BTreeMap::new();
    for object in store.list("servers/").await? {
        let mut parts = object.key.splitn(3, '/').skip(1);
        let (Some(id), Some(rest)) = (parts.next(), parts.next()) else { continue };
        if crate::id::parse_id("server", id).ok().as_deref() != Some(id) {
            continue;
        }
        *found.entry(id.to_string()).or_default() |= rest == "deleted";
    }
    let mut held = Vec::new();
    for (id, deleted) in found {
        if !deleted
            && current(store, &crate::servers::replica_name(&id)).await?.and_then(|c| c.holder).as_deref()
                == Some(shard)
        {
            held.push(id);
        }
    }
    Ok(held)
}

/// Restores a shard's servers into `data`/servers: those the replica has
/// from it ([`held_by`]). Files already there are left alone.
pub async fn restore_shard(store: &Store, data: &Path, key: Option<&EncryptionKey>, shard: &str) -> Result<Restored> {
    let mut restored = Restored::default();
    std::fs::create_dir_all(data.join("servers"))?;
    for id in held_by(store, shard).await? {
        let path = data.join("servers").join(format!("{id}.db"));
        if path.exists() {
            continue;
        }
        let name = crate::servers::replica_name(&id);
        if !restore_file(store, &name, &path, key).await? {
            restored.incomplete.push(name);
        }
        restored.servers += 1;
    }
    Ok(restored)
}

/// Restores one file from its current generation into `dest`, which must not
/// exist. Returns false when its log stopped short of where it was shipped to.
pub async fn restore_file(store: &Store, name: &str, dest: &Path, key: Option<&EncryptionKey>) -> Result<bool> {
    let objects = store.list(&format!("{name}/")).await?;
    let generation = match current(store, name).await? {
        Some(current) => current.generation,
        // No pointer (a crash before the first one): the newest generation with a snapshot.
        None => objects
            .iter()
            .filter_map(|o| o.key.strip_suffix("/snapshot"))
            .filter_map(|prefix| prefix.rsplit_once('/').map(|(_, generation)| generation.to_string()))
            .max()
            .ok_or_else(|| Error::NotFound("replica"))?,
    };
    let prefix = format!("{name}/{generation}/");
    let mut epochs: BTreeMap<u32, Vec<(u64, String)>> = BTreeMap::new();
    let mut has_snapshot = false;
    for object in &objects {
        let Some(last) = object.key.strip_prefix(&prefix) else { continue };
        if last == "snapshot" {
            has_snapshot = true;
        } else if let Some((epoch, offset)) = parse_segment(last) {
            epochs.entry(epoch).or_default().push((offset, object.key.clone()));
        }
    }
    if !has_snapshot {
        return Err(Error::internal(format!("the replica of {name} has no snapshot for generation {generation}")));
    }

    // Built next to its place, then moved in.
    let dir = dest.parent().unwrap_or(Path::new("."));
    let work = dir.join(format!(".restoring-{}", new_id()));
    std::fs::create_dir_all(&work)?;
    let result = async {
        let file = work.join("restoring.db");
        if !store.get_to_file(&snapshot_key(name, &generation), &file).await? {
            return Err(Error::internal(format!("the snapshot of {name} disappeared")));
        }
        let log = sidecar(&file, "-log");
        let mut whole = true;
        // Epochs must follow each other with none missing; each one's log is
        // whole, except perhaps the last.
        for (expected, (epoch, mut segments)) in epochs.into_iter().enumerate() {
            if epoch as usize != expected {
                whole = false;
                break;
            }
            segments.sort();
            let mut bytes: Vec<u8> = Vec::new();
            let mut gap = false;
            for (offset, key) in segments {
                if offset > bytes.len() as u64 {
                    gap = true;
                    break;
                }
                let segment = store.get(&key).await?.ok_or_else(|| Error::internal(format!("{key} disappeared")))?;
                let skip = (bytes.len() as u64 - offset) as usize;
                if skip < segment.len() {
                    bytes.extend_from_slice(&segment[skip..]);
                }
            }
            std::fs::write(&log, &bytes)?;
            // Turso replays the log as it opens the file (commits it already
            // has are skipped), and the checkpoint folds it in.
            let db = db::open(&file, key, &[]).await?;
            db.checkpoint().await?;
            drop(db);
            if gap {
                whole = false;
                break;
            }
        }
        // Opened once more, so a file whose log never had anything still
        // checks out (and the wrong key shows up here).
        let db = db::open(&file, key, &[]).await?;
        db::query_one(&db::connect(&db)?, "SELECT count(*) FROM sqlite_schema", (), |r| r.get::<i64>(0)).await?;
        db.checkpoint().await?;
        drop(db);
        if dest.exists() {
            return Err(Error::internal(format!("{} appeared while restoring it", dest.display())));
        }
        for suffix in ["-log", "-wal"] {
            let side = sidecar(&file, suffix);
            if size(&side) == 0 {
                let _ = std::fs::remove_file(&side);
            } else {
                std::fs::rename(&side, sidecar(dest, suffix))?;
            }
        }
        std::fs::rename(&file, dest)?;
        Ok(whole)
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Which part of a split instance a process is, for restoring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part<'a> {
    /// node.db and the pictures.
    Directory,
    /// The servers it holds, by its id.
    Shard(&'a str),
}

/// Readies a part's data directory before it opens: restores it from the
/// replica when it's empty and `FUWA_RESTORE=if-empty`, and refuses to start
/// empty over a replica that has its files otherwise, so a lost volume
/// never quietly becomes an empty instance writing over its own backup.
pub async fn prepare(
    config: &ReplicaConfig,
    store: &Store,
    data: &Path,
    key: Option<&EncryptionKey>,
    part: Part<'_>,
) -> Result<()> {
    let unreachable =
        |err: Error| Error::internal(format!("couldn't check the replica at {}: {err}", store.describe()));
    let what = match part {
        Part::Directory => {
            if data.join("node.db").exists() || !has_instance(store).await.map_err(unreachable)? {
                return Ok(());
            }
            format!("holds an instance but {} has no node.db", data.display())
        }
        Part::Shard(shard) => {
            // A volume that replicated before isn't new, even with no servers on
            // it: skip asking the replica about every server on each start.
            if has_server_files(&data.join("servers"))? || data.join("replica").join("writer").exists() {
                return Ok(());
            }
            let held = held_by(store, shard).await.map_err(unreachable)?.len();
            if held == 0 {
                return Ok(());
            }
            format!("holds {held} servers from shard {shard} but {} has none", data.join("servers").display())
        }
    };
    if config.restore != Restore::IfEmpty {
        return Err(Error::internal(format!(
            "the replica at {} {what}; set FUWA_RESTORE=if-empty to restore them, or point the replica \
             somewhere else to start afresh",
            store.describe(),
        )));
    }
    let started = Instant::now();
    tracing::info!(replica = %store.describe(), "restoring from the replica");
    let restored = match part {
        Part::Directory => restore_directory(store, data, key).await?,
        Part::Shard(shard) => restore_shard(store, data, key, shard).await?,
    };
    for _ in &restored.incomplete {
        tracing::warn!("its replica stopped short; restored as far as it went");
    }
    tracing::info!(
        node = restored.node,
        dms = restored.dms,
        servers = restored.servers,
        pictures = restored.media,
        seconds = started.elapsed().as_secs_f32(),
        "restored from the replica"
    );
    Ok(())
}

/// Whether a servers/ folder has any server's file.
fn has_server_files(dir: &Path) -> Result<bool> {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                if entry?.file_name().to_string_lossy().ends_with(".db") {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// The databases and pictures a restore would bring back, for `fuwa restore --list`.
pub async fn describe(store: &Store) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let objects = store.list("").await?;
    let mut files: BTreeMap<String, (u64, usize, bool)> = BTreeMap::new();
    let mut media = (0usize, 0u64);
    for object in &objects {
        if let Some(id) = object.key.strip_prefix("media/") {
            if !id.is_empty() {
                media.0 += 1;
                media.1 += object.size;
            }
            continue;
        }
        let name = match object.key.split('/').collect::<Vec<_>>().as_slice() {
            ["node", ..] => "node".to_string(),
            ["dms", ..] => "dms".to_string(),
            ["servers", id, ..] => format!("servers/{id}"),
            _ => continue,
        };
        let entry = files.entry(name).or_default();
        entry.0 += object.size;
        entry.1 += 1;
        entry.2 |= object.key.ends_with("/deleted");
    }
    let mut currents: HashMap<String, Current> = HashMap::new();
    for name in files.keys() {
        if let Some(current) = current(store, name).await? {
            currents.insert(name.clone(), current);
        }
    }
    for (name, (bytes, count, deleted)) in files {
        let current = currents.get(&name);
        let generation = current.map(|c| c.generation.as_str()).unwrap_or("none");
        let shard = match current.and_then(|c| c.holder.as_deref()) {
            Some(shard) => format!(", shard {shard}"),
            None => String::new(),
        };
        let deleted = if deleted { ", deleted" } else { "" };
        lines.push(format!("{name}: generation {generation}{shard}, {count} objects, {bytes} bytes{deleted}"));
    }
    lines.push(format!("media: {} pictures, {} bytes", media.0, media.1));
    Ok(lines)
}

#[cfg(test)]
mod tests;
