//! A video's picture: its track found in the file (MP4 and QuickTime
//! through `mp4`, WebM and Matroska through `matroska-demuxer`), each frame
//! decoded (H.264 through OpenH264, VP8 and VP9 through libvpx), made small
//! enough for a message, and handed to the window when the clock reaches it.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use matroska_demuxer::{Frame, MatroskaFile, TrackType};

use super::{Fetched, Picture, Shared, Unplayable};
use crate::core::voice::vp8::{self, Codec};

/// Frames bigger than this are shrunk before they're drawn: a message shows
/// a video at most 420 pixels wide, twice that on a sharp screen.
const MOST_SHOWN: (usize, usize) = (960, 720);

/// One frame as it is in the file.
struct Packet {
    /// When it's shown, in milliseconds.
    ms: f64,
    key: bool,
    data: Vec<u8>,
}

/// What a video's frames are encoded as.
enum Kind {
    /// With its parameter sets and how long each NAL unit's length is.
    H264 {
        sps: Vec<Vec<u8>>,
        pps: Vec<Vec<u8>>,
        length_size: usize,
    },
    Vpx(Codec),
}

/// A video track to play: where its frames come from and what they are.
pub(super) struct Source {
    demux: Box<dyn Demux>,
    kind: Kind,
    duration_ms: u64,
}

impl Source {
    pub(super) fn duration_ms(&self) -> u64 {
        self.duration_ms
    }
}

trait Demux: Send {
    /// The next frame in decoding order; None at the end.
    fn next(&mut self) -> Option<Packet>;
    /// Goes back to the last keyframe at or before `ms`, so decoding from
    /// there reaches it.
    fn seek(&mut self, ms: f64);
}

/// The video track in a file, if it has one: None for a file without one
/// (or one this side doesn't read, which may still have sound).
pub(super) fn open(bytes: &Fetched, content_type: &str, name: &str) -> Result<Option<Source>, Unplayable> {
    if bytes.get(4..8) == Some(b"ftyp") || bytes.get(4..8) == Some(b"moov") {
        open_mp4(bytes)
    } else if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        open_mkv(bytes)
    } else if content_type.starts_with("video/")
        || crate::core::attachments::family_of(name) == crate::core::attachments::Family::Video
    {
        Err(Unplayable::Kind)
    } else {
        Ok(None)
    }
}

// ───────────────────────── MP4 ─────────────────────────

struct Mp4Demux {
    reader: mp4::Mp4Reader<Cursor<Fetched>>,
    track: u32,
    count: u32,
    /// The next sample, counted from 1.
    next: u32,
    timescale: f64,
    /// Where the edit list starts the track, in its timescale: the frames before it aren't shown.
    shift: i64,
    /// When each sample is decoded, in the timescale, from the start of each run of `stts`.
    runs: Vec<(u32, u64, u32)>,
    /// The keyframes, counted from 1; empty when every frame is one.
    syncs: Vec<u32>,
}

