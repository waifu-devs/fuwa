//! The media part: carries calls' sound and cameras over WebRTC, as an SFU
//! (selective forwarding unit). Everyone in a room sends their microphone
//! (and camera, when it's on) here once, and gets everyone else's back as
//! tracks of their own; nothing is mixed, decoded or kept. Direct-message
//! calls arrive end-to-end encrypted by the apps, and are passed on just the
//! same.
//!
//! Cameras come in up to three sizes at once (simulcast: "h", "m" and "l",
//! full, half and a quarter), and each viewer gets one: the size it asked for
//! over the data channel (by what fits where it shows that camera), or the
//! nearest one the camera is sending. Sizes switch on a keyframe, which the
//! media part asks the camera's app for, so the picture never breaks up.
//!
//! It's WebRTC through [str0m], which does no I/O of its own: one task here
//! owns every connection, reads the one UDP port (and TCP on the same port
//! number, for networks that block UDP, as ICE-TCP with RFC 4571 framing),
//! and feeds each packet to the connection it belongs to.
//!
//! Who may be in a room isn't decided here: the parts that keep who's in each
//! call (shards for voice channels, the directory for direct-message calls)
//! open and close connections, in one process directly ([`Sfu`]) or over
//! the cluster's `MediaService` when split (`cluster/media.rs`).
//!
//! Each app opens a data channel named "fuwa" in its first offer. After that,
//! offers and answers for people coming and going (and the notices
//! "replaced" and "restarting") go over it as JSON, so nothing else needs to
//! be arranged through the API once someone is in.
//!
//! Agents, bots and other programs can be in a room without WebRTC, through
//! a bridge ([`Sfu::bridge`]): it hears each person's frames of Opus as they
//! arrive, labelled with whose they are, and what the program says
//! ([`Sfu::speak`]) goes out to everyone as its own track, paced one 20 ms
//! frame at a time.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Once, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::{ChannelData, ChannelId};
use str0m::format::Codec;
use str0m::media::{
    Direction, Frequency, KeyframeRequest, KeyframeRequestKind, MediaData, MediaKind, MediaTime, Mid, Rid,
};
use str0m::net::{Protocol, Receive, TcpType};
use str0m::rtp::Extension;
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};

/// The UDP and TCP port calls use unless FUWA_MEDIA_PORT says otherwise.
pub const DEFAULT_PORT: u16 = 50000;
/// The data channel every app opens.
const CHANNEL_LABEL: &str = "fuwa";
/// How long someone being replaced (or everyone, when restarting) has to
/// hear why before their connection closes.
const PARTING: Duration = Duration::from_millis(400);
/// The most people in one room.
pub const MAX_ROOM: usize = 99;
/// The largest RFC 4571 frame, and the largest UDP datagram read.
const MAX_PACKET: usize = 2000;

/// How apps reach the media part: FUWA_MEDIA_PORT and FUWA_MEDIA_ADDRESSES.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaConfig {
    /// The UDP port it listens on, and the TCP port.
    pub port: u16,
    /// What it tells apps to send to. Empty: this machine's own address (the
    /// one it reaches the internet from), which is right on a LAN or a
    /// machine with a public address, but not behind NAT.
    pub addresses: Vec<Advertised>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, addresses: vec![] }
    }
}

/// One address apps are told about. Written `HOST`, `HOST:PORT`, or with
/// `udp/` or `tcp/` in front for only that protocol (a TCP proxy's address,
/// say). HOST is an IP address or a name looked up whenever someone joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertised {
    pub udp: bool,
    pub tcp: bool,
    pub host: String,
    /// Unset: the port listened on.
    pub port: Option<u16>,
}

impl Advertised {
    pub fn parse(value: &str) -> std::result::Result<Self, String> {
        let value = value.trim();
        let (udp, tcp, rest) = match value.split_once('/') {
            Some(("udp", rest)) => (true, false, rest),
            Some(("tcp", rest)) => (false, true, rest),
            Some(_) => return Err(format!("{value:?} must start with udp/, tcp/ or nothing")),
            None => (true, true, value),
        };
        let (host, port) = match rest.rsplit_once(':') {
            // An IPv6 address without a port has colons too.
            Some((host, port)) if !host.contains(':') || host.ends_with(']') => {
                let port = port.parse::<u16>().ok().filter(|p| *p > 0);
                (host, Some(port.ok_or_else(|| format!("{value:?} has a port that isn't 1 to 65535"))?))
            }
            _ => (rest, None),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() || host.contains(['/', ' ']) {
            return Err(format!("{value:?} isn't an address"));
        }
        Ok(Self { udp, tcp, host: host.to_string(), port })
    }

    pub fn describe(&self) -> String {
        let proto = match (self.udp, self.tcp) {
            (true, false) => "udp/",
            (false, true) => "tcp/",
            _ => "",
        };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        match self.port {
            Some(port) => format!("{proto}{host}:{port}"),
            None => format!("{proto}{host}"),
        }
    }
}

/// What the data channel carries, both ways.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Signal {
    Offer {
        sdp: String,
    },
    Answer {
        sdp: String,
    },
    /// The same account joined the room from somewhere else.
    Replaced,
    /// The media part is going away: join again now.
    Restarting,
    /// Hung up from the instance's side: the app left, a moderator took
    /// them out, or they can't be in the call any more.
    Closed,
    /// From the app: which size of each camera it wants, by the track's mid.
    Layers {
        layers: HashMap<String, Layer>,
    },
}

/// One of a camera's simulcast sizes, as a viewer asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Layer {
    /// Not showing it: nothing is sent.
    #[serde(rename = "off")]
    Off,
    /// A quarter of the camera's size, for small tiles.
    #[serde(rename = "l")]
    Low,
    /// Half.
    #[serde(rename = "m")]
    Medium,
    /// Full size, for a camera shown big or popped out.
    #[serde(rename = "h")]
    High,
}

impl Layer {
    const SENT: [Layer; 3] = [Layer::Low, Layer::Medium, Layer::High];

    /// The size a simulcast rid stands for.
    fn of(rid: &str) -> Option<Self> {
        match rid {
            "l" => Some(Self::Low),
            "m" => Some(Self::Medium),
            "h" => Some(Self::High),
            _ => None,
        }
    }

    fn rid(self) -> Option<Rid> {
        match self {
            Self::Off => None,
            Self::Low => Some(Rid::from("l")),
            Self::Medium => Some(Rid::from("m")),
            Self::High => Some(Rid::from("h")),
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }

    /// The size to send a viewer who wants `want`, out of the ones a camera
    /// is sending now (`fresh`, a set of [`Layer::bit`]s): the biggest that
    /// isn't bigger than asked, or the smallest there is when they all are.
    /// None when the viewer wants none, or nothing comes.
    fn pick(want: Layer, fresh: u8) -> Option<Layer> {
        if want == Layer::Off {
            return None;
        }
        let sent = || Layer::SENT.into_iter().filter(|l| fresh & l.bit() != 0);
        sent().filter(|l| *l <= want).max().or_else(|| sent().min())
    }
}

/// What someone in a call may do there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct May {
    /// Their sound is passed on.
    pub speak: bool,
    /// They get everyone else's.
    pub hear: bool,
    /// Their camera is passed on.
    pub video: bool,
}

/// What a bridge hears, and how it ends.
#[derive(Debug)]
pub enum Bridged {
    /// A frame of someone's sound, as they sent it (Opus; sealed, in
    /// direct-message calls, but bridges are only for voice channels).
    Frame(Heard),
    /// The bridge is over, and why: the same account joined from somewhere
    /// else, the media part is restarting (open it again), or it was hung up.
    Ended(Ending),
}

