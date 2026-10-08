//! Voice messages in private conversations, as the web app's `voice/` and
//! `fuwa/dms.ts`: recorded from the microphone as Opus in an Ogg file (48 kHz
//! mono, 32 kbps), sealed on this device under a key of its own, uploaded as
//! ciphertext, and sent as a `DirectMessageVoice` carrying the key, length
//! and waveform inside the encrypted message. Playing one fetches the sealed
//! bytes, checks and opens them, and plays the sound through the speakers.
//!
//! Files are byte-for-byte the web's: the same padding, sealing and Ogg
//! layout, so either app plays what the other recorded.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use sha2::{Digest as _, Sha256};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::dms::{Content, DmError};
use crate::core::reports;
use crate::core::vault::VoiceFile;
use crate::core::voice::devices::{Devices, Trouble};
use crate::core::voice::sound::{FRAME, Pipe, RATE};
use crate::pb;
use crate::rpc;

// ───────────────────────── Sealing ─────────────────────────

/// Sealed files' plaintext comes in steps of this many bytes, so the stored
/// size tells the instance only roughly how long a voice message is.
pub const PAD_STEP: usize = 32 * 1024;
/// The fewest sealed bytes there can be: a nonce and a tag.
const SEALED_MIN: usize = 28;
/// The most a voice message is fetched to.
pub const MOST_FETCHED: usize = 8 * 1024 * 1024;
/// What the opened file is.
pub const CONTENT_TYPE: &str = "audio/ogg; codecs=opus";
/// What a voice message sent in a server's channel is called (the web's `VOICE_FILENAME`).
pub const VOICE_FILENAME: &str = "voice-message.ogg";

/// A file sealed for an encrypted message: the bytes for the instance, and
/// the key and digest that go inside the message.
pub struct Sealed {
    pub bytes: Vec<u8>,
    pub key: [u8; 32],
    pub sha256: [u8; 32],
}

fn pad(plain: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; (plain.len() + 1).div_ceil(PAD_STEP) * PAD_STEP];
    out[..plain.len()].copy_from_slice(plain);
    out[plain.len()] = 0x80;
    out
}

fn unpad(mut padded: Vec<u8>) -> Option<Vec<u8>> {
    let end = padded.iter().rposition(|&b| b != 0)?;
    (padded[end] == 0x80).then(|| {
        padded.truncate(end);
        padded
    })
}

/// AES-256-GCM under a fresh random key, kept as the 12-byte nonce then the
/// ciphertext and tag, after padding (one 0x80 byte, then zeros).
pub fn seal(plain: &[u8]) -> Option<Sealed> {
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut key).ok()?;
    getrandom::fill(&mut nonce).ok()?;
    let sealing = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).ok()?);
    let mut body = pad(plain);
    sealing.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut body).ok()?;
    let mut bytes = Vec::with_capacity(12 + body.len());
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&body);
    let sha256 = Sha256::digest(&bytes).into();
    Some(Sealed { bytes, key, sha256 })
}

/// Opens sealed bytes after checking they're the ones the message named.
pub fn open(bytes: &[u8], key: &[u8], sha256: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 32 || bytes.len() < SEALED_MIN || Sha256::digest(bytes).as_slice() != sha256 {
        return None;
    }
    let opening = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let nonce = Nonce::try_assume_unique_for_key(&bytes[..12]).ok()?;
    let mut body = bytes[12..].to_vec();
    let plain = opening.open_in_place(nonce, Aad::empty(), &mut body).ok()?.len();
    body.truncate(plain);
    unpad(body)
}

// ───────────────────────── Ogg Opus ─────────────────────────

static CRC: LazyLock<[u32; 256]> = LazyLock::new(|| {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut r = (i as u32) << 24;
        for _ in 0..8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04c1_1db7 } else { r << 1 };
        }
        *slot = r;
    }
    table
});

fn crc(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |c, &b| (c << 8) ^ CRC[(((c >> 24) as u8) ^ b) as usize])
}

fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(1);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

/// Who wrote it, and nothing about the person.
fn opus_tags() -> Vec<u8> {
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&4u32.to_le_bytes());
    tags.extend_from_slice(b"fuwa");
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