fn open_mp4(bytes: &Fetched) -> Result<Option<Source>, Unplayable> {
    let reader =
        mp4::Mp4Reader::read_header(Cursor::new(bytes.clone()), bytes.len() as u64).map_err(|_| Unplayable::Broken)?;
    let videos: Vec<&mp4::Mp4Track> =
        reader.tracks().values().filter(|t| matches!(t.track_type(), Ok(mp4::TrackType::Video))).collect();
    if videos.is_empty() {
        // An unusual video track (AV1, say) has no type `mp4` knows: its box is still there.
        let unknown = reader.tracks().values().any(|t| t.trak.mdia.hdlr.handler_type.value == *b"vide");
        return if unknown { Err(Unplayable::Kind) } else { Ok(None) };
    }
    let Some(track) = videos
        .iter()
        .filter(|t| matches!(t.media_type(), Ok(mp4::MediaType::H264 | mp4::MediaType::VP9)))
        .min_by_key(|t| t.track_id())
    else {
        return Err(Unplayable::Kind);
    };
    let stsd = &track.trak.mdia.minf.stbl.stsd;
    let kind = match (&stsd.avc1, &stsd.vp09) {
        (Some(avc1), _) => Kind::H264 {
            sps: avc1.avcc.sequence_parameter_sets.iter().map(|n| n.bytes.clone()).collect(),
            pps: avc1.avcc.picture_parameter_sets.iter().map(|n| n.bytes.clone()).collect(),
            length_size: usize::from(avc1.avcc.length_size_minus_one & 3) + 1,
        },
        (None, Some(_)) => Kind::Vpx(Codec::Vp9),
        (None, None) => return Err(Unplayable::Kind),
    };
    let stbl = &track.trak.mdia.minf.stbl;
    let mut runs = Vec::with_capacity(stbl.stts.entries.len());
    let (mut first, mut time) = (1u32, 0u64);
    for entry in &stbl.stts.entries {
        runs.push((first, time, entry.sample_delta));
        first = first.saturating_add(entry.sample_count);
        time += u64::from(entry.sample_count) * u64::from(entry.sample_delta);
    }
    let syncs = stbl.stss.as_ref().map(|s| s.entries.clone()).unwrap_or_default();
    let shift = track
        .trak
        .edts
        .as_ref()
        .and_then(|e| e.elst.as_ref())
        .and_then(|elst| elst.entries.iter().find(|e| e.media_time != u64::from(u32::MAX) && e.media_time != u64::MAX))
        .map_or(0, |e| e.media_time as i64);
    let timescale = f64::from(track.timescale().max(1));
    let duration_ms = (track.duration().as_millis() as u64).max(reader.duration().as_millis() as u64);
    let (track, count) = (track.track_id(), track.sample_count());
    let demux = Mp4Demux { reader, track, count, next: 1, timescale, shift, runs, syncs };
    Ok(Some(Source { demux: Box::new(demux), kind, duration_ms }))
}

impl Mp4Demux {
    /// When sample `n` is decoded, in the timescale.
    fn decode_time(&self, n: u32) -> u64 {
        let at = self.runs.partition_point(|(first, _, _)| *first <= n);
        match at.checked_sub(1).and_then(|i| self.runs.get(i)) {
            Some((first, time, delta)) => time + u64::from(n - first) * u64::from(*delta),
            None => 0,
        }
    }
}

impl Demux for Mp4Demux {
    fn next(&mut self) -> Option<Packet> {
        while self.next <= self.count {
            let n = self.next;
            self.next += 1;
            let Ok(Some(sample)) = self.reader.read_sample(self.track, n) else { continue };
            let shown = sample.start_time as i64 + i64::from(sample.rendering_offset) - self.shift;
            let ms = shown as f64 * 1000.0 / self.timescale;
            return Some(Packet { ms, key: sample.is_sync, data: sample.bytes.to_vec() });
        }
        None
    }

    fn seek(&mut self, ms: f64) {
        let target = (ms * self.timescale / 1000.0) as i64 + self.shift;
        let target = target.max(0) as u64;
        self.next = if self.syncs.is_empty() {
            // Every frame is a keyframe: start at the one shown then.
            let mut n = 1;
            while n < self.count && self.decode_time(n + 1) <= target {
                n += 1;
            }
            n
        } else {
            let before = self.syncs.partition_point(|&s| self.decode_time(s) <= target);
            self.syncs.get(before.saturating_sub(1)).copied().unwrap_or(1)
        };
    }
}

// ───────────────────────── WebM and Matroska ─────────────────────────

struct MkvDemux {
    bytes: Fetched,
    file: Option<MatroskaFile<Cursor<Fetched>>>,
    track: u64,
    codec: Codec,
    /// Milliseconds in one of the file's ticks.
    tick_ms: f64,
    /// Frames already read, waiting to be handed out (after a seek).
    held: VecDeque<Packet>,
    frame: Frame,
}

