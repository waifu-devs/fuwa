//! A shared screen's sound, going out (the web's `openScreen` with sound):
//! what this computer plays, without fuwa's own sound (the call, its cues),
//! so nobody hears the call back, as the browser's `restrictOwnAudio` does.
//! On Windows through WASAPI's process loopback, leaving this process out;
//! on macOS 13 and later through ScreenCaptureKit, which leaves it out
//! itself; on Linux through PulseAudio or PipeWire, every app's stream but
//! this one's, mixed here. 48 kHz mono, like the microphone, and Opus on
//! the wire tuned for music rather than voices: no noise suppression or
//! echo cancelling, which spoil it. With `FUWA_DESKTOP_FAKE_VIDEO=1` (or the
//! test pattern) a tone stands in.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::capture::FAKE_VIDEO;
use super::sound::{FRAME, Pipe, RATE};
use crate::core::i18n::t;

/// The most a screen's sound sends: what the web asks of the browser.
const BITRATE: i32 = 128_000;
/// The largest Opus packet that comes out of the encoder.
const MOST_PACKET: usize = 1275;

/// Why a screen's sound couldn't go out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// This system can't share its sound (too old, no sound server).
    Unsupported,
    /// The system's privacy settings keep it from fuwa.
    Blocked,
    Failed,
}

impl Missing {
    /// What to tell the person, who shares the picture without it.
    pub fn message(self) -> String {
        t(match self {
            Self::Unsupported if cfg!(target_os = "macos") => "desktop.calls.screenSound.needsMacos",
            Self::Unsupported if cfg!(windows) => "desktop.calls.screenSound.needsWindows",
            Self::Unsupported => "desktop.calls.screenSound.needsServer",
            Self::Blocked => "desktop.calls.screenSound.blocked",
            Self::Failed => "desktop.calls.screenSound.failed",
        })
    }

    /// For the anonymous report.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

/// Where the system hands its sound: 48 kHz mono, a block at a time.
type Push = Box<dyn Fn(&[f32]) + Send + Sync>;

/// The sound of this computer while a screen is shared with it. Dropping it stops.
pub struct ScreenSound {
    pipe: Arc<Pipe>,
    /// Whether it goes out: the share's sound button turns it off for everyone.
    on: AtomicBool,
    _capture: Box<dyn Send + Sync>,
}

/// Whether this computer can share its sound at all, as far as can be told
/// without trying: for the share dialog.
pub fn possible() -> bool {
    if std::env::var_os(FAKE_VIDEO).is_some() {
        return true;
    }
    #[cfg(target_os = "linux")]
    return linux::possible();
    #[cfg(target_os = "macos")]
    return macos::possible();
    #[cfg(windows)]
    return true;
    #[allow(unreachable_code)]
    false
}

/// Starts taking this computer's sound (a tone with `fake`). Blocks while
/// the system sets it up, a moment at most: run it off the window's thread.
pub fn open(fake: bool) -> Result<ScreenSound, Missing> {
    let pipe = Arc::new(Pipe::new(FRAME * 10, 0));
    let into = pipe.clone();
    let push: Push = Box::new(move |samples| into.push(samples));
    let capture: Box<dyn Send + Sync> = if fake || std::env::var_os(FAKE_VIDEO).is_some() {
        Box::new(tone(push))
    } else {
        #[cfg(target_os = "linux")]
        let capture = Box::new(linux::open(push)?);
        #[cfg(target_os = "macos")]
        let capture = Box::new(macos::open(push)?);
        #[cfg(windows)]
        let capture = Box::new(windows::open(push)?);
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let capture: Box<dyn Send + Sync> = {
            drop(push);
            return Err(Missing::Unsupported);
        };
        capture
    };
    Ok(ScreenSound { pipe, on: AtomicBool::new(true), _capture: capture })
}

impl ScreenSound {
    /// The next 20 ms, once there's a whole frame. A system running a touch
    /// faster than the call's clock would pile up delay: past 160 ms
    /// behind, the oldest goes.
    pub fn frame(&self) -> Option<[f32; FRAME]> {
        while self.pipe.len() > FRAME * 8 {
            let _ = self.pipe.frame();
        }
        self.pipe.frame()
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }
}

/// Opus for a screen's sound: music and everything else, at the web's rate.
pub struct Encoder(opus::Encoder);

impl Encoder {
    pub fn new() -> Result<Self, opus::Error> {
        let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Audio)?;
        encoder.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(5)?;
        Ok(Self(encoder))
    }

    pub fn encode(&mut self, frame: &[f32; FRAME]) -> Option<Vec<u8>> {
        let mut packet = vec![0u8; MOST_PACKET];
        let n = self.0.encode_float(frame, &mut packet).ok()?;
        packet.truncate(n);
        Some(packet)
    }
}

/// A soft 440 Hz tone, 20 ms at a time, until dropped: the test pattern's sound.
struct Tone(Arc<AtomicBool>);

impl Drop for Tone {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn tone(push: Push) -> Tone {
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    std::thread::spawn(move || {
        let mut n = 0u64;
        let mut next = Instant::now();
        let mut frame = [0.0f32; FRAME];
        while !stopping.load(Ordering::Relaxed) {
            for s in frame.iter_mut() {
                *s = (n as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.25;
                n += 1;
            }
            push(&frame);
            next += Duration::from_millis(20);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    });
    Tone(stop)
}
