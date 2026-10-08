//! This computer's sound on macOS, through ScreenCaptureKit (macOS 13 and
//! later): a stream on the main display that wants its sound, not its
//! picture, and leaves this process's own sound out. It asks for the same
//! permission as sharing the screen.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_core_audio_types::{AudioStreamBasicDescription, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved};
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};

use super::{Missing, Push};
use crate::core::voice::sound::RATE;

/// How long the system gets to answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and Output has no Drop.
    #[unsafe(super(NSObject))]
    #[name = "FuwaScreenSound"]
    #[ivars = Push]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Audio
                && let Some(mono) = mono_of(sample)
            {
                (self.ivars())(&mono);
            }
        }
    }
);

impl Output {
    fn new(push: Push) -> Retained<Self> {
        let this = Self::alloc().set_ivars(push);
        // SAFETY: NSObject's init, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// A buffer of what the system played, as 48 kHz mono floats.
fn mono_of(sample: &CMSampleBuffer) -> Option<Vec<f32>> {
    // SAFETY: reading an audio sample buffer ScreenCaptureKit handed over.
    let format = unsafe { sample.format_description() }?;
    let described = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&format) };
    // SAFETY: null, or a pointer into the format description, alive while it is.
    let AudioStreamBasicDescription { mSampleRate, mFormatFlags, mChannelsPerFrame, mBitsPerChannel, .. } =
        unsafe { described.as_ref() }.copied()?;
    if mSampleRate != f64::from(RATE) || mBitsPerChannel != 32 || mFormatFlags & kAudioFormatFlagIsFloat == 0 {
        return None;
    }
    let data = unsafe { sample.data_buffer() }?;
    let length = unsafe { data.data_length() };
    let mut samples = vec![0f32; length / 4];
    let into = std::ptr::NonNull::new(samples.as_mut_ptr().cast())?;
    // SAFETY: `into` has room for the bytes copied.
    if unsafe { data.copy_data_bytes(0, samples.len() * 4, into) } != 0 {
        return None;
    }
    let channels = mChannelsPerFrame.max(1) as usize;
    if channels == 1 {
        return Some(samples);
    }
    let frames = samples.len() / channels;
    let planar = mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
    let at = |frame: usize, channel: usize| if planar { channel * frames + frame } else { frame * channels + channel };
    Some((0..frames).map(|f| (0..channels).map(|c| samples[at(f, c)]).sum::<f32>() / channels as f32).collect())
}

/// ScreenCaptureKit's objects, kept and dropped from another thread.
struct Sendable<T>(T);
// SAFETY: SCStream and the output are only kept and stopped here, which
// ScreenCaptureKit allows from any thread.
unsafe impl<T> Send for Sendable<T> {}
unsafe impl<T> Sync for Sendable<T> {}

pub struct Capture {
    stream: Sendable<Retained<SCStream>>,
    _output: Sendable<Retained<Output>>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: stopping a stream that was started.
        unsafe { self.stream.0.stopCaptureWithCompletionHandler(None) };
    }
}

/// Whether this macOS shares sound (13 and later).
pub fn possible() -> bool {
    let config = unsafe { SCStreamConfiguration::new() };
    // SAFETY: asking an object whether it answers a selector.
    unsafe { msg_send![&config, respondsToSelector: sel!(setExcludesCurrentProcessAudio:)] }
}

pub fn open(push: Push) -> Result<Capture, Missing> {
    if !possible() {
        return Err(Missing::Unsupported);
    }
    let filter = main_display()?;
    let config = unsafe { SCStreamConfiguration::new() };
    // SAFETY: setters of a configuration nothing else holds yet.
    unsafe {
        // The picture isn't wanted: as small and as seldom as it goes.
        config.setWidth(2);
        config.setHeight(2);
        config.setMinimumFrameInterval(CMTime { value: 1, timescale: 1, flags: CMTimeFlags::Valid, epoch: 0 });
        config.setCapturesAudio(true);
        config.setSampleRate(RATE as isize);
        config.setChannelCount(1);
        config.setExcludesCurrentProcessAudio(true);
    }
    // SAFETY: a new stream on that filter and configuration.
    let stream =
        unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter.0, &config, None) };
    let output = Output::new(push);
    let queue = DispatchQueue::new("fuwa.screen-sound", None);
    let handler = ProtocolObject::from_ref(&*output);
    // SAFETY: the output lives as long as the stream (both in Capture).
    unsafe {
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(handler, SCStreamOutputType::Audio, Some(&queue))
            .map_err(|_| Missing::Failed)?;
        // Pictures nobody takes would each be logged as dropped.
        let _ = stream.addStreamOutput_type_sampleHandlerQueue_error(handler, SCStreamOutputType::Screen, Some(&queue));
    }
    let (started_tx, started) = mpsc::channel();
    let done = RcBlock::new(move |error: *mut NSError| {
        let _ = started_tx.send(error.is_null());
    });
    // SAFETY: the block is kept until the answer comes.
    unsafe { stream.startCaptureWithCompletionHandler(Some(&done)) };
    match started.recv_timeout(ANSWER_WITHIN) {
        Ok(true) => Ok(Capture { stream: Sendable(stream), _output: Sendable(output) }),
        Ok(false) => Err(Missing::Blocked),
        Err(_) => Err(Missing::Failed),
    }
}

/// The main display, to hang the sound on: it's the whole system's either way.
fn main_display() -> Result<Sendable<Retained<SCContentFilter>>, Missing> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: null, or the content ScreenCaptureKit hands over for this call.
        let display = unsafe { content.as_ref() }.and_then(|content| unsafe { content.displays() }.firstObject());
        let filter = display.map(|display| {
            // SAFETY: a filter on a display it just listed.
            Sendable(unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new())
            })
        });
        let _ = tx.send(filter.ok_or(!error.is_null()));
    });
    // SAFETY: the block is kept until the answer comes.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    match rx.recv_timeout(ANSWER_WITHIN) {
        Ok(Ok(filter)) => Ok(filter),
        // An error here is the system saying no.
        Ok(Err(true)) => Err(Missing::Blocked),
        _ => Err(Missing::Failed),
    }
}
