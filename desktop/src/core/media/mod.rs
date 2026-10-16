//! Videos and sounds attached to messages, played where they are, as the
//! web's `<video>` and `<audio>` play them (`chat/Attachments.tsx`): the file
//! is fetched from its instance on the first play and kept in memory while
//! it plays, its sound is decoded and played through the speakers picked in
//! settings (`sound.rs`: AAC, MP3, FLAC, Vorbis, WAV through Symphonia, Opus
//! through libopus), and its picture is decoded a frame at a time and handed
//! to the window when it's due (`video.rs`: MP4 and QuickTime, WebM and
//! Matroska; H.264 through OpenH264, VP8 and VP9 through libvpx).
//!
//! The sound is the clock: the speakers say where in the file they are, and
//! the picture follows. Without sound (none in the file, or no speakers) the
//! picture keeps time by itself.

mod sound;
mod video;

pub(crate) use sound::PROBE;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use http_body_util::BodyExt as _;
use parking_lot::Mutex;
use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;

/// A fetched file's bytes, shared by what plays them.
pub type Fetched = Arc<[u8]>;

/// Why a file can't play in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unplayable {
    /// Nothing in it is something the app plays (AV1 or HEVC video, say).
    Kind,
    /// It didn't read as the kind of file it says it is.
    Broken,
}

/// A frame for the window: BGRA, four bytes a pixel, rows packed.
pub struct Picture {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Where the file is, in milliseconds: from `base` when `since` was, moving
/// on with the time since then; still while `since` is None.
#[derive(Default)]
struct Clock {
    base: f64,
    since: Option<Instant>,
}

impl Clock {
    fn now(&self) -> f64 {
        self.base + self.since.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0)
    }
}

/// What a player shares with the threads that play it.
#[derive(Default)]
struct Shared {
    clock: Mutex<Clock>,
    paused: AtomicBool,
    stop: AtomicBool,
    /// Counts the jumps asked for; each thread follows when it sees a new one.
    jumps: AtomicU64,
    /// Where the last jump went, in milliseconds.
    jump_to: AtomicU64,
    /// How long the file is, in milliseconds; 0 when it doesn't say.
    duration: AtomicU64,
    /// The sound plays and keeps the time: it has some, the speakers opened,
    /// and it hasn't run out.
    sound_leads: AtomicBool,
    sound_done: AtomicBool,
    picture_done: AtomicBool,
    /// It played to the end and stopped there.
    ended: AtomicBool,
    no_speakers: AtomicBool,
    /// An f32's bits, 0 to 1.
    volume: AtomicU32,
    muted: AtomicBool,
    picture: Mutex<Option<Picture>>,
    /// Frames' memory the window is done with, to decode the next ones into.
    spare: Mutex<Vec<Vec<u8>>>,
}

impl Shared {
    fn now(&self) -> f64 {
        self.clock.lock().now()
    }

    /// Lets the picture keep time by itself, from where it is now.
    fn run_clock(&self) {
        let mut clock = self.clock.lock();
        if clock.since.is_none() && !self.paused.load(Ordering::Relaxed) {
            clock.since = Some(Instant::now());
        }
    }

    /// Once both the sound and the picture are through, it stops at the end.
    fn finish_if_through(&self, has_sound: bool, has_picture: bool) {
        let sound = !has_sound || self.sound_done.load(Ordering::Relaxed);
        let picture = !has_picture || self.picture_done.load(Ordering::Relaxed);
        if sound && picture && !self.ended.swap(true, Ordering::Relaxed) {
            self.paused.store(true, Ordering::Relaxed);
            let mut clock = self.clock.lock();
            let end = self.duration.load(Ordering::Relaxed) as f64;
            clock.base = if end > 0.0 { end } else { clock.now() };
            clock.since = None;
        }
    }

    fn gain(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) { 0.0 } else { f32::from_bits(self.volume.load(Ordering::Relaxed)) }
    }
}

/// One video or sound playing in a message, until it's dropped.
pub struct Player {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    has_sound: bool,
    has_picture: bool,
}

impl Player {
    /// Opens a fetched file to play, paused at its start. `name` and
    /// `content_type` are hints for what's in it.
    pub fn open(bytes: Fetched, content_type: &str, name: &str) -> Result<Self, Unplayable> {
        let picture = video::open(&bytes, content_type, name);
        let sound = sound::open(&bytes, content_type, name);
        let picture = match picture {
            Ok(picture) => picture,
            // A sound file's container needn't be one the picture side reads.
            Err(Unplayable::Broken) if sound.is_some() => None,
            Err(why) => return Err(why),
        };
        if picture.is_none() && sound.is_none() {
            return Err(Unplayable::Kind);
        }
        let shared = Arc::new(Shared {
            paused: AtomicBool::new(true),
            volume: AtomicU32::new(1.0f32.to_bits()),
            ..Shared::default()
        });
        let duration =
            picture.as_ref().map_or(0, |p| p.duration_ms()).max(sound.as_ref().map_or(0, |s| s.duration_ms()));
        shared.duration.store(duration, Ordering::Relaxed);
        let (has_sound, has_picture) = (sound.is_some(), picture.is_some());
        shared.sound_leads.store(has_sound, Ordering::Relaxed);
        let mut threads = Vec::new();
        if let Some(sound) = sound {
            let shared = shared.clone();
            let thread = std::thread::Builder::new()
                .name("fuwa-media-sound".into())
                .spawn(move || sound::run(sound, shared, has_picture));
            threads.extend(thread.ok());
        }
        if let Some(picture) = picture {
            let shared = shared.clone();
            let thread = std::thread::Builder::new()
                .name("fuwa-media-picture".into())
                .spawn(move || video::run(picture, shared, has_sound));
            threads.extend(thread.ok());
        }
        Ok(Self { shared, threads, has_sound, has_picture })
    }

