//! The call's connection to the media part: WebRTC through str0m, the same
//! library the media part runs. The app offers its microphone and the "fuwa"
//! data channel; the media part answers with where to reach it, and after
//! that leads with offers of its own as people's sound comes and goes.
//!
//! The only addresses this ever sends to are the ones in the media part's
//! answer: UDP straight to them, or ICE-TCP (RFC 4571 framing) to them
//! when UDP can't get through, as on hosts that only proxy TCP. The app's
//! own addresses aren't offered up front; the media part finds them from
//! the connectivity checks, as it does a browser's.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::ChannelId;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive, TcpType};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The largest packet read or framed.
const MOST_PACKET: usize = 2000;
/// The most people's sound a call keeps track of.
pub const MOST_STREAMS: usize = 64;
/// How long reaching the media part over TCP may take.
const TCP_CONNECT: Duration = Duration::from_secs(4);

/// What the media part says over the data channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// Its part is restarting (a deploy): join again with the same session.
    Restarting,
    /// You joined from somewhere else.
    Replaced,
    /// You were taken out of the call.
    Closed,
}

/// What happened on the connection.
#[derive(Debug)]
pub enum Happened {
    /// A packet of someone's sound, by whose stream it is (their account id,
    /// or "<id>-screen" for a shared screen's sound).
    Heard(String, Vec<u8>),
    /// A stream ended, so its sound can be let go.
    Gone(String),
    Signal(Signal),
    Connected,
    /// The connection broke; `for_good` when ICE gave up.
    Broken {
        for_good: bool,
    },
    /// It came back by itself after breaking.
    Restored,
}

/// One packet from a socket: the protocol, from where, to which of ours.
type Packet = (Protocol, SocketAddr, SocketAddr, Vec<u8>);

struct TcpPath {
    local: SocketAddr,
    remote: SocketAddr,
    out: mpsc::Sender<Vec<u8>>,
}

pub struct Link {
    rtc: Rtc,
    microphone: Mid,
    channel: Option<ChannelId>,
    pending: Option<SdpPendingOffer>,
    udp: Option<Arc<UdpSocket>>,
    /// The media part's UDP addresses: the only places UDP goes.
    udp_remotes: Vec<SocketAddr>,
    tcp: Vec<TcpPath>,
    packets: mpsc::Receiver<Packet>,
    packets_in: mpsc::Sender<Packet>,
    tasks: Vec<JoinHandle<()>>,
    /// Whose stream each received track is.
    streams: HashMap<Mid, String>,
    sent: u64,
}

impl Drop for Link {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Where the media part said to reach it, from the a=candidate lines of its answer.
pub fn candidates(sdp: &str) -> Vec<(Protocol, SocketAddr)> {
    let mut out = Vec::new();
    for line in sdp.lines() {
        let Some(rest) = line.trim().strip_prefix("a=candidate:") else { continue };
        // foundation component protocol priority address port "typ" type ...
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 8 || parts[6] != "typ" {
            continue;
        }
        let protocol = match parts[2].to_ascii_lowercase().as_str() {
            "udp" => Protocol::Udp,
            "tcp" => Protocol::Tcp,
            _ => continue,
        };
        let (Ok(ip), Ok(port)) = (parts[4].parse::<IpAddr>(), parts[5].parse::<u16>()) else { continue };
        let address = SocketAddr::new(ip, port);
        if port != 0 && !out.contains(&(protocol, address)) {
            out.push((protocol, address));
        }
    }
    out
}

/// An SDP without its candidate lines.
fn without_candidates(sdp: &str) -> String {
    let mut out = String::with_capacity(sdp.len());
    for line in sdp.split_inclusive('\n') {
        if !line.starts_with("a=candidate:") {
            out.push_str(line);
        }
    }
    out
}

/// The address this computer would reach `remote` from (no packet is sent).
fn route_to(remote: SocketAddr) -> Option<IpAddr> {
    let any: SocketAddr = if remote.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }.parse().ok()?;
    let socket = std::net::UdpSocket::bind(any).ok()?;
    socket.connect(remote).ok()?;
    socket.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified())
}

