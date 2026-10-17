//! A server's soundboard (docs/soundboard.md), the web's `fuwa/soundboard.ts`
//! and `lib/soundboard.ts`: its sounds, listed once something wants them
//! and kept in step with `SoundsUpdated`, and playing `SoundPlayed` when
//! this app is in that voice channel's call and not deafened. The file is
//! fetched once from the instance (kept for the session), decoded from Ogg
//! Opus with libopus, and mixed into the call's sound at the sound's volume
//! times the Soundboard volume, so it plays on the call's speakers and
//! sounds overlap.
//!
//! Adding one starts from any sound file (the web's `voice/sound-file.ts`):
//! decoded through Symphonia (or libopus for Ogg Opus), made 48 kHz mono,
//! cut to a window of at most 10 seconds, encoded with libopus in 20 ms
//! frames and written as Ogg (`voice_notes::write_ogg`).

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::store::InstanceState;
use crate::core::voice::CallView;
use crate::core::voice::sound::{FRAME, RATE, Resampler};
use crate::core::voice_notes;
use crate::pb;
use crate::rpc;

/// The longest a sound plays, as the instance checks it (`MAX_SOUND_MS`).
pub const MAX_MS: u64 = 10_000;
/// The biggest file the instance takes for one.
pub const MAX_BYTES: usize = 1024 * 1024;
/// A name's length, in characters.
pub const MAX_NAME: usize = 32;
/// The kind of file a sound is uploaded as.
pub const CONTENT_TYPE: &str = "audio/ogg";
/// How long "who played what" shows on their tile.
pub const SHOWN_FOR: Duration = Duration::from_millis(3000);
/// The longest stretch of a picked file read: the window is found in this.
const MOST_PICKED_SECS: usize = 10 * 60;
/// Opus at this many bits a second: a short sound at most ~80 KB.
const BITRATE: i32 = 64_000;
/// The most sounds that play over each other at once; more wait their turn out.
const MOST_AT_ONCE: usize = 8;
/// Decoded sounds kept for the session.
const KEPT: usize = 64;

/// Someone played a sound a moment ago, for their tile.
#[derive(Debug, Clone)]
pub struct Played {
    pub channel_id: String,
    pub user_id: String,
    pub emoji: String,
    pub name: String,
    pub at: Instant,
}

// ───────────────────────── What's heard ─────────────────────────

/// Whether a sound played in `channel_id` on `instance` is heard here: in
/// that voice channel's call, and not deafened (the web's `heardHere`).
pub fn heard_here(call: Option<&CallView>, instance: &str, channel_id: &str) -> bool {
    call.is_some_and(|c| c.in_channel(instance, channel_id) && !c.self_deaf)
}

/// How loud a sound plays: its own volume times yours, both from 0 to 1.
pub fn gain(sound_volume: f32, soundboard_volume: f32) -> f32 {
    let clamp = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
    clamp(sound_volume) * clamp(soundboard_volume)
}

/// Where you stand in the call, for whether you may play a sound for everyone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Standing {
    pub connected: bool,
    pub muted: bool,
    pub deafened: bool,
    /// A moderator muted or deafened you.
    pub server_muted: bool,
    /// SPEAK in the channel.
    pub may_speak: bool,
}

/// Why the soundboard can't play for everyone right now, as a locale key
/// (the instance refuses the same: nobody plays a sound they couldn't say).
pub fn blocked(s: Standing) -> Option<&'static str> {
    Some(if !s.connected {
        "dms-calls.calls.soundboard.connecting"
    } else if s.deafened {
        "dms-calls.calls.soundboard.deafened"
    } else if s.muted {
        "dms-calls.calls.soundboard.muted"
    } else if s.server_muted {
        "dms-calls.calls.soundboard.serverMuted"
    } else if !s.may_speak {
        "dms-calls.calls.soundboard.noSpeak"
    } else {
        return None;
    })
}

/// Applies `SoundsUpdated`: the whole list, as it is now.
pub fn updated(i: &mut InstanceState, server_id: &str, sounds: &[pb::Sound]) {
    i.sounds.insert(server_id.to_owned(), sounds.to_vec());
}

