//! A video's or a sound file's sound: its track read by Symphonia (MP4 and
//! M4A, WebM and Matroska, Ogg, MP3, FLAC, WAV), decoded (AAC, MP3, FLAC,
//! Vorbis and PCM by Symphonia, Opus by libopus), made stereo at the
//! speakers' rate, and played through the speakers picked in settings. The
//! speakers say which moment of the file they're playing, which is the
//! clock the picture follows.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait as _, StreamTrait as _};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use parking_lot::Mutex;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, CODEC_TYPE_OPUS, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::{Hint, Probe};
use symphonia::core::units::{Time, TimeBase};

use super::{Fetched, Shared};
use crate::core::voice::sound::Resampler;

/// The file kinds read for sound. Symphonia's own list would add raw AAC
/// (ADTS), whose reader mistakes some MP3 files for its own and can't cope.
pub(crate) static PROBE: LazyLock<Probe> = LazyLock::new(|| {
    use symphonia::default::formats::{FlacReader, IsoMp4Reader, MkvReader, MpaReader, OggReader, WavReader};
    let mut probe = Probe::default();
    probe.register_all::<IsoMp4Reader>();
    probe.register_all::<MkvReader>();
    probe.register_all::<OggReader>();
    probe.register_all::<WavReader>();
    probe.register_all::<FlacReader>();
    probe.register_all::<MpaReader>();
    probe
});

/// How much sound is decoded ahead of the speakers.
const AHEAD_MS: u32 = 500;

/// The file read as it is, from the start.
fn reader(bytes: &Fetched, ext: &str) -> Option<Box<dyn FormatReader>> {
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes.clone())), Default::default());
    let mut hint = Hint::new();
    if !ext.is_empty() {
        hint.with_extension(ext);
    }
    let options = FormatOptions { enable_gapless: true, ..FormatOptions::default() };
    PROBE.format(&hint, source, &options, &MetadataOptions::default()).ok().map(|p| p.format)
}

enum Decoder {
    Symphonia(Box<dyn symphonia::core::codecs::Decoder>, Option<SampleBuffer<f32>>),
    /// Opus, which Symphonia reads but doesn't decode; `skip` samples at the start are its encoder's delay.
    Opus {
        decoder: opus::Decoder,
        channels: usize,
        skip: usize,
        pcm: Vec<f32>,
    },
}

/// A file's sound track, read and decoded a packet at a time.
pub(super) struct Source {
    bytes: Fetched,
    ext: String,
    format: Box<dyn FormatReader>,
    track: u32,
    time_base: Option<TimeBase>,
    decoder: Decoder,
    /// Its sample rate.
    rate: u32,
    duration_ms: u64,
    /// After a jump: sound before this is decoded but not played.
    from_ms: f64,
    /// After a jump the file couldn't make itself: packets before this are skipped unread.
    skip_until_ms: f64,
}

impl Source {
    pub(super) fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    fn ms(&self, ts: u64) -> f64 {
        match self.time_base {
            Some(tb) => {
                let t = tb.calc_time(ts);
                t.seconds as f64 * 1000.0 + t.frac * 1000.0
            }
            None => ts as f64 * 1000.0 / f64::from(self.rate.max(1)),
        }
    }

