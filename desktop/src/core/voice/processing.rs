//! Cleaning up the microphone before it goes out, as browsers do for the web
//! app: echo cancellation, noise suppression and automatic gain, each its own
//! setting. WebRTC's own audio processing, through Sonora (a Rust port of it),
//! on 10 ms halves of each frame. Echo cancellation hears what the speakers
//! play (`heard`) and takes it back out of the microphone (`clean`).

use sonora::config::{AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression};
use sonora::{AudioProcessing, Config, StreamConfig};

use super::sound::{FRAME, RATE};

/// 10 ms at [`RATE`]: what the processing takes at a time.
const HALF: usize = FRAME / 2;

/// Which of the three are on: the Voice & audio settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub echo: bool,
    pub noise: bool,
    pub gain: bool,
}

impl Choice {
    pub fn of(prefs: &crate::core::config::Prefs) -> Self {
        Self { echo: prefs.echo_cancellation, noise: prefs.noise_suppression, gain: prefs.auto_gain_control }
    }

    fn any(self) -> bool {
        self.echo || self.noise || self.gain
    }

    fn config(self) -> Config {
        Config {
            // Takes out the rumble under a voice, as browsers do with any of these on.
            high_pass_filter: self.any().then(HighPassFilter::default),
            echo_canceller: self.echo.then(EchoCanceller::default),
            noise_suppression: self.noise.then(NoiseSuppression::default),
            gain_controller2: self
                .gain
                .then(|| GainController2 { adaptive_digital: Some(AdaptiveDigital::default()), ..Default::default() }),
            ..Default::default()
        }
    }
}

/// The microphone's processing for one call or recording.
pub struct Processing {
    choice: Choice,
    apm: AudioProcessing,
}

impl Processing {
    pub fn new(choice: Choice) -> Self {
        let stream = StreamConfig::new(RATE, 1);
        let apm =
            AudioProcessing::builder().config(choice.config()).capture_config(stream).render_config(stream).build();
        Self { choice, apm }
    }

    /// Follows the settings as they change.
    pub fn set(&mut self, choice: Choice) {
        if choice != self.choice {
            self.choice = choice;
            self.apm.apply_config(choice.config());
        }
    }

    /// A frame on its way to the speakers, for echo cancellation to know.
    pub fn heard(&mut self, frame: &[f32; FRAME]) {
        if !self.choice.echo {
            return;
        }
        let mut out = [0f32; HALF];
        for half in frame.as_chunks::<HALF>().0 {
            let _ = self.apm.process_render_f32(&[half], &mut [&mut out]);
        }
    }

    /// Cleans a microphone frame in place. `delay` is roughly how long a
    /// sound takes from `heard` to the microphone's frame here, which echo
    /// cancellation starts its own search from.
    pub fn clean(&mut self, frame: &mut [f32; FRAME], delay_ms: u32) {
        if !self.choice.any() {
            return;
        }
        if self.choice.echo {
            let _ = self.apm.set_stream_delay_ms(delay_ms.min(500) as i32);
        }
        let mut out = [0f32; HALF];
        for half in frame.as_chunks_mut::<HALF>().0 {
            if self.apm.process_capture_f32(&[half], &mut [&mut out]).is_ok() {
                half.copy_from_slice(&out);
            }
        }
    }
}

/// How many milliseconds `samples` at [`RATE`] last.
pub fn ms_of(samples: usize) -> u32 {
    (samples * 1000 / RATE as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, loudness: f32, start: usize) -> [f32; FRAME] {
        std::array::from_fn(|i| loudness * (2.0 * std::f32::consts::PI * hz * (start + i) as f32 / RATE as f32).sin())
    }

    fn rms(frame: &[f32]) -> f32 {
        (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
    }

    #[test]
    fn all_off_leaves_the_microphone_as_it_is() {
        let mut p = Processing::new(Choice { echo: false, noise: false, gain: false });
        let mut frame = tone(440.0, 0.3, 0);
        let before = frame;
        p.clean(&mut frame, 0);
        assert_eq!(frame, before);
    }

    #[test]
    fn echo_cancellation_takes_the_speakers_back_out() {
        let mut p = Processing::new(Choice { echo: true, noise: false, gain: false });
        let mut last = 1.0;
        let mut first = 0.0;
        // The speakers play a tone that the microphone hears straight back,
        // a frame late; after a few seconds it's mostly gone.
        let mut previous = [0f32; FRAME];
        for n in 0..250 {
            let played = tone(300.0 + (n % 7) as f32 * 40.0, 0.3, n * FRAME);
            p.heard(&played);
            let mut mic = previous.map(|s| s * 0.5);
            p.clean(&mut mic, 20);
            if n == 5 {
                first = rms(&previous) * 0.5;
            }
            last = rms(&mic);
            previous = played;
        }
        assert!(last < first * 0.5, "echo left: {last} of {first}");
    }

    /// Something like a quiet voice over a faint hiss: a 140 Hz buzz with
    /// its harmonics for half a second, then the hiss alone for half a second.
    fn voice(n: usize, seed: &mut u32) -> [f32; FRAME] {
        let on = if (n / 25).is_multiple_of(2) { 0.01 } else { 0.0 };
        std::array::from_fn(|i| {
            let t = (n * FRAME + i) as f32 / RATE as f32;
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let hiss = (*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            let buzz: f32 = (1..8).map(|h| (2.0 * std::f32::consts::PI * 140.0 * h as f32 * t).sin() / h as f32).sum();
            on * buzz + 0.002 * hiss
        })
    }

    /// How loud the voice and the hiss alone come out after five seconds.
    fn through(choice: Choice) -> (f32, f32) {
        let mut p = Processing::new(choice);
        let (mut seed, mut spoken, mut quiet) = (1, 0.0, 0.0);
        for n in 0..250 {
            let mut frame = voice(n, &mut seed);
            p.clean(&mut frame, 0);
            match n % 50 {
                20 => spoken = rms(&frame),
                45 => quiet = rms(&frame),
                _ => {}
            }
        }
        (spoken, quiet)
    }

    #[test]
    fn gain_brings_a_quiet_voice_up() {
        let (spoken, _) = through(Choice { echo: false, noise: false, gain: true });
        assert!(spoken > 0.03, "still quiet: {spoken}");
    }

    #[test]
    fn noise_suppression_softens_the_hiss_and_keeps_the_voice() {
        let (spoken, quiet) = through(Choice { echo: false, noise: true, gain: false });
        assert!(spoken > 0.007, "the voice went too: {spoken}");
        assert!(quiet < 0.0003, "hiss left: {quiet}");
    }

    #[test]
    fn settings_change_while_running() {
        let mut p = Processing::new(Choice { echo: true, noise: true, gain: true });
        let mut frame = tone(440.0, 0.3, 0);
        p.clean(&mut frame, 0);
        p.set(Choice { echo: false, noise: false, gain: false });
        let mut frame = tone(440.0, 0.3, 0);
        let before = frame;
        p.clean(&mut frame, 0);
        assert_eq!(frame, before);
    }
}