/// What someone in a voice channel last played, while it still shows.
pub fn played_by<'a>(i: &'a InstanceState, channel_id: &str, user_id: &str) -> Option<&'a Played> {
    i.sounds_played
        .iter()
        .rev()
        .find(|p| p.channel_id == channel_id && p.user_id == user_id && p.at.elapsed() < SHOWN_FOR)
}

// ───────────────────────── Mixing ─────────────────────────

/// Sounds playing into the call, each from where it's got to, added
/// together over whatever else plays.
#[derive(Default)]
pub struct Board {
    playing: Mutex<Vec<Playing>>,
}

struct Playing {
    samples: Arc<Vec<f32>>,
    at: usize,
    gain: f32,
}

impl Board {
    /// Starts a sound (48 kHz mono) at `gain`, over any already playing.
    pub fn play(&self, samples: Arc<Vec<f32>>, gain: f32) {
        if gain <= 0.0 || samples.is_empty() {
            return;
        }
        let mut playing = self.playing.lock();
        if playing.len() >= MOST_AT_ONCE {
            playing.remove(0);
        }
        playing.push(Playing { samples, at: 0, gain });
    }

    /// Adds the next `out.len()` samples of everything playing into `out`.
    pub fn mix_into(&self, out: &mut [f32]) {
        let mut playing = self.playing.lock();
        if playing.is_empty() {
            return;
        }
        for p in playing.iter_mut() {
            let end = (p.at + out.len()).min(p.samples.len());
            for (slot, s) in out.iter_mut().zip(&p.samples[p.at..end]) {
                *slot += s * p.gain;
            }
            p.at = end;
        }
        playing.retain(|p| p.at < p.samples.len());
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }

    pub fn is_playing(&self) -> bool {
        !self.playing.lock().is_empty()
    }

    pub fn clear(&self) {
        self.playing.lock().clear();
    }
}

// ───────────────────────── Making one ─────────────────────────

/// A sound file picked to make a sound of, decoded.
#[derive(Clone)]
pub struct Picked {
    /// 48 kHz mono.
    pub samples: Arc<Vec<f32>>,
    /// The name it suggests (its file's).
    pub name: String,
}

impl Picked {
    pub fn duration_ms(&self) -> u64 {
        ms_of(self.samples.len())
    }
}

/// A sound file ready to upload.
#[derive(Clone)]
pub struct Made {
    pub ogg: Vec<u8>,
    pub duration_ms: i32,
}

fn ms_of(samples: usize) -> u64 {
    samples as u64 * 1000 / u64::from(RATE)
}

fn samples_of(ms: u64) -> usize {
    (ms * u64::from(RATE) / 1000) as usize
}

/// A file name made into a sound's name: the name without its kind, spaces
/// tidied, at most [`MAX_NAME`] characters, "sound" when nothing's left.
pub fn name_from_file(file: &str) -> String {
    let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = match base.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => base,
    };
    let words: Vec<&str> = stem.split(|c: char| c.is_whitespace() || c == '_').filter(|w| !w.is_empty()).collect();
    let name: String = words.join(" ").chars().take(MAX_NAME).collect();
    let name = name.trim().to_owned();
    if name.is_empty() { "sound".into() } else { name }
}

/// Whether a name can be a sound's: 1 to 32 characters, not only spaces.
pub fn name_ok(name: &str) -> bool {
    let n = name.trim().chars().count();
    (1..=MAX_NAME).contains(&n)
}

/// Where a window of at most 10 seconds can start in something `total_ms`
/// long: `start_ms` moved back so the window fits.
pub fn window_start(total_ms: u64, start_ms: u64) -> u64 {
    start_ms.min(total_ms.saturating_sub(MAX_MS))
}

/// The window kept: from its start, at most 10 seconds.
pub fn window(samples: &[f32], start_ms: u64) -> &[f32] {
    let start = samples_of(window_start(ms_of(samples.len()), start_ms)).min(samples.len());
    let end = (start + samples_of(MAX_MS)).min(samples.len());
    &samples[start..end]
}