fn open_mkv(bytes: &Fetched) -> Result<Option<Source>, Unplayable> {
    let file = MatroskaFile::open(Cursor::new(bytes.clone())).map_err(|_| Unplayable::Broken)?;
    let videos: Vec<_> = file.tracks().iter().filter(|t| t.track_type() == TrackType::Video).collect();
    if videos.is_empty() {
        return Ok(None);
    }
    let Some((track, codec)) = videos.iter().find_map(|t| match t.codec_id() {
        "V_VP8" => Some((t.track_number().get(), Codec::Vp8)),
        "V_VP9" => Some((t.track_number().get(), Codec::Vp9)),
        _ => None,
    }) else {
        return Err(Unplayable::Kind);
    };
    let tick_ms = file.info().timestamp_scale().get() as f64 / 1_000_000.0;
    let said = file.info().duration().map_or(0.0, |d| d * tick_ms);
    let mut demux = MkvDemux {
        bytes: bytes.clone(),
        file: Some(file),
        track,
        codec,
        tick_ms,
        held: VecDeque::new(),
        frame: Frame::default(),
    };
    // A recording made in a browser doesn't say how long it is: its last frame does.
    let duration_ms = if said > 0.0 {
        said as u64
    } else {
        let mut last = 0.0f64;
        while let Some(p) = demux.next() {
            last = last.max(p.ms);
        }
        demux.seek(0.0);
        last as u64
    };
    Ok(Some(Source { demux: Box::new(demux), kind: Kind::Vpx(codec), duration_ms }))
}

impl MkvDemux {
    fn is_key(&self, frame: &Frame) -> bool {
        frame.is_keyframe.unwrap_or_else(|| match self.codec {
            Codec::Vp8 => vp8::is_keyframe(&frame.data),
            // VP9's frame type is the bit after its marker, profile and show-existing bits.
            Codec::Vp9 => {
                let b = frame.data.first().copied().unwrap_or(0xff);
                let profile = ((b >> 5) & 1) | (((b >> 4) & 1) << 1);
                let at = if profile == 3 { 1 } else { 0 };
                let show_existing = (b >> (3 - at)) & 1;
                show_existing == 0 && (b >> (2 - at)) & 1 == 0
            }
        })
    }

    fn read(&mut self) -> Option<Packet> {
        let file = self.file.as_mut()?;
        loop {
            match file.next_frame(&mut self.frame) {
                Ok(true) if self.frame.track == self.track => {
                    let frame = std::mem::take(&mut self.frame);
                    let key = self.is_key(&frame);
                    return Some(Packet { ms: frame.timestamp as f64 * self.tick_ms, key, data: frame.data });
                }
                Ok(true) => {}
                _ => return None,
            }
        }
    }
}

impl Demux for MkvDemux {
    fn next(&mut self) -> Option<Packet> {
        self.held.pop_front().or_else(|| self.read())
    }

    /// From the start again, reading on to `ms` and keeping the frames from
    /// the last keyframe before it: files without cues seek no faster, and
    /// this never lands past a keyframe.
    fn seek(&mut self, ms: f64) {
        self.held.clear();
        self.file = MatroskaFile::open(Cursor::new(self.bytes.clone())).ok();
        while let Some(p) = self.read() {
            let past = p.ms > ms;
            if p.key && !past {
                self.held.clear();
            }
            self.held.push_back(p);
            if past {
                break;
            }
        }
        // Without a keyframe before it, the first one after it.
        while self.held.front().is_some_and(|p| !p.key) && self.held.len() > 1 {
            self.held.pop_front();
        }
    }
}

// ───────────────────────── Decoding ─────────────────────────

enum Decoder {
    H264 {
        decoder: Box<openh264::decoder::Decoder>,
        sps: Vec<Vec<u8>>,
        pps: Vec<Vec<u8>>,
        length_size: usize,
        /// The parameter sets go before the first frame (and again after a jump).
        primed: bool,
        /// When the frames handed in are shown, earliest first: H.264 hands
        /// pictures back in that order, not the order they went in.
        waiting: BinaryHeap<Reverse<i64>>,
        annex_b: Vec<u8>,
    },
    Vpx(vp8::Decoder),
}

