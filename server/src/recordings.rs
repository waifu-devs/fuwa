//! Voice channels recorded on the server.
//!
//! While anyone in a voice channel has `server_record` on (RECORD there, and
//! the instance allowing it), the part keeping the channel's places (the
//! shard holding its server, or the single process) listens to the call
//! through a bridge on its media part, as a program would, and writes each
//! person's sound to their own Ogg Opus file: the frames as they were sent,
//! never decoded, with silence in between, so every track starts when the
//! recording did and they all line up. A recording ends when nobody has
//! `server_record` on any more, or the part stops (a deploy); after a
//! restart, the apps' next keep starts a new one.
//!
//! Files are under `<data>/recordings/<server>/<recording>/`, one per person
//! (`<account>.opus`), and the server's file has a row for each recording.
//! With FUWA_ENCRYPTION_KEY set they're sealed (`<account>.opus.sealed`):
//! each Ogg page is ChaCha20-Poly1305'd under a key derived from the
//! instance's key for that file alone, and unsealed as it's downloaded. A
//! split instance's replica copies finished recordings to the bucket under
//! `recordings/<server>/<recording>/`, and downloads come from there when a
//! shard doesn't have the files (it took the server over, or lost its disk).
//!
//! Direct-message calls are never recorded here: their sound is end-to-end
//! encrypted, so all a bridge could keep is ciphertext.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb;
use crate::rtc::Bridged;
use crate::servers::{RecordingRow, ServerDb};
use crate::voice::{self, Place};

/// Opus's clock: 48 kHz, whatever the sound's own rate.
const RATE: u64 = 48_000;
/// 20 ms, the frame apps send and the silence filled in between.
const FRAME: u64 = 960;
/// One 20 ms frame of silence (CELT, fullband, mono).
const SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];
/// Frames in each Ogg page: about a second, so a crash loses at most that.
const PAGE_FRAMES: usize = 50;
/// How far a speaker's own clock may drift from the time frames arrive
/// before the track follows the arrival time instead (they reconnected, or
/// their clock jumped).
const RESYNC: i64 = 2 * RATE as i64;
/// How often the recordings going on are checked against who wants them,
/// besides each time someone's place changes.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// How long stopping waits for recordings to finish their files.
const FINISH_WAIT: Duration = Duration::from_secs(10);
/// The pieces a download comes in.
const PIECE: usize = 256 * 1024;
/// Recordings a channel lists.
const LISTED: i64 = 100;

/// The recordings going on in calls this part keeps.
#[derive(Default)]
pub struct Recordings {
    live: Mutex<HashMap<String, Live>>,
    nudged: Notify,
}

/// A recording going on (or finishing its files).
struct Live {
    server_id: String,
    channel_id: String,
    started_by: String,
    started_at: i64,
    stop: CancellationToken,
    tracks: Tracks,
    task: Option<tokio::task::JoinHandle<()>>,
}

/// Each person's track so far, by account.
type Tracks = Arc<Mutex<BTreeMap<String, Stored>>>;

/// One track, as a recording's row lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub user_id: String,
    pub size_bytes: i64,
    pub duration_ms: i64,
}

impl Recordings {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Live>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Someone's place changed: check whether a recording starts or stops.
    pub fn nudge(&self) {
        self.nudged.notify_one();
    }

    /// Whether a recording is still going on or finishing its files.
    pub fn is_live(&self, id: &str) -> bool {
        self.lock().contains_key(id)
    }

    /// A recording going on, as the API shows it.
    fn shown(&self, id: &str) -> Option<pb::Recording> {
        let live = self.lock();
        let rec = live.get(id)?;
        let tracks: Vec<pb::RecordingTrack> = lock(&rec.tracks).values().map(Stored::to_pb).collect();
        Some(pb::Recording {
            id: id.to_string(),
            channel_id: rec.channel_id.clone(),
            started_by: rec.started_by.clone(),
            started_at: Some(timestamp(rec.started_at)),
            ended_at: None,
            size_bytes: tracks.iter().map(|t| t.size_bytes).sum(),
            tracks,
        })
    }

    /// Stops every recording and waits (a while) for their files to finish.
    pub async fn finish_all(&self) {
        let tasks: Vec<_> = self
            .lock()
            .values_mut()
            .filter_map(|rec| {
                rec.stop.cancel();
                rec.task.take()
            })
            .collect();
        let _ = tokio::time::timeout(FINISH_WAIT, futures::future::join_all(tasks)).await;
    }