/// Reads any sound file the app can (MP3, WAV, Ogg, FLAC, M4A...) as 48 kHz
/// mono, at most its first ten minutes.
pub fn open_file(bytes: &[u8], file_name: &str) -> Result<Picked, String> {
    let most = MOST_PICKED_SECS * RATE as usize;
    // Symphonia reads Ogg Opus but doesn't decode it; libopus does.
    let samples = match voice_notes::decode(bytes) {
        Some(mut s) if !s.is_empty() => {
            s.truncate(most);
            s
        }
        _ => {
            let (raw, rate) = decode_any(bytes, MOST_PICKED_SECS).ok_or("That file can't be read as sound.")?;
            let mut out = Vec::with_capacity(raw.len() * RATE as usize / rate.max(1) as usize + 1);
            Resampler::new(rate, RATE).process(&raw, &mut out);
            out.truncate(most);
            out
        }
    };
    if samples.is_empty() {
        return Err("That file has no sound in it.".into());
    }
    Ok(Picked { samples: Arc::new(samples), name: name_from_file(file_name) })
}

/// Any file Symphonia reads, as mono samples at its own rate, at most `most_secs` long.
fn decode_any(bytes: &[u8], most_secs: usize) -> Option<(Vec<f32>, u32)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let source = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes.to_vec())), Default::default());
    let options = FormatOptions { enable_gapless: true, ..FormatOptions::default() };
    let probed = crate::core::media::PROBE.format(&Hint::new(), source, &options, &MetadataOptions::default()).ok()?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL)?;
    let (track_id, mut rate) = (track.id, track.codec_params.sample_rate.unwrap_or(0));
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).ok()?;
    let mut out = Vec::new();
    let mut buf: Option<SampleBuffer<f32>> = None;
    while let Ok(packet) = format.next_packet() {
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
        if rate > 0 && out.len() > most_secs * rate as usize {
            break;
        }
    }
    (rate > 0 && !out.is_empty()).then_some((out, rate))
}

/// Encodes 48 kHz mono samples (at most 10 seconds; more is cut) as Ogg Opus in 20 ms frames.
pub fn encode(samples: &[f32]) -> Result<Made, String> {
    let samples = &samples[..samples.len().min(samples_of(MAX_MS))];
    if samples.is_empty() {
        return Err("There's no sound to keep.".into());
    }
    let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Audio)
        .and_then(|mut e| {
            e.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
            Ok(e)
        })
        .map_err(|_| "Opus didn't start.".to_owned())?;
    let pre_skip = encoder.get_lookahead().map(|n| n.clamp(0, i32::from(u16::MAX)) as u16).unwrap_or(312);
    let mut packets = Vec::with_capacity(samples.len().div_ceil(FRAME));
    let mut packet = vec![0u8; 4000];
    for chunk in samples.chunks(FRAME) {
        let mut frame = [0f32; FRAME];
        frame[..chunk.len()].copy_from_slice(chunk);
        let n = encoder.encode_float(&frame, &mut packet).map_err(|_| "Opus couldn't encode that.".to_owned())?;
        packets.push(packet[..n].to_vec());
    }
    let ogg = voice_notes::write_ogg(&packets, pre_skip);
    if ogg.len() > MAX_BYTES {
        return Err("That sound came out too big.".into());
    }
    Ok(Made { ogg, duration_ms: (packets.len() * 20) as i32 })
}

// ───────────────────────── Playing ─────────────────────────

/// Servers whose sounds are being listed, or couldn't be, by "instance server".
static LISTING: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Default::default);

/// Decoded sounds, by instance and link, for the session.
static DECODED: LazyLock<Mutex<HashMap<String, Arc<Vec<f32>>>>> = LazyLock::new(Default::default);
/// The preview playing outside a call, until it ends or another starts.
static PREVIEW: LazyLock<Mutex<Option<voice_notes::Player>>> = LazyLock::new(Default::default);

