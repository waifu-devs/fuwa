//! The media part: carries calls' sound over WebRTC, as an SFU (selective
//! forwarding unit). Everyone in a room sends their microphone here once, and
//! gets everyone else's sound back as tracks of their own; nothing is mixed,
//! decoded or kept. Direct-message calls arrive end-to-end encrypted by the
//! apps, and are passed on just the same.
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

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Once, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::{ChannelData, ChannelId};
use str0m::media::{Direction, KeyframeRequest, KeyframeRequestKind, MediaData, MediaKind, Mid};
use str0m::net::{Protocol, Receive, TcpType};
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
}

enum Command {
    Open {
        room: String,
        participant: String,
        session_id: String,
        offer: String,
        may_speak: bool,
        may_hear: bool,
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
        may_speak: bool,
        may_hear: bool,
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
    pub async fn open(
        &self,
        room: &str,
        participant: &str,
        session_id: &str,
        offer: &str,
        may_speak: bool,
        may_hear: bool,
    ) -> Result<String> {
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
            may_speak,
            may_hear,
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

    /// Changes whether someone may speak and hear, saying whether they (in
    /// `session_id`, when given) have a connection here at all.
    pub async fn update(
        &self,
        room: &str,
        participant: &str,
        session_id: Option<&str>,
        may_speak: bool,
        may_hear: bool,
    ) -> Result<bool> {
        let (reply, answer) = oneshot::channel();
        let (room, participant, session_id) = (room.into(), participant.into(), session_id.map(Into::into));
        self.send(Command::Update { room, participant, session_id, may_speak, may_hear, reply }).await?;
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

struct TrackOut {
    from: Weak<TrackIn>,
    state: TrackState,
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
    tracks_in: Vec<(Arc<TrackIn>, Option<Instant>)>,
    tracks_out: Vec<TrackOut>,
    may_speak: bool,
    may_hear: bool,
    /// Signals waiting for the data channel to open.
    outbox: Vec<Signal>,
    /// Being let go: it hangs up at this time, and takes no part meanwhile.
    leaving: Option<Instant>,
    /// When its recent offers came, to cap renegotiation.
    offers: VecDeque<Instant>,
    /// Sound it sent this second: (since, bytes).
    sent: (Instant, usize),
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
const MAX_FRAME: usize = 1500;

/// Whether an app's offer asks for what calls allow: sending one audio
/// track at most (video comes later), receiving the others' tracks, and the
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
    sections.len() <= MAX_ROOM + 2
        && sections.iter().all(|(k, _)| matches!(*k, "audio" | "video" | "application"))
        && sending("audio") <= 1
        && sending("video") == 0
        && sections.iter().filter(|(k, _)| *k == "application").count() <= 1
}

/// What one connection's output means for the others.
enum Propagated {
    TrackOpen(ClientId, Weak<TrackIn>),
    Media(ClientId, Box<MediaData>),
    Keyframe(KeyframeRequest, ClientId, Mid),
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
            // One track of sound from each app; anything more it can't send.
            Event::MediaAdded(added) if added.kind != MediaKind::Audio || !self.tracks_in.is_empty() => {}
            Event::MediaAdded(added) => {
                let track = Arc::new(TrackIn {
                    origin: self.id,
                    participant: self.participant.clone(),
                    mid: added.mid,
                    kind: added.kind,
                });
                outputs.propagated.push_back(Propagated::TrackOpen(self.id, Arc::downgrade(&track)));
                self.tracks_in.push((track, None));
            }
            Event::MediaData(data) => {
                if !self.tracks_in.iter().any(|(t, _)| t.mid == data.mid) || !self.within_budget(data.data.len()) {
                    return;
                }
                if !data.contiguous {
                    self.ask_keyframe(data.mid);
                }
                if self.may_speak && self.leaving.is_none() {
                    outputs.propagated.push_back(Propagated::Media(self.id, Box::new(data)));
                }
            }
            Event::KeyframeRequest(request) => {
                let from = self.tracks_out.iter().find(|t| t.mid() == Some(request.mid)).and_then(|t| t.from.upgrade());
                if let Some(from) = from {
                    outputs.propagated.push_back(Propagated::Keyframe(request, from.origin, from.mid));
                }
            }
            _ => {}
        }
    }

    /// Whether a frame this size fits in what one person may send.
    fn within_budget(&mut self, len: usize) -> bool {
        if len > MAX_FRAME {
            return false;
        }
        let now = Instant::now();
        if now.duration_since(self.sent.0) >= Duration::from_secs(1) {
            self.sent = (now, 0);
        }
        self.sent.1 += len;
        self.sent.1 <= MAX_BYTES_PER_SECOND
    }

    fn ask_keyframe(&mut self, mid: Mid) {
        let Some((_, asked)) = self.tracks_in.iter_mut().find(|(t, _)| t.mid == mid) else { return };
        if asked.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        *asked = Some(Instant::now());
        if let Some(mut writer) = self.rtc.writer(mid) {
            let _ = writer.request_keyframe(None, KeyframeRequestKind::Fir);
        }
    }

    fn forward(&mut self, origin: ClientId, data: &MediaData) {
        if !self.may_hear || self.leaving.is_some() {
            return;
        }
        let mid = self.tracks_out.iter().find_map(|out| {
            let from = out.from.upgrade()?;
            (from.origin == origin && from.mid == data.mid).then(|| out.mid()).flatten()
        });
        let Some(mid) = mid else { return };
        let Some(writer) = self.rtc.writer(mid) else { return };
        let Some(pt) = writer.match_params(data.params) else { return };
        if let Err(err) = writer.write(pt, data.network_time, data.time, data.data.clone()) {
            tracing::debug!(client = self.id, error = %err, "couldn't pass sound on");
            self.rtc.disconnect();
        }
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
            Command::Open { room, participant, session_id, offer, may_speak, may_hear, candidates, reply } => {
                let answer = self.open(room, participant, session_id, &offer, may_speak, may_hear, candidates, now);
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
            }
            Command::Update { room, participant, session_id, may_speak, may_hear, reply } => {
                let mut connected = false;
                for client in &mut self.clients {
                    if client.room == room && client.participant == participant {
                        client.may_speak = may_speak;
                        client.may_hear = may_hear;
                        connected |=
                            client.leaving.is_none() && session_id.as_ref().is_none_or(|s| *s == client.session_id);
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
        may_speak: bool,
        may_hear: bool,
        candidates: Vec<(Protocol, SocketAddr)>,
        now: Instant,
    ) -> Result<String> {
        let others = self.clients.iter().filter(|c| c.room == room && c.leaving.is_none() && c.rtc.is_alive());
        if others.filter(|c| c.participant != participant).count() >= MAX_ROOM {
            return Err(Error::ResourceExhausted(format!("a call holds at most {MAX_ROOM} people")));
        }
        if !offer_allowed(offer) {
            return Err(Error::invalid("a call sends one track of sound at most"));
        }
        let offer =
            SdpOffer::from_sdp_string(offer).map_err(|err| Error::invalid(format!("that offer isn't SDP: {err}")))?;
        let mut rtc = Rtc::builder().set_ice_lite(true).build(now);
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

        // Someone already here under this name (another of their devices, or
        // the same app before its connection dropped) makes way.
        for client in &mut self.clients {
            if client.room == room && client.participant == participant {
                if client.session_id == session_id {
                    client.rtc.disconnect();
                } else {
                    client.leave(Signal::Replaced, now);
                }
            }
        }

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
            may_speak,
            may_hear,
            outbox: vec![],
            leaving: None,
            offers: VecDeque::new(),
            sent: (now, 0),
        };
        // Everyone else's sound, offered once the data channel is up.
        for other in self.clients.iter().filter(|c| c.room == client.room && c.leaving.is_none()) {
            for (track, _) in &other.tracks_in {
                client.tracks_out.push(TrackOut { from: Arc::downgrade(track), state: TrackState::ToOpen });
            }
        }
        self.clients.push(client);
        Ok(answer.to_sdp_string())
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
                    client.tracks_out.push(TrackOut { from: track.clone(), state: TrackState::ToOpen });
                }
            }
            Propagated::Media(origin, data) => {
                let Some(room) = origin_room(&self.clients, origin) else { return };
                for client in self.clients.iter_mut().filter(|c| c.id != origin && c.room == room) {
                    client.forward(origin, &data);
                }
            }
            Propagated::Keyframe(request, origin, mid) => {
                if let Some(client) = self.clients.iter_mut().find(|c| c.id == origin)
                    && let Some(mut writer) = client.rtc.writer(mid)
                {
                    let _ = writer.request_keyframe(None, request.kind);
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
    fn offers_send_one_track_of_sound_at_most() {
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
        assert!(!offer_allowed(&sdp(&[("audio", "sendonly"), ("video", "sendonly")])), "video isn't here yet");
        assert!(!offer_allowed(&sdp(&[("text", "sendonly")])));
        assert!(!offer_allowed(&sdp(&vec![("audio", "recvonly"); MAX_ROOM + 3])), "too many");
        assert!(!offer_allowed(&"a".repeat(MAX_SDP + 1)));
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