    /// Stops a server's recordings and waits (a while) for their files to
    /// finish, before it moves to another shard.
    pub async fn finish_server(&self, server_id: &str) {
        let tasks: Vec<_> = self
            .lock()
            .values_mut()
            .filter(|rec| rec.server_id == server_id)
            .filter_map(|rec| {
                rec.stop.cancel();
                rec.task.take()
            })
            .collect();
        let _ = tokio::time::timeout(FINISH_WAIT, futures::future::join_all(tasks)).await;
    }

    /// Starts and stops recordings as people turn them on and off, for as
    /// long as the part runs.
    pub fn spawn(app: Arc<App>) {
        tokio::spawn(async move {
            let mut every = tokio::time::interval(CHECK_EVERY);
            loop {
                tokio::select! {
                    _ = app.shutdown.cancelled() => return,
                    _ = every.tick() => {}
                    _ = app.recordings.nudged.notified() => {}
                }
                reconcile(&app).await;
            }
        });
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Stored {
    fn to_pb(&self) -> pb::RecordingTrack {
        pb::RecordingTrack { user_id: self.user_id.clone(), size_bytes: self.size_bytes, duration_ms: self.duration_ms }
    }
}

/// Brings the recordings going on in line with who wants one.
async fn reconcile(app: &Arc<App>) {
    let settings = app.settings();
    let wanted: Vec<(String, String, String)> = match settings.calls && settings.call_recordings {
        true => app.voice.recorded(),
        false => vec![],
    };
    let starting: Vec<(String, String, String)> = {
        let mut live = app.recordings.lock();
        for rec in live.values_mut() {
            let still = wanted.iter().any(|(s, c, _)| *s == rec.server_id && *c == rec.channel_id);
            if !still && !rec.stop.is_cancelled() {
                tracing::debug!(server = %rec.server_id, channel = %rec.channel_id, "a recording stops");
                rec.stop.cancel();
            }
        }
        wanted
            .into_iter()
            .filter(|(s, c, _)| {
                !live.values().any(|rec| rec.server_id == *s && rec.channel_id == *c && !rec.stop.is_cancelled())
            })
            .collect()
    };
    for (server_id, channel_id, by) in starting {
        if let Err(err) = start(app, &server_id, &channel_id, &by).await {
            tracing::warn!(server = %server_id, channel = %channel_id, error = %err, "couldn't start a recording");
        }
    }
}

/// Where a server's recordings are on disk.
fn server_dir(app: &App, server_id: &str) -> PathBuf {
    app.config.data_path.join("recordings").join(server_id)
}

/// A track's file name.
fn file_name(user_id: &str, sealed: bool) -> String {
    if sealed { format!("{user_id}.opus.sealed") } else { format!("{user_id}.opus") }
}

/// A track's key in the replica.
fn replica_key(server_id: &str, recording_id: &str, file: &str) -> String {
    format!("recordings/{server_id}/{recording_id}/{file}")
}

async fn start(app: &Arc<App>, server_id: &str, channel_id: &str, by: &str) -> Result<()> {
    let sdb = app.servers.get(server_id).await?;
    let row = RecordingRow {
        id: new_id(),
        channel_id: channel_id.to_string(),
        started_by: by.to_string(),
        started_at: now_ms(),
        ended_at: None,
        sealed: app.config.encryption_key.is_some(),
        tracks: "[]".into(),
        size_bytes: 0,
    };
    let dir = server_dir(app, &sdb.id).join(&row.id);
    std::fs::create_dir_all(&dir)?;
    let stop = CancellationToken::new();
    let tracks: Tracks = Arc::default();
    // Live before its row exists, so nothing listing it meanwhile takes it
    // for one a crash left unfinished.
    app.recordings.lock().insert(
        row.id.clone(),
        Live {
            server_id: server_id.to_string(),
            channel_id: row.channel_id.clone(),
            started_by: row.started_by.clone(),
            started_at: row.started_at,
            stop: stop.clone(),
            tracks: tracks.clone(),
            task: None,
        },
    );
    if let Err(err) = sdb.add_recording(&row).await {
        app.recordings.lock().remove(&row.id);
        let _ = std::fs::remove_dir(&dir);
        return Err(err);
    }
    tracing::info!(server = %sdb.id, channel = %channel_id, recording = %row.id, "a recording starts");
    let id = row.id.clone();
    let task = tokio::spawn(record(app.clone(), sdb, row, dir, stop, tracks));
    if let Some(live) = app.recordings.lock().get_mut(&id) {
        live.task = Some(task);
    }
    Ok(())
}

/// Records a call until `stop`: listens through a bridge (opening it again
/// whenever the media part restarts), writes everyone's frames, then
/// finishes the files and the row.
async fn record(
    app: Arc<App>,
    sdb: Arc<ServerDb>,
    row: RecordingRow,
    dir: PathBuf,
    stop: CancellationToken,
    tracks: Tracks,
) {
    use tokio_stream::StreamExt;
    let key = app.config.encryption_key.as_ref().map(|k| k.bytes());
    // The bridge is in the call as nobody anyone sees: it isn't a place in
    // `App::voice`, only on the media part, and it never speaks.
    let place = Place {
        session_id: new_id(),
        room: voice::channel_room(&sdb.id, &row.channel_id),
        state: pb::VoiceState {
            user_id: format!("rec-{}", row.id),
            channel_id: row.channel_id.clone(),
            suppress: true,
            ..Default::default()
        },
        expires: Instant::now(),
    };
    let started = Instant::now();
    let mut writing: HashMap<String, Track> = HashMap::new();
    let mut events: Option<voice::BridgeEvents> = None;
    let mut wait = Duration::ZERO;
    loop {
        let Some(heard) = events.as_mut() else {
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
            match app.media_link.bridge(&place).await {
                Ok(opened) => events = Some(opened),
                Err(err) => tracing::debug!(error = %err, "a recording's bridge couldn't open yet"),
            }
            wait = (wait * 2).clamp(Duration::from_millis(250), Duration::from_secs(4));
            continue;
        };
        tokio::select! {
            _ = stop.cancelled() => break,
            event = heard.next() => match event {
                Some(Bridged::Frame(frame)) => {
                    let Some(user_id) = crate::id::parse_id("account", &frame.participant).ok() else { continue };
                    if user_id != frame.participant {
                        continue;
                    }
                    let now = (started.elapsed().as_micros() as u64) * RATE / 1_000_000;
                    let track = match writing.entry(user_id) {
                        std::collections::hash_map::Entry::Occupied(t) => t.into_mut(),
                        std::collections::hash_map::Entry::Vacant(v) => {
                            let file = dir.join(file_name(v.key(), row.sealed));
                            match Track::create(&file, key.as_ref(), &row.id, v.key()) {
                                Ok(track) => v.insert(track),
                                Err(err) => {
                                    tracing::warn!(recording = %row.id, error = %err, "couldn't start a track");
                                    continue;
                                }
                            }
                        }
                    };
                    track.frame(now, frame.timestamp, &frame.frame);
                    let shown = Stored { user_id: frame.participant, size_bytes: track.size as i64, duration_ms: track.ms() };
                    lock(&tracks).insert(shown.user_id.clone(), shown);
                }
                // Hung up from under it (calls going off, the media part
                // restarting): it opens again for as long as it's wanted.
                Some(Bridged::Ended(_)) | None => {
                    events = None;
                    wait = Duration::from_millis(250);
                }
            }
        }
    }
    app.media_link.close(&place.room, Some(&place.state.user_id), Some(&place.session_id)).await;

    // Every track ends when the recording does, so they're all as long.
    let end = (started.elapsed().as_micros() as u64) * RATE / 1_000_000;
    let mut stored: Vec<Stored> = writing
        .iter_mut()
        .map(|(user_id, track)| {
            track.finish(end);
            Stored { user_id: user_id.clone(), size_bytes: track.size as i64, duration_ms: track.ms() }
        })
        .collect();
    stored.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    drop(writing);
    if let Err(err) = end_row(&app, &sdb, &row, &stored, now_ms()).await {
        tracing::warn!(recording = %row.id, error = %err, "couldn't finish a recording");
    }
    tracing::info!(server = %sdb.id, recording = %row.id, tracks = stored.len(), "a recording ended");
    app.recordings.lock().remove(&row.id);
}

/// Writes down how a recording ended, and copies its files to the replica.
async fn end_row(app: &App, sdb: &ServerDb, row: &RecordingRow, stored: &[Stored], ended_at: i64) -> Result<()> {
    let size: i64 = stored.iter().map(|t| t.size_bytes).sum();
    let json = serde_json::to_string(stored).map_err(|err| Error::internal(err.to_string()))?;
    sdb.end_recording(&row.id, ended_at, &json, size).await?;
    if let Some(replica) = &app.replica {
        let dir = server_dir(app, &sdb.id).join(&row.id);
        for track in stored {
            let file = file_name(&track.user_id, row.sealed);
            if let Err(err) = replica.store().put_file(&replica_key(&sdb.id, &row.id, &file), &dir.join(&file)).await {
                tracing::warn!(recording = %row.id, error = %err, "couldn't copy a recording to the replica");
            }
        }
    }
    Ok(())
}

/// A recording whose part stopped before it could finish it (a crash, or a
/// deploy that didn't wait): its files hold everything up to their last
/// page, so it ends where they do.
async fn settle(app: &App, sdb: &ServerDb, mut row: RecordingRow) -> Result<RecordingRow> {
    if row.ended_at.is_some() || app.recordings.is_live(&row.id) {
        return Ok(row);
    }
    let dir = server_dir(app, &sdb.id).join(&row.id);
    let key = app.config.encryption_key.as_ref().map(|k| k.bytes());
    let mut stored = Vec::new();
    let mut ended_at = row.started_at;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(user_id) = name.strip_suffix(if row.sealed { ".opus.sealed" } else { ".opus" }) else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            if let Ok(modified) = meta.modified()
                && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                ended_at = ended_at.max(since.as_millis() as i64);
            }
            let plain = read_plain(&entry.path(), key.as_ref(), &row.id, user_id, row.sealed).unwrap_or_default();
            stored.push(Stored {
                user_id: user_id.to_string(),
                size_bytes: meta.len() as i64,
                duration_ms: (last_granule(&plain) * 1000 / RATE) as i64,
            });
        }
    }
    stored.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    end_row(app, sdb, &row, &stored, ended_at).await?;
    row.ended_at = Some(ended_at);
    row.size_bytes = stored.iter().map(|t| t.size_bytes).sum();
    row.tracks = serde_json::to_string(&stored).unwrap_or_default();
    Ok(row)
}