/// Scales samples by `gain`.
fn louder(samples: &[f32], gain: f32) -> Arc<Vec<f32>> {
    Arc::new(samples.iter().map(|s| (s * gain).clamp(-1.0, 1.0)).collect())
}

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

impl Core {
    /// A server's sounds, if they're listed.
    pub fn sounds(&self, key: &str, server_id: &str) -> Option<Vec<pb::Sound>> {
        self.shared.read(|s| s.instance(key).and_then(|i| i.sounds.get(server_id).cloned()))
    }

    /// Lists a server's sounds, unless they're listed already (`SoundsUpdated`
    /// keeps them), being listed, or couldn't be (until `retry`).
    pub fn want_sounds(self: &Arc<Self>, key: &str, server_id: &str, retry: bool) {
        if self.sounds(key, server_id).is_some() {
            return;
        }
        let id = format!("{key} {server_id}");
        {
            let mut listing = LISTING.lock();
            match listing.get(&id) {
                Some(true) => return,
                Some(false) if !retry => return,
                _ => {}
            }
            listing.insert(id.clone(), true);
        }
        let (core, key, server_id) = (self.clone(), key.to_owned(), server_id.to_owned());
        self.runtime.spawn(async move {
            let listed = core.list_sounds(&key, &server_id).await.is_ok();
            let mut listing = LISTING.lock();
            if listed {
                listing.remove(&id);
            } else {
                listing.insert(id, false);
            }
            drop(listing);
            // The window hears it either way.
            core.shared.update(|_| ());
        });
    }

    /// Whether listing a server's sounds failed (until it's tried again).
    pub fn sounds_failed(&self, key: &str, server_id: &str) -> bool {
        LISTING.lock().get(&format!("{key} {server_id}")) == Some(&false)
    }

