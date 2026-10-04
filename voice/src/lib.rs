//! Hear and talk in fuwa voice channels from a program: agents, bots and
//! apps, without WebRTC.
//!
//! ```no_run
//! # async fn run() -> Result<(), fuwa_voice::Error> {
//! let client = fuwa_voice::Client::connect("https://fuwa.chat", "<agent token>").await?;
//! let (mut heard, speaker) = client.join("<server id>", "<voice channel id>").await?;
//! while let Some(frame) = heard.next().await? {
//!     println!("{} said {} bytes of Opus", frame.user_id, frame.opus.len());
//!     // Say something back: frames of Opus, 48 kHz, 20 ms each.
//!     speaker.say(vec![frame.opus]).await?;
//! }
//! # Ok(()) }
//! ```
//!
//! The program is in the channel as its own account, with that account's
//! permissions (CONNECT to join, SPEAK to be heard), and everyone sees it
//! there, with an AGENT badge when it's an agent. Sound comes and goes as
//! Opus frames: decode and encode them with any Opus library (48 kHz, 20 ms
//! frames, mono or stereo). Each person's frames come labelled with whose
//! they are, so a program can tell who said what.
//!
//! Under the hood it's two calls of fuwa's API, `CallService.ListenVoice`
//! (a stream) and `CallService.SpeakVoice`, so any language with gRPC can do
//! the same: see docs/calls.md in the fuwa repository.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Request, Status, Streaming};

/// fuwa's API types, as generated from its protos (`fuwa.v1`).
pub mod pb {
    tonic::include_proto!("fuwa.v1");
}

pub type Error = Status;

/// Frames sent in one go: 100 ms of sound.
const BATCH: usize = 5;
/// Keep about this much sound waiting on the instance, so it never runs dry.
const AHEAD: u32 = 10;
const FRAME: Duration = Duration::from_millis(20);

#[derive(Clone)]
struct Bearer(MetadataValue<Ascii>);

impl Interceptor for Bearer {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert("authorization", self.0.clone());
        Ok(request)
    }
}

type Calls = pb::call_service_client::CallServiceClient<InterceptedService<Channel, Bearer>>;

/// A connection to a fuwa instance, as one account (an agent's token, say).
#[derive(Clone)]
pub struct Client {
    calls: Calls,
}

impl Client {
    /// Connects to an instance at its address (`https://fuwa.chat`), with
    /// the token the account signs in with (an agent's, from Settings, Agents).
    /// Plain `http://` is only for this machine (localhost, 127.0.0.1, ::1):
    /// anywhere else, the token would cross the network readable.
    pub async fn connect(url: &str, token: &str) -> Result<Self, Error> {
        if !secure_enough(url) {
            return Err(Status::invalid_argument("use https:// (plain http:// is only for this machine)"));
        }
        let bearer = MetadataValue::try_from(format!("Bearer {token}"))
            .map_err(|_| Status::invalid_argument("that token has characters a header can't carry"))?;
        let mut endpoint =
            Endpoint::from_shared(url.to_string()).map_err(|err| Status::invalid_argument(err.to_string()))?;
        if url.starts_with("https:") {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_webpki_roots())
                .map_err(|err| Status::invalid_argument(err.to_string()))?;
        }
        let channel = endpoint.connect().await.map_err(|err| Status::unavailable(err.to_string()))?;
        Ok(Self { calls: pb::call_service_client::CallServiceClient::with_interceptor(channel, Bearer(bearer)) })
    }

    /// Joins a voice channel: what's said there comes out of the [`Heard`],
    /// and the [`Speaker`] talks. Dropping the `Heard` leaves.
    pub async fn join(&self, server_id: &str, channel_id: &str) -> Result<(Heard, Speaker), Error> {
        let mut heard = Heard {
            calls: self.calls.clone(),
            server_id: server_id.to_string(),
            channel_id: channel_id.to_string(),
            session: Arc::new(Mutex::new(String::new())),
            stream: None,
            state: None,
        };
        heard.listen().await?;
        let speaker =
            Speaker { calls: self.calls.clone(), server_id: server_id.to_string(), session: heard.session.clone() };
        Ok((heard, speaker))
    }
}

fn secure_enough(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else { return url.starts_with("https://") };
    let host = rest.split('/').next().unwrap_or_default();
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// One frame of someone's sound.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Whose it is: their account id.
    pub user_id: String,
    /// One Opus packet, 20 ms at 48 kHz.
    pub opus: Vec<u8>,
    /// When it was spoken, in 48 kHz ticks of their own clock (960 a frame).
    pub timestamp: u32,
}

