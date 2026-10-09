//! fuwa's little sounds, made on the spot like the web's `lib/sounds.ts`
//! (the same notes, steps and lengths): a soft two-note blip for a message,
//! a brighter chime for a mention, and a rising sparkle when someone joins.
//! Each plays on its own short-lived output stream, on the speakers picked
//! in Voice & video (or the system's), at the volume set in Notifications.
//!
//! Any of them can play another of the built-in tunes instead, or a sound
//! file of your own (`SoundPick`): copied into the app's `sounds` folder,
//! named by what's in it, and decoded once when it first plays.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

use crate::core::config::SoundPick;
use crate::core::voice::access::{self, Access};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    Message,
    Mention,
    /// A direct message.
    Dm,
    Join,
    Call,
    Ring,
    /// A call's cues (the web's `cue`): coming in or switching on goes up, going goes down.
    Connect,
    Disconnect,
    SomeoneJoined,
    SomeoneLeft,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    /// Someone in the call started recording: two soft, even beeps.
    Recording,
}

impl Sound {
    /// A call's cues, in the order the settings list them.
    pub const CUES: [Sound; 9] = [
        Sound::Connect,
        Sound::Disconnect,
        Sound::SomeoneJoined,
        Sound::SomeoneLeft,
        Sound::Mute,
        Sound::Unmute,
        Sound::Deafen,
        Sound::Undeafen,
        Sound::Recording,
    ];

    /// Its name in the settings file (`Prefs::sound_picks`).
    pub fn id(self) -> &'static str {
        match self {
            Sound::Message => "message",
            Sound::Mention => "mention",
            Sound::Dm => "dm",
            Sound::Join => "join",
            Sound::Call => "call",
            Sound::Ring => "ring",
            Sound::Connect => "connect",
            Sound::Disconnect => "disconnect",
            Sound::SomeoneJoined => "someone_joined",
            Sound::SomeoneLeft => "someone_left",
            Sound::Mute => "mute",
            Sound::Unmute => "unmute",
            Sound::Deafen => "deafen",
            Sound::Undeafen => "undeafen",
            Sound::Recording => "recording",
        }
    }

    /// The built-in tune it plays unless another is picked.
    pub fn own_tune(self) -> &'static str {
        match self {
            Sound::Message => "blip",
            Sound::Mention | Sound::Dm => "chime",
            Sound::Join => "sparkle",
            Sound::Call => "hello",
            Sound::Ring => "ring",
            Sound::Connect => "rise",
            Sound::Disconnect => "fall",
            Sound::SomeoneJoined => "up",
            Sound::SomeoneLeft => "down",
            Sound::Mute => "tap_off",
            Sound::Unmute => "tap_on",
            Sound::Deafen => "hush",
            Sound::Undeafen => "unhush",
            Sound::Recording => "beeps",
        }
    }

    /// The longest a file of yours plays for it: a ringtone stops before it rings again.
    fn longest(self) -> f32 {
        match self {
            Sound::Ring => 2.5,
            _ => MOST_SECS,
        }
    }
}

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Triangle,
}

pub struct Tune {
    notes: &'static [f32],
    step: f32,
    length: f32,
    wave: Wave,
}