impl Link {
    /// A connection that sends the microphone and opens the data channel,
    /// and the offer for JoinVoice.
    pub fn offer() -> (Self, String) {
        let mut rtc = Rtc::builder().build(Instant::now());
        let mut change = rtc.sdp_api();
        let microphone = change.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
        change.add_channel("fuwa".into());
        let (offer, pending) = change.apply().expect("an offer with a track and a channel");
        let (packets_in, packets) = mpsc::channel(512);
        let link = Self {
            rtc,
            microphone,
            channel: None,
            pending: Some(pending),
            udp: None,
            udp_remotes: Vec::new(),
            tcp: Vec::new(),
            packets,
            packets_in,
            tasks: Vec::new(),
            streams: HashMap::new(),
            sent: 0,
        };
        (link, offer.to_sdp_string())
    }

    /// Takes the media part's answer and opens the ways to reach it.
    pub async fn connect(&mut self, answer: &str) -> Result<(), String> {
        let remote = candidates(answer);
        if remote.is_empty() {
            return Err("the media part gave no address".into());
        }
        let udp_remotes: Vec<SocketAddr> =
            remote.iter().filter(|(p, _)| *p == Protocol::Udp).map(|(_, a)| *a).collect();
        if let Some(first) = udp_remotes.iter().find(|a| a.is_ipv4()).or(udp_remotes.first()) {
            let any = if first.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
            if let Ok(socket) = UdpSocket::bind(any).await {
                let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
                let socket = Arc::new(socket);
                let mut added = Vec::new();
                for remote in udp_remotes.iter().filter(|a| a.is_ipv4() == first.is_ipv4()) {
                    let Some(ip) = route_to(*remote) else { continue };
                    let local = SocketAddr::new(ip, port);
                    if !added.contains(&local)
                        && let Ok(candidate) = Candidate::host(local, "udp")
                    {
                        self.rtc.add_local_candidate(candidate);
                        added.push(local);
                    }
                }
                if !added.is_empty() {
                    self.tasks.push(tokio::spawn(read_udp(socket.clone(), self.packets_in.clone())));
                    self.udp = Some(socket);
                    self.udp_remotes = udp_remotes.clone();
                }
            }
        }
        let tcp_remotes = remote.iter().filter(|(p, _)| *p == Protocol::Tcp).map(|(_, a)| *a);
        let connecting = tcp_remotes.map(|remote| async move {
            let stream = tokio::time::timeout(TCP_CONNECT, TcpStream::connect(remote)).await.ok()?.ok()?;
            let _ = stream.set_nodelay(true);
            Some((remote, stream))
        });
        for (remote, stream) in futures::future::join_all(connecting).await.into_iter().flatten() {
            let Ok(local) = stream.local_addr() else { continue };
            let Ok(candidate) = Candidate::builder().tcp().host(local).tcptype(TcpType::Active).build() else {
                continue;
            };
            self.rtc.add_local_candidate(candidate);
            let (out, outgoing) = mpsc::channel(256);
            self.tasks.push(tokio::spawn(serve_tcp(stream, local, remote, outgoing, self.packets_in.clone())));
            self.tcp.push(TcpPath { local, remote, out });
        }
        if self.udp.is_none() && self.tcp.is_empty() {
            return Err("couldn't reach the media part".into());
        }
        let answer =
            SdpAnswer::from_sdp_string(answer).map_err(|_| "the media part's answer didn't read".to_string())?;
        let pending = self.pending.take().ok_or("answered twice")?;
        self.rtc
            .sdp_api()
            .accept_answer(pending, answer)
            .map_err(|_| "the media part's answer didn't fit".to_string())?;
        Ok(())
    }