    /// Lists a server's sounds again.
    pub async fn list_sounds(&self, key: &str, server_id: &str) -> Result<Vec<pb::Sound>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.soundboard(), list_sounds(pb::ListSoundsRequest { server_id: server_id.into() })).await?;
        self.shared.instance(key, |i| updated(i, server_id, &res.sounds));
        Ok(res.sounds)
    }

    fn put_sounds(&self, key: &str, server_id: &str, change: impl FnOnce(&mut Vec<pb::Sound>)) {
        self.shared.instance(key, |i| change(i.sounds.entry(server_id.to_owned()).or_default()));
    }

    /// Plays a sound for everyone in the voice channel you're in.
    pub async fn play_sound(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        sound_id: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.soundboard(),
            play_sound(pb::PlaySoundRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                sound_id: sound_id.into(),
            })
        )
        .await?;
        crate::core::reports::used("soundboard.play");
        Ok(())
    }

    /// Uploads a made sound file and adds it to the server's soundboard.
    pub async fn create_sound(
        self: &Arc<Self>,
        key: &str,
        server_id: &str,
        name: &str,
        emoji: &str,
        volume: f32,
        made: Made,
    ) -> Result<pb::Sound, Problem> {
        let duration_ms = made.duration_ms;
        let url = self.upload_picture_for(key, server_id, pb::MediaPurpose::Sound, CONTENT_TYPE, made.ogg).await?;
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.soundboard(),
            create_sound(pb::CreateSoundRequest {
                server_id: server_id.into(),
                name: name.trim().into(),
                url,
                emoji: emoji.trim().into(),
                volume: Some(volume.clamp(0.0, 1.0)),
                duration_ms,
            })
        )
        .await?;
        let sound = res.sound.unwrap_or_default();
        self.put_sounds(key, server_id, |list| {
            list.retain(|s| s.id != sound.id);
            list.push(sound.clone());
        });
        Ok(sound)
    }

    /// Renames a sound, or changes its emoji or volume; `None` keeps each.
    pub async fn update_sound(
        &self,
        key: &str,
        server_id: &str,
        sound_id: &str,
        name: Option<String>,
        emoji: Option<String>,
        volume: Option<f32>,
    ) -> Result<pb::Sound, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.soundboard(),
            update_sound(pb::UpdateSoundRequest {
                server_id: server_id.into(),
                sound_id: sound_id.into(),
                name: name.map(|n| n.trim().to_owned()),
                emoji: emoji.map(|e| e.trim().to_owned()),
                volume: volume.map(|v| v.clamp(0.0, 1.0)),
            })
        )
        .await?;
        let sound = res.sound.unwrap_or_default();
        self.put_sounds(key, server_id, |list| {
            if let Some(s) = list.iter_mut().find(|s| s.id == sound.id) {
                *s = sound.clone();
            }
        });
        Ok(sound)
    }

    pub async fn delete_sound(&self, key: &str, server_id: &str, sound_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.soundboard(),
            delete_sound(pb::DeleteSoundRequest { server_id: server_id.into(), sound_id: sound_id.into() })
        )
        .await?;
        self.put_sounds(key, server_id, |list| list.retain(|s| s.id != sound_id));
        Ok(())
    }

    /// A sound's file, decoded: fetched from its instance the first time.
    pub async fn sound_samples(&self, key: &str, sound: &pb::Sound) -> Result<Arc<Vec<f32>>, Problem> {
        let id = format!("{key} {}", sound.url);
        if let Some(samples) = DECODED.lock().get(&id) {
            return Ok(samples.clone());
        }
        let size = if sound.size > 0 { sound.size } else { MAX_BYTES as i64 };
        let bytes = self.fetch_media(key, &sound.url, size, Arc::new(AtomicU64::new(0))).await?;
        let samples = voice_notes::decode(&bytes)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Problem::new(Code::DataLoss, "That sound can't be played."))?;
        let samples = Arc::new(samples);
        let mut kept = DECODED.lock();
        if kept.len() >= KEPT {
            kept.clear();
        }
        kept.insert(id, samples.clone());
        Ok(samples)
    }

    /// Someone played a sound in a voice channel (`SoundPlayed`): shown on
    /// their tile a moment, and played if you're in that call and can hear.
    pub(crate) fn sound_played(self: &Arc<Self>, key: &str, server_id: &str, played: &pb::SoundPlayed) {
        let Some(sound) = played.sound.clone() else { return };
        let shown = Played {
            channel_id: played.channel_id.clone(),
            user_id: played.user_id.clone(),
            emoji: sound.emoji.clone(),
            name: sound.name.clone(),
            at: Instant::now(),
        };
        self.shared.instance(key, |i| {
            i.sounds_played.retain(|p| p.at.elapsed() < SHOWN_FOR && p.user_id != shown.user_id);
            i.sounds_played.push(shown);
            // A sound changed since the list was read plays as it is now.
            if let Some(list) = i.sounds.get_mut(server_id)
                && let Some(s) = list.iter_mut().find(|s| s.id == sound.id)
            {
                *s = sound.clone();
            }
        });
        // Gone from their tile once it's shown long enough.
        let (core, instance) = (self.clone(), key.to_owned());
        self.runtime.spawn(async move {
            tokio::time::sleep(SHOWN_FOR + Duration::from_millis(50)).await;
            core.shared.instance(&instance, |i| i.sounds_played.retain(|p| p.at.elapsed() < SHOWN_FOR));
        });
        let here = |core: &Core, key: &str, channel_id: &str| heard_here(core.call().as_ref(), key, channel_id);
        if !here(self, key, &played.channel_id) {
            return;
        }
        let gain = gain(sound.volume, self.shared.prefs().soundboard_volume);
        if gain <= 0.0 {
            return;
        }
        let core = self.clone();
        let (key, channel_id) = (key.to_owned(), played.channel_id.clone());
        self.runtime.spawn(async move {
            // Still there once the file's in (it's fetched only the first time).
            if let Ok(samples) = core.sound_samples(&key, &sound).await
                && here(&core, &key, &channel_id)
            {
                core.voice.board.play(samples, gain);
            }
        });
    }

    /// Plays a sound for you alone, at its volume times yours: into the
    /// call when you're in one and can hear, on the speakers otherwise.
    pub async fn preview_sound(&self, key: &str, sound: &pb::Sound) -> Result<(), Problem> {
        let samples = self.sound_samples(key, sound).await?;
        self.preview_samples(&samples, gain(sound.volume, self.shared.prefs().soundboard_volume));
        Ok(())
    }

    /// Plays 48 kHz mono samples for you alone at `gain` (a sound being made, say).
    pub fn preview_samples(&self, samples: &[f32], gain: f32) {
        let in_call = self.call().is_some_and(|c| !c.self_deaf);
        if in_call {
            self.voice.board.play(louder(samples, 1.0), gain);
        } else {
            *PREVIEW.lock() = Some(voice_notes::Player::play(louder(samples, gain), 0, 1.0));
        }
    }

    /// Stops a preview playing on the speakers.
    pub fn stop_preview(&self) {
        *PREVIEW.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: u64) -> Vec<f32> {
        (0..samples_of(ms)).map(|n| (n as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5).collect()
    }

    #[test]
    fn heard_only_in_that_call_and_not_deafened() {
        let call = CallView {
            instance: "k".into(),
            server_id: "s".into(),
            channel_id: "v".into(),
            conversation_id: String::new(),
            status: crate::core::voice::Status::Connected,
            self_mute: true,
            self_deaf: false,
            self_video: false,
            self_stream: false,
            self_record: false,
            server_record: false,
            server_recordings: false,
            instance_camera: crate::core::voice::ceiling::Ceiling::NONE,
            since: None,
            speaking: Default::default(),
            playing: Default::default(),
            trouble: vec![],
            quality: Default::default(),
            video_suppress: false,
            video_off: false,
            video_trouble: None,
            screen_sound_offered: false,
            screen_sound: None,
            screen_sounds: Default::default(),
            quiet_screens: Default::default(),
        };
        assert!(heard_here(Some(&call), "k", "v"), "muted still hears");
        assert!(!heard_here(Some(&call), "k", "other"));
        assert!(!heard_here(Some(&call), "other", "v"));
        assert!(!heard_here(None, "k", "v"));
        let deaf = CallView { self_deaf: true, ..call.clone() };
        assert!(!heard_here(Some(&deaf), "k", "v"));
        let dm = CallView { conversation_id: "d".into(), ..call };
        assert!(!heard_here(Some(&dm), "k", "v"));
    }

    #[test]
    fn volumes_multiply_and_stay_in_range() {
        assert_eq!(gain(0.5, 0.5), 0.25);
        assert_eq!(gain(2.0, 1.0), 1.0);
        assert_eq!(gain(-1.0, 1.0), 0.0);
        assert_eq!(gain(f32::NAN, 0.5), 0.5);
        let ok = Standing { connected: true, may_speak: true, ..Standing::default() };
        assert_eq!(blocked(ok), None);
        assert_eq!(
            blocked(Standing { muted: true, deafened: true, ..ok }),
            Some("dms-calls.calls.soundboard.deafened")
        );
        assert_eq!(blocked(Standing { muted: true, ..ok }), Some("dms-calls.calls.soundboard.muted"));
        assert_eq!(blocked(Standing { server_muted: true, ..ok }), Some("dms-calls.calls.soundboard.serverMuted"));
        assert_eq!(blocked(Standing { may_speak: false, ..ok }), Some("dms-calls.calls.soundboard.noSpeak"));
        assert_eq!(blocked(Standing { connected: false, ..ok }), Some("dms-calls.calls.soundboard.connecting"));
    }

    #[test]
    fn sounds_overlap_on_the_board_and_end() {
        let board = Board::default();
        board.play(Arc::new(vec![0.25; 30]), 1.0);
        board.play(Arc::new(vec![0.5; 10]), 0.5);
        let mut out = [0.1f32; 20];
        board.mix_into(&mut out);
        assert!((out[0] - 0.6).abs() < 1e-6, "both, over what was there: {}", out[0]);
        assert!((out[15] - 0.35).abs() < 1e-6, "the short one ended: {}", out[15]);
        let mut out = [0.0f32; 20];
        board.mix_into(&mut out);
        assert!((out[0] - 0.25).abs() < 1e-6 && out[10] == 0.0);
        assert!(!board.is_playing());
        board.play(Arc::new(vec![1.0; 10]), 1.0);
        board.play(Arc::new(vec![1.0; 10]), 1.0);
        let mut out = [0.0f32; 4];
        board.mix_into(&mut out);
        assert_eq!(out[0], 1.0, "kept in range");
        board.clear();
        assert!(!board.is_playing());
        board.play(Arc::new(vec![1.0; 10]), 0.0);
        assert!(!board.is_playing(), "silent sounds don't play");
    }

    #[test]
    fn names_come_from_files() {
        assert_eq!(name_from_file("/home/me/Air Horn.mp3"), "Air Horn");
        assert_eq!(name_from_file("C:\\sounds\\gg_wp.ogg"), "gg wp");
        assert_eq!(name_from_file(".wav"), ".wav");
        assert_eq!(name_from_file("   .mp3"), "sound");
        assert_eq!(name_from_file(&format!("{}.flac", "a".repeat(40))).chars().count(), MAX_NAME);
        assert!(name_ok("gg") && !name_ok("   ") && !name_ok(&"a".repeat(33)));
    }

    #[test]
    fn the_window_fits_inside_the_file() {
        assert_eq!(window_start(30_000, 25_000), 20_000);
        assert_eq!(window_start(30_000, 5_000), 5_000);
        assert_eq!(window_start(4_000, 2_000), 0);
        let long = tone(30_000);
        assert_eq!(window(&long, 25_000).len(), samples_of(MAX_MS));
        let short = tone(4_000);
        assert_eq!(window(&short, 1_000).len(), short.len());
    }

    #[test]
    fn a_made_sound_is_ogg_opus_of_its_length() {
        let made = encode(&tone(2_500)).unwrap();
        assert_eq!(made.duration_ms, 2_500);
        let ogg = voice_notes::read_ogg(&made.ogg).unwrap();
        assert_eq!(ogg.channels, 1);
        assert_eq!(ogg.samples, samples_of(2_500) as u64);
        let back = voice_notes::decode(&made.ogg).unwrap();
        // All but the encoder's delay at the end, as voice messages too.
        assert_eq!(back.len(), samples_of(2_500) - ogg.pre_skip);
        // A tone comes back a tone, not silence.
        let level = crate::core::voice::sound::level(&back[RATE as usize..RATE as usize + FRAME]);
        assert!(level > 0.2, "{level}");
        // Never past 10 seconds, and within the instance's size.
        let made = encode(&tone(12_000)).unwrap();
        assert_eq!(made.duration_ms, MAX_MS as i32);
        assert!(made.ogg.len() <= MAX_BYTES);
        assert!(encode(&[]).is_err());
    }

    #[test]
    fn files_open_at_48_khz() {
        // A WAV at 24 kHz, two seconds.
        let rate = 24_000u32;
        let n = rate as usize * 2;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend(&(36 + n as u32 * 2).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(&16u32.to_le_bytes());
        wav.extend(&1u16.to_le_bytes());
        wav.extend(&1u16.to_le_bytes());
        wav.extend(&rate.to_le_bytes());
        wav.extend(&(rate * 2).to_le_bytes());
        wav.extend(&2u16.to_le_bytes());
        wav.extend(&16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(&(n as u32 * 2).to_le_bytes());
        for i in 0..n {
            let s = ((i as f32 * 300.0 * std::f32::consts::TAU / rate as f32).sin() * 12_000.0) as i16;
            wav.extend(&s.to_le_bytes());
        }
        let picked = open_file(&wav, "Boop.wav").unwrap();
        assert_eq!(picked.name, "Boop");
        assert!((picked.duration_ms() as i64 - 2_000).abs() <= 2, "{}", picked.duration_ms());
        // An Ogg Opus file opens through libopus.
        let made = encode(&tone(1_000)).unwrap();
        let again = open_file(&made.ogg, "x.ogg").unwrap();
        assert!((990..=1_000).contains(&again.duration_ms()), "{}", again.duration_ms());
        assert!(open_file(b"not a sound", "x.mp3").is_err());
    }
}