impl Decoder {
    fn new(kind: &Kind) -> Option<Self> {
        Some(match kind {
            Kind::H264 { sps, pps, length_size } => {
                let config =
                    openh264::decoder::DecoderConfig::new().flush_after_decode(openh264::decoder::Flush::NoFlush);
                let decoder =
                    openh264::decoder::Decoder::with_api_config(openh264::OpenH264API::from_source(), config).ok()?;
                Decoder::H264 {
                    decoder: Box::new(decoder),
                    sps: sps.clone(),
                    pps: pps.clone(),
                    length_size: *length_size,
                    primed: false,
                    waiting: BinaryHeap::new(),
                    annex_b: Vec::new(),
                }
            }
            Kind::Vpx(codec) => Decoder::Vpx(vp8::Decoder::of(*codec).ok()?),
        })
    }

    /// Decodes one frame, giving the picture that came out of it (BGRA in
    /// `out`) with when it's shown.
    fn decode(&mut self, packet: &Packet, out: &mut Vec<u8>) -> Option<(f64, u32, u32)> {
        match self {
            Decoder::Vpx(decoder) => {
                let (w, h) = decoder.decode(&packet.data, out)?;
                let (w, h) = shrink_bgra(out, w, h);
                Some((packet.ms, w, h))
            }
            Decoder::H264 { decoder, sps, pps, length_size, primed, waiting, annex_b } => {
                annex_b.clear();
                if !*primed {
                    for set in sps.iter().chain(pps.iter()) {
                        annex_b.extend_from_slice(&[0, 0, 0, 1]);
                        annex_b.extend_from_slice(set);
                    }
                    *primed = true;
                }
                to_annex_b(&packet.data, *length_size, annex_b);
                waiting.push(Reverse((packet.ms * 1000.0) as i64));
                // A stream that never gives a picture back shouldn't keep every time it was handed.
                while waiting.len() > 32 {
                    waiting.pop();
                }
                let picture = decoder.decode(annex_b).ok().flatten()?;
                let Reverse(us) = waiting.pop()?;
                let (w, h) = yuv_to_bgra(&picture, out);
                Some((us as f64 / 1000.0, w, h))
            }
        }
    }

    /// What's left inside once the frames run out, with when each is shown.
    fn flush(&mut self) -> Vec<(f64, Vec<u8>, u32, u32)> {
        let Decoder::H264 { decoder, waiting, .. } = self else { return Vec::new() };
        let Ok(pictures) = decoder.flush_remaining() else { return Vec::new() };
        let mut out = Vec::new();
        for picture in pictures {
            let Some(Reverse(us)) = waiting.pop() else { break };
            let mut bgra = Vec::new();
            let (w, h) = yuv_to_bgra(&picture, &mut bgra);
            out.push((us as f64 / 1000.0, bgra, w, h));
        }
        out
    }
}

/// MP4's NAL units, each after its length, as the start-code stream OpenH264 reads.
fn to_annex_b(mut data: &[u8], length_size: usize, out: &mut Vec<u8>) {
    while data.len() > length_size {
        let len = data[..length_size].iter().fold(0usize, |n, b| (n << 8) | usize::from(*b));
        data = &data[length_size..];
        if len == 0 || len > data.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[..len]);
        data = &data[len..];
    }
}