/// Every built-in tune, by id, in the order pickers list them: each sound's
/// own (the web's), then a few more to choose from. Names are
/// `desktop.sounds.tune.<id>`.
pub const TUNES: [(&str, Tune); 19] = [
    ("blip", Tune { notes: &[880.0, 1174.66], step: 0.07, length: 0.16, wave: Wave::Sine }),
    ("chime", Tune { notes: &[987.77, 1318.51, 1760.0], step: 0.075, length: 0.22, wave: Wave::Triangle }),
    ("sparkle", Tune { notes: &[659.25, 830.61, 987.77, 1318.51], step: 0.06, length: 0.18, wave: Wave::Sine }),
    ("hello", Tune { notes: &[587.33, 880.0], step: 0.08, length: 0.2, wave: Wave::Sine }),
    (
        "ring",
        Tune { notes: &[783.99, 987.77, 1174.66, 987.77, 1174.66], step: 0.11, length: 0.3, wave: Wave::Triangle },
    ),
    ("rise", Tune { notes: &[523.25, 659.25, 783.99, 1046.5], step: 0.07, length: 0.2, wave: Wave::Sine }),
    ("fall", Tune { notes: &[783.99, 659.25, 523.25], step: 0.08, length: 0.22, wave: Wave::Sine }),
    ("up", Tune { notes: &[659.25, 987.77], step: 0.07, length: 0.18, wave: Wave::Sine }),
    ("down", Tune { notes: &[987.77, 659.25], step: 0.07, length: 0.18, wave: Wave::Sine }),
    ("tap_off", Tune { notes: &[698.46, 523.25], step: 0.05, length: 0.12, wave: Wave::Triangle }),
    ("tap_on", Tune { notes: &[523.25, 698.46], step: 0.05, length: 0.12, wave: Wave::Triangle }),
    ("hush", Tune { notes: &[587.33, 440.0, 349.23], step: 0.05, length: 0.14, wave: Wave::Triangle }),
    ("unhush", Tune { notes: &[349.23, 440.0, 587.33], step: 0.05, length: 0.14, wave: Wave::Triangle }),
    ("beeps", Tune { notes: &[880.0, 880.0], step: 0.16, length: 0.1, wave: Wave::Sine }),
    ("bell", Tune { notes: &[1046.5, 1567.98], step: 0.0, length: 0.6, wave: Wave::Sine }),
    ("pop", Tune { notes: &[1396.91], step: 0.05, length: 0.08, wave: Wave::Sine }),
    ("drop", Tune { notes: &[1567.98, 1174.66, 880.0], step: 0.05, length: 0.14, wave: Wave::Sine }),
    (
        "twinkle",
        Tune { notes: &[1318.51, 1567.98, 2093.0, 1567.98, 2093.0], step: 0.06, length: 0.16, wave: Wave::Triangle },
    ),
    ("pluck", Tune { notes: &[659.25, 987.77], step: 0.09, length: 0.1, wave: Wave::Triangle }),
];

/// The built-in tune by id.
pub fn tune_by_id(id: &str) -> Option<&'static Tune> {
    TUNES.iter().find(|(i, _)| *i == id).map(|(_, t)| t)
}

fn tune(sound: Sound) -> &'static Tune {
    tune_by_id(sound.own_tune()).unwrap_or(&TUNES[0].1)
}

/// The tune as samples at `rate`: each note rises in over 12 ms and dies away
/// exponentially over its length, as the web's gain ramps do.
fn render(tune: &Tune, rate: u32, volume: f32) -> Vec<f32> {
    let rate_f = rate as f32;
    let total = tune.step * (tune.notes.len().saturating_sub(1)) as f32 + tune.length + 0.03;
    let mut out = vec![0.0f32; (total * rate_f) as usize];
    let peak = 0.18 * volume;
    for (n, freq) in tune.notes.iter().enumerate() {
        let start = (n as f32 * tune.step * rate_f) as usize;
        let len = (tune.length * rate_f) as usize;
        for i in 0..len {
            let at = i as f32 / rate_f;
            let phase = (at * freq).fract();
            let wave = match tune.wave {
                Wave::Sine => (phase * std::f32::consts::TAU).sin(),
                Wave::Triangle => 4.0 * (phase - (phase + 0.5).floor()).abs() - 1.0,
            };
            let gain = if at < 0.012 {
                peak * at / 0.012
            } else {
                // From the peak down to 0.0001 by the note's end.
                let k = (at - 0.012) / (tune.length - 0.012).max(0.001);
                peak * (0.0001f32 / peak.max(0.0001)).powf(k)
            };
            if let Some(slot) = out.get_mut(start + i) {
                *slot += wave * gain;
            }
        }
    }
    out
}

/// How many sounds are playing, so a burst of messages doesn't stack up dozens of streams.
static PLAYING: AtomicUsize = AtomicUsize::new(0);

/// Plays a sound at `volume` (0 to 100) on the speakers named `device` ("" for the system's),
/// as picked in the settings. Whether it should play at all (the setting, streamer mode) is
/// for the caller to decide.
pub fn play(sound: Sound, volume: u8, device: &str) {
    let pick = PICKS.lock().picks.get(sound.id()).cloned();
    play_with(sound, pick, volume, device);
}

/// Plays `sound` as `pick` would have it, for trying one in the settings.
pub fn play_pick(sound: Sound, pick: &SoundPick, volume: u8, device: &str) {
    play_with(sound, Some(pick.clone()), volume, device);
}

fn play_with(sound: Sound, pick: Option<SoundPick>, volume: u8, device: &str) {
    if volume == 0 || PLAYING.load(Ordering::Relaxed) >= 3 {
        return;
    }
    let device = device.to_owned();
    PLAYING.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("fuwa-sound".into()).spawn(move || {
        play_now(sound, pick.as_ref(), f32::from(volume) / 100.0, &device);
        PLAYING.fetch_sub(1, Ordering::Relaxed);
    });
    if spawned.is_err() {
        PLAYING.fetch_sub(1, Ordering::Relaxed);
    }
}