    pub fn has_picture(&self) -> bool {
        self.has_picture
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// Plays from where it is, or from the start once it has ended.
    pub fn play(&self) {
        if self.shared.ended.load(Ordering::Relaxed) {
            self.seek_ms(0);
        }
        self.shared.paused.store(false, Ordering::Relaxed);
        if !self.shared.sound_leads.load(Ordering::Relaxed) {
            self.shared.run_clock();
        }
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
        let mut clock = self.shared.clock.lock();
        clock.base = clock.now();
        clock.since = None;
    }

    /// Jumps to `fraction` (0 to 1) of the way through; nothing when its length isn't known.
    pub fn seek(&self, fraction: f32) {
        let duration = self.shared.duration.load(Ordering::Relaxed);
        if duration > 0 {
            self.seek_ms((f64::from(fraction.clamp(0.0, 1.0)) * duration as f64) as u64);
        }
    }

    fn seek_ms(&self, ms: u64) {
        let shared = &self.shared;
        shared.ended.store(false, Ordering::Relaxed);
        shared.sound_done.store(false, Ordering::Relaxed);
        shared.picture_done.store(false, Ordering::Relaxed);
        let leads = self.has_sound && !shared.no_speakers.load(Ordering::Relaxed);
        shared.sound_leads.store(leads, Ordering::Relaxed);
        {
            let mut clock = shared.clock.lock();
            clock.base = ms as f64;
            // The sound starts the clock again once it's playing from there.
            clock.since = (!leads && !shared.paused.load(Ordering::Relaxed)).then(Instant::now);
        }
        shared.jump_to.store(ms, Ordering::Relaxed);
        shared.jumps.fetch_add(1, Ordering::Relaxed);
    }

    /// Where it is and how long it is, in milliseconds (0 when it doesn't say).
    pub fn position_ms(&self) -> (u64, u64) {
        let duration = self.shared.duration.load(Ordering::Relaxed);
        let at = self.shared.now().max(0.0) as u64;
        (if duration > 0 { at.min(duration) } else { at }, duration)
    }

    /// It played to the end.
    pub fn has_ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    /// It has only sound, and there were no speakers to play it on.
    pub fn had_no_speakers(&self) -> bool {
        self.shared.no_speakers.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    /// The newest frame, if there's one the window hasn't taken.
    pub fn take_picture(&self) -> Option<Picture> {
        self.shared.picture.lock().take()
    }

    /// Hands a frame's memory back, to decode a later one into.
    pub fn give_back(&self, bgra: Vec<u8>) {
        let mut spare = self.shared.spare.lock();
        if spare.len() < 3 {
            spare.push(bgra);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// How long a fetch may wait for the instance's next bytes before it gives up.
const STALL: std::time::Duration = std::time::Duration::from_secs(60);

impl Core {
    /// Fetches an attached file from its instance to play it, counting what
    /// has come in `got`. `size` is the file's size as its message gives it:
    /// a file that runs past it (or past `MOST_READ`) stops.
    pub async fn fetch_media(&self, key: &str, url: &str, size: i64, got: Arc<AtomicU64>) -> Result<Fetched, Problem> {
        let unreachable = || Problem::new(Code::Unavailable, "Couldn't reach this instance right now.");
        let too_long = || Problem::new(Code::DeadlineExceeded, "That took too long.");
        let mut body = self.request_attachment(key, url).await?;
        let most = crate::core::attachments::most_saved(size);
        let mut bytes: Vec<u8> =
            Vec::with_capacity(usize::try_from(most).unwrap_or(0).min(usize::try_from(size.max(0)).unwrap_or(0)));
        loop {
            let Some(frame) = tokio::time::timeout(STALL, body.frame()).await.map_err(|_| too_long())? else {
                break;
            };
            let frame = frame.map_err(|_| unreachable())?;
            if let Some(data) = frame.data_ref() {
                if (bytes.len() + data.len()) as u64 > most {
                    return Err(Problem::new(Code::OutOfRange, "That file is bigger than its message said."));
                }
                bytes.extend_from_slice(data);
                got.store(bytes.len() as u64, Ordering::Relaxed);
            }
        }
        crate::core::reports::used("attachment.play");
        Ok(bytes.into())
    }
}

#[cfg(test)]
mod tests;
