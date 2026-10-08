//! The sound of a call between the devices and the network: 48 kHz mono in
//! 20 ms frames, Opus on the wire. The microphone's frames are encoded and
//! sent; everyone else's are decoded, held a moment against the network's
//! jitter, and mixed into one stream for the speakers. Who is speaking comes
//! from how loud each frame is.

use std::collections::{HashMap, HashSet, VecDeque};

use parking_lot::Mutex;

/// The rate everything here runs at, as Opus and the web app do.
pub const RATE: u32 = 48_000;
/// 20 ms at [`RATE`]: one Opus frame.
pub const FRAME: usize = 960;
/// The most a person's sound may lag behind before the oldest is dropped.
const MOST_HELD: usize = FRAME * 10;
/// How much of someone's sound to hold before playing it, against jitter.
const HOLD_BEFORE_PLAYING: usize = FRAME * 2;
/// How loud a frame must be (RMS, 1.0 at full scale) to count as speaking:
/// about -36 dBFS, above a quiet room's hum.
const SPEAKING_LEVEL: f32 = 0.016;
/// How many quiet frames keep someone "speaking" between words (300 ms).
const SPEAKING_HOLD: u32 = 15;
/// The largest Opus packet that comes out of the encoder.
const MOST_PACKET: usize = 1275;

/// Samples on their way between a device's own thread and the call:
/// 48 kHz mono, the oldest dropped when more than `most` are waiting.
pub struct Pipe {
    samples: Mutex<VecDeque<f32>>,
    most: usize,
    /// A speaker pipe waits for this much before it plays again after
    /// running dry, so a late frame doesn't come out in crackles.
    refill: usize,
    playing: Mutex<bool>,
}

impl Pipe {
    pub fn new(most: usize, refill: usize) -> Self {
        Self { samples: Mutex::new(VecDeque::with_capacity(most)), most, refill, playing: Mutex::new(false) }
    }

    /// The microphone's: up to 200 ms waiting.
    pub fn microphone() -> Self {
        Self::new(FRAME * 10, 0)
    }

    /// The speakers': up to 200 ms waiting, 40 ms gathered before playing.
    pub fn speakers() -> Self {
        Self::new(FRAME * 10, FRAME * 2)
    }

    pub fn push(&self, samples: &[f32]) {
        let mut queue = self.samples.lock();
        queue.extend(samples);
        let over = queue.len().saturating_sub(self.most);
        queue.drain(..over);
    }

    /// Fills `out` with what's waiting, silence for the rest; false when it's all silence.
    pub fn pull(&self, out: &mut [f32]) -> bool {
        let mut queue = self.samples.lock();
        let mut playing = self.playing.lock();
        if !*playing && queue.len() < self.refill.max(1) {
            out.fill(0.0);
            return false;
        }
        *playing = true;
        let n = out.len().min(queue.len());
        for (slot, sample) in out.iter_mut().zip(queue.drain(..n)) {
            *slot = sample;
        }
        out[n..].fill(0.0);
        if queue.is_empty() {
            *playing = false;
        }
        true
    }

    /// Takes exactly one frame, if a whole one is waiting.
    pub fn frame(&self) -> Option<[f32; FRAME]> {
        let mut queue = self.samples.lock();
        if queue.len() < FRAME {
            return None;
        }
        let mut out = [0.0; FRAME];
        for (slot, sample) in out.iter_mut().zip(queue.drain(..FRAME)) {
            *slot = sample;
        }
        Some(out)
    }

    pub fn len(&self) -> usize {
        self.samples.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.samples.lock().clear();
        *self.playing.lock() = false;
    }
}

/// Changes a stream's rate, a block at a time, by straight lines between
/// samples: plenty for voices, and nothing to tune.
pub struct Resampler {
    step: f64,
    /// Where the next output sample falls, counted from `last`.
    at: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { step: f64::from(from) / f64::from(to), at: 1.0, last: 0.0 }
    }

    pub fn is_identity(&self) -> bool {
        self.step == 1.0
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        // Position 0 is `last` (the end of the previous block), 1 the first new sample.
        let len = input.len() as f64;
        while self.at <= len {
            let i = self.at.floor() as usize;
            let frac = (self.at - self.at.floor()) as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = if i < input.len() { input[i] } else { a };
            out.push(a + (b - a) * frac);
            self.at += self.step;
        }
        self.at -= len;
        if let Some(&end) = input.last() {
            self.last = end;
        }
    }
}