fn shown(row: &RecordingRow) -> pb::Recording {
    let tracks: Vec<Stored> = serde_json::from_str(&row.tracks).unwrap_or_default();
    pb::Recording {
        id: row.id.clone(),
        channel_id: row.channel_id.clone(),
        started_by: row.started_by.clone(),
        started_at: Some(timestamp(row.started_at)),
        ended_at: row.ended_at.map(timestamp),
        tracks: tracks.iter().map(Stored::to_pb).collect(),
        size_bytes: row.size_bytes,
    }
}

/// A channel's recordings, newest first, the one going on now included.
pub async fn list(app: &App, sdb: &ServerDb, channel_id: &str) -> Result<Vec<pb::Recording>> {
    let mut out = Vec::new();
    for row in sdb.recordings(channel_id, LISTED).await? {
        match app.recordings.shown(&row.id) {
            Some(live) => out.push(live),
            None => out.push(shown(&settle(app, sdb, row).await?)),
        }
    }
    Ok(out)
}

/// A recording, as its row says.
pub async fn find(sdb: &ServerDb, id: &str) -> Result<RecordingRow> {
    let id = crate::id::parse_id("recording", id)?;
    sdb.recording(&id).await?.ok_or(Error::NotFound("recording"))
}

/// A recording, finished (settled if its part stopped mid-way).
pub async fn finished(app: &App, sdb: &ServerDb, row: RecordingRow) -> Result<RecordingRow> {
    if app.recordings.is_live(&row.id) {
        return Err(Error::FailedPrecondition("that recording is still going on".into()));
    }
    settle(app, sdb, row).await
}

