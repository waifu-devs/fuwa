//! This computer's sound on Linux, through PulseAudio or PipeWire (its
//! PulseAudio server): one record stream on each app's sound but this
//! process's own (a sink input's monitor, `direct_on_input`), mixed here.
//! The list of what's playing is read again every second, over a second
//! connection, since the client has no call for it.

use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::executor::block_on;
use parking_lot::Mutex;
use pulseaudio::protocol::stream::{BufferAttr, StreamFlags};
use pulseaudio::protocol::{
    self, AuthParams, AuthReply, ChannelMap, Command, Prop, Props, RecordStreamParams, SampleFormat, SampleSpec,
    SetClientNameReply, SinkInfo, SinkInputInfoList,
};
use pulseaudio::{Client, RecordStream};

use super::{Missing, Push};
use crate::core::voice::sound::{FRAME, RATE};

/// How often the list of what's playing is read again.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// How far an app's sound may lag behind before the oldest goes.
const MOST_HELD: usize = FRAME * 10;
/// Gathered before an app's sound is mixed in, against the bursts it comes in.
const HOLD: usize = FRAME * 2;
const NAME: &CStr = c"fuwa";

pub struct Capture(Arc<AtomicBool>);

impl Drop for Capture {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub fn possible() -> bool {
    pulseaudio::socket_path_from_env().is_some()
}

pub fn open(push: Push) -> Result<Capture, Missing> {
    let socket = pulseaudio::socket_path_from_env().ok_or(Missing::Unsupported)?;
    let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok());
    let mut lister = Lister::connect(&socket, cookie.clone()).map_err(|_| Missing::Unsupported)?;
    // Fails here rather than later when the server can't list what's playing.
    lister.playing().map_err(|_| Missing::Failed)?;
    let stream = UnixStream::connect(&socket).map_err(|_| Missing::Unsupported)?;
    let client = Client::new_unix(NAME, stream, cookie).map_err(|_| Missing::Failed)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    std::thread::Builder::new()
        .name("fuwa-screen-sound".into())
        .spawn(move || run(lister, client, &stopping, &push))
        .map_err(|_| Missing::Failed)?;
    Ok(Capture(stop))
}

/// The connection that asks what's playing.
struct Lister {
    socket: BufReader<UnixStream>,
    version: u16,
    seq: u32,
}

/// Something playing that isn't fuwa: its sink input, and the monitor of the sink it plays on.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Playing {
    input: u32,
    monitor: u32,
}

impl Lister {
    fn connect(path: &Path, cookie: Option<Vec<u8>>) -> Result<Self, protocol::ProtocolError> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut this = Self { socket: BufReader::new(stream), version: protocol::MAX_VERSION, seq: 0 };
        let auth = AuthParams {
            version: protocol::MAX_VERSION,
            supports_shm: false,
            supports_memfd: false,
            cookie: cookie.unwrap_or_default(),
        };
        let reply: AuthReply = this.ask(Command::Auth(auth))?;
        this.version = this.version.min(reply.version);
        let mut props = Props::new();
        props.set(Prop::ApplicationName, NAME);
        let _: SetClientNameReply = this.ask(Command::SetClientName(props))?;
        Ok(this)
    }

    fn ask<R: protocol::CommandReply>(&mut self, command: Command) -> Result<R, protocol::ProtocolError> {
        self.seq += 1;
        protocol::write_command_message(self.socket.get_mut(), self.seq, &command, self.version)?;
        let (seq, reply) = protocol::read_reply_message(&mut self.socket, self.version)?;
        if seq != self.seq {
            return Err(protocol::ProtocolError::Invalid("a reply out of order".into()));
        }
        Ok(reply)
    }

    /// What's playing now, but this process's own sound. The sound server
    /// stamps each stream with the process it came from: the call and its
    /// cues go through ALSA to it, so they carry this one's.
    fn playing(&mut self) -> Result<Vec<Playing>, protocol::ProtocolError> {
        let me = std::process::id().to_string();
        let inputs: SinkInputInfoList = self.ask(Command::GetSinkInputInfoList)?;
        let mut monitors = HashMap::new();
        let mut out = Vec::new();
        for input in inputs {
            let pid = input.props.get(Prop::ApplicationProcessId).map(|v| v.strip_suffix(&[0]).unwrap_or(v));
            if pid == Some(me.as_bytes()) {
                continue;
            }
            let monitor = match monitors.get(&input.sink_index) {
                Some(monitor) => *monitor,
                None => {
                    let sink: SinkInfo = self.ask(Command::GetSinkInfo(protocol::GetSinkInfo {
                        index: Some(input.sink_index),
                        name: None,
                    }))?;
                    monitors.insert(input.sink_index, sink.monitor_source_index);
                    sink.monitor_source_index
                }
            };
            if let Some(monitor) = monitor {
                out.push(Playing { input: input.index, monitor });
            }
        }
        Ok(out)
    }
}