fn page(packets: &[&[u8]], granule: u64, serial: u32, sequence: u32, flags: u8) -> Vec<u8> {
    let mut lacing = Vec::new();
    for p in packets {
        lacing.extend(std::iter::repeat_n(255u8, p.len() / 255));
        lacing.push((p.len() % 255) as u8);
    }
    let mut out = b"OggS".to_vec();
    out.push(0);
    out.push(flags);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(lacing.len() as u8);
    out.extend_from_slice(&lacing);
    for p in packets {
        out.extend_from_slice(p);
    }
    let sum = crc(&out);
    out[22..26].copy_from_slice(&sum.to_le_bytes());
    out
}

/// An Ogg Opus file from 20 ms mono Opus packets. `pre_skip` is what the
/// decoder drops from the start.
pub fn write_ogg(packets: &[Vec<u8>], pre_skip: u16) -> Vec<u8> {
    let mut serial = [0u8; 4];
    let _ = getrandom::fill(&mut serial);
    let serial = u32::from_le_bytes(serial);
    let mut file = page(&[&opus_head(pre_skip)], 0, serial, 0, 0x02);
    file.extend(page(&[&opus_tags()], 0, serial, 1, 0));
    let mut sequence = 2;
    let mut granule = u64::from(pre_skip);
    let mut batch: Vec<&[u8]> = Vec::new();
    let mut count = 0;
    for (n, p) in packets.iter().enumerate() {
        let segments = p.len() / 255 + 1;
        // About a second of sound a page, and never more than a page can name.
        if !batch.is_empty() && (count + segments > 255 || batch.len() >= 50) {
            file.extend(page(&batch, granule, serial, sequence, 0));
            sequence += 1;
            batch.clear();
            count = 0;
        }
        batch.push(p);
        count += segments;
        granule += FRAME as u64;
        if n == packets.len() - 1 {
            file.extend(page(&batch, granule, serial, sequence, 0x04));
        }
    }
    if packets.is_empty() {
        file.extend(page(&[], granule, serial, sequence, 0x04));
    }
    file
}

/// What an Ogg Opus file holds.
pub struct OggOpus {
    pub channels: u8,
    pub pre_skip: usize,
    /// Its length in 48 kHz samples.
    pub samples: u64,
    pub packets: Vec<Vec<u8>>,
}

/// Reads an Ogg Opus file's packets (one stream), checking each page's CRC.
pub fn read_ogg(file: &[u8]) -> Option<OggOpus> {
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    let mut at = 0;
    let mut last_granule = 0i64;
    while at < file.len() {
        if at + 27 > file.len() || &file[at..at + 4] != b"OggS" {
            return None;
        }
        let count = usize::from(file[at + 26]);
        let lacing = file.get(at + 27..at + 27 + count)?;
        let body_len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
        let end = at + 27 + count + body_len;
        let page = file.get(at..end)?;
        let mut copy = page.to_vec();
        copy[22..26].fill(0);
        if crc(&copy) != u32::from_le_bytes(page[22..26].try_into().ok()?) {
            return None;
        }
        let granule = i64::from_le_bytes(page[6..14].try_into().ok()?);
        if granule > 0 {
            last_granule = granule;
        }
        let mut body = at + 27 + count;
        for &l in lacing {
            partial.extend_from_slice(&file[body..body + usize::from(l)]);
            body += usize::from(l);
            if l < 255 {
                packets.push(std::mem::take(&mut partial));
            }
        }
        at = end;
    }
    let head = packets.first().filter(|h| h.len() >= 19 && &h[..8] == b"OpusHead")?;
    let pre_skip = usize::from(u16::from_le_bytes([head[10], head[11]]));
    Some(OggOpus {
        channels: head[9],
        pre_skip,
        samples: (last_granule.max(0) as u64).saturating_sub(pre_skip as u64),
        packets: packets.split_off(2.min(packets.len())),
    })
}

/// Longest a voice message is decoded to: past this it plays cut short.
const MOST_DECODED_SECS: u64 = 15 * 60;

/// The sound in an Ogg Opus file, as 48 kHz mono samples.
pub fn decode(file: &[u8]) -> Option<Vec<f32>> {
    let ogg = read_ogg(file)?;
    let channels = match ogg.channels {
        1 => opus::Channels::Mono,
        2 => opus::Channels::Stereo,
        _ => return None,
    };
    let per = usize::from(ogg.channels);
    let mut decoder = opus::Decoder::new(RATE, channels).ok()?;
    let most = (MOST_DECODED_SECS * u64::from(RATE)) as usize;
    let mut out = Vec::with_capacity((ogg.samples as usize).min(most));
    let mut buf = vec![0f32; 5760 * per];
    for p in &ogg.packets {
        let Ok(n) = decoder.decode_float(p, &mut buf, false) else { continue };
        out.extend(buf[..n * per].chunks(per).map(|c| c.iter().sum::<f32>() / per as f32));
        if out.len() > most + ogg.pre_skip {
            break;
        }
    }
    let end = if ogg.samples > 0 { (ogg.pre_skip + ogg.samples as usize).min(out.len()) } else { out.len() };
    Some(out.get(ogg.pre_skip.min(end)..end)?.iter().take(most).copied().collect())
}