/// A decoded H.264 picture as BGRA, shrunk first when it's big.
fn yuv_to_bgra(picture: &openh264::decoder::DecodedYUV<'_>, out: &mut Vec<u8>) -> (u32, u32) {
    use openh264::formats::YUVSource as _;
    let (w, h) = picture.dimensions();
    let (ys, us, vs) = picture.strides();
    let k = shrink_factor(w, h);
    if k == 1 {
        vp8::to_bgra((picture.y(), ys), (picture.u(), us), (picture.v(), vs), w as u32, h as u32, false, out);
        return (w as u32, h as u32);
    }
    let (nw, nh) = (w / k, h / k);
    let (cw, ch) = (nw.div_ceil(2), nh.div_ceil(2));
    let y = shrink_plane(picture.y(), ys, w, h, k, nw, nh);
    let u = shrink_plane(picture.u(), us, w.div_ceil(2), h.div_ceil(2), k, cw, ch);
    let v = shrink_plane(picture.v(), vs, w.div_ceil(2), h.div_ceil(2), k, cw, ch);
    vp8::to_bgra((&y, nw), (&u, cw), (&v, cw), nw as u32, nh as u32, false, out);
    (nw as u32, nh as u32)
}

/// By how much a picture this size is shrunk, a whole number of times.
fn shrink_factor(w: usize, h: usize) -> usize {
    w.div_ceil(MOST_SHOWN.0).max(h.div_ceil(MOST_SHOWN.1)).max(1)
}

/// A plane averaged over `k` by `k` squares into `nw` by `nh`.
fn shrink_plane(src: &[u8], stride: usize, w: usize, h: usize, k: usize, nw: usize, nh: usize) -> Vec<u8> {
    let mut out = vec![0u8; nw * nh];
    for oy in 0..nh {
        for ox in 0..nw {
            let (mut sum, mut n) = (0u32, 0u32);
            for dy in 0..k {
                let y = (oy * k + dy).min(h.saturating_sub(1));
                let row = &src[y * stride..];
                for dx in 0..k {
                    let x = (ox * k + dx).min(w.saturating_sub(1));
                    sum += u32::from(row.get(x).copied().unwrap_or(0));
                    n += 1;
                }
            }
            out[oy * nw + ox] = (sum / n.max(1)) as u8;
        }
    }
    out
}

/// A BGRA picture shrunk in place when it's big, giving its new size.
fn shrink_bgra(bgra: &mut Vec<u8>, w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (w as usize, h as usize);
    let k = shrink_factor(w, h);
    if k == 1 {
        return (w as u32, h as u32);
    }
    let (nw, nh) = (w / k, h / k);
    let mut out = vec![0u8; nw * nh * 4];
    for oy in 0..nh {
        for ox in 0..nw {
            let mut sum = [0u32; 4];
            for dy in 0..k {
                let row = (oy * k + dy) * w * 4;
                for dx in 0..k {
                    let at = row + (ox * k + dx) * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += u32::from(bgra[at + c]);
                    }
                }
            }
            let at = (oy * nw + ox) * 4;
            for (c, s) in sum.iter().enumerate() {
                out[at + c] = (s / (k * k) as u32) as u8;
            }
        }
    }
    *bgra = out;
    (nw as u32, nh as u32)
}

// ───────────────────────── Playing ─────────────────────────

/// A decoded frame waiting for its time.
struct Ready {
    ms: f64,
    bgra: Vec<u8>,
    width: u32,
    height: u32,
}