/// One person's track of a finished recording, as plain Ogg Opus in pieces.
pub async fn download(
    app: &App,
    sdb: &ServerDb,
    row: &RecordingRow,
    user_id: &str,
) -> Result<tokio::sync::mpsc::Receiver<std::result::Result<Vec<u8>, Error>>> {
    let tracks: Vec<Stored> = serde_json::from_str(&row.tracks).unwrap_or_default();
    let track = tracks.iter().find(|t| t.user_id == user_id).ok_or(Error::NotFound("track"))?;
    let file = file_name(&track.user_id, row.sealed);
    let dir = server_dir(app, &sdb.id).join(&row.id);
    let path = dir.join(&file);
    if !path.exists() {
        let replica = app.replica.as_ref().ok_or(Error::NotFound("recording file"))?;
        std::fs::create_dir_all(&dir)?;
        if !replica.store().get_to_file(&replica_key(&sdb.id, &row.id, &file), &path).await? {
            return Err(Error::NotFound("recording file"));
        }
    }
    let key = app.config.encryption_key.as_ref().map(|k| k.bytes());
    if row.sealed && key.is_none() {
        return Err(Error::FailedPrecondition(
            "this recording was sealed with an encryption key the instance no longer has".into(),
        ));
    }
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let (id, user, sealed) = (row.id.clone(), track.user_id.clone(), row.sealed);
    tokio::task::spawn_blocking(move || {
        let sent = pieces(&path, key.as_ref(), &id, &user, sealed, &mut |piece| tx.blocking_send(Ok(piece)).is_ok());
        if let Err(err) = sent {
            let _ = tx.blocking_send(Err(err));
        }
    });
    Ok(rx)
}

