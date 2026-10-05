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
//! A server's recordings may be capped, all together
//! (`ServerLimits.recording_bytes`, FUWA_LIMIT_RECORDING_STORAGE): once
//! they reach it, the one going on stops and no new one starts until some
//! are deleted. And finished ones may delete themselves after a number of
//! days (FUWA_CALL_RECORDINGS_KEEP_DAYS). Neither is set by default.
//!
//! With the server's `record_video` on (only where the instance's
//! `call_recording_video` lets it), the bridge watches too, and each
//! person's camera and shared screen go to files of their own next to
//! their sound (`<account>.camera.webm`, `<account>.screen.webm`): WebM
//! with the VP8 frames as they were sent (cameras at half their full size),
//! never decoded or put together, each starting at its first keyframe and
//! timed from the recording's start like the sound. Changing the setting
//! ends the recording going on and everyone's ask to record ([`end_asks`]):
//! the next starts when someone presses Record, so nobody starts being
//! filmed without everyone hearing a recording start.
//!
//! Direct-message calls are never recorded here: their sound is end-to-end
//! encrypted, so all a bridge could keep is ciphertext.

use std::collections::{BTreeMap, HashMap, HashSet};
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
use crate::servers::{Payload, RecordingRow, ServerDb};
use crate::voice::{self, Place};

/// A WebM cluster closes after this long, so a crash loses at most this
/// much of a picture.
const CLUSTER_MS: u64 = 2_000;
/// A picture's clock is followed while it's within this of when its frames
/// arrive, in ms; past that (it started again) arrival time wins.
const PICTURE_RESYNC: i64 = 2_000;

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
/// How often recordings older than the instance keeps them are looked for.
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// Recordings one sweep deletes per server at most; the next sweep does the rest.
const SWEPT: i64 = 200;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The recordings going on in calls this part keeps.
#[derive(Default)]
pub struct Recordings {
    live: Mutex<HashMap<String, Live>>,
    nudged: Notify,
    /// (server, user) whose RECORD on the server a change to what
    /// recordings keep turned off: they press Record again to start a new
    /// one, so nobody is filmed without everyone hearing it start.
    ended: Mutex<HashSet<(String, String)>>,
}

/// A recording going on (or finishing its files).
struct Live {
    server_id: String,
    channel_id: String,
    started_by: String,
    started_at: i64,
    /// It keeps cameras and screens too.
    video: bool,
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
    /// The sound's file; zero when there's none (they never spoke).
    pub size_bytes: i64,
    pub duration_ms: i64,
    #[serde(default)]
    pub camera_bytes: i64,
    #[serde(default)]
    pub screen_bytes: i64,
}

/// One of a person's files in a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Part {
    Sound,
    Camera,
    Screen,
}

impl Part {
    const ALL: [Part; 3] = [Part::Sound, Part::Camera, Part::Screen];

    fn suffix(self) -> &'static str {
        match self {
            Self::Sound => ".opus",
            Self::Camera => ".camera.webm",
            Self::Screen => ".screen.webm",
        }
    }

    /// What tells its sealing key apart from the person's other files'.
    fn key_name(self, user_id: &str) -> String {
        match self {
            // As it always was, so sealed sound from before video opens.
            Self::Sound => user_id.to_string(),
            Self::Camera => format!("{user_id}/camera"),
            Self::Screen => format!("{user_id}/screen"),
        }
    }

    pub fn of(part: i32) -> Result<Self> {
        match pb::RecordingPart::try_from(part) {
            Ok(pb::RecordingPart::Unspecified) => Ok(Self::Sound),
            Ok(pb::RecordingPart::Camera) => Ok(Self::Camera),
            Ok(pb::RecordingPart::Screen) => Ok(Self::Screen),
            Err(_) => Err(Error::invalid("unknown part of a recording")),
        }
    }
}

impl Recordings {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Live>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// What recordings keep changed: those who were recording on the
    /// server stop, and must ask again.
    pub fn end_for(&self, server_id: &str, users: impl IntoIterator<Item = String>) {
        let mut ended = self.ended.lock().unwrap_or_else(|p| p.into_inner());
        ended.extend(users.into_iter().map(|u| (server_id.to_string(), u)));
        drop(ended);
        self.nudge();
    }

    /// Whether `user` asking to record on the server is still the ask a
    /// change ended (and forgets it: the next ask is a new one).
    pub fn was_ended(&self, server_id: &str, user_id: &str) -> bool {
        let mut ended = self.ended.lock().unwrap_or_else(|p| p.into_inner());
        ended.remove(&(server_id.to_string(), user_id.to_string()))
    }