/// How loud a frame is: its RMS, 1.0 at full scale.
pub fn level(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Whether someone is speaking, kept on through the gaps between words.
#[derive(Default)]
pub struct Speaking {
    quiet_for: u32,
    on: bool,
}

impl Speaking {
    pub fn hear(&mut self, level: f32) -> bool {
        if level >= SPEAKING_LEVEL {
            self.quiet_for = 0;
            self.on = true;
        } else if self.on {
            self.quiet_for += 1;
            self.on = self.quiet_for <= SPEAKING_HOLD;
        }
        self.on
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    pub fn stop(&mut self) {
        *self = Self::default();
    }
}

/// The microphone's end: 20 ms frames in, Opus packets out.
pub struct Microphone {
    encoder: opus::Encoder,
    pub speaking: Speaking,
}

impl Microphone {
    pub fn new() -> Result<Self, opus::Error> {
        let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)?;
        // As browsers send voice: in-band error correction for lost packets.
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(5)?;
        Ok(Self { encoder, speaking: Speaking::default() })
    }

    pub fn encode(&mut self, frame: &[f32; FRAME]) -> Option<Vec<u8>> {
        self.speaking.hear(level(frame));
        let mut packet = vec![0u8; MOST_PACKET];
        let n = self.encoder.encode_float(frame, &mut packet).ok()?;
        packet.truncate(n);
        Some(packet)
    }
}

/// One person's sound, waiting to be mixed.
struct Voice {
    decoder: opus::Decoder,
    waiting: VecDeque<f32>,
    playing: bool,
    speaking: Speaking,
}

/// Everyone else in the call, mixed into one stream.
#[derive(Default)]
pub struct Mixer {
    voices: HashMap<String, Voice>,
    /// How loud each person is for you (1 is as they sent it), by stream; missing is 1.
    gains: HashMap<String, f32>,
}

impl Mixer {
    /// A packet of someone's sound arrived.
    pub fn hear(&mut self, who: &str, packet: &[u8]) {
        if !self.voices.contains_key(who) {
            if self.voices.len() >= super::link::MOST_STREAMS {
                return;
            }
            // Decoded to mono whatever was sent: a stereo screen's sound too.
            let Ok(decoder) = opus::Decoder::new(RATE, opus::Channels::Mono) else { return };
            let voice = Voice { decoder, waiting: VecDeque::new(), playing: false, speaking: Speaking::default() };
            self.voices.insert(who.to_string(), voice);
        }
        let Some(voice) = self.voices.get_mut(who) else { return };
        // Opus frames are at most 120 ms.
        let mut out = [0.0f32; FRAME * 6];
        let Ok(n) = voice.decoder.decode_float(packet, &mut out, false) else { return };
        voice.waiting.extend(&out[..n]);
        let over = voice.waiting.len().saturating_sub(MOST_HELD);
        voice.waiting.drain(..over);
    }

    /// Someone left: their sound goes with them.
    pub fn forget(&mut self, who: &str) {
        self.voices.remove(who);
    }

    pub fn clear(&mut self) {
        self.voices.clear();
    }

    /// How loud each person is for you, by account id (the web's per-person volume).
    pub fn set_gains(&mut self, gains: &HashMap<String, f32>) {
        if &self.gains != gains {
            self.gains = gains.clone();
        }
    }

    /// The next 20 ms of everyone together, and who among them is speaking.
    pub fn mix(&mut self, out: &mut [f32; FRAME]) -> HashSet<String> {
        out.fill(0.0);
        let mut speaking = HashSet::new();
        for (who, voice) in &mut self.voices {
            if !voice.playing && voice.waiting.len() >= HOLD_BEFORE_PLAYING {
                voice.playing = true;
            }
            if !voice.playing {
                if voice.speaking.hear(0.0) {
                    speaking.insert(who.clone());
                }
                continue;
            }
            let n = FRAME.min(voice.waiting.len());
            let mut sum = 0.0;
            let gain = self.gains.get(who).copied().unwrap_or(1.0);
            for (slot, sample) in out.iter_mut().zip(voice.waiting.drain(..n)) {
                *slot += sample * gain;
                sum += sample * sample;
            }
            if voice.waiting.is_empty() {
                // Ran dry: gather a little again before playing on.
                voice.playing = false;
            }
            if voice.speaking.hear((sum / FRAME as f32).sqrt()) {
                speaking.insert(who.clone());
            }
        }
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
        speaking
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, frames: usize, loudness: f32) -> Vec<[f32; FRAME]> {
        (0..frames)
            .map(|f| {
                std::array::from_fn(|i| {
                    let t = (f * FRAME + i) as f32 / RATE as f32;
                    (t * hz * std::f32::consts::TAU).sin() * loudness
                })
            })
            .collect()
    }

    #[test]
    fn a_voice_comes_through_opus_and_the_mixer() {
        let mut microphone = Microphone::new().unwrap();
        let mut mixer = Mixer::default();
        let mut heard = vec![];
        for frame in tone(440.0, 25, 0.3) {
            let packet = microphone.encode(&frame).unwrap();
            assert!(!packet.is_empty() && packet.len() < 400, "a voice frame is small: {}", packet.len());
            mixer.hear("bob", &packet);
            let mut out = [0.0; FRAME];
            let speaking = mixer.mix(&mut out);
            heard.push((level(&out), speaking.contains("bob")));
        }
        assert!(microphone.speaking.on, "a 440 Hz tone at -10 dBFS is speaking");
        // Nothing plays until 40 ms are held, then it plays at about the level sent.
        assert_eq!(heard[0], (0.0, false));
        let (loudness, speaking) = heard[20];
        assert!((0.1..0.35).contains(&loudness) && speaking, "{loudness}");
    }

    #[test]
    fn people_are_mixed_together_and_forgotten_when_they_go() {
        let (mut a, mut b) = (Microphone::new().unwrap(), Microphone::new().unwrap());
        let mut mixer = Mixer::default();
        let mut out = [0.0; FRAME];
        let mut speaking = HashSet::new();
        for (x, y) in tone(300.0, 10, 0.2).iter().zip(tone(500.0, 10, 0.2).iter()) {
            mixer.hear("a", &a.encode(x).unwrap());
            mixer.hear("b", &b.encode(y).unwrap());
            speaking = mixer.mix(&mut out);
        }
        assert_eq!(speaking, HashSet::from(["a".to_string(), "b".to_string()]));
        mixer.forget("a");
        assert_eq!(mixer.voices.len(), 1);
    }

    #[test]
    fn someone_keeps_speaking_between_words_then_stops() {
        let mut speaking = Speaking::default();
        assert!(!speaking.hear(0.001));
        assert!(speaking.hear(0.1));
        for _ in 0..SPEAKING_HOLD {
            assert!(speaking.hear(0.0));
        }
        assert!(!speaking.hear(0.0));
    }

    #[test]
    fn a_long_wait_drops_the_oldest_sound() {
        let mut microphone = Microphone::new().unwrap();
        let mut mixer = Mixer::default();
        for frame in tone(440.0, 40, 0.3) {
            mixer.hear("bob", &microphone.encode(&frame).unwrap());
        }
        assert_eq!(mixer.voices["bob"].waiting.len(), MOST_HELD);
    }

    #[test]
    fn rates_change_without_losing_or_making_up_time() {
        for (from, to) in [(44_100, RATE), (RATE, 44_100), (16_000, RATE), (RATE, RATE)] {
            let mut resampler = Resampler::new(from, to);
            let input: Vec<f32> =
                (0..from).map(|i| (i as f32 / from as f32 * 100.0 * std::f32::consts::TAU).sin()).collect();
            let mut out = vec![];
            // In odd blocks, as devices hand them over.
            for block in input.chunks(441) {
                resampler.process(block, &mut out);
            }
            let want = to as i64;
            assert!((out.len() as i64 - want).abs() <= 2, "{from} -> {to}: {} samples", out.len());
            // A 100 Hz tone stays one: same loudness.
            assert!((level(&out) - level(&input)).abs() < 0.01, "{from} -> {to}");
        }
    }

    #[test]
    fn the_speakers_wait_for_enough_then_play_until_dry() {
        let pipe = Pipe::speakers();
        let mut out = [1.0; 480];
        pipe.push(&[0.5; FRAME]);
        assert!(!pipe.pull(&mut out), "20 ms isn't enough to start on");
        assert_eq!(out[0], 0.0);
        pipe.push(&[0.5; FRAME]);
        assert!(pipe.pull(&mut out));
        assert_eq!(out[479], 0.5);
        // Once playing, what's there plays even when it's less than the refill.
        for _ in 0..3 {
            assert!(pipe.pull(&mut out));
        }
        assert!(!pipe.pull(&mut out));
        pipe.push(&[0.5; FRAME * 12]);
        assert_eq!(pipe.len(), FRAME * 10, "the oldest goes past 200 ms");
    }
}