    /// Sends what's ready, collects what happened, and says when to come back.
    pub async fn poll(&mut self, happened: &mut Vec<Happened>) -> Instant {
        loop {
            match self.rtc.poll_output() {
                Ok(Output::Timeout(at)) => return at,
                Ok(Output::Transmit(t)) => self.transmit(t.proto, t.source, t.destination, &t.contents).await,
                Ok(Output::Event(event)) => self.on_event(event, happened),
                Err(_) => {
                    happened.push(Happened::Broken { for_good: true });
                    return Instant::now() + Duration::from_secs(1);
                }
            }
        }
    }

    async fn transmit(&self, proto: Protocol, source: SocketAddr, destination: SocketAddr, contents: &[u8]) {
        match proto {
            Protocol::Udp => {
                if let Some(udp) = &self.udp
                    && self.udp_remotes.contains(&destination)
                {
                    let _ = udp.send_to(contents, destination).await;
                }
            }
            Protocol::Tcp => {
                // Each connection carries only its own pair.
                if let Some(path) = self.tcp.iter().find(|p| p.local == source && p.remote == destination) {
                    let _ = path.out.try_send(contents.to_vec());
                }
            }
            _ => {}
        }
    }

    /// The next packet from the media part; never ends while the link lives.
    pub async fn packet(&mut self) -> Packet {
        match self.packets.recv().await {
            Some(packet) => packet,
            // Can't happen: the link holds a sender of its own.
            None => std::future::pending().await,
        }
    }

    pub fn receive(&mut self, (proto, source, destination, bytes): Packet) {
        let Ok(receive) = Receive::new(proto, source, destination, &bytes) else { return };
        let input = Input::Receive(Instant::now(), receive);
        if self.rtc.accepts(&input) {
            let _ = self.rtc.handle_input(input);
        }
    }

    pub fn tick(&mut self) {
        let _ = self.rtc.handle_input(Input::Timeout(Instant::now()));
    }

    pub fn is_connected(&self) -> bool {
        self.rtc.is_connected()
    }

    /// Sends 20 ms of the microphone, encoded.
    pub fn speak(&mut self, packet: Vec<u8>) {
        let time = MediaTime::new(self.sent * 960, str0m::media::Frequency::FORTY_EIGHT_KHZ);
        self.sent += 1;
        let Some(writer) = self.rtc.writer(self.microphone) else { return };
        let Some(pt) = writer.payload_params().find(|p| p.spec().codec == Codec::Opus).map(|p| p.pt()) else {
            return;
        };
        let _ = writer.write(pt, Instant::now(), time, packet);
    }

    /// Skips the time the microphone was quiet (muted), so the timestamps
    /// say how long the gap was.
    pub fn skip(&mut self) {
        self.sent += 1;
    }

    fn say(&mut self, json: &serde_json::Value) {
        let Some(id) = self.channel else { return };
        if let Some(mut channel) = self.rtc.channel(id) {
            let _ = channel.write(false, json.to_string().as_bytes());
        }
    }