/// What's said in the voice channel, frame by frame.
pub struct Heard {
    calls: Calls,
    server_id: String,
    channel_id: String,
    session: Arc<Mutex<String>>,
    stream: Option<Streaming<pb::ListenVoiceResponse>>,
    state: Option<pb::VoiceState>,
}

impl Heard {
    async fn listen(&mut self) -> Result<(), Error> {
        let session_id = self.session.lock().await.clone();
        let request = pb::ListenVoiceRequest {
            server_id: self.server_id.clone(),
            channel_id: self.channel_id.clone(),
            session_id,
            ..Default::default()
        };
        let mut stream = self.calls.listen_voice(request).await?.into_inner();
        match stream.next().await {
            Some(Ok(pb::ListenVoiceResponse { event: Some(pb::listen_voice_response::Event::Joined(joined)) })) => {
                *self.session.lock().await = joined.session_id;
                self.state = joined.state;
                self.stream = Some(stream);
                Ok(())
            }
            Some(Err(status)) => Err(status),
            _ => Err(Status::unavailable("the instance didn't say we're in")),
        }
    }

    /// How the program is in the channel: its own voice state.
    pub fn state(&self) -> Option<&pb::VoiceState> {
        self.state.as_ref()
    }

    /// The next frame anyone says. `Ok(None)` never comes while the program
    /// is in the channel: it's there until it's taken out (an error), or
    /// this is dropped. A dropped connection or a restarting instance is
    /// joined again by itself, keeping the same place.
    pub async fn next(&mut self) -> Result<Option<Frame>, Error> {
        let mut wait = Duration::from_millis(250);
        loop {
            let message = match self.stream.as_mut() {
                Some(stream) => stream.next().await,
                None => None,
            };
            match message {
                Some(Ok(pb::ListenVoiceResponse { event: Some(pb::listen_voice_response::Event::Frame(f)) })) => {
                    return Ok(Some(Frame { user_id: f.user_id, opus: f.opus, timestamp: f.timestamp }));
                }
                Some(Ok(_)) => continue,
                // Taken out, moved, or joined somewhere else: that's final.
                Some(Err(status))
                    if matches!(
                        status.code(),
                        Code::FailedPrecondition | Code::PermissionDenied | Code::NotFound | Code::Unauthenticated
                    ) =>
                {
                    self.stream = None;
                    return Err(status);
                }
                // Anything else (a deploy, a dropped connection): join again.
                _ => {
                    self.stream = None;
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(8));
                    match self.listen().await {
                        Ok(()) => wait = Duration::from_millis(250),
                        Err(status) if status.code() == Code::FailedPrecondition => return Err(status),
                        Err(_) => {}
                    }
                }
            }
        }
    }
}

/// Talks in the voice channel [`Client::join`] put the program in.
#[derive(Clone)]
pub struct Speaker {
    calls: Calls,
    server_id: String,
    session: Arc<Mutex<String>>,
}

impl Speaker {
    /// Says frames of Opus (48 kHz, 20 ms each), returning about when the
    /// last of them is heard: it sends them as they play, so saying a long
    /// clip takes about as long as the clip.
    pub async fn say(&self, frames: impl IntoIterator<Item = Vec<u8>>) -> Result<(), Error> {
        let mut calls = self.calls.clone();
        let mut frames = frames.into_iter().peekable();
        while frames.peek().is_some() {
            let batch: Vec<Vec<u8>> = frames.by_ref().take(BATCH).collect();
            let session_id = self.session.lock().await.clone();
            let request = pb::SpeakVoiceRequest {
                server_id: self.server_id.clone(),
                session_id,
                frames: batch,
                interrupt: false,
            };
            let queued = match calls.speak_voice(request.clone()).await {
                Ok(response) => response.into_inner().queued,
                // Rejoining after a restart: give it a moment, once.
                Err(status) if status.code() == Code::Unavailable => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    calls.speak_voice(request).await?.into_inner().queued
                }
                Err(status) => return Err(status),
            };
            if queued > AHEAD {
                tokio::time::sleep(FRAME * (queued - AHEAD)).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tokens_cross_the_network_only_encrypted() {
        for ok in ["https://fuwa.chat", "http://localhost:8080", "http://127.0.0.1:1/x", "http://[::1]:9"] {
            assert!(super::secure_enough(ok), "{ok}");
        }
        for no in ["http://fuwa.chat", "http://localhost.evil.com", "http://10.0.0.1", "ftp://x", "http://[::2]"] {
            assert!(!super::secure_enough(no), "{no}");
        }
    }
}