/// Deletes a finished recording: its row, its files, and the replica's copies.
pub async fn delete(app: &App, sdb: &ServerDb, row: &RecordingRow) -> Result<()> {
    sdb.delete_recording(&row.id).await?;
    let dir = server_dir(app, &sdb.id).join(&row.id);
    let _ = std::fs::remove_dir_all(&dir);
    if let Some(replica) = &app.replica {
        let tracks: Vec<Stored> = serde_json::from_str(&row.tracks).unwrap_or_default();
        for track in tracks {
            let key = replica_key(&sdb.id, &row.id, &file_name(&track.user_id, row.sealed));
            if let Err(err) = replica.store().delete(&key).await {
                tracing::warn!(recording = %row.id, error = %err, "couldn't delete a recording from the replica");
            }
        }
    }
    Ok(())
}

// ───────────────────────────── Tracks ─────────────────────────────

/// One person's file, being written.
struct Track {
    /// None once writing failed (a full disk): the rest of the track is lost,
    /// not the recording.
    file: Option<std::fs::File>,
    ogg: Ogg,
    sealer: Option<Sealer>,
    /// Samples written, from the recording's start.
    samples: u64,
    /// Where the speaker's clock lines up with the recording's: a frame
    /// stamped `.1` goes at sample `.0`.
    anchor: Option<(u64, u32)>,
    /// Bytes in the file.
    size: u64,
}

impl Track {
    fn create(path: &Path, key: Option<&[u8; 32]>, recording_id: &str, user_id: &str) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new().create_new(true).write(true).open(path)?;
        let mut serial = [0u8; 4];
        let _ = getrandom::fill(&mut serial);
        let mut track = Self {
            file: Some(file),
            ogg: Ogg::new(u32::from_le_bytes(serial)),
            sealer: key.map(|k| Sealer::new(file_key(k, recording_id, user_id))),
            samples: 0,
            anchor: None,
            size: 0,
        };
        let headers = track.ogg.headers();
        track.write(headers);
        Ok(track)
    }

    fn ms(&self) -> i64 {
        (self.samples * 1000 / RATE) as i64
    }

    /// Writes a frame the speaker stamped `stamp` that arrived at sample
    /// `now`, after enough silence to put it where their clock says.
    fn frame(&mut self, now: u64, stamp: u32, packet: &[u8]) {
        let Some(samples) = opus_samples(packet) else { return };
        let by_clock = self.anchor.and_then(|(at, anchor)| {
            let since = i64::from(stamp.wrapping_sub(anchor) as i32);
            let place = at as i64 + since;
            (since >= 0 && (place - now as i64).abs() <= RESYNC).then_some(place as u64)
        });
        let start = match by_clock {
            Some(start) => start,
            None => {
                let start = now.max(self.samples);
                self.anchor = Some((start, stamp));
                start
            }
        };
        // Sent again, or too late to fit.
        if start + samples <= self.samples {
            return;
        }
        self.silence_until(start);
        self.push(packet, samples);
    }

    fn silence_until(&mut self, at: u64) {
        while self.samples + FRAME <= at {
            self.push(&SILENCE, FRAME);
        }
    }

    fn push(&mut self, packet: &[u8], samples: u64) {
        self.samples += samples;
        if let Some(page) = self.ogg.push(packet, samples) {
            self.write(page);
        }
    }

    fn write(&mut self, bytes: Vec<u8>) {
        let Some(file) = &mut self.file else { return };
        let bytes = match &mut self.sealer {
            Some(sealer) => sealer.seal(bytes),
            None => bytes,
        };
        match file.write_all(&bytes) {
            Ok(()) => self.size += bytes.len() as u64,
            Err(err) => {
                tracing::warn!(error = %err, "a recording's track stopped: couldn't write");
                self.file = None;
            }
        }
    }

    /// Pads the track with silence to `end` and closes the stream.
    fn finish(&mut self, end: u64) {
        self.silence_until(end);
        let last = self.ogg.flush(true);
        self.write(last);
        if let Some(file) = &self.file {
            let _ = file.sync_all();
        }
    }
}