#[derive(Debug, Clone)]
pub struct Heard {
    /// Whose sound: their account.
    pub participant: String,
    pub frame: Vec<u8>,
    /// When it was spoken, in 48 kHz ticks of the speaker's own clock: the
    /// gaps between frames, not the time of day.
    pub timestamp: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Replaced,
    Restarting,
    Closed,
}

impl Ending {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replaced => "replaced",
            Self::Restarting => "restarting",
            Self::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "replaced" => Self::Replaced,
            "closed" => Self::Closed,
            _ => Self::Restarting,
        }
    }
}

/// One Opus frame of what a bridge says goes out every this long.
pub const FRAME_TIME: Duration = Duration::from_millis(20);
/// Frames a bridge may have waiting: a second of sound.
pub const MAX_QUEUED: usize = 50;
/// Frames a bridge holds for its program before it drops what it hears.
const BRIDGE_BUFFER: usize = 512;

enum Command {
    Bridge {
        room: String,
        participant: String,
        session_id: String,
        may: May,
        events: mpsc::Sender<Bridged>,
        reply: oneshot::Sender<Result<()>>,
    },
    Speak {
        room: String,
        participant: String,
        session_id: String,
        frames: Vec<Vec<u8>>,
        reply: oneshot::Sender<Result<usize>>,
    },
    Open {
        room: String,
        participant: String,
        session_id: String,
        offer: String,
        may: May,
        candidates: Vec<(Protocol, SocketAddr)>,
        reply: oneshot::Sender<Result<String>>,
    },
    Close {
        room: String,
        participant: Option<String>,
        session_id: Option<String>,
    },
    Update {
        room: String,
        participant: String,
        session_id: Option<String>,
        may: May,
        reply: oneshot::Sender<bool>,
    },
}

/// A running media part in this process.
#[derive(Clone)]
pub struct Sfu {
    commands: mpsc::Sender<Command>,
    config: Arc<MediaConfig>,
}

impl Sfu {
    /// Listens on the configured port (UDP and TCP) and starts carrying
    /// calls, until `shutdown`, when everyone is told to join again.
    pub async fn start(config: MediaConfig, shutdown: CancellationToken) -> Result<Self> {
        static CRYPTO: Once = Once::new();
        CRYPTO.call_once(|| str0m::crypto::from_feature_flags().install_process_default());

        let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), config.port);
        let udp = UdpSocket::bind(bind)
            .await
            .map_err(|err| Error::internal(format!("couldn't listen for calls on UDP {bind}: {err}")))?;
        // Port 0 picks one (in tests): TCP takes the same number UDP got.
        let port = udp.local_addr()?.port();
        let bind = SocketAddr::new(bind.ip(), port);
        let tcp = TcpListener::bind(bind)
            .await
            .map_err(|err| Error::internal(format!("couldn't listen for calls on TCP {bind}: {err}")))?;
        let config = MediaConfig { port, ..config };
        let (commands, receiver) = mpsc::channel(256);
        let (packets, tcp_in) = mpsc::channel(1024);
        tokio::spawn(accept_tcp(tcp, packets, shutdown.clone()));
        tokio::spawn(Engine::new(udp, tcp_in).run(receiver, shutdown));
        Ok(Self { commands, config: Arc::new(config) })
    }

    pub fn config(&self) -> &MediaConfig {
        &self.config
    }

    /// The addresses apps are told about, as configured or worked out.
    pub fn describe(&self) -> Vec<String> {
        if self.config.addresses.is_empty() {
            return own_address().map(|ip| vec![format!("{ip} (this machine)")]).unwrap_or_default();
        }
        self.config.addresses.iter().map(Advertised::describe).collect()
    }

    /// Takes someone's offer for a room and answers it.
    pub async fn open(&self, room: &str, participant: &str, session_id: &str, offer: &str, may: May) -> Result<String> {
        if offer.len() > 64 * 1024 {
            return Err(Error::invalid("that offer is too big"));
        }
        let candidates = self.candidates().await?;
        let (reply, answer) = oneshot::channel();
        self.send(Command::Open {
            room: room.into(),
            participant: participant.into(),
            session_id: session_id.into(),
            offer: offer.into(),
            may,
            candidates,
            reply,
        })
        .await?;
        answer.await.map_err(|_| Error::Unavailable("calls are restarting; try again".into()))?
    }

    /// Hangs someone up (only if it's still `session_id`, when given), or
    /// everyone in the room.
    pub async fn close(&self, room: &str, participant: Option<&str>, session_id: Option<&str>) -> Result<()> {
        let command = Command::Close {
            room: room.into(),
            participant: participant.map(Into::into),
            session_id: session_id.filter(|s| !s.is_empty()).map(Into::into),
        };
        self.send(command).await
    }

    /// Puts a program in a room without WebRTC: what it hears comes out of
    /// the receiver, until it's dropped (which leaves) or it ends.
    pub async fn bridge(
        &self,
        room: &str,
        participant: &str,
        session_id: &str,
        may: May,
    ) -> Result<mpsc::Receiver<Bridged>> {
        let (events, heard) = mpsc::channel(BRIDGE_BUFFER);
        let (reply, answer) = oneshot::channel();
        self.send(Command::Bridge {
            room: room.into(),
            participant: participant.into(),
            session_id: session_id.into(),
            may,
            events,
            reply,
        })
        .await?;
        answer.await.map_err(|_| Error::Unavailable("calls are restarting; try again".into()))??;
        Ok(heard)
    }

    /// Queues frames of Opus for a bridge to say, giving back how many are
    /// waiting now. They go out one every 20 ms.
    pub async fn speak(&self, room: &str, participant: &str, session_id: &str, frames: Vec<Vec<u8>>) -> Result<usize> {
        let (reply, answer) = oneshot::channel();
        let (room, participant, session_id) = (room.into(), participant.into(), session_id.into());
        self.send(Command::Speak { room, participant, session_id, frames, reply }).await?;
        answer.await.map_err(|_| Error::Unavailable("calls are restarting; try again".into()))?
    }

    /// Changes what someone may do, saying whether they (in `session_id`,
    /// when given) have a connection here at all.
    pub async fn update(&self, room: &str, participant: &str, session_id: Option<&str>, may: May) -> Result<bool> {
        let (reply, answer) = oneshot::channel();
        let (room, participant, session_id) = (room.into(), participant.into(), session_id.map(Into::into));
        self.send(Command::Update { room, participant, session_id, may, reply }).await?;
        answer.await.map_err(|_| Error::Unavailable("calls are restarting; try again".into()))
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.commands.send(command).await.map_err(|_| Error::Unavailable("calls are restarting; try again".into()))
    }

    /// Where apps send to: the configured addresses (names looked up now),
    /// or this machine's own.
    async fn candidates(&self) -> Result<Vec<(Protocol, SocketAddr)>> {
        let port = self.config.port;
        let mut out = Vec::new();
        if self.config.addresses.is_empty() {
            let ip =
                own_address().ok_or_else(|| Error::Unavailable("calls can't find this machine's address".into()))?;
            out.push((Protocol::Udp, SocketAddr::new(ip, port)));
            out.push((Protocol::Tcp, SocketAddr::new(ip, port)));
            return Ok(out);
        }
        for advertised in &self.config.addresses {
            let port = advertised.port.unwrap_or(port);
            let ips: Vec<IpAddr> = match advertised.host.parse::<IpAddr>() {
                Ok(ip) => vec![ip],
                Err(_) => match tokio::net::lookup_host((advertised.host.as_str(), port)).await {
                    Ok(found) => found.map(|addr| addr.ip()).collect(),
                    Err(err) => {
                        tracing::warn!(address = %advertised.host, error = %err, "couldn't look up a media address");
                        vec![]
                    }
                },
            };
            for ip in ips {
                if advertised.udp {
                    out.push((Protocol::Udp, SocketAddr::new(ip, port)));
                }
                if advertised.tcp {
                    out.push((Protocol::Tcp, SocketAddr::new(ip, port)));
                }
            }
        }
        if out.is_empty() {
            return Err(Error::Unavailable("calls can't look up their own address right now".into()));
        }
        Ok(out)
    }
}