/// One app's sound as it comes in.
#[derive(Default)]
struct Heard {
    waiting: VecDeque<f32>,
    playing: bool,
}

fn run(mut lister: Lister, client: Client, stop: &AtomicBool, push: &Push) {
    let mut streams: HashMap<Playing, (RecordStream, Arc<Mutex<Heard>>)> = HashMap::new();
    let mut looked: Option<Instant> = None;
    let mut next = Instant::now();
    let mut frame = [0.0f32; FRAME];
    while !stop.load(Ordering::Relaxed) {
        if looked.is_none_or(|at| at.elapsed() >= LOOK_EVERY) {
            looked = Some(Instant::now());
            let Ok(now) = lister.playing() else { break };
            let gone: Vec<Playing> = streams.keys().filter(|p| !now.contains(p)).copied().collect();
            for p in gone {
                if let Some((stream, _)) = streams.remove(&p) {
                    let _ = block_on(stream.delete());
                }
            }
            for p in now {
                if streams.contains_key(&p) {
                    continue;
                }
                let heard = Arc::new(Mutex::new(Heard::default()));
                let into = heard.clone();
                let sink = move |bytes: &[u8]| {
                    let mut heard = into.lock();
                    heard.waiting.extend(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
                    let over = heard.waiting.len().saturating_sub(MOST_HELD);
                    heard.waiting.drain(..over);
                };
                if let Ok(stream) = block_on(client.create_record_stream(params(p), sink)) {
                    streams.insert(p, (stream, heard));
                }
            }
        }
        // 20 ms of every app together.
        frame.fill(0.0);
        let mut any = false;
        for (_, heard) in streams.values() {
            let mut heard = heard.lock();
            if !heard.playing && heard.waiting.len() >= HOLD {
                heard.playing = true;
            }
            if !heard.playing {
                continue;
            }
            any = true;
            let n = FRAME.min(heard.waiting.len());
            for (slot, sample) in frame.iter_mut().zip(heard.waiting.drain(..n)) {
                *slot += sample;
            }
            if heard.waiting.is_empty() {
                heard.playing = false;
            }
        }
        if any {
            for sample in frame.iter_mut() {
                *sample = sample.clamp(-1.0, 1.0);
            }
            push(&frame);
        }
        next += Duration::from_millis(20);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    for (_, (stream, _)) in streams.drain() {
        let _ = block_on(stream.delete());
    }
}

/// A record stream of one sink input's sound alone, as 48 kHz mono floats, 10 ms at a time.
fn params(p: Playing) -> RecordStreamParams {
    let mut props = Props::new();
    props.set(Prop::MediaName, c"Shared screen's sound");
    RecordStreamParams {
        sample_spec: SampleSpec { format: SampleFormat::Float32Le, channels: 1, sample_rate: RATE },
        channel_map: ChannelMap::mono(),
        source_index: Some(p.monitor),
        direct_on_input_index: Some(p.input),
        buffer_attr: BufferAttr { max_length: u32::MAX, fragment_size: RATE / 100 * 4, ..Default::default() },
        flags: StreamFlags { adjust_latency: true, ..Default::default() },
        props,
        ..Default::default()
    }
}