// ───────────────────────── Picks and files ─────────────────────────

/// The longest a sound file plays, in seconds: past this it's cut short.
pub const MOST_SECS: f32 = 8.0;
/// The biggest sound file taken.
pub const MOST_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// The file types offered (anything symphonia or `voice_notes` reads is taken).
pub const FILE_TYPES: &str = "WAV, MP3, FLAC, OGG";

struct Picks {
    dir: PathBuf,
    picks: BTreeMap<String, SoundPick>,
}

/// Where sound files are kept and which sound plays what, from the settings.
static PICKS: parking_lot::Mutex<Picks> =
    parking_lot::Mutex::new(Picks { dir: PathBuf::new(), picks: BTreeMap::new() });

/// Sound files already decoded, by file name.
static CLIPS: std::sync::LazyLock<parking_lot::Mutex<HashMap<String, Arc<Clip>>>> =
    std::sync::LazyLock::new(Default::default);

/// Remembers the picks in the settings and the folder their files are in.
pub fn set_picks(dir: &Path, picks: &BTreeMap<String, SoundPick>) {
    let mut held = PICKS.lock();
    if held.dir != dir {
        held.dir = dir.to_owned();
    }
    if &held.picks != picks {
        held.picks = picks.clone();
    }
}

/// Deletes the sound files no pick uses any more (at startup, so a file
/// just copied in is never swept before its pick is saved).
pub fn sweep(dir: &Path, picks: &BTreeMap<String, SoundPick>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !picks.values().any(|p| p.file == name) && valid_file_name(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A file name this module made: hex, a dot and a short extension, so a
/// hand-edited settings file can't point anywhere else.
fn valid_file_name(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else { return false };
    (8..=64).contains(&stem.len())
        && stem.bytes().all(|b| b.is_ascii_hexdigit())
        && (1..=5).contains(&ext.len())
        && ext.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Copies the sound file at `path` into `dir` for one of the sounds, once it's
/// known to play: what to save as the pick, or what went wrong to say.
pub fn import(dir: &Path, path: &Path) -> Result<SoundPick, String> {
    use sha2::Digest as _;
    let too_big = || {
        crate::core::i18n::t_with(
            "desktop.sounds.tooBig",
            &[("size", crate::core::i18n::Arg::Str(&format!("{} MB", MOST_FILE_BYTES / 1024 / 1024)))],
        )
    };
    let meta = std::fs::metadata(path).map_err(|_| crate::core::i18n::t("desktop.look.cantRead"))?;
    if meta.len() > MOST_FILE_BYTES {
        return Err(too_big());
    }
    let bytes = std::fs::read(path).map_err(|_| crate::core::i18n::t("desktop.look.cantRead"))?;
    let clip = Clip::read(&bytes).ok_or_else(|| {
        crate::core::i18n::t_with("desktop.sounds.notSound", &[("types", crate::core::i18n::Arg::Str(FILE_TYPES))])
    })?;
    let ext: String = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| (1..=5).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
        .unwrap_or_else(|| "sound".into());
    let hash = sha2::Sha256::digest(&bytes);
    let file = format!("{}.{ext}", hash.iter().take(12).map(|b| format!("{b:02x}")).collect::<String>());
    std::fs::create_dir_all(dir).map_err(|_| crate::core::i18n::t("desktop.sounds.cantSave"))?;
    let to = dir.join(&file);
    if !to.exists() {
        // Written beside it and moved in, so a half-written file never plays.
        let part = dir.join(format!("{file}.part"));
        std::fs::write(&part, &bytes)
            .and_then(|_| std::fs::rename(&part, &to))
            .map_err(|_| crate::core::i18n::t("desktop.sounds.cantSave"))?;
    }
    CLIPS.lock().insert(file.clone(), Arc::new(clip));
    let name = path.file_name().map(|n| n.to_string_lossy().chars().take(80).collect()).unwrap_or_default();
    Ok(SoundPick { tune: String::new(), file, name })
}

/// A sound file's sound, ready to play: mono, levelled, without the silence before it.
struct Clip {
    samples: Vec<f32>,
    rate: u32,
}

impl Clip {
    /// Decodes a sound file: Ogg Opus through `voice_notes`, anything else through symphonia.
    fn read(bytes: &[u8]) -> Option<Self> {
        let opus = bytes.starts_with(b"OggS") && bytes.windows(8).take(128).any(|w| w == b"OpusHead");
        let (mut samples, rate) = if opus {
            let mut s = crate::core::voice_notes::decode(bytes)?;
            s.truncate((MOST_SECS * 48_000.0) as usize + 48_000);
            (s, 48_000)
        } else {
            decode_any(bytes)?
        };
        // Starts at once, however much quiet the file opens with.
        let start = samples.iter().position(|s| s.abs() > 0.002)?;
        samples.drain(..start);
        samples.truncate((MOST_SECS * rate as f32) as usize);
        // As loud as the built-in tunes at most, and never boosted more than 4 times.
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        let gain = (0.4 / peak).min(4.0);
        for s in &mut samples {
            *s *= gain;
        }
        (!samples.is_empty() && rate > 0).then_some(Self { samples, rate })
    }

    /// Its samples at `rate` and `volume`, at most `longest` seconds, fading out if cut short.
    fn render(&self, rate: u32, volume: f32, longest: f32) -> Vec<f32> {
        let step = self.rate as f64 / f64::from(rate.max(1));
        let len = ((self.samples.len() as f64 / step) as usize).min((longest * rate as f32) as usize);
        let mut out: Vec<f32> = (0..len)
            .map(|i| {
                let at = i as f64 * step;
                let n = at as usize;
                let frac = (at - n as f64) as f32;
                let a = self.samples.get(n).copied().unwrap_or(0.0);
                let b = self.samples.get(n + 1).copied().unwrap_or(a);
                (a + (b - a) * frac) * volume
            })
            .collect();
        let fade = ((0.03 * rate as f32) as usize).min(out.len());
        let from = out.len() - fade;
        for (k, s) in out[from..].iter_mut().enumerate() {
            *s *= 1.0 - k as f32 / fade.max(1) as f32;
        }
        out
    }
}

/// Any file symphonia reads, as mono samples at its own rate.
fn decode_any(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let source = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes.to_vec())), Default::default());
    let probed = symphonia::default::get_probe()
        .format(&Hint::new(), source, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL)?;
    let (track_id, mut rate) = (track.id, track.codec_params.sample_rate.unwrap_or(0));
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).ok()?;
    let mut out = Vec::new();
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(_)) => break,
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        let channels = spec.channels.count().max(1);
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.capacity() * channels {
            *b = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        out.extend(b.samples().chunks(channels).map(|c| c.iter().sum::<f32>() / channels as f32));
        // A little past the longest it plays, for the silence it may start with.
        if rate > 0 && out.len() as f32 > (MOST_SECS + 2.0) * rate as f32 {
            break;
        }
    }
    (rate > 0 && !out.is_empty()).then_some((out, rate))
}