/// How many 48 kHz samples an Opus packet holds, from its first byte (RFC
/// 6716 section 3.1). None for a packet that isn't one.
fn opus_samples(packet: &[u8]) -> Option<u64> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    // Each frame's length, in samples at 48 kHz.
    let frame: u64 = match config {
        0..=11 => [480, 960, 1920, 2880][usize::from(config % 4)],
        12..=15 => [480, 960][usize::from(config % 2)],
        _ => [120, 240, 480, 960][usize::from(config % 4)],
    };
    let frames: u64 = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u64::from(packet.get(1)? & 0x3F),
    };
    let samples = frame * frames;
    // A packet holds at most 120 ms.
    (frames > 0 && samples <= 5760).then_some(samples)
}

/// An Ogg Opus stream being written (RFC 7845), a page at a time.
struct Ogg {
    serial: u32,
    sequence: u32,
    /// Samples up to the end of the last packet pushed.
    granule: u64,
    lacing: Vec<u8>,
    data: Vec<u8>,
    packets: usize,
}

impl Ogg {
    fn new(serial: u32) -> Self {
        Self { serial, sequence: 0, granule: 0, lacing: vec![], data: vec![], packets: 0 }
    }

    /// The two header pages: OpusHead (mono, 48 kHz, nothing to skip, so
    /// every track lines up from sample 0) and OpusTags.
    fn headers(&mut self) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&0u16.to_le_bytes()); // pre-skip
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes()); // gain
        head.push(0); // channel mapping
        let vendor = b"fuwa";
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        let mut out = self.page(0x02, 0, &lacing(head.len()), &head);
        out.extend(self.page(0x00, 0, &lacing(tags.len()), &tags));
        out
    }

    /// Adds a packet, giving back a page when one is full.
    fn push(&mut self, packet: &[u8], samples: u64) -> Option<Vec<u8>> {
        let lace = lacing(packet.len());
        let mut out = None;
        if self.lacing.len() + lace.len() > 255 {
            out = Some(self.flush(false));
        }
        self.lacing.extend_from_slice(&lace);
        self.data.extend_from_slice(packet);
        self.granule += samples;
        self.packets += 1;
        if self.packets >= PAGE_FRAMES {
            out.get_or_insert_with(Vec::new).extend(self.flush(false));
        }
        out
    }

    /// The packets waiting, as a page (the stream's last when `last`).
    fn flush(&mut self, last: bool) -> Vec<u8> {
        if self.packets == 0 && !last {
            return vec![];
        }
        let (lacing, data) = (std::mem::take(&mut self.lacing), std::mem::take(&mut self.data));
        self.packets = 0;
        self.page(if last { 0x04 } else { 0x00 }, self.granule, &lacing, &data)
    }

    fn page(&mut self, kind: u8, granule: u64, lacing: &[u8], data: &[u8]) -> Vec<u8> {
        let mut page = Vec::with_capacity(27 + lacing.len() + data.len());
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(kind);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(lacing.len() as u8);
        page.extend_from_slice(lacing);
        page.extend_from_slice(data);
        let crc = ogg_crc(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.sequence += 1;
        page
    }
}

/// A packet's lacing values: 255s, then what's left (0 when it's a multiple).
fn lacing(len: usize) -> Vec<u8> {
    let mut out = vec![255u8; len / 255];
    out.push((len % 255) as u8);
    out
}

/// Ogg's CRC-32: polynomial 0x04C11DB7, not reflected, starting at 0.
fn ogg_crc(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            }
            *entry = r;
        }
        table
    });
    bytes.iter().fold(0u32, |crc, &b| (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize])
}

