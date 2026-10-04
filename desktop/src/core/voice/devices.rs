//! The microphone and speakers, through cpal: the system's default devices
//! at whatever rate and channels they run at, turned into the call's 48 kHz
//! mono on a thread of their own (cpal's streams can't move between threads
//! on every system).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use parking_lot::Mutex;

use super::sound::{FRAME, Pipe, RATE, Resampler};

/// What went wrong with a device, said for the call bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    NoMicrophone,
    NoSpeakers,
}

/// The devices a call uses, open until this is dropped.
pub struct Devices {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    trouble: Arc<Mutex<Vec<Trouble>>>,
}

impl Devices {
    /// Opens the default microphone into `microphone` and plays `speakers`.
    /// A device that can't open is reported, and the call goes on without it.
    pub fn open(microphone: Arc<Pipe>, speakers: Arc<Pipe>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let trouble = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let (stop, trouble) = (stop.clone(), trouble.clone());
            std::thread::Builder::new()
                .name("fuwa-sound".into())
                .spawn(move || run(microphone, speakers, stop, trouble))
                .ok()
        };
        Self { stop, thread, trouble }
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

fn run(microphone: Arc<Pipe>, speakers: Arc<Pipe>, stop: Arc<AtomicBool>, trouble: Arc<Mutex<Vec<Trouble>>>) {
    let host = cpal::default_host();
    let report = |what: Trouble| {
        let mut list = trouble.lock();
        if !list.contains(&what) {
            list.push(what);
        }
    };
    let input = host.default_input_device().and_then(|device| {
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
    });
    if input.is_none() {
        report(Trouble::NoMicrophone);
    }
    let output = host.default_output_device().and_then(|device| {
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
    if output.is_none() {
        report(Trouble::NoSpeakers);
    }
    while !stop.load(Ordering::Relaxed) {
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