    fn on_event(&mut self, event: Event, happened: &mut Vec<Happened>) {
        match event {
            Event::Connected => happened.push(Happened::Connected),
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                happened.push(Happened::Broken { for_good: false });
            }
            Event::IceConnectionStateChange(IceConnectionState::Connected | IceConnectionState::Completed) => {
                happened.push(Happened::Restored);
            }
            Event::ChannelOpen(id, label) if label == "fuwa" => self.channel = Some(id),
            Event::ChannelData(data) => {
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data.data) else { return };
                match value["type"].as_str() {
                    // The media part leads: someone's sound came or went.
                    Some("offer") => {
                        let Some(offer) = value["sdp"].as_str().and_then(|s| SdpOffer::from_sdp_string(s).ok()) else {
                            return;
                        };
                        let Some(answer) = self.answer_offer(offer) else {
                            happened.push(Happened::Broken { for_good: true });
                            return;
                        };
                        self.say(&serde_json::json!({ "type": "answer", "sdp": answer }));
                        self.forget_inactive(happened);
                    }
                    Some("restarting") => happened.push(Happened::Signal(Signal::Restarting)),
                    Some("replaced") => happened.push(Happened::Signal(Signal::Replaced)),
                    Some("closed") => happened.push(Happened::Signal(Signal::Closed)),
                    _ => {}
                }
            }
            Event::MediaAdded(added) if added.kind == MediaKind::Audio && added.mid != self.microphone => {
                if self.streams.len() >= MOST_STREAMS {
                    return;
                }
                if let Some(media) = self.rtc.media(added.mid) {
                    self.streams.insert(added.mid, media.stream_id().to_string());
                }
            }
            Event::MediaData(data) if data.mid != self.microphone => {
                let who = match self.streams.get(&data.mid) {
                    Some(who) => who.clone(),
                    None if self.streams.len() >= MOST_STREAMS => return,
                    None => match self.rtc.media(data.mid) {
                        Some(media) => {
                            let who = media.stream_id().to_string();
                            self.streams.insert(data.mid, who.clone());
                            who
                        }
                        None => return,
                    },
                };
                happened.push(Happened::Heard(who, data.data.to_vec()));
            }
            _ => {}
        }
    }

    /// The answer to one of the media part's offers, without this
    /// computer's addresses: str0m would list them, and the media part
    /// learns the one it needs from the connectivity checks anyway.
    fn answer_offer(&mut self, offer: SdpOffer) -> Option<String> {
        let answer = self.rtc.sdp_api().accept_offer(offer).ok()?;
        Some(without_candidates(&answer.to_sdp_string()))
    }

    /// After a renegotiation: tracks the media part stopped sending on are
    /// someone who left, so their sound goes.
    fn forget_inactive(&mut self, happened: &mut Vec<Happened>) {
        let rtc = &self.rtc;
        let gone: Vec<Mid> = self
            .streams
            .keys()
            .filter(|mid| rtc.media(**mid).is_none_or(|m| m.direction() == Direction::Inactive))
            .copied()
            .collect();
        for mid in gone {
            if let Some(who) = self.streams.remove(&mid) {
                happened.push(Happened::Gone(who));
            }
        }
    }
}

async fn read_udp(socket: Arc<UdpSocket>, packets: mpsc::Sender<Packet>) {
    let Ok(local) = socket.local_addr() else { return };
    let mut buffer = vec![0u8; MOST_PACKET];
    // What the ICE agent matches is the candidate's address, not the
    // unspecified one the socket is bound to: the route to each sender.
    let mut routes: HashMap<IpAddr, SocketAddr> = HashMap::new();
    loop {
        let Ok((n, source)) = socket.recv_from(&mut buffer).await else { return };
        // Anyone can send to the port; only the media part's matter, so a few are enough.
        if routes.len() > 8 && !routes.contains_key(&source.ip()) {
            routes.clear();
        }
        let local = *routes.entry(source.ip()).or_insert_with(|| match route_to(source) {
            Some(ip) => SocketAddr::new(ip, local.port()),
            None => local,
        });
        if packets.send((Protocol::Udp, source, local, buffer[..n].to_vec())).await.is_err() {
            return;
        }
    }
}