/// The granule (samples) of the last whole page in an Ogg stream.
fn last_granule(ogg: &[u8]) -> u64 {
    let mut at = 0;
    let mut last = 0;
    while at + 27 <= ogg.len() && &ogg[at..at + 4] == b"OggS" {
        let segments = usize::from(ogg[at + 26]);
        let Some(lacing) = ogg.get(at + 27..at + 27 + segments) else { break };
        let size = 27 + segments + lacing.iter().map(|&l| usize::from(l)).sum::<usize>();
        if at + size > ogg.len() {
            break;
        }
        last = u64::from_le_bytes(ogg[at + 6..at + 14].try_into().unwrap_or_default());
        at += size;
    }
    last
}

// ───────────────────────────── Sealing ─────────────────────────────

/// The key one track's file is sealed with, from the instance's key.
fn file_key(master: &[u8; 32], recording_id: &str, user_id: &str) -> LessSafeKey {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"fuwa call recordings");
    let info = [recording_id.as_bytes(), b"/", user_id.as_bytes()];
    let prk = salt.extract(master);
    let okm = prk.expand(&info, &aead::CHACHA20_POLY1305).expect("one key's length is in range");
    LessSafeKey::new(UnboundKey::from(okm))
}

fn nonce(counter: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::assume_unique_for_key(bytes)
}

/// Seals a file's chunks in order: each is its length (4 bytes, big endian)
/// then the ciphertext and tag, under the chunk's number as its nonce. Every
/// file has its own key, so numbers never repeat under one.
struct Sealer {
    key: LessSafeKey,
    counter: u64,
}

impl Sealer {
    fn new(key: LessSafeKey) -> Self {
        Self { key, counter: 0 }
    }

    fn seal(&mut self, mut chunk: Vec<u8>) -> Vec<u8> {
        if chunk.is_empty() {
            return chunk;
        }
        self.key.seal_in_place_append_tag(nonce(self.counter), Aad::empty(), &mut chunk).expect("chunks are small");
        self.counter += 1;
        let mut out = (chunk.len() as u32).to_be_bytes().to_vec();
        out.extend(chunk);
        out
    }
}

/// Reads a track's file as plain Ogg, handing it on in pieces of about
/// [`PIECE`] until `send` says stop. A sealed file's last chunk, cut short
/// by a crash, is left out.
fn pieces(
    path: &Path,
    key: Option<&[u8; 32]>,
    recording_id: &str,
    user_id: &str,
    sealed: bool,
    send: &mut dyn FnMut(Vec<u8>) -> bool,
) -> Result<()> {
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    if !sealed {
        loop {
            let mut piece = vec![0u8; PIECE];
            let read = read_up_to(&mut file, &mut piece)?;
            if read == 0 {
                return Ok(());
            }
            piece.truncate(read);
            if !send(piece) {
                return Ok(());
            }
        }
    }
    let key = file_key(key.ok_or_else(|| Error::internal("no key to unseal a recording with"))?, recording_id, user_id);
    let mut counter = 0u64;
    let mut piece = Vec::with_capacity(PIECE);
    loop {
        let mut length = [0u8; 4];
        if read_up_to(&mut file, &mut length)? < 4 {
            break;
        }
        let mut chunk = vec![0u8; u32::from_be_bytes(length) as usize];
        if read_up_to(&mut file, &mut chunk)? < chunk.len() {
            break;
        }
        let plain = key
            .open_in_place(nonce(counter), Aad::empty(), &mut chunk)
            .map_err(|_| Error::internal("a recording's file doesn't unseal"))?;
        counter += 1;
        piece.extend_from_slice(plain);
        if piece.len() >= PIECE && !send(std::mem::take(&mut piece)) {
            return Ok(());
        }
    }
    if !piece.is_empty() {
        send(piece);
    }
    Ok(())
}