/// What a pick plays: a sound file of yours (decoded once), or a built-in tune.
enum Voice {
    Tune(&'static Tune),
    File(Arc<Clip>),
}

fn voice_of(sound: Sound, pick: Option<&SoundPick>) -> Voice {
    let Some(pick) = pick else { return Voice::Tune(tune(sound)) };
    if !pick.file.is_empty() && valid_file_name(&pick.file) {
        let cached = CLIPS.lock().get(&pick.file).cloned();
        if let Some(clip) = cached {
            return Voice::File(clip);
        }
        let dir = PICKS.lock().dir.clone();
        let read = std::fs::metadata(dir.join(&pick.file))
            .ok()
            .filter(|m| m.len() <= MOST_FILE_BYTES)
            .and_then(|_| std::fs::read(dir.join(&pick.file)).ok())
            .and_then(|bytes| Clip::read(&bytes));
        match read {
            Some(clip) => {
                let clip = Arc::new(clip);
                CLIPS.lock().insert(pick.file.clone(), clip.clone());
                return Voice::File(clip);
            }
            // Gone or broken: its own tune rather than nothing.
            None => tracing::debug!("a sound file couldn't be read"),
        }
    }
    Voice::Tune(tune_by_id(&pick.tune).unwrap_or_else(|| tune(sound)))
}

/// The output device by name, or the system's when there's no such device.
pub fn output_device(name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if !name.is_empty()
        && let Ok(mut devices) = host.output_devices()
        && let Some(found) = devices.find(|d| d.to_string() == name)
    {
        return Some(found);
    }
    host.default_output_device()
}

/// The microphone by name, or the system's when there's no such device.
pub fn input_device(name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if !name.is_empty()
        && let Ok(mut devices) = host.input_devices()
        && let Some(found) = devices.find(|d| d.to_string() == name)
    {
        return Some(found);
    }
    host.default_input_device()
}

/// The devices picked in Voice & video, for calls to open: (microphone, speakers), "" for the system's.
static PICKED: parking_lot::Mutex<(String, String)> = parking_lot::Mutex::new((String::new(), String::new()));

/// Remembers the devices picked in the settings, for the next call.
pub fn pick_devices(input: &str, output: &str) {
    *PICKED.lock() = (input.to_owned(), output.to_owned());
}

/// The microphone and speakers calls use: the ones picked, or the system's.
pub fn picked_input() -> Option<cpal::Device> {
    input_device(&PICKED.lock().0)
}

pub fn picked_output() -> Option<cpal::Device> {
    output_device(&PICKED.lock().1)
}

/// The microphones (`input`) or speakers there are, by name.
pub fn device_names(input: bool) -> Vec<String> {
    let host = cpal::default_host();
    let list = if input {
        host.input_devices().map(|d| d.collect::<Vec<_>>())
    } else {
        host.output_devices().map(|d| d.collect())
    };
    let mut names: Vec<String> =
        list.unwrap_or_default().iter().map(|d| d.to_string()).filter(|n| !n.is_empty()).collect();
    names.dedup();
    names
}

/// A running mic test: the microphone's level, in dB (-100 to 0), as it comes.
pub struct MicTest {
    level: Arc<std::sync::atomic::AtomicI32>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    pub failed: Arc<std::sync::atomic::AtomicBool>,
    /// It failed because the system won't let the app use the microphone.
    pub blocked: Arc<std::sync::atomic::AtomicBool>,
}

impl MicTest {
    /// Opens the microphone named `device` ("" for the system's) on a thread of its own.
    pub fn start(device: &str) -> Self {
        let level = Arc::new(std::sync::atomic::AtomicI32::new(-100));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (l, s, f, b, name) = (level.clone(), stop.clone(), failed.clone(), blocked.clone(), device.to_owned());
        let _ = std::thread::Builder::new().name("fuwa-mic-test".into()).spawn(move || {
            // Waits while the system's prompt is up.
            loop {
                match access::check(access::Device::Microphone) {
                    Access::Allowed => break,
                    Access::Asking if !s.load(Ordering::Relaxed) => std::thread::sleep(Duration::from_millis(250)),
                    Access::Asking => return,
                    Access::Blocked => {
                        b.store(true, Ordering::Relaxed);
                        f.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
            let Some(device) = input_device(&name) else {
                f.store(true, Ordering::Relaxed);
                return;
            };
            let Ok(config) = device.default_input_config() else {
                f.store(true, Ordering::Relaxed);
                return;
            };
            let channels = usize::from(config.channels().max(1));
            let meter = l.clone();
            let take = move |data: &[f32]| {
                let rms = (data.iter().map(|x| x * x).sum::<f32>() / data.len().max(1) as f32).sqrt();
                let db = if rms > 0.0 { (20.0 * rms.log10()).clamp(-100.0, 0.0) } else { -100.0 };
                meter.store(db.round() as i32, Ordering::Relaxed);
            };
            let stream = match config.sample_format() {
                SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
                    config.config(),
                    move |data: &[f32], _| take(&data.iter().step_by(channels).copied().collect::<Vec<_>>()),
                    |_| {},
                    None,
                ),
                SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
                    config.config(),
                    move |data: &[i16], _| {
                        take(&data.iter().step_by(channels).map(|s| f32::from(*s) / 32768.0).collect::<Vec<_>>())
                    },
                    |_| {},
                    None,
                ),
                _ => {
                    f.store(true, Ordering::Relaxed);
                    return;
                }
            };
            let Ok(stream) = stream else {
                f.store(true, Ordering::Relaxed);
                return;
            };
            if stream.play().is_err() {
                f.store(true, Ordering::Relaxed);
                return;
            }
            while !s.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
            drop(stream);
        });
        Self { level, stop, failed, blocked }
    }

    pub fn level(&self) -> f32 {
        self.level.load(Ordering::Relaxed) as f32
    }
}

impl Drop for MicTest {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn play_now(sound: Sound, pick: Option<&SoundPick>, volume: f32, device: &str) {
    let voice = voice_of(sound, pick);
    let Some(device) = output_device(device) else { return };
    let Ok(config) = device.default_output_config() else { return };
    let samples = Arc::new(match voice {
        Voice::Tune(tune) => render(tune, config.sample_rate(), volume),
        Voice::File(clip) => clip.render(config.sample_rate(), volume, sound.longest()),
    });
    let seconds = samples.len() as f32 / config.sample_rate() as f32;
    let stream = match config.sample_format() {
        SampleFormat::F32 => stream::<f32>(&device, config.config(), samples),
        SampleFormat::I16 => stream::<i16>(&device, config.config(), samples),
        SampleFormat::U16 => stream::<u16>(&device, config.config(), samples),
        SampleFormat::I32 => stream::<i32>(&device, config.config(), samples),
        _ => None,
    };
    let Some(stream) = stream else { return };
    if stream.play().is_err() {
        return;
    }
    std::thread::sleep(Duration::from_secs_f32(seconds + 0.1));
}

fn stream<T>(device: &cpal::Device, config: StreamConfig, samples: Arc<Vec<f32>>) -> Option<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let mut at = 0usize;
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                for frame in data.chunks_mut(channels) {
                    let s = samples.get(at).copied().unwrap_or(0.0);
                    at += 1;
                    for slot in frame {
                        *slot = T::from_sample(s);
                    }
                }
            },
            |_| tracing::debug!("a sound couldn't play"),
            None,
        )
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunes_ring_out_and_stay_quiet_enough() {
        let samples = render(tune(Sound::Mention), 48_000, 1.0);
        // Three notes 75 ms apart, each 220 ms long.
        assert!(samples.len() > 48_000 * 37 / 100);
        assert!(samples.iter().all(|s| s.abs() <= 0.18 * 3.0));
        assert!(samples.last().unwrap().abs() < 0.001);
        assert!(render(tune(Sound::Message), 48_000, 0.0).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn every_sound_has_a_tune_and_tunes_are_named_once() {
        let all = [Sound::Message, Sound::Mention, Sound::Dm, Sound::Join, Sound::Call, Sound::Ring];
        for sound in all.into_iter().chain(Sound::CUES) {
            assert!(tune_by_id(sound.own_tune()).is_some(), "{}", sound.id());
        }
        let mut ids: Vec<&str> = TUNES.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TUNES.len());
    }

    /// A 16-bit mono WAV of `seconds` of a 440 Hz tone after a little silence.
    fn wav(rate: u32, seconds: f32) -> Vec<u8> {
        let n = (rate as f32 * seconds) as usize;
        let quiet = rate as usize / 10;
        let data: Vec<u8> = (0..quiet + n)
            .flat_map(|i| {
                let v =
                    if i < quiet { 0.0 } else { (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.9 };
                ((v * 32767.0) as i16).to_le_bytes()
            })
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn sound_files_play_levelled_trimmed_and_cut_short() {
        let clip = Clip::read(&wav(22_050, 12.0)).unwrap();
        assert_eq!(clip.rate, 22_050);
        // The silence it opens with is gone, and it's no louder than a tune.
        assert!(clip.samples[..50].iter().any(|s| s.abs() > 0.01));
        assert!(clip.samples.iter().all(|s| s.abs() <= 0.41));
        assert!(clip.samples.len() <= (MOST_SECS * 22_050.0) as usize);
        // Played at 48 kHz, a ringtone stops before it rings again, fading out.
        let ring = clip.render(48_000, 1.0, Sound::Ring.longest());
        assert_eq!(ring.len(), (2.5 * 48_000.0) as usize);
        assert!(ring.last().unwrap().abs() < 0.01);
        assert!(Clip::read(b"not a sound at all").is_none());
    }

    #[test]
    fn imported_files_are_kept_by_what_they_hold() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("Ding Dong.WAV");
        std::fs::write(&from, wav(48_000, 0.5)).unwrap();
        let store = dir.path().join("sounds");
        let pick = import(&store, &from).unwrap();
        assert_eq!(pick.name, "Ding Dong.WAV");
        assert!(valid_file_name(&pick.file) && pick.file.ends_with(".wav"));
        assert!(store.join(&pick.file).exists());
        assert!(matches!(voice_of(Sound::Message, Some(&pick)), Voice::File(_)));
        // Nothing uses it: swept.
        sweep(&store, &BTreeMap::new());
        assert!(!store.join(&pick.file).exists());
        // Not a sound: refused, nothing kept.
        let text = dir.path().join("notes.mp3");
        std::fs::write(&text, b"hello").unwrap();
        assert!(import(&store, &text).is_err());
        assert!(!valid_file_name("../settings.json") && !valid_file_name("abc.wav"));
    }
}