/// The address this machine reaches the internet from (no packet is sent to
/// find it out).
fn own_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip()).filter(|ip| !ip.is_unspecified())
}

/// What the TCP side hands the engine.
enum Tcp {
    Connected(SocketAddr, mpsc::Sender<Vec<u8>>),
    Packet(SocketAddr, Vec<u8>),
    Gone(SocketAddr),
}

async fn accept_tcp(listener: TcpListener, packets: mpsc::Sender<Tcp>, shutdown: CancellationToken) {
    loop {
        let (stream, peer) = tokio::select! {
            _ = shutdown.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(err) => {
                    tracing::debug!(error = %err, "couldn't take a call's TCP connection");
                    continue;
                }
            },
        };
        let _ = stream.set_nodelay(true);
        tokio::spawn(serve_tcp(stream, peer, packets.clone()));
    }
}

/// One ICE-TCP connection: frames are a 2-byte length then the packet (RFC 4571).
async fn serve_tcp(stream: TcpStream, peer: SocketAddr, packets: mpsc::Sender<Tcp>) {
    let (mut reader, mut writer) = stream.into_split();
    let (out, mut outgoing) = mpsc::channel::<Vec<u8>>(256);
    if packets.send(Tcp::Connected(peer, out)).await.is_err() {
        return;
    }
    let write = async move {
        while let Some(packet) = outgoing.recv().await {
            let length = (packet.len() as u16).to_be_bytes();
            if writer.write_all(&length).await.is_err() || writer.write_all(&packet).await.is_err() {
                return;
            }
        }
    };
    let read = async {
        let mut length = [0u8; 2];
        loop {
            if reader.read_exact(&mut length).await.is_err() {
                return;
            }
            let length = u16::from_be_bytes(length) as usize;
            if length == 0 || length > MAX_PACKET {
                return;
            }
            let mut packet = vec![0; length];
            if reader.read_exact(&mut packet).await.is_err() {
                return;
            }
            if packets.send(Tcp::Packet(peer, packet)).await.is_err() {
                return;
            }
        }
    };
    tokio::select! {
        _ = write => {}
        _ = read => {}
    }
    let _ = packets.send(Tcp::Gone(peer)).await;
}

type ClientId = u64;

struct TrackIn {
    origin: ClientId,
    /// Whose sound it is: the account.
    participant: String,
    mid: Mid,
    kind: MediaKind,
}

/// A track someone sends here.
struct Incoming {
    track: Arc<TrackIn>,
    /// When a keyframe was last asked for, of each size (none, l, m, h).
    asked: [Option<Instant>; 4],
    /// Whether the others got it yet: a camera's track goes out once its
    /// first frame came, so cameras never turned on cost nobody anything.
    shared: bool,
    /// When each of a camera's sizes last arrived, by [`Layer`].
    seen: [Option<Instant>; 4],
}

impl Incoming {
    fn new(track: Arc<TrackIn>) -> Self {
        let shared = track.kind == MediaKind::Audio;
        Self { track, asked: [None; 4], shared, seen: [None; 4] }
    }

    /// The sizes that came lately, as [`Layer::bit`]s.
    fn fresh(&self, now: Instant) -> u8 {
        Layer::SENT
            .into_iter()
            .filter(|l| self.seen[*l as usize].is_some_and(|at| now.duration_since(at) < STALE_LAYER))
            .fold(0, |bits, l| bits | l.bit())
    }
}

struct TrackOut {
    from: Weak<TrackIn>,
    state: TrackState,
    /// The camera size this viewer wants.
    want: Layer,
    /// The size it's getting now.
    rid: Option<Rid>,
    /// When it last asked the camera for a keyframe of another size.
    asked: Option<Instant>,
    /// Moves the camera's clock when sizes switch, so the viewer's stays even.
    shift: i64,
    /// The last frame's time on the viewer's clock (90 kHz).
    last: Option<i64>,
}