    /// Whether a change ended `user`'s ask to record on the server, and
    /// they haven't been told yet.
    pub fn is_ended(&self, server_id: &str, user_id: &str) -> bool {
        let ended = self.ended.lock().unwrap_or_else(|p| p.into_inner());
        ended.contains(&(server_id.to_string(), user_id.to_string()))
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
            size_bytes: tracks.iter().map(|t| t.size_bytes + t.camera_bytes + t.screen_bytes).sum(),
            tracks,
            video: rec.video,
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
            let mut swept: Option<Instant> = None;
            let mut filming = None;
            loop {
                tokio::select! {
                    _ = app.shutdown.cancelled() => return,
                    _ = every.tick() => {}
                    _ = app.recordings.nudged.notified() => {}
                }
                let allowed = app.settings().call_recording_video;
                if !allowed && filming != Some(false) {
                    stop_filming(&app).await;
                }
                filming = Some(allowed);
                reconcile(&app).await;
                if swept.is_none_or(|at| at.elapsed() >= SWEEP_EVERY) {
                    swept = Some(Instant::now());
                    sweep(&app).await;
                }
            }
        });
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Stored {
    fn to_pb(&self) -> pb::RecordingTrack {
        pb::RecordingTrack {
            user_id: self.user_id.clone(),
            size_bytes: self.size_bytes,
            duration_ms: self.duration_ms,
            camera_bytes: self.camera_bytes,
            screen_bytes: self.screen_bytes,
        }
    }

    fn total(&self) -> i64 {
        self.size_bytes + self.camera_bytes + self.screen_bytes
    }

    fn bytes(&self, part: Part) -> i64 {
        match part {
            Part::Sound => self.size_bytes,
            Part::Camera => self.camera_bytes,
            Part::Screen => self.screen_bytes,
        }
    }

    fn bytes_mut(&mut self, part: Part) -> &mut i64 {
        match part {
            Part::Sound => &mut self.size_bytes,
            Part::Camera => &mut self.camera_bytes,
            Part::Screen => &mut self.screen_bytes,
        }
    }

    /// The parts it has files for.
    fn parts(&self) -> impl Iterator<Item = Part> + '_ {
        Part::ALL.into_iter().filter(|p| self.bytes(*p) > 0)
    }
}

/// What a server's recordings come to (the ones going on included), and
/// the most they may.
pub async fn usage(app: &App, sdb: &ServerDb) -> Result<(i64, Option<i64>)> {
    let cap = sdb.limits(&app.settings().limits).await?.recording_bytes;
    let live: i64 = app
        .recordings
        .lock()
        .values()
        .filter(|rec| rec.server_id == sdb.id)
        .map(|rec| lock(&rec.tracks).values().map(Stored::total).sum::<i64>())
        .sum();
    Ok((sdb.recording_bytes().await? + live, cap))
}

/// Whether a server's recordings reached its cap, so none may go on.
pub async fn full(app: &App, sdb: &ServerDb) -> Result<bool> {
    let (used, cap) = usage(app, sdb).await?;
    Ok(cap.is_some_and(|cap| used >= cap))
}

/// What a server's recordings keep changed: a recording going on ends, and
/// everyone recording on the server stops until they press Record again, so
/// a new one starts the way any recording does, for everyone to see and hear.
pub fn end_asks(app: &App, server_id: &str) {
    let asking: Vec<String> =
        app.voice.list(server_id).into_iter().filter(|p| p.state.server_record).map(|p| p.state.user_id).collect();
    // Ended before their Record turns off, so a KeepVoice landing in between
    // can't count as a new ask.
    app.recordings.end_for(server_id, asking.iter().cloned());
    for user_id in asking {
        if let Some(place) = app.voice.update(server_id, &user_id, |p| p.state.server_record = false) {
            app.hub.publish([pb::Event {
                id: new_id(),
                server_id: server_id.to_string(),
                sequence: 0,
                actor_id: user_id,
                created_at: Some(timestamp(now_ms())),
                payload: Some(Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state) })),
            }]);
        }
    }
}

/// The instance stopped letting servers record video: turns it off in every
/// server this part holds that had it on, and tells everyone.
async fn stop_filming(app: &Arc<App>) {
    for sdb in app.servers.all() {
        if !sdb.server().await.is_ok_and(|s| s.record_video) {
            continue;
        }
        let written = sdb
            .write("", async |conn, events| {
                let now = now_ms();
                let turned = conn
                    .execute("UPDATE server SET record_video = 0, updated_at = ?1 WHERE record_video = 1", [now])
                    .await?;
                if turned == 0 {
                    return Ok(None);
                }
                let server = crate::servers::load_server(conn).await?;
                events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                Ok(Some(server))
            })
            .await;
        match written {
            Ok(Some(server)) => {
                app.server_changed(&server).await;
                end_asks(app, &sdb.id);
            }
            Ok(None) => {}
            Err(_) => tracing::warn!("couldn't stop a server recording video"),
        }
    }
}

