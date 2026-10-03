//! An app's end of a call, for the tests: WebRTC through str0m.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::ChannelId;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid, Rid, Simulcast, SimulcastLayer};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use tokio::net::UdpSocket;

/// The sizes a test camera sends, as browsers do.
pub const SIZES: [&str; 3] = ["l", "m", "h"];

/// An app's end of a call: its own UDP socket and WebRTC connection,
/// sending a microphone track (and a camera, in three sizes, if asked) and
/// opening the "fuwa" data channel.
pub struct Peer {
    pub rtc: Rtc,
    socket: UdpSocket,
    mic: Mid,
    camera: Option<Mid>,
    channel: Option<ChannelId>,
    pending: Option<SdpPendingOffer>,
    /// Sound received, by whose it is (the stream id).
    pub heard: Vec<(Mid, Vec<u8>)>,
    /// Camera frames received: the track, the frame, and whether it's a keyframe.
    pub seen: Vec<(Mid, Vec<u8>, bool)>,
    pub signals: Vec<String>,
    sent: u64,
    filmed: u64,
    /// Sizes asked for a keyframe.
    keyframes: Vec<Rid>,
}

impl Peer {
    pub async fn new() -> (Self, String) {
        Self::start(false).await
    }

    /// One that sends a camera too, in three sizes.
    pub async fn with_camera() -> (Self, String) {
        Self::start(true).await
    }