impl TrackOut {
    fn new(from: Weak<TrackIn>) -> Self {
        Self { from, state: TrackState::ToOpen, want: Layer::Low, rid: None, asked: None, shift: 0, last: None }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrackState {
    ToOpen,
    Negotiating(Mid),
    Open(Mid),
    ToStop(Mid),
    Stopping(Mid),
}

impl TrackOut {
    fn mid(&self) -> Option<Mid> {
        match self.state {
            TrackState::ToOpen => None,
            TrackState::Negotiating(m) | TrackState::Open(m) | TrackState::ToStop(m) | TrackState::Stopping(m) => {
                Some(m)
            }
        }
    }
}

/// One person's connection.
struct Client {
    id: ClientId,
    room: String,
    participant: String,
    session_id: String,
    rtc: Rtc,
    channel: Option<ChannelId>,
    pending: Option<SdpPendingOffer>,
    tracks_in: Vec<Incoming>,
    tracks_out: Vec<TrackOut>,
    may: May,
    /// Signals waiting for the data channel to open.
    outbox: Vec<Signal>,
    /// Being let go: it hangs up at this time, and takes no part meanwhile.
    leaving: Option<Instant>,
    /// When its recent offers came, to cap renegotiation.
    offers: VecDeque<Instant>,
    /// Sound it sent this second: (since, bytes).
    sent: (Instant, usize),
    /// Camera it sent this second, every size together: (since, bytes).
    filmed: (Instant, usize),
}

/// The most an app's offer may be.
const MAX_SDP: usize = 32 * 1024;
/// Offers an app may make in [`OFFER_WINDOW`]; more hangs it up.
const MAX_OFFERS: usize = 10;
const OFFER_WINDOW: Duration = Duration::from_secs(10);
/// Sound one person may send a second, far above any Opus voice (510 kbit/s at most).
const MAX_BYTES_PER_SECOND: usize = 80 * 1024;
/// The largest frame of sound passed on: Opus frames are 1275 bytes at most,
/// plus the encryption trailer in direct-message calls.
pub const MAX_FRAME: usize = 1500;
/// Camera one person may send a second, every size together: well above
/// what browsers send for 1080p in three sizes (about 4 Mbit/s).
const MAX_VIDEO_BYTES_PER_SECOND: usize = 1024 * 1024;
/// The largest frame of camera passed on (a 1080p keyframe is about 200 KB).
const MAX_VIDEO_FRAME: usize = 512 * 1024;
/// A camera size that hasn't come for this long isn't being sent.
const STALE_LAYER: Duration = Duration::from_millis(700);
/// How often a viewer waiting for another size asks the camera for a keyframe.
const SWITCH_ASK: Duration = Duration::from_millis(500);
/// How often a camera is asked for a keyframe of one size at most.
const KEYFRAME_EVERY: Duration = Duration::from_millis(300);
/// The most camera sizes an app may ask for in one message.
const MAX_LAYER_ASKS: usize = 2 * MAX_ROOM;

/// Whether an app's offer asks for what calls allow: sending one audio
/// track and one video track at most, receiving the others' tracks, and the
/// data channel. Anything else is turned down before it reaches str0m.
fn offer_allowed(sdp: &str) -> bool {
    if sdp.len() > MAX_SDP {
        return false;
    }
    let mut sections: Vec<(&str, bool)> = Vec::new();
    for line in sdp.lines() {
        if let Some(m) = line.strip_prefix("m=") {
            sections.push((m.split(' ').next().unwrap_or(""), true));
        } else if let Some(last) = sections.last_mut()
            && matches!(line.trim_end(), "a=recvonly" | "a=inactive")
        {
            last.1 = false;
        }
    }
    let sending = |kind: &str| sections.iter().filter(|(k, sends)| *k == kind && *sends).count();
    sections.len() <= 2 * MAX_ROOM + 3
        && sections.iter().all(|(k, _)| matches!(*k, "audio" | "video" | "application"))
        && sending("audio") <= 1
        && sending("video") <= 1
        && sections.iter().filter(|(k, _)| *k == "application").count() <= 1
}

/// A camera frame's time in 90 kHz ticks.
fn ticks(time: MediaTime) -> i64 {
    (u128::from(time.numer()) * 90_000 / u128::from(time.denom().max(1))) as i64
}

/// What one connection's output means for the others.
enum Propagated {
    TrackOpen(ClientId, Weak<TrackIn>),
    /// A frame, and which of its camera's sizes are coming (as [`Layer::bit`]s).
    Media(ClientId, Box<MediaData>, u8),
    /// Ask a sender for a keyframe: of its track `Mid`, in size `Rid`.
    Keyframe(KeyframeRequest, ClientId, Mid, Option<Rid>),
}

impl Client {
    fn say(&mut self, signal: Signal) {
        let Some(mut channel) = self.channel.and_then(|id| self.rtc.channel(id)) else {
            self.outbox.push(signal);
            return;
        };
        let json = serde_json::to_string(&signal).unwrap_or_default();
        if !matches!(channel.write(false, json.as_bytes()), Ok(true)) {
            tracing::debug!(client = self.id, "couldn't tell an app something over its data channel");
        }
    }

    fn leave(&mut self, signal: Signal, now: Instant) {
        if self.leaving.is_none() {
            self.say(signal);
            self.leaving = Some(now + PARTING);
        }
    }

    /// Offers the tracks that came or went since the last offer, unless one
    /// is already waiting for its answer.
    fn negotiate(&mut self) {
        if self.channel.is_none() || self.pending.is_some() || self.leaving.is_some() {
            return;
        }
        for track in &mut self.tracks_out {
            if let TrackState::Open(mid) = track.state
                && track.from.upgrade().is_none()
            {
                track.state = TrackState::ToStop(mid);
            }
        }
        let mut change = self.rtc.sdp_api();
        for track in &mut self.tracks_out {
            match track.state {
                TrackState::ToOpen => {
                    if let Some(from) = track.from.upgrade() {
                        // The stream is named for whoever's sound it is, so the
                        // app knows whose track it got.
                        let stream = from.participant.clone();
                        let mid = change.add_media(from.kind, Direction::SendOnly, Some(stream), None, None);
                        track.state = TrackState::Negotiating(mid);
                    }
                }
                TrackState::ToStop(mid) => {
                    change.stop_media(mid);
                    track.state = TrackState::Stopping(mid);
                }
                _ => {}
            }
        }
        if !change.has_changes() {
            return;
        }
        let Some((offer, pending)) = change.apply() else { return };
        self.pending = Some(pending);
        self.say(Signal::Offer { sdp: offer.to_sdp_string() });
    }

    fn on_signal(&mut self, data: ChannelData) {
        let signal = match serde_json::from_slice::<Signal>(&data.data) {
            Ok(signal) => signal,
            Err(_) => return,
        };
        match signal {
            Signal::Offer { sdp } => {
                let now = Instant::now();
                self.offers.retain(|at| now.duration_since(*at) < OFFER_WINDOW);
                self.offers.push_back(now);
                if self.offers.len() > MAX_OFFERS || !offer_allowed(&sdp) {
                    tracing::debug!(client = self.id, "an app's offer asked for too much; hanging it up");
                    self.rtc.disconnect();
                    return;
                }
                let Ok(offer) = SdpOffer::from_sdp_string(&sdp) else { return };
                // Ours wins when two offers cross: the app (the polite side)
                // takes ours back, answers it, and offers again after.
                if self.pending.is_some() {
                    return;
                }
                match self.rtc.sdp_api().accept_offer(offer) {
                    Ok(answer) => {
                        for track in &mut self.tracks_out {
                            match track.state {
                                TrackState::Negotiating(_) => track.state = TrackState::ToOpen,
                                TrackState::Stopping(mid) => track.state = TrackState::ToStop(mid),
                                _ => {}
                            }
                        }
                        self.say(Signal::Answer { sdp: answer.to_sdp_string() });
                    }
                    Err(err) => tracing::debug!(client = self.id, error = %err, "an app's offer didn't work"),
                }
            }
            Signal::Answer { sdp } => {
                let (Ok(answer), Some(pending)) = (SdpAnswer::from_sdp_string(&sdp), self.pending.take()) else {
                    return;
                };
                if let Err(err) = self.rtc.sdp_api().accept_answer(pending, answer) {
                    tracing::debug!(client = self.id, error = %err, "an app's answer didn't work");
                    self.rtc.disconnect();
                    return;
                }
                for track in &mut self.tracks_out {
                    if let TrackState::Negotiating(mid) = track.state {
                        track.state = TrackState::Open(mid);
                    }
                }
                self.tracks_out.retain(|t| !matches!(t.state, TrackState::Stopping(_)));
            }
            Signal::Layers { layers } => {
                if layers.len() > MAX_LAYER_ASKS {
                    return;
                }
                for track in &mut self.tracks_out {
                    if let Some(want) = track.mid().and_then(|mid| layers.get(&*mid)) {
                        track.want = *want;
                    }
                }
            }
            Signal::Replaced | Signal::Restarting | Signal::Closed => {}
        }
    }

    fn poll(&mut self, outputs: &mut Outputs) -> Option<Instant> {
        loop {
            if !self.rtc.is_alive() {
                return None;
            }
            self.negotiate();
            let output = match self.rtc.poll_output() {
                Ok(output) => output,
                Err(err) => {
                    tracing::debug!(client = self.id, error = %err, "a call connection failed");
                    self.rtc.disconnect();
                    return None;
                }
            };
            match output {
                Output::Timeout(at) => return Some(at),
                Output::Transmit(transmit) => outputs.transmits.push_back(transmit),
                Output::Event(event) => self.on_event(event, outputs),
            }
        }
    }

    fn on_event(&mut self, event: Event, outputs: &mut Outputs) {
        match event {
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => self.rtc.disconnect(),
            Event::ChannelOpen(id, label) if label == CHANNEL_LABEL => {
                self.channel = Some(id);
                for signal in std::mem::take(&mut self.outbox) {
                    self.say(signal);
                }
            }
            Event::ChannelData(data) if Some(data.id) == self.channel => self.on_signal(data),
            // One track of sound and one of camera from each app; anything
            // more it can't send.
            Event::MediaAdded(added) if self.tracks_in.iter().any(|t| t.track.kind == added.kind) => {}
            Event::MediaAdded(added) => {
                let track = Arc::new(TrackIn {
                    origin: self.id,
                    participant: self.participant.clone(),
                    mid: added.mid,
                    kind: added.kind,
                });
                let incoming = Incoming::new(track);
                if incoming.shared {
                    outputs.propagated.push_back(Propagated::TrackOpen(self.id, Arc::downgrade(&incoming.track)));
                }
                self.tracks_in.push(incoming);
            }
            Event::MediaData(data) => {
                let Some(kind) = self.tracks_in.iter().find(|t| t.track.mid == data.mid).map(|t| t.track.kind) else {
                    return;
                };
                if !self.within_budget(kind, data.data.len()) {
                    return;
                }
                if !data.contiguous {
                    self.ask_keyframe(data.mid, data.rid, KeyframeRequestKind::Fir);
                }
                let allowed = if kind == MediaKind::Video { self.may.video } else { self.may.speak };
                if !allowed || self.leaving.is_some() {
                    return;
                }
                let now = Instant::now();
                let Some(incoming) = self.tracks_in.iter_mut().find(|t| t.track.mid == data.mid) else { return };
                if let Some(layer) = data.rid.as_deref().and_then(Layer::of) {
                    incoming.seen[layer as usize] = Some(now);
                }
                if !incoming.shared {
                    incoming.shared = true;
                    outputs.propagated.push_back(Propagated::TrackOpen(self.id, Arc::downgrade(&incoming.track)));
                }
                let fresh = incoming.fresh(now);
                outputs.propagated.push_back(Propagated::Media(self.id, Box::new(data), fresh));
            }
            Event::KeyframeRequest(request) => {
                let Some(out) = self.tracks_out.iter().find(|t| t.mid() == Some(request.mid)) else { return };
                if let Some(from) = out.from.upgrade() {
                    outputs.propagated.push_back(Propagated::Keyframe(request, from.origin, from.mid, out.rid));
                }
            }
            _ => {}
        }
    }

    /// Whether a frame this size fits in what one person may send.
    fn within_budget(&mut self, kind: MediaKind, len: usize) -> bool {
        let (most, per_second, spent) = match kind {
            MediaKind::Video => (MAX_VIDEO_FRAME, MAX_VIDEO_BYTES_PER_SECOND, &mut self.filmed),
            MediaKind::Audio => (MAX_FRAME, MAX_BYTES_PER_SECOND, &mut self.sent),
        };
        if len > most {
            return false;
        }
        let now = Instant::now();
        if now.duration_since(spent.0) >= Duration::from_secs(1) {
            *spent = (now, 0);
        }
        spent.1 += len;
        spent.1 <= per_second
    }

    /// Asks this app for a keyframe on one of its tracks (of one size, for a
    /// camera), not more often than [`KEYFRAME_EVERY`].
    fn ask_keyframe(&mut self, mid: Mid, rid: Option<Rid>, kind: KeyframeRequestKind) {
        let Some(incoming) = self.tracks_in.iter_mut().find(|t| t.track.mid == mid) else { return };
        let slot = rid.as_deref().and_then(Layer::of).map_or(0, |l| l as usize);
        if incoming.asked[slot].is_some_and(|at| at.elapsed() < KEYFRAME_EVERY) {
            return;
        }
        incoming.asked[slot] = Some(Instant::now());
        if let Some(mut writer) = self.rtc.writer(mid) {
            let _ = writer.request_keyframe(rid, kind);
        }
    }

    /// The track this connection hears `origin`'s `mid` on, once it's open.
    fn track_from(&self, origin: ClientId, mid: Mid) -> Option<Mid> {
        self.tracks_out.iter().find_map(|out| {
            let from = out.from.upgrade()?;
            (from.origin == origin && from.mid == mid).then(|| out.mid()).flatten()
        })
    }

    /// Passes on a frame from `origin`, giving back the camera size to ask
    /// it for a keyframe of, when this viewer is waiting to switch.
    fn forward(&mut self, origin: ClientId, data: &MediaData, fresh: u8, now: Instant) -> Option<Rid> {
        if !self.may.hear || self.leaving.is_some() {
            return None;
        }
        let out = self.tracks_out.iter_mut().find(|out| {
            out.from.upgrade().is_some_and(|from| from.origin == origin && from.mid == data.mid) && out.mid().is_some()
        })?;
        let mid = out.mid()?;
        let mut ask = None;
        let mut time = data.time;
        if data.params.spec().codec.is_video() {
            // A camera starts on a keyframe, so none goes to a track the
            // viewer hasn't taken yet.
            if out.want == Layer::Off || !matches!(out.state, TrackState::Open(_)) {
                return None;
            }
            if let Some(rid) = data.rid {
                let layer = Layer::of(&rid)?;
                let target = Layer::pick(out.want, fresh)?;
                let current = out.rid.as_deref().and_then(Layer::of);
                let flowing = current.is_some_and(|c| fresh & c.bit() != 0);
                let switch = data.is_keyframe() && current != Some(layer) && (layer == target || !flowing);
                let getting = if switch { Some(layer) } else { current };
                if getting != Some(target) && out.asked.is_none_or(|at| now.duration_since(at) >= SWITCH_ASK) {
                    out.asked = Some(now);
                    ask = target.rid();
                }
                if switch {
                    out.rid = Some(rid);
                    // Sizes may each keep their own clock: carry on from the
                    // last frame the viewer got, a frame's time later.
                    let at = ticks(data.time);
                    if let Some(last) = out.last
                        && (at + out.shift - last).abs() > 90_000
                    {
                        out.shift = last + 3_000 - at;
                    }
                } else if current != Some(layer) {
                    return ask;
                }
            }
            let at = ticks(data.time) + out.shift;
            out.last = Some(at);
            time = MediaTime::new(at.max(0) as u64, Frequency::NINETY_KHZ);
        }
        let writer = self.rtc.writer(mid)?;
        let pt = writer.match_params(data.params)?;
        if let Err(err) = writer.write(pt, data.network_time, time, data.data.clone()) {
            tracing::debug!(client = self.id, error = %err, "couldn't pass a frame on");
            self.rtc.disconnect();
        }
        ask
    }

    /// Passes on a frame of Opus a bridge said.
    fn forward_said(&mut self, origin: ClientId, from: Mid, now: Instant, time: MediaTime, frame: &[u8]) {
        if !self.may.hear || self.leaving.is_some() {
            return;
        }
        let Some(mid) = self.track_from(origin, from) else { return };
        let Some(writer) = self.rtc.writer(mid) else { return };
        let Some(pt) = writer.payload_params().find(|p| p.spec().codec == Codec::Opus).map(|p| p.pt()) else { return };
        if let Err(err) = writer.write(pt, now, time, frame.to_vec()) {
            tracing::debug!(client = self.id, error = %err, "couldn't pass a bridge's sound on");
        }
    }
}

/// A program in a room without WebRTC: it hears through `events`, and says
/// what's in `queue`, one frame every [`FRAME_TIME`].
struct Bridge {
    id: ClientId,
    room: String,
    participant: String,
    session_id: String,
    events: mpsc::Sender<Bridged>,
    /// Its sound, as everyone else gets it.
    track: Arc<TrackIn>,
    may: May,
    queue: VecDeque<Vec<u8>>,
    /// When it started: its RTP clock counts from here.
    started: Instant,
    /// When the next frame goes out.
    next: Instant,
    /// Sound it said this second: (since, bytes).
    said: (Instant, usize),
}

impl Bridge {
    fn end(&self, ending: Ending) {
        let _ = self.events.try_send(Bridged::Ended(ending));
    }

    fn hear(&self, participant: &str, frame: &[u8], timestamp: u32) {
        if !self.may.hear {
            return;
        }
        let heard = Heard { participant: participant.to_string(), frame: frame.to_vec(), timestamp };
        // A program that doesn't keep up misses sound rather than holding up the call.
        let _ = self.events.try_send(Bridged::Frame(heard));
    }

    /// The next frame to say, if one is due, and its time on the bridge's clock.
    fn due(&mut self, now: Instant) -> Option<(Vec<u8>, MediaTime)> {
        if self.queue.is_empty() || now < self.next {
            return None;
        }
        // After a pause the clock moves on with the time that passed, so
        // listeners hear the gap; while talking, frames are 20 ms apart.
        if now.duration_since(self.next) > FRAME_TIME * 5 {
            self.next = now;
        }
        let at = self.next;
        self.next += FRAME_TIME;
        let frame = self.queue.pop_front()?;
        let ticks = at.duration_since(self.started).as_micros() as u64 * 48 / 1000;
        let ticks = ticks - ticks % 960;
        Some((frame, MediaTime::new(ticks, Frequency::FORTY_EIGHT_KHZ)))
    }

    fn within_budget(&mut self, len: usize) -> bool {
        let now = Instant::now();
        if now.duration_since(self.said.0) >= Duration::from_secs(1) {
            self.said = (now, 0);
        }
        self.said.1 += len;
        self.said.1 <= MAX_BYTES_PER_SECOND
    }
}

#[derive(Default)]
struct Outputs {
    transmits: VecDeque<str0m::net::Transmit>,
    propagated: VecDeque<Propagated>,
}

struct Engine {
    udp: UdpSocket,
    tcp: mpsc::Receiver<Tcp>,
    tcp_peers: HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>,
    clients: Vec<Client>,
    bridges: Vec<Bridge>,
    next_id: ClientId,
    /// Where packets "arrive" for str0m: the addresses last given to apps,
    /// one per protocol and IP family. The sockets listen on every address,
    /// so this is what tells str0m which of its candidates a packet was for.
    destinations: Vec<(Protocol, SocketAddr)>,
    /// When each connection next needs time moved on.
    timeouts: Vec<Instant>,
}

impl Engine {
    fn new(udp: UdpSocket, tcp: mpsc::Receiver<Tcp>) -> Self {
        Self {
            udp,
            tcp,
            tcp_peers: HashMap::new(),
            clients: vec![],
            bridges: vec![],
            next_id: 1,
            destinations: vec![],
            timeouts: vec![],
        }
    }

    async fn run(mut self, mut commands: mpsc::Receiver<Command>, shutdown: CancellationToken) {
        let mut buffer = vec![0u8; MAX_PACKET];
        let mut closing: Option<Instant> = None;
        loop {
            let now = Instant::now();
            self.step(now).await;
            if closing.is_some_and(|at| now >= at) {
                return;
            }
            let next = self
                .clients
                .iter()
                .filter_map(|c| c.leaving)
                .chain(self.bridges.iter().filter(|b| !b.queue.is_empty()).map(|b| b.next))
                .chain(self.timeouts.iter().copied())
                .chain(closing)
                .min()
                .unwrap_or(now + Duration::from_millis(100))
                .max(now + Duration::from_millis(1));
            tokio::select! {
                _ = shutdown.cancelled(), if closing.is_none() => {
                    let now = Instant::now();
                    for client in &mut self.clients {
                        client.leave(Signal::Restarting, now);
                    }
                    for bridge in self.bridges.drain(..) {
                        bridge.end(Ending::Restarting);
                    }
                    closing = Some(now + PARTING);
                    tracing::info!(people = self.clients.len(), "calls are moving to the next media part");
                }
                command = commands.recv(), if closing.is_none() => match command {
                    Some(command) => self.on_command(command, Instant::now()),
                    None => return,
                },
                received = self.udp.recv_from(&mut buffer) => {
                    if let Ok((length, source)) = received {
                        self.receive(Protocol::Udp, source, &buffer[..length]);
                    }
                }
                packet = self.tcp.recv() => match packet {
                    Some(Tcp::Connected(peer, sender)) => {
                        self.tcp_peers.insert(peer, sender);
                    }
                    Some(Tcp::Packet(peer, packet)) => self.receive(Protocol::Tcp, peer, &packet),
                    Some(Tcp::Gone(peer)) => {
                        self.tcp_peers.remove(&peer);
                    }
                    None => {}
                },
                _ = tokio::time::sleep_until(next.into()) => {}
            }
        }
    }

    fn destination(&self, proto: Protocol, source: SocketAddr) -> Option<SocketAddr> {
        let matching = |same_family: bool| {
            self.destinations
                .iter()
                .find(|(p, addr)| *p == proto && (!same_family || addr.is_ipv4() == source.is_ipv4()))
                .map(|(_, addr)| *addr)
        };
        matching(true).or_else(|| matching(false))
    }

    fn receive(&mut self, proto: Protocol, source: SocketAddr, packet: &[u8]) {
        let Some(destination) = self.destination(proto, source) else { return };
        let Ok(receive) = Receive::new(proto, source, destination, packet) else { return };
        let input = Input::Receive(Instant::now(), receive);
        if let Some(client) = self.clients.iter_mut().find(|c| c.rtc.accepts(&input))
            && let Err(err) = client.rtc.handle_input(input)
        {
            tracing::debug!(client = client.id, error = %err, "a call connection failed");
            client.rtc.disconnect();
        }
    }

    fn on_command(&mut self, command: Command, now: Instant) {
        match command {
            Command::Bridge { room, participant, session_id, may, events, reply } => {
                let _ = reply.send(self.bridge(room, participant, session_id, may, events, now));
            }
            Command::Speak { room, participant, session_id, frames, reply } => {
                let _ = reply.send(self.speak(&room, &participant, &session_id, frames));
            }
            Command::Open { room, participant, session_id, offer, may, candidates, reply } => {
                let answer = self.open(room, participant, session_id, &offer, may, candidates, now);
                let _ = reply.send(answer);
            }
            Command::Close { room, participant, session_id } => {
                let now = Instant::now();
                for client in &mut self.clients {
                    let named = participant.as_ref().is_none_or(|p| *p == client.participant);
                    let same = session_id.as_ref().is_none_or(|s| *s == client.session_id);
                    if client.room == room && named && same {
                        client.leave(Signal::Closed, now);
                    }
                }
                self.bridges.retain(|bridge| {
                    let named = participant.as_ref().is_none_or(|p| *p == bridge.participant);
                    let same = session_id.as_ref().is_none_or(|s| *s == bridge.session_id);
                    let closing = bridge.room == room && named && same;
                    if closing {
                        bridge.end(Ending::Closed);
                    }
                    !closing
                });
            }
            Command::Update { room, participant, session_id, may, reply } => {
                let mut connected = false;
                for client in &mut self.clients {
                    if client.room == room && client.participant == participant {
                        client.may = may;
                        connected |=
                            client.leaving.is_none() && session_id.as_ref().is_none_or(|s| *s == client.session_id);
                    }
                }
                for bridge in &mut self.bridges {
                    if bridge.room == room && bridge.participant == participant {
                        bridge.may = May { video: false, ..may };
                        connected |= session_id.as_ref().is_none_or(|s| *s == bridge.session_id);
                    }
                }
                let _ = reply.send(connected);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn open(
        &mut self,
        room: String,
        participant: String,
        session_id: String,
        offer: &str,
        may: May,
        candidates: Vec<(Protocol, SocketAddr)>,
        now: Instant,
    ) -> Result<String> {
        if self.in_room(&room, &participant) >= MAX_ROOM {
            return Err(Error::ResourceExhausted(format!("a call holds at most {MAX_ROOM} people")));
        }
        if !offer_allowed(offer) {
            return Err(Error::invalid("a call sends one track of sound and one of camera at most"));
        }
        let offer =
            SdpOffer::from_sdp_string(offer).map_err(|err| Error::invalid(format!("that offer isn't SDP: {err}")))?;
        // No audio level extension: it would tell this server, in the clear,
        // how loud each packet is, even in end-to-end encrypted calls, and
        // apps work out who's speaking themselves. Opus and VP8 only, so
        // every app can take every other's frames as they are.
        let mut rtc = Rtc::builder()
            .set_ice_lite(true)
            .clear_codecs()
            .enable_opus(true, false)
            .enable_vp8(true)
            .clear_extension_map()
            .set_extension(2, Extension::AbsoluteSendTime)
            .set_extension(3, Extension::TransportSequenceNumber)
            .set_extension(4, Extension::RtpMid)
            .set_extension(10, Extension::RtpStreamId)
            .set_extension(11, Extension::RepairedRtpStreamId)
            .build(now);
        for (proto, addr) in &candidates {
            let candidate = match proto {
                Protocol::Tcp => Candidate::builder().tcp().host(*addr).tcptype(TcpType::Passive).build(),
                _ => Candidate::builder().udp().host(*addr).build(),
            };
            match candidate {
                Ok(candidate) => {
                    rtc.add_local_candidate(candidate);
                }
                Err(err) => tracing::warn!(address = %addr, error = %err, "a media address can't be used"),
            }
        }
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|err| Error::invalid(format!("that offer didn't work: {err}")))?;
        for candidate in candidates {
            if !self.destinations.contains(&candidate) {
                self.destinations.retain(|(p, a)| !(*p == candidate.0 && a.is_ipv4() == candidate.1.is_ipv4()));
                self.destinations.push(candidate);
            }
        }

        self.make_way(&room, &participant, &session_id, now);

        let id = self.next_id;
        self.next_id += 1;
        let mut client = Client {
            id,
            room,
            participant,
            session_id,
            rtc,
            channel: None,
            pending: None,
            tracks_in: vec![],
            tracks_out: vec![],
            may,
            outbox: vec![],
            leaving: None,
            offers: VecDeque::new(),
            sent: (now, 0),
            filmed: (now, 0),
        };
        // Everyone else's sound and cameras, offered once the data channel is up.
        for other in self.clients.iter().filter(|c| c.room == client.room && c.leaving.is_none()) {
            for incoming in other.tracks_in.iter().filter(|t| t.shared) {
                client.tracks_out.push(TrackOut::new(Arc::downgrade(&incoming.track)));
            }
        }
        for bridge in self.bridges.iter().filter(|b| b.room == client.room) {
            client.tracks_out.push(TrackOut::new(Arc::downgrade(&bridge.track)));
        }
        self.clients.push(client);
        Ok(answer.to_sdp_string())
    }

    /// Everyone in a room but `participant`.
    fn in_room(&self, room: &str, participant: &str) -> usize {
        let clients = self.clients.iter().filter(|c| c.room == room && c.leaving.is_none() && c.rtc.is_alive());
        let bridges = self.bridges.iter().filter(|b| b.room == room);
        clients.filter(|c| c.participant != participant).count()
            + bridges.filter(|b| b.participant != participant).count()
    }

    /// Someone already here under this name (another of their devices, or
    /// the same app or program before its connection dropped) makes way.
    fn make_way(&mut self, room: &str, participant: &str, session_id: &str, now: Instant) {
        for client in &mut self.clients {
            if client.room == room && client.participant == participant {
                if client.session_id == session_id {
                    client.rtc.disconnect();
                } else {
                    client.leave(Signal::Replaced, now);
                }
            }
        }
        self.bridges.retain(|bridge| {
            let here = bridge.room == room && bridge.participant == participant;
            if here && bridge.session_id != session_id {
                bridge.end(Ending::Replaced);
            }
            !here
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn bridge(
        &mut self,
        room: String,
        participant: String,
        session_id: String,
        may: May,
        events: mpsc::Sender<Bridged>,
        now: Instant,
    ) -> Result<()> {
        // Voice channels only: direct-message calls are end-to-end encrypted.
        if !room.starts_with("s/") {
            return Err(Error::invalid("programs can only be in voice channels"));
        }
        if self.in_room(&room, &participant) >= MAX_ROOM {
            return Err(Error::ResourceExhausted(format!("a call holds at most {MAX_ROOM} people")));
        }
        self.make_way(&room, &participant, &session_id, now);
        let id = self.next_id;
        self.next_id += 1;
        let track = Arc::new(TrackIn {
            origin: id,
            participant: participant.clone(),
            mid: Mid::from("bridge"),
            kind: MediaKind::Audio,
        });
        for client in self.clients.iter_mut().filter(|c| c.room == room) {
            client.tracks_out.push(TrackOut::new(Arc::downgrade(&track)));
        }
        self.bridges.push(Bridge {
            id,
            room,
            participant,
            session_id,
            events,
            track,
            // Programs only ever get sound.
            may: May { video: false, ..may },
            queue: VecDeque::new(),
            started: now,
            next: now,
            said: (now, 0),
        });
        Ok(())
    }

    fn speak(&mut self, room: &str, participant: &str, session_id: &str, frames: Vec<Vec<u8>>) -> Result<usize> {
        let bridge = self
            .bridges
            .iter_mut()
            .find(|b| b.room == room && b.participant == participant && b.session_id == session_id)
            .ok_or_else(|| Error::FailedPrecondition("you're not in that call any more".into()))?;
        if frames.iter().any(|f| f.is_empty() || f.len() > MAX_FRAME) {
            return Err(Error::invalid(format!("each frame is one Opus packet, 1 to {MAX_FRAME} bytes")));
        }
        if bridge.queue.len() + frames.len() > MAX_QUEUED {
            return Err(Error::ResourceExhausted(
                "more than a second of sound is waiting to go out; send it as it plays".into(),
            ));
        }
        if !bridge.may.speak {
            return Ok(bridge.queue.len());
        }
        for frame in frames {
            if bridge.within_budget(frame.len()) {
                bridge.queue.push_back(frame);
            }
        }
        Ok(bridge.queue.len())
    }
}

impl Engine {
    /// Moves time on for every connection, sends what they have to send,
    /// and passes sound between them.
    async fn step(&mut self, now: Instant) {
        let mut outputs = Outputs::default();
        for client in &mut self.clients {
            if client.leaving.is_some_and(|at| now >= at) {
                client.rtc.disconnect();
            }
            if client.rtc.is_alive()
                && let Err(err) = client.rtc.handle_input(Input::Timeout(now))
            {
                tracing::debug!(client = client.id, error = %err, "a call connection failed");
                client.rtc.disconnect();
            }
        }
        let before = self.clients.len();
        self.clients.retain(|c| c.rtc.is_alive());
        if self.clients.len() != before {
            tracing::debug!(people = self.clients.len(), "someone hung up");
        }

        self.bridges.retain(|b| !b.events.is_closed());
        let mut said = Vec::new();
        for bridge in &mut self.bridges {
            while let Some((frame, time)) = bridge.due(now) {
                let who = (bridge.id, bridge.room.clone(), bridge.participant.clone(), bridge.track.mid);
                said.push((who, time, frame));
            }
        }
        for ((origin, room, participant, mid), time, frame) in said {
            for client in self.clients.iter_mut().filter(|c| c.room == room) {
                client.forward_said(origin, mid, now, time, &frame);
            }
            // Programs hear each other too.
            for bridge in self.bridges.iter().filter(|b| b.room == room && b.id != origin) {
                bridge.hear(&participant, &frame, time.numer() as u32);
            }
        }

        self.timeouts.clear();
        loop {
            for client in &mut self.clients {
                if let Some(at) = client.poll(&mut outputs) {
                    self.timeouts.push(at);
                }
            }
            if outputs.propagated.is_empty() {
                break;
            }
            while let Some(propagated) = outputs.propagated.pop_front() {
                self.propagate(propagated);
            }
            self.timeouts.clear();
        }
        while let Some(transmit) = outputs.transmits.pop_front() {
            self.transmit(transmit).await;
        }
    }

    fn propagate(&mut self, propagated: Propagated) {
        let origin_room =
            |clients: &[Client], id: ClientId| clients.iter().find(|c| c.id == id).map(|c| c.room.clone());
        match propagated {
            Propagated::TrackOpen(origin, track) => {
                let Some(room) = origin_room(&self.clients, origin) else { return };
                for client in self.clients.iter_mut().filter(|c| c.id != origin && c.room == room) {
                    client.tracks_out.push(TrackOut::new(track.clone()));
                }
            }
            Propagated::Media(origin, data, fresh) => {
                let Some(from) = self.clients.iter().find(|c| c.id == origin) else { return };
                let (room, participant) = (from.room.clone(), from.participant.clone());
                let now = Instant::now();
                let mut asks = Vec::new();
                for client in self.clients.iter_mut().filter(|c| c.id != origin && c.room == room) {
                    if let Some(rid) = client.forward(origin, &data, fresh, now)
                        && !asks.contains(&rid)
                    {
                        asks.push(rid);
                    }
                }
                if let Some(from) = self.clients.iter_mut().find(|c| c.id == origin) {
                    for rid in asks {
                        from.ask_keyframe(data.mid, Some(rid), KeyframeRequestKind::Fir);
                    }
                }
                // Programs get sound only, never a camera's frames.
                let audio = data.params.spec().codec.is_audio();
                let ticks = data.time.numer().saturating_mul(48_000) / u64::from(data.time.denom().max(1));
                for bridge in self.bridges.iter().filter(|b| audio && b.room == room && b.participant != participant) {
                    bridge.hear(&participant, &data.data, ticks as u32);
                }
            }
            Propagated::Keyframe(request, origin, mid, rid) => {
                if let Some(client) = self.clients.iter_mut().find(|c| c.id == origin) {
                    client.ask_keyframe(mid, rid, request.kind);
                }
            }
        }
    }

    async fn transmit(&mut self, transmit: str0m::net::Transmit) {
        match transmit.proto {
            Protocol::Udp => {
                if let Err(err) = self.udp.send_to(&transmit.contents, transmit.destination).await {
                    // Never the address: it's a participant's.
                    tracing::debug!(error = %err, "couldn't send a call packet");
                }
            }
            Protocol::Tcp => {
                if let Some(peer) = self.tcp_peers.get(&transmit.destination) {
                    let _ = peer.try_send(transmit.contents.to_vec());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_send_one_microphone_and_one_camera_at_most() {
        let sdp = |sections: &[(&str, &str)]| {
            let mut s = String::from("v=0\r\n");
            for (kind, dir) in sections {
                s += &format!("m={kind} 9 UDP/TLS/RTP/SAVPF 111\r\na={dir}\r\n");
            }
            s
        };
        assert!(offer_allowed(&sdp(&[("audio", "sendonly"), ("application", "sendrecv")])));
        assert!(offer_allowed(&sdp(&[("audio", "sendrecv"), ("application", "sendrecv"), ("audio", "recvonly")])));
        assert!(!offer_allowed(&sdp(&[("audio", "sendonly"), ("audio", "sendrecv")])), "two microphones");
        assert!(offer_allowed(&sdp(&[("audio", "sendonly"), ("video", "sendonly"), ("application", "sendrecv")])));
        assert!(!offer_allowed(&sdp(&[("video", "sendonly"), ("video", "sendonly")])), "two cameras");
        assert!(!offer_allowed(&sdp(&[("text", "sendonly")])));
        assert!(!offer_allowed(&sdp(&vec![("audio", "recvonly"); 2 * MAX_ROOM + 4])), "too many");
        assert!(!offer_allowed(&"a".repeat(MAX_SDP + 1)));
    }

    #[tokio::test]
    async fn programs_stay_out_of_direct_message_calls() {
        let config = MediaConfig { port: 0, addresses: vec![Advertised::parse("127.0.0.1").unwrap()] };
        let sfu = Sfu::start(config, CancellationToken::new()).await.unwrap();
        let may = May { speak: true, hear: true, video: true };
        assert!(sfu.bridge("d/conversation", "bot", "s1", may).await.is_err());
        assert!(sfu.bridge("s/server/channel", "bot", "s1", may).await.is_ok());
    }

    #[test]
    fn viewers_get_the_camera_size_nearest_what_they_asked() {
        let (l, m, h) = (Layer::Low.bit(), Layer::Medium.bit(), Layer::High.bit());
        assert_eq!(Layer::pick(Layer::High, l | m | h), Some(Layer::High));
        assert_eq!(Layer::pick(Layer::Medium, l | m | h), Some(Layer::Medium));
        assert_eq!(Layer::pick(Layer::Low, l | m | h), Some(Layer::Low));
        // A camera short on upload drops its biggest size first.
        assert_eq!(Layer::pick(Layer::High, l | m), Some(Layer::Medium));
        // Asked for smaller than anything there: the smallest there is.
        assert_eq!(Layer::pick(Layer::Low, m | h), Some(Layer::Medium));
        assert_eq!(Layer::pick(Layer::Off, l | m | h), None);
        assert_eq!(Layer::pick(Layer::High, 0), None);
        let asked: HashMap<String, Layer> = serde_json::from_str(r#"{"1": "h", "2": "off", "3": "l"}"#).unwrap();
        assert_eq!(asked["1"], Layer::High);
        assert_eq!(asked["2"], Layer::Off);
    }

    #[test]
    fn addresses_read_like_people_write_them() {
        let both = Advertised::parse("203.0.113.5").unwrap();
        assert!(both.udp && both.tcp && both.port.is_none());
        let tcp = Advertised::parse("tcp/proxy.example.net:12345").unwrap();
        assert!(!tcp.udp && tcp.tcp);
        assert_eq!((tcp.host.as_str(), tcp.port), ("proxy.example.net", Some(12345)));
        let v6 = Advertised::parse("2001:db8::1").unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("2001:db8::1", None));
        let v6 = Advertised::parse("udp/[2001:db8::1]:5000").unwrap();
        assert_eq!((v6.host.as_str(), v6.port, v6.tcp), ("2001:db8::1", Some(5000), false));
        assert_eq!(v6.describe(), "udp/[2001:db8::1]:5000");
        assert!(Advertised::parse("sctp/1.2.3.4").is_err());
        assert!(Advertised::parse("1.2.3.4:0").is_err());
        assert!(Advertised::parse("").is_err());
    }
}