    /// The next packet's sound, appended to `out` as stereo at its own rate,
    /// with when it starts; None for a packet that gave none; Err at the end.
    fn next(&mut self, out: &mut Vec<f32>) -> Result<Option<f64>, ()> {
        let packet = match self.format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                if let Decoder::Symphonia(decoder, _) = &mut self.decoder {
                    decoder.reset();
                }
                return Ok(None);
            }
            Err(_) => return Err(()),
        };
        if packet.track_id() != self.track {
            return Ok(None);
        }
        let start = self.ms(packet.ts());
        if start < self.skip_until_ms {
            return Ok(None);
        }
        let before = out.len();
        match &mut self.decoder {
            Decoder::Symphonia(decoder, buffer) => {
                let Ok(decoded) = decoder.decode(&packet) else { return Ok(None) };
                let spec = *decoded.spec();
                let channels = spec.channels.count().max(1);
                let b = buffer.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
                if b.capacity() < decoded.capacity() * channels {
                    *b = SampleBuffer::new(decoded.capacity() as u64, spec);
                }
                b.copy_interleaved_ref(decoded);
                stereo(b.samples(), channels, out);
            }
            Decoder::Opus { decoder, channels, skip, pcm } => {
                pcm.resize(5760 * *channels, 0.0);
                let Ok(n) = decoder.decode_float(&packet.data, pcm, false) else { return Ok(None) };
                let from = (*skip).min(n);
                *skip -= from;
                stereo(&pcm[from * *channels..n * *channels], *channels, out);
            }
        }
        // What comes before a jump's place is cut off.
        let cut = ((self.from_ms - start).max(0.0) * f64::from(self.rate) / 1000.0) as usize * 2;
        if cut > 0 {
            let end = (before + cut).min(out.len());
            out.drain(before..end);
            if out.len() == before {
                return Ok(None);
            }
            return Ok(Some(self.from_ms));
        }
        Ok((out.len() > before).then_some(start))
    }

    /// Goes to `ms`, through the file's own seeking or, when it can't, from
    /// the start, skipping what's before.
    fn seek(&mut self, ms: f64) {
        self.from_ms = ms;
        self.skip_until_ms = 0.0;
        let to = SeekTo::Time { time: Time::from(ms / 1000.0), track_id: Some(self.track) };
        if self.format.seek(SeekMode::Accurate, to).is_err() {
            if let Some(fresh) = reader(&self.bytes, &self.ext) {
                self.format = fresh;
            }
            // A packet starting a little before still holds the sound at `ms`.
            self.skip_until_ms = ms - 200.0;
        }
        match &mut self.decoder {
            Decoder::Symphonia(decoder, _) => decoder.reset(),
            Decoder::Opus { decoder, skip, .. } => {
                let _ = decoder.reset_state();
                *skip = 0;
            }
        }
    }
}

/// Any number of channels made stereo: one is both sides, more give their first two.
fn stereo(samples: &[f32], channels: usize, out: &mut Vec<f32>) {
    for frame in samples.chunks_exact(channels) {
        let left = frame[0];
        let right = if channels > 1 { frame[1] } else { left };
        out.push(left);
        out.push(right);
    }
}

/// The sound in a file, if it has some that plays here.
pub(super) fn open(bytes: &Fetched, content_type: &str, name: &str) -> Option<Source> {
    let mut ext = crate::core::attachments::extension_of(name);
    if ext.is_empty() {
        ext = content_type.split(['/', ';']).nth(1).unwrap_or_default().trim().to_owned();
    }
    let format = reader(bytes, &ext)?;
    let codecs = symphonia::default::get_codecs();
    let mut found = None;
    for track in format.tracks() {
        let params = &track.codec_params;
        if params.codec == CODEC_TYPE_NULL {
            continue;
        }
        let decoder = if params.codec == CODEC_TYPE_OPUS {
            let channels = params.channels.map_or_else(|| opus_channels(params.extra_data.as_deref()), |c| c.count());
            let layout = if channels >= 2 { opus::Channels::Stereo } else { opus::Channels::Mono };
            // More than two channels are several Opus streams together, which this doesn't take apart.
            if channels > 2 {
                continue;
            }
            let Ok(decoder) = opus::Decoder::new(48_000, layout) else { continue };
            let skip = params.extra_data.as_deref().and_then(opus_pre_skip).unwrap_or(0);
            Decoder::Opus { decoder, channels: channels.max(1), skip, pcm: Vec::new() }
        } else {
            match codecs.make(params, &DecoderOptions::default()) {
                Ok(decoder) => Decoder::Symphonia(decoder, None),
                Err(_) => continue,
            }
        };
        let rate = if params.codec == CODEC_TYPE_OPUS { 48_000 } else { params.sample_rate.unwrap_or(0) };
        if rate == 0 {
            continue;
        }
        found = Some((track.id, params.time_base, params.n_frames, decoder, rate));
        break;
    }
    let (track, time_base, frames, decoder, rate) = found?;
    let mut source = Source {
        bytes: bytes.clone(),
        ext,
        format,
        track,
        time_base,
        decoder,
        rate,
        duration_ms: 0,
        from_ms: 0.0,
        skip_until_ms: 0.0,
    };
    source.duration_ms = frames.map_or(0, |n| source.ms(n) as u64);
    Some(source)
}