/// Reads until `buf` is full or the file ends, giving back how much came.
fn read_up_to(file: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// A whole track as plain Ogg.
fn read_plain(path: &Path, key: Option<&[u8; 32]>, recording_id: &str, user_id: &str, sealed: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    pieces(path, key, recording_id, user_id, sealed, &mut |piece| {
        out.extend(piece);
        true
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_packets_say_how_long_they_are() {
        assert_eq!(opus_samples(&SILENCE), Some(960));
        // SILK, 20 ms, one frame.
        assert_eq!(opus_samples(&[0x08]), Some(960));
        // CELT, 10 ms, two frames.
        assert_eq!(opus_samples(&[(30 << 3) | 1]), Some(960));
        // Code 3: the count is in the second byte.
        assert_eq!(opus_samples(&[(31 << 3) | 3, 3]), Some(2880));
        assert_eq!(opus_samples(&[]), None);
        assert_eq!(opus_samples(&[(31 << 3) | 3, 0]), None);
    }

    #[test]
    fn ogg_pages_check_out() {
        // The CRC of "OggS" pages as libogg computes it.
        assert_eq!(ogg_crc(b""), 0);
        assert_eq!(ogg_crc(b"123456789"), 0x89A1_897F);
        assert_eq!(lacing(0), vec![0]);
        assert_eq!(lacing(255), vec![255, 0]);
        assert_eq!(lacing(300), vec![255, 45]);
    }

    fn track(dir: &Path, key: Option<&[u8; 32]>) -> (Track, PathBuf) {
        let path = dir.join(file_name("01J00000000000000000000000", key.is_some()));
        (Track::create(&path, key, "01J00000000000000000000001", "01J00000000000000000000000").unwrap(), path)
    }

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fuwa-recording-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tracks_line_up_with_silence_and_follow_the_speakers_clock() {
        let dir = temp();
        let (mut t, path) = track(&dir, None);
        let frame = [0xFCu8, 1, 2, 3];
        // First heard a second in: a second of silence comes first.
        t.frame(RATE, 1000, &frame);
        assert_eq!(t.samples, RATE + FRAME);
        // The next frame 20 ms later on their clock, arriving a bit late.
        t.frame(RATE + 1500, 1000 + 960, &frame);
        assert_eq!(t.samples, RATE + 2 * FRAME);
        // Sent again: left out.
        t.frame(RATE + 2000, 1000 + 960, &frame);
        assert_eq!(t.samples, RATE + 2 * FRAME);
        // Half a second quiet on their clock.
        t.frame(RATE + 30_000, 1000 + 960 + 24_000, &frame);
        assert_eq!(t.samples, RATE + 960 + 24_000 + FRAME);
        // Their clock jumps (they joined again): arrival time wins.
        t.frame(10 * RATE, 7, &frame);
        assert_eq!(t.samples, 10 * RATE + FRAME);
        t.finish(12 * RATE);
        assert_eq!(t.samples, 12 * RATE);
        let ogg = std::fs::read(&path).unwrap();
        assert_eq!(&ogg[..4], b"OggS");
        assert_eq!(last_granule(&ogg), 12 * RATE);
        assert_eq!(t.size, ogg.len() as u64);
        // Every page's CRC is right.
        let mut at = 0;
        while at < ogg.len() {
            let segments = usize::from(ogg[at + 26]);
            let size = 27 + segments + ogg[at + 27..at + 27 + segments].iter().map(|&l| usize::from(l)).sum::<usize>();
            let mut page = ogg[at..at + size].to_vec();
            let crc = u32::from_le_bytes(page[22..26].try_into().unwrap());
            page[22..26].copy_from_slice(&[0; 4]);
            assert_eq!(ogg_crc(&page), crc);
            at += size;
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sealed_tracks_read_back_as_plain_ogg() {
        let dir = temp();
        let key = [7u8; 32];
        let (mut t, path) = track(&dir, Some(&key));
        for i in 0..200u32 {
            t.frame(u64::from(i) * FRAME, i * 960, &[0xFC, i as u8]);
        }
        t.finish(200 * FRAME);
        let sealed = std::fs::read(&path).unwrap();
        assert!(!sealed.windows(4).any(|w| w == b"OggS"), "nothing readable on disk");
        let plain =
            read_plain(&path, Some(&key), "01J00000000000000000000001", "01J00000000000000000000000", true).unwrap();
        assert_eq!(&plain[..4], b"OggS");
        assert_eq!(last_granule(&plain), 200 * FRAME);
        // Another key (another recording) doesn't open it.
        assert!(
            read_plain(&path, Some(&key), "01J00000000000000000000002", "01J00000000000000000000000", true).is_err()
        );
        // Cut short by a crash: what's whole still reads.
        std::fs::write(&path, &sealed[..sealed.len() - 10]).unwrap();
        let cut =
            read_plain(&path, Some(&key), "01J00000000000000000000001", "01J00000000000000000000000", true).unwrap();
        assert!(last_granule(&cut) > 0 && cut.len() < plain.len() && plain.starts_with(&cut));
        let _ = std::fs::remove_dir_all(dir);
    }
}