// ───────────────────────── Waveforms ─────────────────────────

/// How many values a waveform carries.
pub const BARS: usize = 64;
/// The most a received one is read to.
pub const MAX_BARS: usize = 128;

/// Peak levels (0 to 1), one per 20 ms of sound, folded into `bars` bytes
/// (0 silence, 255 loudest). Square-rooted: quiet speech still shows.
pub fn waveform(levels: &[f32], bars: usize) -> Vec<u8> {
    let n = bars.min(levels.len().max(1));
    if levels.is_empty() {
        return vec![0; n];
    }
    let loudest = levels.iter().copied().fold(0.02f32, f32::max);
    (0..n)
        .map(|i| {
            let from = i * levels.len() / n;
            let to = ((i + 1) * levels.len() / n).max(from + 1);
            let peak = levels[from..to].iter().copied().fold(0f32, f32::max);
            ((peak / loudest).min(1.0).sqrt() * 255.0).round() as u8
        })
        .collect()
}

/// A received waveform as `bars` heights from 0 to 1, stretched or squeezed to fit.
pub fn heights(data: &[u8], bars: usize) -> Vec<f32> {
    let src: &[u8] = if data.is_empty() { &[0] } else { &data[..data.len().min(MAX_BARS)] };
    (0..bars)
        .map(|i| {
            let from = i * src.len() / bars;
            let to = ((i + 1) * src.len() / bars).max(from + 1).min(src.len());
            f32::from(
                src[from.min(src.len() - 1)..to.max(from.min(src.len() - 1) + 1)].iter().copied().max().unwrap_or(0),
            ) / 255.0
        })
        .collect()
}

/// "1:05".
pub fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

// ───────────────────────── Recording ─────────────────────────

/// Recordings shorter than this aren't sent.
pub const MIN_MS: u64 = 500;
/// Opus at this many bits a second, as the web records.
const BITRATE: i32 = 32_000;

/// A finished recording, ready to send.
#[derive(Clone)]
pub struct Clip {
    pub ogg: Arc<Vec<u8>>,
    pub duration_ms: u32,
    pub waveform: Vec<u8>,
}

/// How a recording is going, for the recording bar.
#[derive(Default, Clone)]
pub struct Progress {
    pub elapsed_ms: u64,
    /// One peak level a 20 ms frame, the newest last (the most recent few seconds).
    pub levels: VecDeque<f32>,
    /// It reached the instance's longest voice message and stopped.
    pub limited: bool,
    /// The microphone couldn't be opened.
    pub no_microphone: bool,
    /// Because the system won't let the app use it (macOS's privacy settings).
    pub microphone_blocked: bool,
}

#[derive(Default)]
struct Taken {
    packets: Vec<Vec<u8>>,
    levels: Vec<f32>,
    pre_skip: u16,
    progress: Progress,
}