/// One ICE-TCP connection: frames are a 2-byte length then the packet (RFC 4571).
async fn serve_tcp(
    stream: TcpStream,
    local: SocketAddr,
    remote: SocketAddr,
    mut outgoing: mpsc::Receiver<Vec<u8>>,
    packets: mpsc::Sender<Packet>,
) {
    let (mut reader, mut writer) = stream.into_split();
    let write = async move {
        while let Some(packet) = outgoing.recv().await {
            let Ok(length) = u16::try_from(packet.len()) else { continue };
            let mut framed = Vec::with_capacity(packet.len() + 2);
            framed.extend_from_slice(&length.to_be_bytes());
            framed.extend_from_slice(&packet);
            if writer.write_all(&framed).await.is_err() {
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
            let length = usize::from(u16::from_be_bytes(length));
            if length == 0 || length > MOST_PACKET {
                return;
            }
            let mut packet = vec![0; length];
            if reader.read_exact(&mut packet).await.is_err() {
                return;
            }
            if packets.send((Protocol::Tcp, remote, local, packet)).await.is_err() {
                return;
            }
        }
    };
    tokio::select! {
        _ = write => {}
        _ = read => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_parts_addresses_come_from_its_answer() {
        let sdp = "v=0\r\n\
            a=candidate:1 1 udp 2130706431 203.0.113.7 50000 typ host\r\n\
            a=candidate:2 1 TCP 2105458943 203.0.113.7 27725 typ host tcptype passive\r\n\
            a=candidate:1 1 udp 2130706431 203.0.113.7 50000 typ host\r\n\
            a=candidate:3 1 udp 2130706431 2001:db8::1 50000 typ host\r\n\
            a=candidate:4 1 sctp 1 203.0.113.7 1 typ host\r\n\
            a=candidate:5 1 udp 1 not-an-address 1 typ host\r\n";
        let found = candidates(sdp);
        assert_eq!(
            found,
            vec![
                (Protocol::Udp, "203.0.113.7:50000".parse().unwrap()),
                (Protocol::Tcp, "203.0.113.7:27725".parse().unwrap()),
                (Protocol::Udp, "[2001:db8::1]:50000".parse().unwrap()),
            ]
        );
    }

    /// The media part leads with offers as people come and go; the app's
    /// answers to them name none of its addresses, not even the ones it
    /// connects from.
    #[test]
    fn answers_to_the_media_parts_offers_keep_this_computers_addresses_out() {
        let (mut link, offer) = Link::offer();
        let mut media = Rtc::builder().set_ice_lite(true).build(Instant::now());
        media.add_local_candidate(Candidate::host("203.0.113.7:50000".parse().unwrap(), "udp").unwrap());
        let answer = media.sdp_api().accept_offer(SdpOffer::from_sdp_string(&offer).unwrap()).unwrap();
        link.rtc.add_local_candidate(Candidate::host("192.168.1.23:40000".parse().unwrap(), "udp").unwrap());
        let pending = link.pending.take().unwrap();
        link.rtc.sdp_api().accept_answer(pending, answer).unwrap();

        let mut change = media.sdp_api();
        change.add_media(MediaKind::Audio, Direction::SendOnly, Some("someone".into()), None, None);
        let (offer, pending) = change.apply().unwrap();
        let raw = link.rtc.sdp_api().accept_offer(SdpOffer::from_sdp_string(&offer.to_sdp_string()).unwrap()).unwrap();
        assert!(raw.to_sdp_string().contains("192.168.1.23"), "str0m would have said it");
        media.sdp_api().accept_answer(pending, raw).unwrap();

        let mut change = media.sdp_api();
        change.add_media(MediaKind::Audio, Direction::SendOnly, Some("someone-else".into()), None, None);
        let (offer, _) = change.apply().unwrap();
        let answer = link.answer_offer(SdpOffer::from_sdp_string(&offer.to_sdp_string()).unwrap()).unwrap();
        assert!(!answer.contains("a=candidate") && !answer.contains("192.168.1.23"), "{answer}");
        assert!(SdpAnswer::from_sdp_string(&answer).is_ok(), "still an answer");
    }

    #[test]
    fn the_offer_sends_a_microphone_and_opens_the_data_channel() {
        let (_, offer) = Link::offer();
        assert!(offer.contains("m=audio") && offer.contains("a=sendonly") && offer.contains("opus"));
        assert!(offer.contains("m=application"), "the data channel");
        assert!(!offer.contains("m=video"), "sound only");
        assert!(!offer.contains("a=candidate"), "none of this computer's addresses");
    }
}