    async fn start(filming: bool) -> (Self, String) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut rtc = Rtc::builder().build(Instant::now());
        rtc.add_local_candidate(Candidate::host(socket.local_addr().unwrap(), "udp").unwrap());
        let mut change = rtc.sdp_api();
        let mic = change.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
        let camera = filming.then(|| {
            let mut simulcast = Simulcast::new();
            for size in SIZES {
                simulcast.add_send_layer(SimulcastLayer::new(size));
            }
            change.add_media(MediaKind::Video, Direction::SendOnly, None, None, Some(simulcast))
        });
        change.add_channel("fuwa".into());
        let (offer, pending) = change.apply().unwrap();
        let peer = Self {
            rtc,
            socket,
            mic,
            camera,
            channel: None,
            pending: Some(pending),
            heard: vec![],
            seen: vec![],
            signals: vec![],
            sent: 0,
            filmed: 0,
            keyframes: SIZES.iter().map(|s| Rid::from(*s)).collect(),
        };
        (peer, offer.to_sdp_string())
    }

    /// Says something to the media part over the data channel.
    pub fn say(&mut self, json: serde_json::Value) {
        let channel = self.channel.expect("the data channel is open");
        self.rtc.channel(channel).unwrap().write(false, json.to_string().as_bytes()).unwrap();
    }

    /// The sizes of camera frames seen since `from`, in order (the frames say which they are).
    pub fn sizes_seen(&self, from: usize) -> Vec<String> {
        self.seen[from..].iter().map(|(_, frame, _)| String::from_utf8_lossy(&frame[1..2]).to_string()).collect()
    }

    pub fn answer(&mut self, sdp: &str) {
        let answer = SdpAnswer::from_sdp_string(sdp).unwrap();
        self.rtc.sdp_api().accept_answer(self.pending.take().unwrap(), answer).unwrap();
    }

    /// Runs the connection for a while: sends, receives, answers the media
    /// part's offers, and speaks 20 ms of "sound" at a time once it can.
    pub async fn run_for(&mut self, time: Duration) {
        let until = Instant::now() + time;
        let mut buffer = vec![0u8; 2000];
        let mut next_frame = Instant::now();
        while Instant::now() < until {
            let timeout = loop {
                match self.rtc.poll_output().unwrap() {
                    Output::Timeout(at) => break at,
                    Output::Transmit(t) => {
                        self.socket.send_to(&t.contents, t.destination).await.unwrap();
                    }
                    Output::Event(event) => self.on_event(event),
                }
            };
            if Instant::now() >= next_frame && self.rtc.is_connected() {
                self.speak();
                // A camera frame every other sound frame: 25 a second.
                if self.sent.is_multiple_of(2) {
                    self.film();
                }
                next_frame = Instant::now() + Duration::from_millis(20);
            }
            let wait = timeout.min(next_frame).min(until).saturating_duration_since(Instant::now());
            let wait = wait.max(Duration::from_millis(1));
            match tokio::time::timeout(wait, self.socket.recv_from(&mut buffer)).await {
                Ok(Ok((n, source))) => {
                    let receive = Receive::new(Protocol::Udp, source, self.socket.local_addr().unwrap(), &buffer[..n]);
                    if let Ok(receive) = receive {
                        self.rtc.handle_input(Input::Receive(Instant::now(), receive)).unwrap();
                    }
                }
                _ => self.rtc.handle_input(Input::Timeout(Instant::now())).unwrap(),
            }
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::ChannelOpen(id, label) if label == "fuwa" => self.channel = Some(id),
            Event::ChannelData(data) => {
                let text = String::from_utf8(data.data).unwrap();
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["type"] == "offer" {
                    let offer = SdpOffer::from_sdp_string(value["sdp"].as_str().unwrap()).unwrap();
                    let answer = self.rtc.sdp_api().accept_offer(offer).unwrap();
                    let json = serde_json::json!({ "type": "answer", "sdp": answer.to_sdp_string() }).to_string();
                    self.rtc.channel(data.id).unwrap().write(false, json.as_bytes()).unwrap();
                }
                self.signals.push(value["type"].as_str().unwrap_or_default().to_string());
            }
            Event::MediaData(data) if data.params.spec().codec.is_video() => {
                let keyframe = data.is_keyframe();
                self.seen.push((data.mid, data.data.to_vec(), keyframe));
            }
            Event::MediaData(data) => self.heard.push((data.mid, data.data.to_vec())),
            Event::KeyframeRequest(request) => {
                if let Some(rid) = request.rid
                    && !self.keyframes.contains(&rid)
                {
                    self.keyframes.push(rid);
                }
            }
            _ => {}
        }
    }

    /// A frame of each size. Not real VP8, but its first byte says
    /// keyframe or not as VP8's does, and the next says which size it is.
    fn film(&mut self) {
        let Some(camera) = self.camera else { return };
        let time = MediaTime::new(self.filmed * 3600, str0m::media::Frequency::NINETY_KHZ);
        self.filmed += 1;
        for size in SIZES {
            let rid = Rid::from(size);
            let keyframe = self.keyframes.contains(&rid);
            self.keyframes.retain(|r| *r != rid);
            let Some(writer) = self.rtc.writer(camera) else { return };
            let Some(pt) = writer.payload_params().find(|p| p.spec().codec == Codec::Vp8).map(|p| p.pt()) else {
                return;
            };
            let mut frame = vec![if keyframe { 0x00 } else { 0x01 }];
            frame.extend(format!("{size} frame {}", self.filmed).into_bytes());
            writer.rid(rid).write(pt, Instant::now(), time, frame).unwrap();
        }
    }

    fn speak(&mut self) {
        let Some(writer) = self.rtc.writer(self.mic) else { return };
        let Some(pt) = writer.payload_params().find(|p| p.spec().codec == Codec::Opus).map(|p| p.pt()) else {
            return;
        };
        let time = MediaTime::new(self.sent * 960, str0m::media::Frequency::FORTY_EIGHT_KHZ);
        self.sent += 1;
        // Not real Opus, but the media part never looks inside.
        let frame = format!("frame {}", self.sent).into_bytes();
        writer.write(pt, Instant::now(), time, frame).unwrap();
    }
}

/// Runs both peers side by side for a while.
pub async fn talk(a: &mut Peer, b: &mut Peer, time: Duration) {
    tokio::join!(a.run_for(time), b.run_for(time));
}