/// Brings the recordings going on in line with who wants one, and the caps.
async fn reconcile(app: &Arc<App>) {
    let settings = app.settings();
    let mut wanted: Vec<(String, String, String)> = match settings.calls && settings.call_recordings {
        true => app.voice.recorded(),
        false => vec![],
    };
    let mut servers: Vec<String> = wanted.iter().map(|(s, _, _)| s.clone()).collect();
    servers.sort();
    servers.dedup();
    let mut over = Vec::new();
    let mut filmed = Vec::new();
    for server_id in servers {
        let Ok(sdb) = app.servers.get(&server_id).await else { continue };
        if full(app, &sdb).await.unwrap_or(false) {
            over.push(server_id);
        } else if settings.call_recording_video && sdb.server().await.is_ok_and(|s| s.record_video) {
            filmed.push(server_id);
        }
    }
    if !over.is_empty() {
        tracing::debug!(servers = over.len(), "recordings stop at their servers' caps");
        wanted.retain(|(s, _, _)| !over.contains(s));
    }
    // A recording keeping more or less than its server now says ends, and
    // so does everyone's ask: a new one only starts when someone presses
    // Record again, so nobody is filmed without hearing it start.
    let mut changed: Vec<String> = Vec::new();
    let starting: Vec<(String, String, String)> = {
        let mut live = app.recordings.lock();
        for rec in live.values_mut() {
            let asked = wanted.iter().any(|(s, c, _)| *s == rec.server_id && *c == rec.channel_id);
            let as_it_says = filmed.contains(&rec.server_id) == rec.video;
            if asked && !as_it_says && !rec.stop.is_cancelled() {
                changed.push(rec.server_id.clone());
            }
            if !(asked && as_it_says) && !rec.stop.is_cancelled() {
                tracing::debug!("a recording stops");
                rec.stop.cancel();
            }
        }
        wanted.retain(|(s, _, _)| !changed.contains(s));
        wanted
            .into_iter()
            .filter(|(s, c, _)| {
                !live.values().any(|rec| rec.server_id == *s && rec.channel_id == *c && !rec.stop.is_cancelled())
            })
            .collect()
    };
    changed.sort();
    changed.dedup();
    for server_id in &changed {
        end_asks(app, server_id);
    }
    for (server_id, channel_id, by) in starting {
        let video = filmed.contains(&server_id);
        if start(app, &server_id, &channel_id, &by, video).await.is_err() {
            tracing::warn!("couldn't start a recording");
        }
    }
}

/// Deletes finished recordings older than the instance keeps them, in every
/// server this part holds.
pub async fn sweep(app: &Arc<App>) {
    let Some(days) = app.settings().call_recordings_keep_days else { return };
    let before = now_ms() - days.saturating_mul(DAY_MS);
    for sdb in app.servers.all() {
        let rows = match sdb.recordings_ended_before(before, SWEPT).await {
            Ok(rows) => rows,
            Err(_) => {
                tracing::warn!("couldn't look for old recordings");
                continue;
            }
        };
        for row in rows {
            match delete(app, &sdb, &row).await {
                Ok(()) => tracing::debug!("an old recording deleted itself"),
                Err(_) => {
                    tracing::warn!("couldn't delete an old recording")
                }
            }
        }
    }
}

/// Where a server's recordings are on disk.
fn server_dir(app: &App, server_id: &str) -> PathBuf {
    app.config.data_path.join("recordings").join(server_id)
}

/// A track's file name.
fn file_name(user_id: &str, part: Part, sealed: bool) -> String {
    let suffix = part.suffix();
    if sealed { format!("{user_id}{suffix}.sealed") } else { format!("{user_id}{suffix}") }
}

/// Whose file and which, from its name.
fn parse_file_name(name: &str, sealed: bool) -> Option<(&str, Part)> {
    let name = if sealed { name.strip_suffix(".sealed")? } else { name };
    Part::ALL.into_iter().find_map(|part| Some((name.strip_suffix(part.suffix())?, part)))
}

/// A track's key in the replica.
fn replica_key(server_id: &str, recording_id: &str, file: &str) -> String {
    format!("recordings/{server_id}/{recording_id}/{file}")
}

