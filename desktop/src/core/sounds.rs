//! fuwa's little sounds, made on the spot like the web's `lib/sounds.ts`
//! (the same notes, steps and lengths): a soft two-note blip for a message,
//! a brighter chime for a mention, and a rising sparkle when someone joins.
//! Each plays on its own short-lived output stream, on the speakers picked
//! in Voice & video (or the system's), at the volume set in Notifications.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Message,
    Mention,
    Join,
    Call,
    Ring,
}

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Triangle,
}

struct Tune {
    notes: &'static [f32],
    step: f32,
    length: f32,
    wave: Wave,
}

fn tune(sound: Sound) -> Tune {
    match sound {
        Sound::Message => Tune { notes: &[880.0, 1174.66], step: 0.07, length: 0.16, wave: Wave::Sine },
        Sound::Mention => Tune { notes: &[987.77, 1318.51, 1760.0], step: 0.075, length: 0.22, wave: Wave::Triangle },
        Sound::Join => Tune { notes: &[659.25, 830.61, 987.77, 1318.51], step: 0.06, length: 0.18, wave: Wave::Sine },
        Sound::Call => Tune { notes: &[587.33, 880.0], step: 0.08, length: 0.2, wave: Wave::Sine },
        Sound::Ring => {
            Tune { notes: &[783.99, 987.77, 1174.66, 987.77, 1174.66], step: 0.11, length: 0.3, wave: Wave::Triangle }
        }
    }
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

/// Plays a sound at `volume` (0 to 100) on the speakers named `device` ("" for the system's).
/// Whether it should play at all (the setting, streamer mode) is for the caller to decide.
pub fn play(sound: Sound, volume: u8, device: &str) {
    if volume == 0 || PLAYING.load(Ordering::Relaxed) >= 3 {
        return;
    }
    let device = device.to_owned();
    PLAYING.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("fuwa-sound".into()).spawn(move || {
        play_now(sound, f32::from(volume) / 100.0, &device);
        PLAYING.fetch_sub(1, Ordering::Relaxed);
    });
    if spawned.is_err() {
        PLAYING.fetch_sub(1, Ordering::Relaxed);
    }
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

fn play_now(sound: Sound, volume: f32, device: &str) {
    let Some(device) = output_device(device) else { return };
    let Ok(config) = device.default_output_config() else { return };
    let samples = Arc::new(render(&tune(sound), config.sample_rate(), volume));
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
        let samples = render(&tune(Sound::Mention), 48_000, 1.0);
        // Three notes 75 ms apart, each 220 ms long.
        assert!(samples.len() > 48_000 * 37 / 100);
        assert!(samples.iter().all(|s| s.abs() <= 0.18 * 3.0));
        assert!(samples.last().unwrap().abs() < 0.001);
        assert!(render(&tune(Sound::Message), 48_000, 0.0).iter().all(|s| *s == 0.0));
    }
}