/// Decodes the picture and hands each frame to the window when the clock
/// reaches it, following jumps, until the player goes.
pub(super) fn run(mut source: Source, shared: Arc<Shared>, has_sound: bool) {
    let Some(mut decoder) = Decoder::new(&source.kind) else {
        shared.picture_done.store(true, Ordering::Relaxed);
        shared.finish_if_through(has_sound, true);
        return;
    };
    let mut jumps = shared.jumps.load(Ordering::Relaxed);
    // Frames before this are decoded but not shown (after a jump).
    let mut from = 0.0f64;
    // The first frame after opening or a jump shows at once, even while paused.
    let mut first = true;
    let mut ready: Option<Ready> = None;
    let mut flushed: VecDeque<Ready> = VecDeque::new();
    let mut ended = false;
    while !shared.stop.load(Ordering::Relaxed) {
        let now_jumps = shared.jumps.load(Ordering::Relaxed);
        if now_jumps != jumps {
            jumps = now_jumps;
            from = shared.jump_to.load(Ordering::Relaxed) as f64;
            source.demux.seek(from);
            match Decoder::new(&source.kind) {
                Some(fresh) => decoder = fresh,
                None => break,
            }
            ready = None;
            flushed.clear();
            ended = false;
            first = true;
        }
        if ready.is_none() {
            if let Some(next) = flushed.pop_front() {
                ready = Some(next);
            } else if ended {
                shared.picture_done.store(true, Ordering::Relaxed);
                shared.finish_if_through(has_sound, true);
                std::thread::sleep(Duration::from_millis(20));
                continue;
            } else if let Some(packet) = source.demux.next() {
                let mut bgra = shared.spare.lock().pop().unwrap_or_default();
                match decoder.decode(&packet, &mut bgra) {
                    // A frame a little before the jump's place still shows if it's the one there.
                    Some((ms, width, height)) if ms + 40.0 >= from => ready = Some(Ready { ms, bgra, width, height }),
                    _ => {
                        let mut spare = shared.spare.lock();
                        if spare.len() < 3 {
                            spare.push(bgra);
                        }
                    }
                }
            } else {
                ended = true;
                flushed.extend(
                    decoder
                        .flush()
                        .into_iter()
                        .filter(|(ms, ..)| ms + 40.0 >= from)
                        .map(|(ms, bgra, width, height)| Ready { ms, bgra, width, height }),
                );
            }
            continue;
        }
        let Some(frame) = ready.as_ref() else { continue };
        let now = shared.now();
        if first || frame.ms <= now {
            first = false;
            if let Some(frame) = ready.take() {
                let picture = Picture { bgra: frame.bgra, width: frame.width, height: frame.height };
                if let Some(old) = shared.picture.lock().replace(picture) {
                    let mut spare = shared.spare.lock();
                    if spare.len() < 3 {
                        spare.push(old.bgra);
                    }
                }
            }
            continue;
        }
        let wait = (frame.ms - now).clamp(1.0, 10.0);
        std::thread::sleep(Duration::from_secs_f64(wait / 1000.0));
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Every frame decoded from the start (or from a jump to `from`), as the player would show them.
    pub(in crate::core::media) fn decode_all(mut source: Source, from: Option<f64>) -> Vec<(f64, u32, u32)> {
        let mut decoder = Decoder::new(&source.kind).unwrap();
        if let Some(from) = from {
            source.demux.seek(from);
        }
        let from = from.unwrap_or(0.0);
        let (mut out, mut bgra) = (Vec::new(), Vec::new());
        while let Some(packet) = source.demux.next() {
            if let Some((ms, w, h)) = decoder.decode(&packet, &mut bgra) {
                assert_eq!(bgra.len(), (w * h * 4) as usize);
                out.push((ms, w, h));
            }
        }
        out.extend(decoder.flush().into_iter().map(|(ms, _, w, h)| (ms, w, h)));
        out.retain(|(ms, ..)| ms + 40.0 >= from);
        out
    }

    #[test]
    fn big_frames_are_shrunk_whole_times() {
        assert_eq!(shrink_factor(640, 480), 1);
        assert_eq!(shrink_factor(1920, 1080), 2);
        assert_eq!(shrink_factor(3840, 2160), 4);
        let mut bgra = vec![200u8; 1920 * 1080 * 4];
        assert_eq!(shrink_bgra(&mut bgra, 1920, 1080), (960, 540));
        assert_eq!(bgra.len(), 960 * 540 * 4);
        assert!(bgra.iter().all(|&b| b == 200));
    }

    #[test]
    fn length_prefixed_units_get_start_codes() {
        let mut out = Vec::new();
        to_annex_b(&[0, 0, 0, 2, 0x65, 0x88, 0, 0, 0, 1, 0x41], 4, &mut out);
        assert_eq!(out, [0, 0, 0, 1, 0x65, 0x88, 0, 0, 0, 1, 0x41]);
        // A length running past the end stops there.
        out.clear();
        to_annex_b(&[0, 0, 0, 9, 0x65], 4, &mut out);
        assert!(out.is_empty());
    }
}