/// The microphone, recording until stopped (or until `max_ms`).
pub struct Recorder {
    taken: Arc<Mutex<Taken>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Levels kept for the live waveform.
const LIVE: usize = 256;

impl Recorder {
    /// Starts recording; `max_ms` 0 for no cap.
    pub fn start(max_ms: u64) -> Self {
        let taken = Arc::new(Mutex::new(Taken::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (taken, stop) = (taken.clone(), stop.clone());
            std::thread::Builder::new().name("fuwa-voice-note".into()).spawn(move || record(taken, stop, max_ms)).ok()
        };
        if thread.is_none() {
            taken.lock().progress.no_microphone = true;
        }
        Self { taken, stop, thread }
    }

    pub fn progress(&self) -> Progress {
        self.taken.lock().progress.clone()
    }

    /// Stops and gives the recording, unless it's too short to send.
    pub fn finish(mut self) -> Option<Clip> {
        self.halt();
        let taken = std::mem::take(&mut *self.taken.lock());
        let duration_ms = taken.packets.len() as u64 * 20;
        (duration_ms >= MIN_MS).then(|| Clip {
            ogg: Arc::new(write_ogg(&taken.packets, taken.pre_skip)),
            duration_ms: duration_ms as u32,
            waveform: waveform(&taken.levels, BARS),
        })
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.halt();
    }
}

fn record(taken: Arc<Mutex<Taken>>, stop: Arc<AtomicBool>, max_ms: u64) {
    let encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).and_then(|mut e| {
        e.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
        Ok(e)
    });
    let Ok(mut encoder) = encoder else {
        taken.lock().progress.no_microphone = true;
        return;
    };
    taken.lock().pre_skip = encoder.get_lookahead().map(|n| n.clamp(0, i32::from(u16::MAX)) as u16).unwrap_or(312);
    let microphone = Arc::new(Pipe::new(FRAME * 50, 0));
    let devices = Devices::open(microphone.clone(), Arc::new(Pipe::speakers()), true);
    let most_frames = if max_ms > 0 { max_ms.div_ceil(20) as usize } else { usize::MAX };
    let started = Instant::now();
    let mut packet = vec![0u8; 4000];
    while !stop.load(Ordering::Relaxed) {
        while let Some(frame) = microphone.frame() {
            let level = frame.iter().fold(0f32, |m, s| m.max(s.abs())).min(1.0);
            let Ok(n) = encoder.encode_float(&frame, &mut packet) else { continue };
            let mut t = taken.lock();
            t.packets.push(packet[..n].to_vec());
            t.levels.push(level);
            t.progress.elapsed_ms = t.packets.len() as u64 * 20;
            t.progress.levels.push_back(level);
            if t.progress.levels.len() > LIVE {
                t.progress.levels.pop_front();
            }
            if t.packets.len() >= most_frames {
                t.progress.limited = true;
                return;
            }
        }
        // The microphone gets a moment to open before it's called missing.
        let trouble = devices.trouble();
        if trouble.contains(&Trouble::MicrophoneBlocked) {
            let progress = &mut taken.lock().progress;
            (progress.no_microphone, progress.microphone_blocked) = (true, true);
            return;
        }
        if started.elapsed() > Duration::from_millis(400) && trouble.contains(&Trouble::NoMicrophone) {
            taken.lock().progress.no_microphone = true;
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ───────────────────────── Playing ─────────────────────────

/// One voice message playing through the speakers, until it ends or this is dropped.
pub struct Player {
    total: u64,
    /// Where it is, in samples.
    at: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    /// The speakers couldn't be opened, so it stopped.
    no_speakers: Arc<AtomicBool>,
    seek: Arc<Mutex<Option<u64>>>,
    stop: Arc<AtomicBool>,
    /// How fast it plays (an f32's bits): 1, 1.5 or 2, the pitch kept.
    rate: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn play(sound: Arc<Vec<f32>>, from_ms: u64, rate: f32) -> Self {
        let total = sound.len() as u64;
        let at = Arc::new(AtomicU64::new((from_ms * u64::from(RATE) / 1000).min(total)));
        let (paused, done, stop) = (Arc::default(), Arc::<AtomicBool>::default(), Arc::<AtomicBool>::default());
        let seek = Arc::new(Mutex::new(None));
        let no_speakers = Arc::new(AtomicBool::new(false));
        let rate = Arc::new(AtomicU32::new(rate.to_bits()));
        let thread = {
            let state = Playback {
                at: at.clone(),
                paused: Arc::clone(&paused),
                done: done.clone(),
                no_speakers: no_speakers.clone(),
                seek: seek.clone(),
                stop: stop.clone(),
                rate: rate.clone(),
            };
            std::thread::Builder::new().name("fuwa-voice-play".into()).spawn(move || play(sound, state)).ok()
        };
        if thread.is_none() {
            no_speakers.store(true, Ordering::Relaxed);
            done.store(true, Ordering::Relaxed);
        }
        Self { total, at, paused, done, no_speakers, seek, stop, rate, thread }
    }

    /// Plays faster or slower from here on, at the same pitch.
    pub fn set_rate(&self, rate: f32) {
        self.rate.store(rate.to_bits(), Ordering::Relaxed);
    }

    /// Where it is and how long it is, in milliseconds.
    pub fn position_ms(&self) -> (u64, u64) {
        let ms = |s: u64| s * 1000 / u64::from(RATE);
        (ms(self.at.load(Ordering::Relaxed).min(self.total)), ms(self.total))
    }

    /// Where it is, from 0 to 1, for drawing.
    pub fn fraction(&self) -> Arc<dyn Fn() -> f32 + Send + Sync> {
        let (at, total) = (self.at.clone(), self.total.max(1));
        Arc::new(move || (at.load(Ordering::Relaxed).min(total) as f64 / total as f64) as f32)
    }

    pub fn pause(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn seek(&self, fraction: f32) {
        let to = (f64::from(fraction.clamp(0.0, 1.0)) * self.total as f64) as u64;
        self.at.store(to, Ordering::Relaxed);
        *self.seek.lock() = Some(to);
    }

    /// It played to the end (or couldn't play at all).
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    /// There were no speakers to play it on.
    pub fn had_no_speakers(&self) -> bool {
        self.no_speakers.load(Ordering::Relaxed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the playing thread shares with its [`Player`].
struct Playback {
    at: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    no_speakers: Arc<AtomicBool>,
    seek: Arc<Mutex<Option<u64>>>,
    stop: Arc<AtomicBool>,
    rate: Arc<AtomicU32>,
}

/// Plays a sound faster (or slower) at the same pitch, a frame at a time:
/// overlapping windows taken from the sound `rate` times as far apart as
/// they're laid down, each nudged to where it best lines up with what came
/// before (WSOLA), so speech stays clear.
struct Stretch {
    /// The second half of the last window, waiting for the next to overlap it.
    tail: Vec<f32>,
    /// Where the last window was taken from in the sound.
    last: Option<usize>,
}

/// Each window is two frames long; one frame comes out per window.
const WINDOW: usize = FRAME * 2;
/// How far a window may move to line up (10 ms each way), and in what steps.
const SEARCH: usize = 480;
const SEARCH_STEP: usize = 8;

impl Stretch {
    fn new() -> Self {
        Self { tail: vec![0.0; FRAME], last: None }
    }

    /// The next frame, from around `at` in the sound; None past its end.
    fn next(&mut self, sound: &[f32], at: usize) -> Option<Vec<f32>> {
        if at >= sound.len() {
            return None;
        }
        // Where the last window would naturally go on: line up with it.
        let pos = match self.last {
            Some(last) => {
                let natural = last + FRAME;
                let from = at.saturating_sub(SEARCH);
                let to = (at + SEARCH).min(sound.len().saturating_sub(FRAME));
                let mut best = (f32::MIN, at.min(to));
                let mut d = from;
                while d <= to {
                    let mut score = 0.0f32;
                    for k in (0..FRAME).step_by(4) {
                        score +=
                            sound.get(d + k).copied().unwrap_or(0.0) * sound.get(natural + k).copied().unwrap_or(0.0);
                    }
                    if score > best.0 {
                        best = (score, d);
                    }
                    d += SEARCH_STEP;
                }
                best.1
            }
            None => at,
        };
        self.last = Some(pos);
        let hann = |i: usize| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos();
        let sample = |i: usize| sound.get(pos + i).copied().unwrap_or(0.0) * hann(i);
        let out: Vec<f32> = (0..FRAME).map(|i| self.tail[i] + sample(i)).collect();
        self.tail = (FRAME..WINDOW).map(sample).collect();
        Some(out)
    }
}

fn play(sound: Arc<Vec<f32>>, state: Playback) {
    let Playback { at, paused, done, no_speakers, seek, stop, rate } = state;
    let mut stretch: Option<Stretch> = None;
    // Where each frame handed to the speakers came from, to say where it's at.
    let mut handed: VecDeque<usize> = VecDeque::new();
    let speakers = Arc::new(Pipe::new(FRAME * 10, FRAME * 2));
    let devices = Devices::open(Arc::new(Pipe::microphone()), speakers.clone(), false);
    let started = Instant::now();
    let total = sound.len();
    let mut fed = at.load(Ordering::Relaxed) as usize;
    let mut was_paused = false;
    let (mut last_left, mut still_since) = (usize::MAX, Instant::now());
    while !stop.load(Ordering::Relaxed) {
        if let Some(to) = seek.lock().take() {
            speakers.clear();
            handed.clear();
            stretch = None;
            fed = to as usize;
        }
        let pausing = paused.load(Ordering::Relaxed);
        if pausing {
            if !was_paused {
                // What was waiting would play on: it waits for the next play instead.
                fed = handed.front().copied().unwrap_or(fed);
                speakers.clear();
                handed.clear();
                stretch = None;
            }
            was_paused = true;
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        was_paused = false;
        let speed = f32::from_bits(rate.load(Ordering::Relaxed)).clamp(0.5, 3.0);
        while speakers.len() < FRAME * 6 && fed < total {
            handed.push_back(fed);
            if (speed - 1.0).abs() < 0.01 {
                stretch = None;
                let end = (fed + FRAME).min(total);
                speakers.push(&sound[fed..end]);
                fed = end;
            } else {
                let frame = stretch.get_or_insert_with(Stretch::new).next(&sound, fed);
                if let Some(frame) = frame {
                    speakers.push(&frame);
                }
                fed = (fed + (FRAME as f32 * speed) as usize).min(total);
            }
        }
        // What's still waiting in the speakers hasn't played: it's at the oldest of those.
        let waiting = speakers.len().div_ceil(FRAME);
        while handed.len() > waiting {
            handed.pop_front();
        }
        at.store(handed.front().copied().unwrap_or(fed) as u64, Ordering::Relaxed);
        // The speakers get a moment to open before they're called missing.
        if started.elapsed() > Duration::from_millis(400) && devices.trouble().contains(&Trouble::NoSpeakers) {
            no_speakers.store(true, Ordering::Relaxed);
            done.store(true, Ordering::Relaxed);
            return;
        }
        // Everything's handed over: it ends when the speakers take the rest, or
        // when they stop taking any (a device that went away mid-way).
        let left = speakers.len();
        if fed >= total && left != last_left {
            last_left = left;
            still_since = Instant::now();
        }
        if fed >= total && (left == 0 || still_since.elapsed() > Duration::from_secs(1)) {
            at.store(total as u64, Ordering::Relaxed);
            done.store(true, Ordering::Relaxed);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ───────────────────────── Sending and fetching ─────────────────────────

/// An instance's voice message caps, kept a while: 0 for none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub max_ms: u64,
    pub max_bytes: i64,
}

static LIMITS: LazyLock<Mutex<HashMap<String, (Instant, Limits)>>> = LazyLock::new(Default::default);
/// Voice messages opened on this run (and this device's own recordings), by media id.
static OPENED: LazyLock<Mutex<VecDeque<Opened>>> = LazyLock::new(Default::default);
/// An opened voice message's Ogg file, by media id.
type Opened = (String, Arc<Vec<u8>>);
/// How many opened voice messages are kept.
const KEPT: usize = 24;

fn keep_opened(media_id: &str, ogg: Arc<Vec<u8>>) {
    let mut kept = OPENED.lock();
    kept.retain(|(id, _)| id != media_id);
    kept.push_back((media_id.to_owned(), ogg));
    while kept.len() > KEPT {
        kept.pop_front();
    }
}

/// Forgets every opened voice message (signing out does).
pub fn forget_opened() {
    OPENED.lock().clear();
}

fn opened(media_id: &str) -> Option<Arc<Vec<u8>>> {
    OPENED.lock().iter().find(|(id, _)| id == media_id).map(|(_, ogg)| ogg.clone())
}

impl Core {
    /// The instance's longest voice message and biggest sealed one, asked at
    /// most every ten minutes. Unknown counts as no cap: the instance still
    /// refuses an upload that's too big.
    pub async fn voice_limits(&self, key: &str) -> Limits {
        if let Some((at, limits)) = LIMITS.lock().get(key)
            && at.elapsed() < Duration::from_secs(600)
        {
            return *limits;
        }
        let Some(api) = self.api(key) else { return Limits::default() };
        let Ok(res) = rpc!(api.dms(), get_voice_limits(pb::GetVoiceLimitsRequest {})).await else {
            return Limits::default();
        };
        let limits = Limits {
            max_ms: res.max_seconds.unwrap_or(0).max(0) as u64 * 1000,
            max_bytes: res.max_bytes.unwrap_or(0).max(0),
        };
        LIMITS.lock().insert(key.to_owned(), (Instant::now(), limits));
        limits
    }

    /// Sends a recording: sealed on this device under a key of its own, the
    /// sealed bytes uploaded, and the key, waveform and length sent inside
    /// an encrypted message.
    pub async fn send_voice(&self, key: &str, id: &str, clip: &Clip, reply_to: i64) -> Result<(), DmError> {
        let api = self.api(key).ok_or_else(|| DmError("That instance isn't here.".into()))?;
        let engine = self.dm_engine(key).ok_or_else(|| DmError("Encrypted messages aren't ready yet.".into()))?;
        let sealed = seal(&clip.ogg).ok_or_else(|| DmError("The recording couldn't be encrypted.".into()))?;
        let limits = self.voice_limits(key).await;
        if limits.max_bytes > 0 && sealed.bytes.len() as i64 > limits.max_bytes {
            return Err(DmError("That voice message is too big to send here.".into()));
        }
        let req = pb::CreateSealedUploadRequest {
            conversation_id: id.into(),
            size: sealed.bytes.len() as i64,
            kind: pb::SealedKind::Voice as i32,
        };
        let reserved = rpc!(api.dms(), create_sealed_upload(req)).await?;
        // The bytes go to the instance's own address, whatever name it gave the link.
        let token = reserved.upload_url.rsplit('/').next().unwrap_or_default();
        let target = format!("{}/media/upload/{token}", api.url.trim_end_matches('/'));
        crate::core::account::send(http::Method::PUT, &target, "application/octet-stream", sealed.bytes.clone())
            .await?;
        // This device already has the sound: no need to fetch it back to play it.
        keep_opened(&reserved.media_id, clip.ogg.clone());
        let voice = pb::DirectMessageVoice {
            file: Some(pb::SealedFile {
                media_id: reserved.media_id,
                url: String::new(),
                key: sealed.key.to_vec(),
                sha256: sealed.sha256.to_vec(),
                size: sealed.bytes.len() as i64,
                content_type: CONTENT_TYPE.into(),
                ..Default::default()
            }),
            duration_ms: clip.duration_ms,
            waveform: clip.waveform.clone(),
            reply_to_sequence: reply_to,
        };
        engine.send(id, Content::Voice(voice)).await?;
        reports::used("dm.voice");
        Ok(())
    }

    /// Uploads a recording for a message in a server's channel, as the web's
    /// `uploadVoice`: a plain Ogg Opus attachment (channels aren't end-to-end
    /// encrypted) carrying its length and waveform, to be the message's only file.
    pub async fn upload_voice(&self, key: &str, server_id: &str, clip: &Clip) -> Result<pb::Attachment, Problem> {
        reports::used("message.send_voice");
        let mut file = self.upload_bytes(key, server_id, VOICE_FILENAME.into(), "audio/ogg", clip.ogg.to_vec()).await?;
        // This device already has the sound: no need to fetch it back to play it.
        let id = if file.id.is_empty() {
            file.url.rsplit('/').next().unwrap_or_default().to_owned()
        } else {
            file.id.clone()
        };
        keep_opened(&id, clip.ogg.clone());
        file.voice = Some(pb::VoiceNote { duration_ms: clip.duration_ms, waveform: clip.waveform.clone() });
        Ok(file)
    }

    /// A voice message's sound, ready to play: fetched from the instance,
    /// checked against its digest and opened (once a run). One sent in a
    /// server's channel (no key) is a plain file, played as it comes.
    pub async fn voice_sound(&self, key: &str, file: &VoiceFile) -> Result<Arc<Vec<f32>>, Problem> {
        let unopenable = || Problem::new(tonic::Code::DataLoss, "That voice message can't be played.");
        let ogg = match opened(&file.media_id) {
            Some(ogg) => ogg,
            None => {
                let api =
                    self.api(key).ok_or_else(|| Problem::new(tonic::Code::Unavailable, "That instance isn't here."))?;
                if file.size as usize > MOST_FETCHED {
                    return Err(unopenable());
                }
                let url = format!("{}/media/{}", api.url.trim_end_matches('/'), file.media_id);
                let bytes = crate::core::account::fetch(&url, MOST_FETCHED).await?;
                if file.key.is_empty() {
                    let ogg = Arc::new(bytes);
                    keep_opened(&file.media_id, ogg.clone());
                    let sound = tokio::task::spawn_blocking(move || decode(&ogg))
                        .await
                        .ok()
                        .flatten()
                        .ok_or_else(unopenable)?;
                    return Ok(Arc::new(sound));
                }
                if bytes.len() as i64 != file.size {
                    return Err(unopenable());
                }
                let ogg = Arc::new(open(&bytes, &file.key, &file.sha256).ok_or_else(unopenable)?);
                keep_opened(&file.media_id, ogg.clone());
                ogg
            }
        };
        let sound = tokio::task::spawn_blocking(move || decode(&ogg)).await.ok().flatten().ok_or_else(unopenable)?;
        Ok(Arc::new(sound))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_files_open_to_what_went_in_padded_to_steps() {
        for length in [0, 1, PAD_STEP - 1, PAD_STEP, 100_000] {
            let plain: Vec<u8> = (0..length).map(|i| ((i * 31) & 0xff) as u8).collect();
            let sealed = seal(&plain).unwrap();
            assert_eq!((sealed.bytes.len() - 28) % PAD_STEP, 0);
            assert!(sealed.bytes.len() - 28 > length);
            assert_eq!(open(&sealed.bytes, &sealed.key, &sealed.sha256).unwrap(), plain);
        }
    }

    #[test]
    fn a_changed_sealed_file_does_not_open() {
        let sealed = seal(&[1, 2, 3]).unwrap();
        let mut changed = sealed.bytes.clone();
        changed[40] ^= 1;
        assert!(open(&changed, &sealed.key, &sealed.sha256).is_none());
        // Even with a digest that matches the change, the tag doesn't.
        let digest: [u8; 32] = Sha256::digest(&changed).into();
        assert!(open(&changed, &sealed.key, &digest).is_none());
    }

    /// The web's `seal.ts` output, made with a fixed key and nonce: the
    /// desktop opens it, so the two seal the same way.
    #[test]
    fn opens_what_the_web_seals() {
        let key = [7u8; 32];
        let nonce = [9u8; 12];
        let sealing = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap());
        let mut body = pad(b"OggS");
        assert_eq!((body.len(), body[4], body[5]), (PAD_STEP, 0x80, 0));
        sealing.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut body).unwrap();
        let bytes = [nonce.as_slice(), &body].concat();
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        assert_eq!(open(&bytes, &key, &digest).unwrap(), b"OggS");
    }

    #[test]
    fn ogg_files_read_back_and_play() {
        let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let pre_skip = encoder.get_lookahead().unwrap() as u16;
        let mut packets = Vec::new();
        let mut buf = vec![0u8; 4000];
        for n in 0..150 {
            let frame: Vec<f32> = (0..FRAME).map(|i| ((n * FRAME + i) as f32 * 0.05).sin() * 0.3).collect();
            let len = encoder.encode_float(&frame, &mut buf).unwrap();
            packets.push(buf[..len].to_vec());
        }
        let file = write_ogg(&packets, pre_skip);
        let ogg = read_ogg(&file).unwrap();
        assert_eq!((ogg.channels, ogg.pre_skip, ogg.samples), (1, usize::from(pre_skip), 150 * FRAME as u64));
        assert_eq!(ogg.packets, packets);
        let sound = decode(&file).unwrap();
        // What the encoder held back at the start is dropped, as every Ogg Opus player does.
        assert_eq!(sound.len(), 150 * FRAME - usize::from(pre_skip));
        assert!(sound.iter().any(|s| s.abs() > 0.1));
        // A damaged page is refused.
        let mut bad = file.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(read_ogg(&bad).is_none());
    }

    #[test]
    fn stretching_keeps_the_pitch_and_the_level() {
        // A 440 Hz tone played twice as fast stays a 440 Hz tone, about as loud, in frames.
        let tone: Vec<f32> =
            (0..RATE as usize).map(|n| (n as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5).collect();
        let mut stretch = Stretch::new();
        let (mut out, mut at) = (Vec::new(), 0usize);
        while let Some(frame) = stretch.next(&tone, at) {
            assert_eq!(frame.len(), FRAME);
            out.extend(frame);
            at += FRAME * 2;
        }
        assert!(out.len() <= tone.len() / 2 + FRAME, "{}", out.len());
        // Skip the first frame (it fades in), then count rising zero crossings over a second's worth.
        let body = &out[FRAME..];
        let crossings = body.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32;
        let hz = crossings * RATE as f32 / body.len() as f32;
        assert!((hz - 440.0).abs() < 25.0, "{hz}");
        let peak = body.iter().copied().fold(0f32, |a, b| a.max(b.abs()));
        assert!((0.35..=0.65).contains(&peak), "{peak}");
    }

    #[test]
    fn waveforms_fold_peaks_into_bars() {
        assert_eq!(waveform(&[], BARS), vec![0]);
        let levels: Vec<f32> = (0..640).map(|i| if i < 320 { 0.25 } else { 1.0 }).collect();
        let w = waveform(&levels, BARS);
        assert_eq!(w.len(), BARS);
        assert_eq!((w[0], w[63]), (128, 255));
        // Silence stays silent rather than stretching to the top.
        assert!(waveform(&[0.001; 10], BARS).iter().all(|&b| b < 64));
        assert_eq!(heights(&[0, 255], 4), vec![0.0, 0.0, 1.0, 1.0]);
        assert_eq!(heights(&[], 3).len(), 3);
        assert_eq!(clock(65_400), "1:05");
    }
}