async fn start(app: &Arc<App>, server_id: &str, channel_id: &str, by: &str, video: bool) -> Result<()> {
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
        video,
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
            video,
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
    tracing::info!("a recording starts");
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
    let mut filming: HashMap<(String, Part), Film> = HashMap::new();
    let mut events: Option<voice::BridgeEvents> = None;
    let mut wait = Duration::ZERO;
    loop {
        let Some(heard) = events.as_mut() else {
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
            match app.media_link.bridge(&place, row.video).await {
                Ok(opened) => events = Some(opened),
                Err(_) => tracing::debug!("a recording's bridge couldn't open yet"),
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
                            let file = dir.join(file_name(v.key(), Part::Sound, row.sealed));
                            match Track::create(&file, key.as_ref(), &row.id, v.key()) {
                                Ok(track) => v.insert(track),
                                Err(_) => {
                                    tracing::warn!("couldn't start a track");
                                    continue;
                                }
                            }
                        }
                    };
                    track.frame(now, frame.timestamp, &frame.frame);
                    let (size, ms) = (track.size as i64, track.ms());
                    let mut tracks = lock(&tracks);
                    let shown = tracks.entry(frame.participant.clone()).or_default();
                    shown.user_id = frame.participant;
                    (shown.size_bytes, shown.duration_ms) = (size, ms);
                }
                Some(Bridged::Picture(picture)) => {
                    if !row.video {
                        continue;
                    }
                    let Some(user_id) = crate::id::parse_id("account", &picture.participant).ok() else { continue };
                    if user_id != picture.participant {
                        continue;
                    }
                    let part = if picture.screen { Part::Screen } else { Part::Camera };
                    let ms = started.elapsed().as_millis() as u64;
                    let film = match filming.entry((user_id, part)) {
                        std::collections::hash_map::Entry::Occupied(f) => f.into_mut(),
                        std::collections::hash_map::Entry::Vacant(v) => {
                            // Nothing before a keyframe can be shown.
                            if !picture.keyframe {
                                continue;
                            }
                            let user_id = &v.key().0;
                            let file = dir.join(file_name(user_id, part, row.sealed));
                            match Film::create(&file, key.as_ref(), &row.id, &part.key_name(user_id)) {
                                Ok(film) => v.insert(film),
                                Err(_) => {
                                    tracing::warn!("couldn't start a picture");
                                    continue;
                                }
                            }
                        }
                    };
                    film.picture(ms, picture.time, &picture.frame, picture.keyframe);
                    let size = film.size as i64;
                    let mut tracks = lock(&tracks);
                    let shown = tracks.entry(picture.participant.clone()).or_default();
                    shown.user_id = picture.participant;
                    *shown.bytes_mut(part) = size;
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
    let mut by_user: BTreeMap<String, Stored> = BTreeMap::new();
    for (user_id, track) in &mut writing {
        track.finish(end);
        let stored = by_user.entry(user_id.clone()).or_default();
        (stored.size_bytes, stored.duration_ms) = (track.size as i64, track.ms());
    }
    for ((user_id, part), film) in &mut filming {
        film.finish();
        *by_user.entry(user_id.clone()).or_default().bytes_mut(*part) = film.size as i64;
    }
    let stored: Vec<Stored> = by_user.into_iter().map(|(user_id, stored)| Stored { user_id, ..stored }).collect();
    drop((writing, filming));
    if end_row(&app, &sdb, &row, &stored, now_ms()).await.is_err() {
        tracing::warn!("couldn't finish a recording");
    }
    tracing::info!(tracks = stored.len(), "a recording ended");
    app.recordings.lock().remove(&row.id);
}

/// Writes down how a recording ended, and copies its files to the replica.
async fn end_row(app: &App, sdb: &ServerDb, row: &RecordingRow, stored: &[Stored], ended_at: i64) -> Result<()> {
    let size: i64 = stored.iter().map(Stored::total).sum();
    let json = serde_json::to_string(stored).map_err(|err| Error::internal(err.to_string()))?;
    sdb.end_recording(&row.id, ended_at, &json, size).await?;
    if let Some(replica) = &app.replica {
        let dir = server_dir(app, &sdb.id).join(&row.id);
        for track in stored {
            for part in track.parts() {
                let file = file_name(&track.user_id, part, row.sealed);
                if replica.store().put_file(&replica_key(&sdb.id, &row.id, &file), &dir.join(&file)).await.is_err() {
                    tracing::warn!("couldn't copy a recording to the replica");
                }
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
    let mut by_user: BTreeMap<String, Stored> = BTreeMap::new();
    let mut ended_at = row.started_at;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((user_id, part)) = parse_file_name(&name, row.sealed) else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() == 0 {
                continue;
            }
            if let Ok(modified) = meta.modified()
                && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                ended_at = ended_at.max(since.as_millis() as i64);
            }
            let stored = by_user.entry(user_id.to_string()).or_default();
            *stored.bytes_mut(part) = meta.len() as i64;
            if part == Part::Sound {
                let key_name = part.key_name(user_id);
                let plain = read_plain(&entry.path(), key.as_ref(), &row.id, &key_name, row.sealed).unwrap_or_default();
                stored.duration_ms = (last_granule(&plain) * 1000 / RATE) as i64;
            }
        }
    }
    let stored: Vec<Stored> = by_user.into_iter().map(|(user_id, stored)| Stored { user_id, ..stored }).collect();
    end_row(app, sdb, &row, &stored, ended_at).await?;
    row.ended_at = Some(ended_at);
    row.size_bytes = stored.iter().map(Stored::total).sum();
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
        video: row.video,
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

/// One person's file of a finished recording, plain (Ogg Opus or WebM), in
/// pieces.
pub async fn download(
    app: &App,
    sdb: &ServerDb,
    row: &RecordingRow,
    user_id: &str,
    part: i32,
) -> Result<tokio::sync::mpsc::Receiver<std::result::Result<Vec<u8>, Error>>> {
    let part = Part::of(part)?;
    let tracks: Vec<Stored> = serde_json::from_str(&row.tracks).unwrap_or_default();
    let track = tracks.iter().find(|t| t.user_id == user_id && t.bytes(part) > 0).ok_or(Error::NotFound("track"))?;
    let file = file_name(&track.user_id, part, row.sealed);
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
    let (id, user, sealed) = (row.id.clone(), part.key_name(&track.user_id), row.sealed);
    tokio::task::spawn_blocking(move || {
        let sent = pieces(&path, key.as_ref(), &id, &user, sealed, &mut |piece| tx.blocking_send(Ok(piece)).is_ok());
        if let Err(err) = sent {
            let _ = tx.blocking_send(Err(err));
        }
    });
    Ok(rx)
}

/// Deletes a finished recording: the replica's copies first, then its files
/// and its row. A copy that won't go keeps the row, so deleting it again
/// (or the next sweep) tries once more rather than leaving it behind.
pub async fn delete(app: &App, sdb: &ServerDb, row: &RecordingRow) -> Result<()> {
    if let Some(replica) = &app.replica {
        let tracks: Vec<Stored> = serde_json::from_str(&row.tracks).unwrap_or_default();
        for track in &tracks {
            for part in track.parts() {
                let key = replica_key(&sdb.id, &row.id, &file_name(&track.user_id, part, row.sealed));
                replica.store().delete(&key).await.map_err(|_| {
                    tracing::warn!("couldn't delete a recording from the replica");
                    Error::Unavailable("couldn't delete the recording's copy; try again".into())
                })?;
            }
        }
    }
    let dir = server_dir(app, &sdb.id).join(&row.id);
    let _ = std::fs::remove_dir_all(&dir);
    sdb.delete_recording(&row.id).await
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
            Err(_) => {
                tracing::warn!("a recording's track stopped: couldn't write");
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

// ───────────────────────────── Pictures ─────────────────────────────

/// One person's camera or screen, being written as WebM: a VP8 track, the
/// frames as they came, in clusters of up to [`CLUSTER_MS`]. The segment's
/// size is left unknown (as a live stream's is), so nothing needs going
/// back to; what's written is playable even if the part stops mid-way.
struct Film {
    /// None once writing failed: the rest of the picture is lost, not the
    /// recording.
    file: Option<std::fs::File>,
    sealer: Option<Sealer>,
    /// Where the camera's clock lines up with the recording's: a frame
    /// filmed at `.1` (90 kHz) goes at `.0` ms.
    anchor: Option<(u64, u64)>,
    /// The last frame's time, in ms from the recording's start.
    last: u64,
    /// The cluster being filled: its time and blocks.
    cluster: Option<(u64, Vec<u8>)>,
    /// Whether the header is written (it waits for a keyframe, which says
    /// how big the picture is).
    started: bool,
    size: u64,
}

impl Film {
    fn create(path: &Path, key: Option<&[u8; 32]>, recording_id: &str, key_name: &str) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new().create_new(true).write(true).open(path)?;
        Ok(Self {
            file: Some(file),
            sealer: key.map(|k| Sealer::new(file_key(k, recording_id, key_name))),
            anchor: None,
            last: 0,
            cluster: None,
            started: false,
            size: 0,
        })
    }

    /// Adds a frame filmed at `time` (90 kHz) that arrived `now` ms into
    /// the recording.
    fn picture(&mut self, now: u64, time: u64, frame: &[u8], keyframe: bool) {
        if !self.started {
            let Some((width, height)) = keyframe.then(|| vp8_size(frame)).flatten() else { return };
            let header = webm_header(width, height);
            self.write(header);
            self.started = true;
        }
        let by_clock = self.anchor.and_then(|(at, anchor)| {
            let since = time.checked_sub(anchor)? / 90;
            let place = at + since;
            ((place as i64 - now as i64).abs() <= PICTURE_RESYNC).then_some(place)
        });
        let at = match by_clock {
            Some(at) => at,
            None => {
                let at = now.max(self.last);
                self.anchor = Some((at, time));
                at
            }
        }
        .max(self.last);
        self.last = at;
        let full = self.cluster.as_ref().is_some_and(|(start, blocks)| {
            (keyframe || at - start >= CLUSTER_MS || blocks.len() > 8 << 20) && !blocks.is_empty()
        });
        if full {
            self.close_cluster();
        }
        let (start, blocks) = self.cluster.get_or_insert_with(|| (at, Vec::new()));
        let mut block = vec![0x81];
        block.extend_from_slice(&((at - *start) as i16).to_be_bytes());
        block.push(if keyframe { 0x80 } else { 0 });
        block.extend_from_slice(frame);
        blocks.extend(element(0xA3, &block));
    }

    fn close_cluster(&mut self) {
        let Some((start, blocks)) = self.cluster.take() else { return };
        let mut body = uint(0xE7, start);
        body.extend(blocks);
        self.write(element(0x1F43_B675, &body));
    }

    fn write(&mut self, bytes: Vec<u8>) {
        let Some(file) = &mut self.file else { return };
        let bytes = match &mut self.sealer {
            Some(sealer) => sealer.seal(bytes),
            None => bytes,
        };
        match file.write_all(&bytes) {
            Ok(()) => self.size += bytes.len() as u64,
            Err(_) => {
                tracing::warn!("a recording's picture stopped: couldn't write");
                self.file = None;
            }
        }
    }

    fn finish(&mut self) {
        self.close_cluster();
        if let Some(file) = &self.file {
            let _ = file.sync_all();
        }
    }
}

/// A VP8 keyframe's width and height (RFC 6386 section 9.1). None for
/// anything else.
fn vp8_size(frame: &[u8]) -> Option<(u16, u16)> {
    if frame.len() < 10 || frame[0] & 1 != 0 || frame[3..6] != [0x9D, 0x01, 0x2A] {
        return None;
    }
    let width = u16::from_le_bytes([frame[6], frame[7]]) & 0x3FFF;
    let height = u16::from_le_bytes([frame[8], frame[9]]) & 0x3FFF;
    (width > 0 && height > 0).then_some((width, height))
}

/// The start of a WebM file with one VP8 track: the EBML header, the
/// segment (of unknown size), its info (times in ms) and the track.
fn webm_header(width: u16, height: u16) -> Vec<u8> {
    let mut ebml = uint(0x4286, 1); // EBMLVersion
    ebml.extend(uint(0x42F7, 1)); // EBMLReadVersion
    ebml.extend(uint(0x42F2, 4)); // EBMLMaxIDLength
    ebml.extend(uint(0x42F3, 8)); // EBMLMaxSizeLength
    ebml.extend(element(0x4282, b"webm")); // DocType
    ebml.extend(uint(0x4287, 2)); // DocTypeVersion
    ebml.extend(uint(0x4285, 2)); // DocTypeReadVersion
    let mut out = element(0x1A45_DFA3, &ebml);
    // Segment, its size unknown.
    out.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let mut info = uint(0x2A_D7B1, 1_000_000); // TimestampScale: ms
    info.extend(element(0x4D80, b"fuwa")); // MuxingApp
    info.extend(element(0x5741, b"fuwa")); // WritingApp
    out.extend(element(0x1549_A966, &info));
    let mut video = uint(0xB0, u64::from(width)); // PixelWidth
    video.extend(uint(0xBA, u64::from(height))); // PixelHeight
    let mut track = uint(0xD7, 1); // TrackNumber
    track.extend(uint(0x73C5, 1)); // TrackUID
    track.extend(uint(0x83, 1)); // TrackType: video
    track.extend(uint(0x9C, 0)); // FlagLacing
    track.extend(element(0x86, b"V_VP8")); // CodecID
    track.extend(element(0xE0, &video));
    out.extend(element(0x1654_AE6B, &element(0xAE, &track)));
    out
}

/// An EBML element: its ID (with its length marker, as the spec writes
/// them), its size, then `body`.
fn element(id: u32, body: &[u8]) -> Vec<u8> {
    let id_bytes = id.to_be_bytes();
    let skip = id_bytes.iter().take_while(|b| **b == 0).count().min(3);
    let mut out = id_bytes[skip..].to_vec();
    out.extend(size_vint(body.len() as u64));
    out.extend_from_slice(body);
    out
}

/// An unsigned integer element, in as few bytes as it fits.
fn uint(id: u32, value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    element(id, &bytes[skip..])
}

/// An element's size as EBML writes it: the fewest bytes that hold it, the
/// length marked by the first byte's leading zeros (all ones is reserved).
fn size_vint(size: u64) -> Vec<u8> {
    let len = (1..=8u32).find(|len| size < (1u64 << (7 * len)) - 1).unwrap_or(8);
    let mut out = size.to_be_bytes()[8 - len as usize..].to_vec();
    out[0] |= 0x80 >> (len - 1);
    out
}

// ───────────────────────────── Sealing ─────────────────────────────

/// The key one file is sealed with, from the instance's key: `name` is
/// [`Part::key_name`], so every file has its own.
fn file_key(master: &[u8; 32], recording_id: &str, name: &str) -> LessSafeKey {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"fuwa call recordings");
    let info = [recording_id.as_bytes(), b"/", name.as_bytes()];
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

/// Reads a file as plain Ogg or WebM, handing it on in pieces of about
/// [`PIECE`] until `send` says stop. A sealed file's last chunk, cut short
/// by a crash, is left out.
fn pieces(
    path: &Path,
    key: Option<&[u8; 32]>,
    recording_id: &str,
    key_name: &str,
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
    let key =
        file_key(key.ok_or_else(|| Error::internal("no key to unseal a recording with"))?, recording_id, key_name);
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
fn read_plain(
    path: &Path,
    key: Option<&[u8; 32]>,
    recording_id: &str,
    key_name: &str,
    sealed: bool,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    pieces(path, key, recording_id, key_name, sealed, &mut |piece| {
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
        let path = dir.join(file_name("01J00000000000000000000000", Part::Sound, key.is_some()));
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

    /// A VP8 keyframe's first bytes for a picture `width` by `height`.
    fn keyframe(width: u16, height: u16, rest: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x10, 0x02, 0x00, 0x9D, 0x01, 0x2A];
        frame.extend_from_slice(&width.to_le_bytes());
        frame.extend_from_slice(&height.to_le_bytes());
        frame.extend_from_slice(rest);
        frame
    }

    /// Reads one EBML element at `at`: its ID, its body's range (to the end
    /// for an unknown size).
    fn read_element(bytes: &[u8], at: usize) -> (u32, std::ops::Range<usize>) {
        let id_len = bytes[at].leading_zeros() as usize + 1;
        let id = bytes[at..at + id_len].iter().fold(0u32, |id, b| (id << 8) | u32::from(*b));
        let at = at + id_len;
        let len = bytes[at].leading_zeros() as usize + 1;
        let mut size = u64::from(bytes[at]) & (0xFF >> len);
        for b in &bytes[at + 1..at + len] {
            size = (size << 8) | u64::from(*b);
        }
        let start = at + len;
        let unknown = size == (1u64 << (7 * len)) - 1;
        (id, start..if unknown { bytes.len() } else { start + size as usize })
    }

    #[test]
    fn ebml_sizes_take_the_fewest_bytes() {
        assert_eq!(size_vint(0), vec![0x80]);
        assert_eq!(size_vint(126), vec![0xFE]);
        // 127 is all ones in one byte: reserved for "unknown".
        assert_eq!(size_vint(127), vec![0x40, 0x7F]);
        assert_eq!(size_vint(300), vec![0x41, 0x2C]);
        assert_eq!(uint(0xD7, 1), vec![0xD7, 0x81, 0x01]);
        assert_eq!(uint(0x2A_D7B1, 1_000_000), vec![0x2A, 0xD7, 0xB1, 0x83, 0x0F, 0x42, 0x40]);
        assert_eq!(&element(0x1A45_DFA3, b"")[..], &[0x1A, 0x45, 0xDF, 0xA3, 0x80]);
        assert_eq!(vp8_size(&keyframe(640, 360, &[])), Some((640, 360)));
        assert_eq!(vp8_size(&[0x11, 0, 0, 0x9D, 0x01, 0x2A, 0, 0, 0, 0]), None, "not a keyframe");
    }

    #[test]
    fn pictures_are_webm_lined_up_with_the_recording() {
        let dir = temp();
        let path = dir.join(file_name("01J00000000000000000000000", Part::Camera, false));
        let mut film = Film::create(&path, None, "01J00000000000000000000001", "x").unwrap();
        // Nothing until a keyframe.
        film.picture(100, 9_000, &[0x31, 0, 0], false);
        assert_eq!(film.size, 0);
        // The camera turned on 1.5 s in; 30 frames a second on its clock.
        film.picture(1_500, 90_000, &keyframe(640, 360, &[1]), true);
        for n in 1..90u64 {
            film.picture(1_500 + n * 33 + n % 7, 90_000 + n * 3_000, &[0x31, n as u8], false);
        }
        // It went off and on again: its clock starts over, arrival time wins.
        film.picture(10_000, 5, &keyframe(320, 180, &[2]), true);
        film.finish();
        let webm = std::fs::read(&path).unwrap();
        assert_eq!(film.size, webm.len() as u64);
        let (id, header) = read_element(&webm, 0);
        assert_eq!(id, 0x1A45_DFA3);
        assert!(webm[header.clone()].windows(4).any(|w| w == b"webm"));
        let (id, segment) = read_element(&webm, header.end);
        assert_eq!((id, segment.end), (0x1853_8067, webm.len()), "a segment to the end");
        let mut at = segment.start;
        let mut clusters = Vec::new();
        let mut size = None;
        while at < segment.end {
            let (id, body) = read_element(&webm, at);
            match id {
                0x1654_AE6B => {
                    let (_, entry) = read_element(&webm, body.start);
                    let mut inner = entry.start;
                    while inner < entry.end {
                        let (id, field) = read_element(&webm, inner);
                        if id == 0xE0 {
                            let (_, w) = read_element(&webm, field.start);
                            let (_, h) = read_element(&webm, w.end);
                            let n =
                                |r: std::ops::Range<usize>| webm[r].iter().fold(0u64, |n, b| (n << 8) | u64::from(*b));
                            size = Some((n(w), n(h)));
                        }
                        if id == 0x86 {
                            assert_eq!(&webm[field.clone()], b"V_VP8");
                        }
                        inner = field.end;
                    }
                }
                0x1F43_B675 => {
                    let (id, time) = read_element(&webm, body.start);
                    assert_eq!(id, 0xE7);
                    let time = webm[time.clone()].iter().fold(0u64, |n, b| (n << 8) | u64::from(*b));
                    let mut blocks = Vec::new();
                    let mut inner = read_element(&webm, body.start).1.end;
                    while inner < body.end {
                        let (id, block) = read_element(&webm, inner);
                        assert_eq!((id, webm[block.start]), (0xA3, 0x81));
                        let rel = i16::from_be_bytes([webm[block.start + 1], webm[block.start + 2]]);
                        blocks.push((time + rel as u64, webm[block.start + 3] & 0x80 != 0));
                        inner = block.end;
                    }
                    clusters.push(blocks);
                }
                _ => {}
            }
            at = body.end;
        }
        assert_eq!(size, Some((640, 360)));
        let frames: Vec<(u64, bool)> = clusters.iter().flatten().copied().collect();
        assert_eq!(frames.len(), 91);
        assert_eq!(frames[0], (1_500, true), "starts when it did, on a keyframe");
        // Its own clock: a frame every 33 ms, whatever the arrival jitter.
        assert_eq!(frames[89].0, 1_500 + 89 * 3_000 / 90);
        assert!(frames.windows(2).all(|w| w[0].0 <= w[1].0), "in order");
        assert_eq!(frames[90], (10_000, true));
        // Clusters of about two seconds, a new one at each keyframe.
        assert!(clusters.len() >= 3 && clusters.iter().all(|c| c.last().unwrap().0 - c[0].0 < CLUSTER_MS));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sealed_pictures_read_back_with_their_own_key() {
        let dir = temp();
        let key = [7u8; 32];
        let (rec, user) = ("01J00000000000000000000001", "01J00000000000000000000000");
        let path = dir.join(file_name(user, Part::Screen, true));
        let mut film = Film::create(&path, Some(&key), rec, &Part::Screen.key_name(user)).unwrap();
        film.picture(0, 0, &keyframe(1280, 720, b"secret pixels"), true);
        film.finish();
        let sealed = std::fs::read(&path).unwrap();
        assert!(!sealed.windows(13).any(|w| w == b"secret pixels"));
        let plain = read_plain(&path, Some(&key), rec, &Part::Screen.key_name(user), true).unwrap();
        assert!(plain.windows(13).any(|w| w == b"secret pixels"));
        // Not with the key of the same person's sound, or camera.
        assert!(read_plain(&path, Some(&key), rec, &Part::Sound.key_name(user), true).is_err());
        assert!(read_plain(&path, Some(&key), rec, &Part::Camera.key_name(user), true).is_err());
        assert_eq!(parse_file_name(&format!("{user}.screen.webm.sealed"), true), Some((user, Part::Screen)));
        assert_eq!(parse_file_name(&format!("{user}.opus"), false), Some((user, Part::Sound)));
        assert_eq!(parse_file_name(&format!("{user}.camera.webm"), true), None);
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