/// How many channels an Opus header says.
fn opus_channels(head: Option<&[u8]>) -> usize {
    head.and_then(|h| h.get(9)).map_or(2, |&c| usize::from(c))
}

/// How many samples an Opus header says to drop at the start.
fn opus_pre_skip(head: &[u8]) -> Option<usize> {
    let bytes = head.get(10..12)?;
    Some(usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
}

// ───────────────────────── Playing ─────────────────────────

/// Decoded sound on its way to the speakers.
#[derive(Default)]
struct Ring {
    chunks: VecDeque<Chunk>,
    /// Stereo frames waiting, in all the chunks.
    frames: usize,
    /// The file has no more after these.
    end: bool,
}

/// Stereo frames at the speakers' rate, from `at_ms` in the file.
struct Chunk {
    at_ms: f64,
    samples: Vec<f32>,
    /// How many frames the speakers have taken.
    used: usize,
}

/// Decodes the sound ahead of the speakers and plays it, following jumps,
/// until the player goes.
pub(super) fn run(mut source: Source, shared: Arc<Shared>, has_picture: bool) {
    let ring = Arc::new(Mutex::new(Ring::default()));
    let opened = crate::core::sounds::picked_output()
        .and_then(|device| open_output(&device, ring.clone(), shared.clone(), has_picture));
    let Some((stream, rate)) = opened else {
        // Without speakers a video still plays, keeping time by itself.
        shared.no_speakers.store(true, Ordering::Relaxed);
        shared.sound_leads.store(false, Ordering::Relaxed);
        shared.sound_done.store(true, Ordering::Relaxed);
        shared.run_clock();
        return;
    };
    let mut sides = [Resampler::new(source.rate, rate), Resampler::new(source.rate, rate)];
    let mut jumps = shared.jumps.load(Ordering::Relaxed);
    let (mut decoded, mut side, mut made) = (Vec::new(), Vec::new(), [Vec::new(), Vec::new()]);
    let ahead = (rate * AHEAD_MS / 1000) as usize;
    let mut ended = false;
    while !shared.stop.load(Ordering::Relaxed) {
        let now_jumps = shared.jumps.load(Ordering::Relaxed);
        if now_jumps != jumps {
            jumps = now_jumps;
            source.seek(shared.jump_to.load(Ordering::Relaxed) as f64);
            *ring.lock() = Ring::default();
            sides = [Resampler::new(source.rate, rate), Resampler::new(source.rate, rate)];
            ended = false;
        }
        if ended || ring.lock().frames >= ahead {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        decoded.clear();
        match source.next(&mut decoded) {
            Ok(Some(at_ms)) => {
                for (n, resampler) in sides.iter_mut().enumerate() {
                    side.clear();
                    side.extend(decoded.iter().skip(n).step_by(2));
                    made[n].clear();
                    resampler.process(&side, &mut made[n]);
                }
                let frames = made[0].len().min(made[1].len());
                let mut samples = Vec::with_capacity(frames * 2);
                for (left, right) in made[0].iter().zip(&made[1]) {
                    samples.push(*left);
                    samples.push(*right);
                }
                if frames > 0 {
                    let mut ring = ring.lock();
                    ring.frames += frames;
                    ring.chunks.push_back(Chunk { at_ms, samples, used: 0 });
                }
            }
            Ok(None) => {}
            Err(()) => {
                ended = true;
                ring.lock().end = true;
            }
        }
    }
    drop(stream);
}

/// The speakers, playing what's in `ring`; with their rate.
fn open_output(
    device: &cpal::Device,
    ring: Arc<Mutex<Ring>>,
    shared: Arc<Shared>,
    has_picture: bool,
) -> Option<(cpal::Stream, u32)> {
    let config = device.default_output_config().ok()?;
    let rate = config.config().sample_rate;
    let stream = match config.sample_format() {
        SampleFormat::F32 => output::<f32>(device, config.config(), ring, shared, has_picture),
        SampleFormat::I16 => output::<i16>(device, config.config(), ring, shared, has_picture),
        SampleFormat::U16 => output::<u16>(device, config.config(), ring, shared, has_picture),
        SampleFormat::I32 => output::<i32>(device, config.config(), ring, shared, has_picture),
        _ => None,
    }?;
    stream.play().ok()?;
    Some((stream, rate))
}

fn output<T>(
    device: &cpal::Device,
    config: StreamConfig,
    ring: Arc<Mutex<Ring>>,
    shared: Arc<Shared>,
    has_picture: bool,
) -> Option<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let rate = f64::from(config.sample_rate.max(1));
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                if shared.paused.load(Ordering::Relaxed) {
                    data.fill(T::from_sample(0.0));
                    return;
                }
                let gain = shared.gain();
                let (mut first, mut taken) = (None, 0);
                let mut ring = ring.lock();
                for frame in data.chunks_mut(channels) {
                    let (left, right) = match ring.chunks.front_mut() {
                        Some(chunk) => {
                            first.get_or_insert(chunk.at_ms + chunk.used as f64 * 1000.0 / rate);
                            let at = chunk.used * 2;
                            let pair = (chunk.samples[at], chunk.samples[at + 1]);
                            chunk.used += 1;
                            if chunk.used * 2 >= chunk.samples.len() {
                                ring.chunks.pop_front();
                            }
                            taken += 1;
                            pair
                        }
                        None => (0.0, 0.0),
                    };
                    let (left, right) = (left * gain, right * gain);
                    if channels == 1 {
                        frame[0] = T::from_sample((left + right) / 2.0);
                    } else {
                        frame[0] = T::from_sample(left);
                        frame[1] = T::from_sample(right);
                        for rest in &mut frame[2..] {
                            *rest = T::from_sample(0.0);
                        }
                    }
                }
                ring.frames = ring.frames.saturating_sub(taken);
                let through = ring.end && ring.chunks.is_empty();
                drop(ring);
                if let Some(at) = first {
                    if shared.sound_leads.load(Ordering::Relaxed) {
                        let mut clock = shared.clock.lock();
                        clock.base = at;
                        clock.since = Some(Instant::now());
                    }
                } else if through {
                    // The sound is over: a longer picture goes on keeping time by itself.
                    if !shared.sound_done.swap(true, Ordering::Relaxed) {
                        shared.sound_leads.store(false, Ordering::Relaxed);
                        shared.run_clock();
                        shared.finish_if_through(true, has_picture);
                    }
                } else if shared.sound_leads.load(Ordering::Relaxed) {
                    // Waiting for sound: the picture waits too.
                    let mut clock = shared.clock.lock();
                    clock.base = clock.now();
                    clock.since = None;
                }
            },
            |_| tracing::debug!("the speakers stopped working"),
            None,
        )
        .ok()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// How long a file says its sound is, and how long it played, in milliseconds.
    pub(in crate::core::media) fn play_through(mut source: Source) -> (u64, f64) {
        let mut out = Vec::new();
        while source.next(&mut out).is_ok() {}
        (source.duration_ms(), out.len() as f64 / 2.0 * 1000.0 / f64::from(source.rate))
    }

    #[test]
    fn channels_become_two() {
        let mut out = Vec::new();
        stereo(&[0.5, -0.5], 1, &mut out);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);
        out.clear();
        stereo(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 3, &mut out);
        assert_eq!(out, [0.1, 0.2, 0.4, 0.5]);
    }
}
