//! The microphone and speakers, through cpal: the system's default devices
//! at whatever rate and channels they run at, turned into the call's 48 kHz
//! mono on a thread of their own (cpal's streams can't move between threads
//! on every system).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use parking_lot::Mutex;

use super::sound::{FRAME, Pipe, RATE, Resampler};

/// What went wrong with a device, said for the call bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    NoMicrophone,
    NoSpeakers,
}

/// Opens and closes the microphone from anywhere, at once: closed while
/// muted or deafened, so the system's "microphone in use" light goes off too.
#[derive(Clone)]
pub struct Listener {
    /// What the sound thread goes by.
    listening: Arc<AtomicBool>,
    /// What was last asked for, so asking again changes nothing (a failed
    /// open isn't retried until the next unmute).
    wanted: Arc<Mutex<bool>>,
    thread: Option<std::thread::Thread>,
}

impl Listener {
    /// One with no sound thread behind it: for calls without devices (the tests).
    pub fn detached(on: bool) -> Self {
        Self { listening: Arc::new(AtomicBool::new(on)), wanted: Arc::new(Mutex::new(on)), thread: None }
    }

    pub fn listen(&self, on: bool) {
        // Both change together, so two calls at once can't leave them disagreeing.
        let mut wanted = self.wanted.lock();
        if *wanted == on {
            return;
        }
        *wanted = on;
        self.listening.store(on, Ordering::Relaxed);
        drop(wanted);
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
    }

    /// Whether the microphone is meant to be open.
    pub fn is_listening(&self) -> bool {
        *self.wanted.lock()
    }
}

/// The devices a call uses, open until this is dropped.
pub struct Devices {
    stop: Arc<AtomicBool>,
    listener: Listener,
    thread: Option<JoinHandle<()>>,
    trouble: Arc<Mutex<Vec<Trouble>>>,
}

impl Devices {
    /// Plays `speakers`, and opens the default microphone into `microphone`
    /// when `listening`. A device that can't open is reported, and the call
    /// goes on without it.
    pub fn open(microphone: Arc<Pipe>, speakers: Arc<Pipe>, listening: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let listening = Arc::new(AtomicBool::new(listening));
        let trouble = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let (stop, listening, trouble) = (stop.clone(), listening.clone(), trouble.clone());
            std::thread::Builder::new()
                .name("fuwa-sound".into())
                .spawn(move || run(microphone, speakers, stop, listening, trouble))
                .ok()
        };
        let wanted = Arc::new(Mutex::new(listening.load(Ordering::Relaxed)));
        let listener = Listener { listening, wanted, thread: thread.as_ref().map(|t| t.thread().clone()) };
        Self { stop, listener, thread, trouble }
    }

    pub fn listener(&self) -> Listener {
        self.listener.clone()
    }

    /// What isn't working, if anything.
    pub fn trouble(&self) -> Vec<Trouble> {
        self.trouble.lock().clone()
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

fn run(
    microphone: Arc<Pipe>,
    speakers: Arc<Pipe>,
    stop: Arc<AtomicBool>,
    listening: Arc<AtomicBool>,
    trouble: Arc<Mutex<Vec<Trouble>>>,
) {
    let report = |what: Trouble, on: bool| {
        let mut list = trouble.lock();
        list.retain(|t| *t != what);
        if on {
            list.push(what);
        }
    };
    let open_input = || {
        let device = crate::core::sounds::picked_input()?;
        let config = device.default_input_config().ok()?;
        let stream = match config.sample_format() {
            SampleFormat::F32 => input_stream::<f32>(&device, config.config(), microphone.clone()),
            SampleFormat::I16 => input_stream::<i16>(&device, config.config(), microphone.clone()),
            SampleFormat::U16 => input_stream::<u16>(&device, config.config(), microphone.clone()),
            SampleFormat::I32 => input_stream::<i32>(&device, config.config(), microphone.clone()),
            _ => None,
        }?;
        stream.play().ok()?;
        Some(stream)
    };
    let output = crate::core::sounds::picked_output().and_then(|device| {
        let config = device.default_output_config().ok()?;
        let stream = match config.sample_format() {
            SampleFormat::F32 => output_stream::<f32>(&device, config.config(), speakers.clone()),
            SampleFormat::I16 => output_stream::<i16>(&device, config.config(), speakers.clone()),
            SampleFormat::U16 => output_stream::<u16>(&device, config.config(), speakers.clone()),
            SampleFormat::I32 => output_stream::<i32>(&device, config.config(), speakers.clone()),
            _ => None,
        }?;
        stream.play().ok()?;
        Some(stream)
    });
    report(Trouble::NoSpeakers, output.is_none());
    let mut input = None;
    while !stop.load(Ordering::Relaxed) {
        let want = listening.load(Ordering::Relaxed);
        if want && input.is_none() {
            // Nothing stale from before a mute goes out after it.
            microphone.clear();
            input = open_input();
            report(Trouble::NoMicrophone, input.is_none());
            if input.is_none() {
                // Tried once per unmute; not again every few moments.
                listening.store(false, Ordering::Relaxed);
            }
        } else if !want && input.is_some() {
            input = None;
        }
        std::thread::park_timeout(Duration::from_millis(250));
    }
    drop((input, output));
}

fn input_stream<T>(device: &cpal::Device, config: StreamConfig, pipe: Arc<Pipe>) -> Option<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let mut resampler = Resampler::new(config.sample_rate, RATE);
    let (mut mono, mut out) = (Vec::with_capacity(FRAME), Vec::with_capacity(FRAME));
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                mono.clear();
                mono.extend(
                    data.chunks(channels).map(|c| {
                        c.iter().map(|s| <f32 as FromSample<T>>::from_sample_(*s)).sum::<f32>() / c.len() as f32
                    }),
                );
                out.clear();
                resampler.process(&mono, &mut out);
                pipe.push(&out);
            },
            // Never anything about the device in the logs: its name can be a person's.
            |_| tracing::debug!("the microphone stopped working"),
            None,
        )
        .ok()
}

fn output_stream<T>(device: &cpal::Device, config: StreamConfig, pipe: Arc<Pipe>) -> Option<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let mut resampler = Resampler::new(RATE, config.sample_rate);
    let mut ready: std::collections::VecDeque<f32> = Default::default();
    let (mut block, mut out) = (vec![0.0f32; FRAME / 4], Vec::with_capacity(FRAME));
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                let frames = data.len() / channels;
                while ready.len() < frames {
                    pipe.pull(&mut block);
                    out.clear();
                    resampler.process(&block, &mut out);
                    ready.extend(&out);
                }
                for frame in data.chunks_mut(channels) {
                    let sample = T::from_sample(ready.pop_front().unwrap_or(0.0));
                    frame.fill(sample);
                }
            },
            |_| tracing::debug!("the speakers stopped working"),
            None,
        )
        .ok()
}
