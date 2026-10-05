//! Who's in which call, and reaching the media part that carries them.
//!
//! Places in calls live in memory only, on the part that owns them: the
//! shard holding a server keeps its voice channels', the directory keeps
//! direct-message calls'. Apps keep their place every few seconds
//! (`CallService.KeepVoice`); a place nobody kept for [`LEASE`] is let go.
//! That's also what makes restarts go unnoticed: a part that comes back has
//! forgotten everyone, and gets each place back with its app's next keep,
//! while the sound itself never went through it.
//!
//! The sound goes through a media part ([`crate::rtc`]): this process's own,
//! or the split instance's media parts (FUWA_MEDIA_URL), picked per room so
//! everyone in a call meets on the same one.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::cluster::{Keyed, WithKey};
use crate::cpb;
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::pb;
use crate::rtc::{Bridged, Ending, Heard, May, Sfu, Watched};

/// What a bridge hears, wherever its media part is.
pub type BridgeEvents = std::pin::Pin<Box<dyn futures::Stream<Item = Bridged> + Send>>;

/// How long a place lasts without being kept. Apps keep theirs every 5 seconds.
pub const LEASE: Duration = Duration::from_secs(15);

/// Someone's place in a call.
#[derive(Debug, Clone)]
pub struct Place {
    pub session_id: String,
    /// The media part's name for the call: "s/<server>/<channel>" or "d/<conversation>".
    pub room: String,
    pub state: pb::VoiceState,
    pub expires: Instant,
}

impl Place {
    /// Whether the media part should pass their sound on.
    pub fn may_speak(&self) -> bool {
        !self.state.server_mute && !self.state.suppress
    }

    pub fn may_hear(&self) -> bool {
        !self.state.server_deaf
    }

    /// Whether the media part should pass their camera on: only while they
    /// say it's on, so nobody films while everyone sees their camera off.
    pub fn may_video(&self) -> bool {
        self.state.self_video && !self.state.video_suppress && !self.state.server_video_off
    }

    /// Whether the media part should pass their shared screen on.
    pub fn may_screen(&self) -> bool {
        self.state.self_stream && !self.state.video_suppress && !self.state.server_video_off
    }

    pub fn may(&self) -> May {
        May { speak: self.may_speak(), hear: self.may_hear(), video: self.may_video(), screen: self.may_screen() }
    }
}

/// The room a voice channel's call is in.
pub fn channel_room(server_id: &str, channel_id: &str) -> String {
    format!("s/{server_id}/{channel_id}")
}

/// The room a conversation's call is in.
pub fn dm_room(conversation_id: &str) -> String {
    format!("d/{conversation_id}")
}

/// Everyone in calls under one owner: a server (all its voice channels) or a
/// conversation.
#[derive(Debug, Default)]
struct Scope {
    places: HashMap<String, Place>,
    started_at: i64,
    started_by: String,
}

/// The places this part keeps, by scope ("<server id>" or "dm:<conversation id>").
#[derive(Default)]
pub struct Voice {
    scopes: Mutex<HashMap<String, Scope>>,
}

pub fn dm_scope(conversation_id: &str) -> String {
    format!("dm:{conversation_id}")
}

impl Voice {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Scope>> {
        self.scopes.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn get(&self, scope: &str, user_id: &str) -> Option<Place> {
        self.lock().get(scope)?.places.get(user_id).cloned()
    }

    /// Puts someone in a call (or moves them), giving back where they were.
    pub fn put(&self, scope: &str, place: Place) -> Option<Place> {
        let mut scopes = self.lock();
        let entry = scopes.entry(scope.to_string()).or_insert_with(|| Scope {
            started_at: now_ms(),
            started_by: place.state.user_id.clone(),
            ..Default::default()
        });
        entry.places.insert(place.state.user_id.clone(), place)
    }

    /// Takes someone out (only from `session_id`, when given).
    pub fn remove(&self, scope: &str, user_id: &str, session_id: Option<&str>) -> Option<Place> {
        let mut scopes = self.lock();
        let entry = scopes.get_mut(scope)?;
        if session_id.is_some_and(|s| !s.is_empty() && entry.places.get(user_id).is_some_and(|p| p.session_id != s)) {
            return None;
        }
        let removed = entry.places.remove(user_id);
        if entry.places.is_empty() {
            scopes.remove(scope);
        }
        removed
    }

    /// Changes someone's place, if they have one.
    pub fn update(&self, scope: &str, user_id: &str, f: impl FnOnce(&mut Place)) -> Option<Place> {
        let mut scopes = self.lock();
        let place = scopes.get_mut(scope)?.places.get_mut(user_id)?;
        f(place);
        Some(place.clone())
    }

    pub fn list(&self, scope: &str) -> Vec<Place> {
        let scopes = self.lock();
        let mut places: Vec<Place> =
            scopes.get(scope).map(|s| s.places.values().cloned().collect()).unwrap_or_default();
        places.sort_by(|a, b| {
            let joined = |p: &Place| p.state.joined_at.as_ref().map(crate::id::millis).unwrap_or_default();
            joined(a).cmp(&joined(b)).then_with(|| a.state.user_id.cmp(&b.state.user_id))
        });
        places
    }

    /// When a scope's call started and who started it.
    pub fn started(&self, scope: &str) -> Option<(i64, String)> {
        self.lock().get(scope).map(|s| (s.started_at, s.started_by.clone()))
    }

    /// The scopes with anyone in them that start with `prefix`.
    pub fn scopes(&self, prefix: &str) -> Vec<String> {
        self.lock().keys().filter(|k| k.starts_with(prefix)).cloned().collect()
    }

    /// The voice channels someone wants recorded on the server, as (server,
    /// channel, who turned it on first).
    pub fn recorded(&self) -> Vec<(String, String, String)> {
        let scopes = self.lock();
        let mut out: Vec<(String, String, String, i64)> = Vec::new();
        for (scope, entry) in scopes.iter().filter(|(scope, _)| !scope.starts_with("dm:")) {
            for place in entry.places.values().filter(|p| p.state.server_record) {
                let joined = place.state.joined_at.as_ref().map(crate::id::millis).unwrap_or_default();
                match out.iter_mut().find(|(s, c, _, _)| s == scope && *c == place.state.channel_id) {
                    Some(found) if found.3 <= joined => {}
                    Some(found) => {
                        *found = (scope.clone(), place.state.channel_id.clone(), place.state.user_id.clone(), joined)
                    }
                    None => {
                        out.push((scope.clone(), place.state.channel_id.clone(), place.state.user_id.clone(), joined))
                    }
                }
            }
        }
        out.into_iter().map(|(s, c, u, _)| (s, c, u)).collect()
    }

    /// Lets go of the places nobody kept, as of `now`.
    pub fn expire(&self, now: Instant) -> Vec<(String, Place)> {
        let mut scopes = self.lock();
        let mut gone = Vec::new();
        scopes.retain(|scope, entry| {
            entry.places.retain(|_, place| {
                let keep = place.expires > now;
                if !keep {
                    gone.push((scope.clone(), place.clone()));
                }
                keep
            });
            !entry.places.is_empty()
        });
        gone
    }
}

/// A direct-message call as the API shows it.
pub fn dm_call(voice: &Voice, conversation_id: &str) -> pb::DmCall {
    let scope = dm_scope(conversation_id);
    let (started_at, started_by) = voice.started(&scope).unwrap_or_default();
    pb::DmCall {
        conversation_id: conversation_id.to_string(),
        participants: voice.list(&scope).into_iter().map(|p| p.state).collect(),
        started_at: (started_at > 0).then(|| timestamp(started_at)),
        started_by,
    }
}

pub type MediaClient = cpb::media_service_client::MediaServiceClient<Keyed>;

/// How this process reaches a media part.
pub enum MediaLink {
    /// No calls: why not.
    Off(String),
    /// In this process.
    Local(Sfu),
    /// The split instance's media parts, by their URLs.
    Remote(Vec<(String, MediaClient)>),
}

impl MediaLink {
    pub fn remote(urls: &[String], key: tonic::metadata::AsciiMetadataValue) -> Result<Self> {
        let mut parts = Vec::new();
        for url in urls {
            let client = cpb::media_service_client::MediaServiceClient::with_interceptor(
                crate::cluster::channel(url)?,
                WithKey(key.clone()),
            );
            parts.push((url.clone(), client));
        }
        Ok(Self::Remote(parts))
    }

    pub fn is_on(&self) -> bool {
        !matches!(self, Self::Off(_))
    }

    /// The media part a room's call goes through: the same one for everyone
    /// in it, and the same one again after a restart (rendezvous hashing, so
    /// adding a part moves only the rooms that land on it).
    fn part(&self, room: &str) -> Result<MediaClient> {
        match self {
            Self::Remote(parts) => parts
                .iter()
                .max_by_key(|(url, _)| {
                    use sha2::Digest;
                    let digest = sha2::Sha256::digest(format!("{url}\n{room}").as_bytes());
                    u64::from_be_bytes(digest[..8].try_into().unwrap_or_default())
                })
                .map(|(_, client)| client.clone())
                .ok_or_else(|| Error::Unavailable("calls aren't set up on this instance".into())),
            _ => Err(Error::internal("no media parts")),
        }
    }

    fn off(&self) -> Error {
        match self {
            Self::Off(why) => Error::FailedPrecondition(format!("calls are off on this instance: {why}")),
            _ => Error::internal("calls are on"),
        }
    }

    pub async fn open(&self, place: &Place, offer: &str) -> Result<String> {
        let user_id = &place.state.user_id;
        match self {
            Self::Off(_) => Err(self.off()),
            Self::Local(sfu) => sfu.open(&place.room, user_id, &place.session_id, offer, place.may()).await,
            Self::Remote(_) => {
                let client = self.part(&place.room)?;
                let request = cpb::OpenRequest {
                    room: place.room.clone(),
                    participant: user_id.clone(),
                    session_id: place.session_id.clone(),
                    offer: offer.to_string(),
                    may_speak: place.may_speak(),
                    may_hear: place.may_hear(),
                    may_video: place.may_video(),
                    may_screen: place.may_screen(),
                };
                // Opening twice only replaces the first connection, so a
                // media part that's restarting is waited for.
                let answer = crate::cluster::ride_out(crate::cluster::RIDE_OUT, || {
                    let (mut client, request) = (client.clone(), request.clone());
                    async move { client.open(request).await }
                })
                .await?;
                Ok(answer.into_inner().answer)
            }
        }
    }

    /// Hangs someone up (only `session_id`'s connection, when given). A
    /// media part that can't be reached has no connection to close, so
    /// failing is only logged.
    pub async fn close(&self, room: &str, participant: Option<&str>, session_id: Option<&str>) {
        let result = match self {
            Self::Off(_) => Ok(()),
            Self::Local(sfu) => sfu.close(room, participant, session_id).await,
            Self::Remote(_) => match self.part(room) {
                Ok(mut client) => client
                    .close(cpb::CloseRequest {
                        room: room.into(),
                        participant: participant.unwrap_or_default().into(),
                        session_id: session_id.unwrap_or_default().into(),
                    })
                    .await
                    .map(|_| ())
                    .map_err(Error::retried),
                Err(err) => Err(err),
            },
        };
        if result.is_err() {
            tracing::info!("couldn't hang up a call on its media part");
        }
    }

    /// Puts a program in a place's call without WebRTC (see [`Sfu::bridge`]),
    /// getting cameras and screens too when it `watch`es. The stream ends
    /// with [`Bridged::Ended`], or just ends when the media part went away,
    /// which is worth opening again.
    pub async fn bridge(&self, place: &Place, watch: bool) -> Result<BridgeEvents> {
        use tokio_stream::StreamExt;
        let user_id = &place.state.user_id;
        match self {
            Self::Off(_) => Err(self.off()),
            Self::Local(sfu) => {
                let heard = sfu.bridge(&place.room, user_id, &place.session_id, place.may(), watch).await?;
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(heard)))
            }
            Self::Remote(_) => {
                let client = self.part(&place.room)?;
                let request = cpb::BridgeRequest {
                    room: place.room.clone(),
                    participant: user_id.clone(),
                    session_id: place.session_id.clone(),
                    may_speak: place.may_speak(),
                    may_hear: place.may_hear(),
                    watch,
                };
                let stream = crate::cluster::ride_out(crate::cluster::RIDE_OUT, || {
                    let (mut client, request) = (client.clone(), request.clone());
                    async move { client.bridge(request).await }
                })
                .await?
                .into_inner();
                let events = stream.map_while(|event| match event.ok()?.event? {
                    cpb::bridge_response::Event::Frame(f) => Some(Bridged::Frame(Heard {
                        participant: f.participant,
                        frame: f.frame,
                        timestamp: f.timestamp,
                    })),
                    cpb::bridge_response::Event::Picture(p) => Some(Bridged::Picture(Watched {
                        participant: p.participant,
                        frame: p.frame,
                        keyframe: p.keyframe,
                        screen: p.screen,
                        time: p.time,
                    })),
                    cpb::bridge_response::Event::Ended(why) => Some(Bridged::Ended(Ending::parse(&why))),
                });
                Ok(Box::pin(events))
            }
        }
    }

    /// Queues frames for a place's bridge to say, dropping what was still
    /// waiting first when `interrupt`; how many are waiting.
    pub async fn speak(&self, place: &Place, frames: Vec<Vec<u8>>, interrupt: bool) -> Result<usize> {
        let user_id = &place.state.user_id;
        match self {
            Self::Off(_) => Err(self.off()),
            Self::Local(sfu) => sfu.speak(&place.room, user_id, &place.session_id, frames, interrupt).await,
            Self::Remote(_) => {
                let mut client = self.part(&place.room)?;
                let request = cpb::SpeakRequest {
                    room: place.room.clone(),
                    participant: user_id.clone(),
                    session_id: place.session_id.clone(),
                    frames,
                    interrupt,
                };
                let queued = client.speak(request).await.map_err(Error::retried)?.into_inner().queued;
                Ok(queued as usize)
            }
        }
    }

    /// Passes on what someone may do (speak, hear, film), saying whether their
    /// place's session has a connection on the media part (false when it
    /// can't be reached).
    pub async fn update(&self, place: &Place) -> bool {
        let (room, user_id) = (&place.room, &place.state.user_id);
        let session = Some(place.session_id.as_str());
        let result = match self {
            Self::Off(_) => Ok(false),
            Self::Local(sfu) => sfu.update(room, user_id, session, place.may()).await,
            Self::Remote(_) => match self.part(room) {
                Ok(mut client) => client
                    .update(cpb::UpdateRequest {
                        room: room.clone(),
                        participant: user_id.clone(),
                        may_speak: place.may_speak(),
                        may_hear: place.may_hear(),
                        session_id: place.session_id.clone(),
                        may_video: place.may_video(),
                        may_screen: place.may_screen(),
                    })
                    .await
                    .map(|r| r.into_inner().connected)
                    .map_err(Error::retried),
                Err(err) => Err(err),
            },
        };
        result.unwrap_or_else(|_| {
            tracing::info!("couldn't change a call on its media part");
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(user: &str, session: &str, expires: Instant) -> Place {
        Place {
            session_id: session.into(),
            room: "s/x/y".into(),
            state: pb::VoiceState { user_id: user.into(), channel_id: "y".into(), ..Default::default() },
            expires,
        }
    }

    #[test]
    fn places_come_and_go() {
        let voice = Voice::default();
        let now = Instant::now();
        assert!(voice.put("x", place("a", "1", now + LEASE)).is_none());
        assert!(voice.put("x", place("b", "2", now)).is_none());
        assert_eq!(voice.started("x").unwrap().1, "a");
        // Someone else's session doesn't take them out.
        assert!(voice.remove("x", "a", Some("9")).is_none());
        assert_eq!(voice.list("x").len(), 2);
        let gone = voice.expire(now + Duration::from_millis(1));
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].1.state.user_id, "b");
        assert!(voice.remove("x", "a", Some("1")).is_some());
        assert!(voice.started("x").is_none(), "an empty call is over");
    }

    #[test]
    fn rooms_land_on_one_part() {
        let key = tonic::metadata::AsciiMetadataValue::from_static("k");
        let link = tokio::runtime::Runtime::new().unwrap().block_on(async {
            MediaLink::remote(&["http://a:1".into(), "http://b:1".into(), "http://c:1".into()], key).unwrap()
        });
        let MediaLink::Remote(parts) = &link else { unreachable!() };
        let pick = |room: &str| {
            use sha2::Digest;
            parts
                .iter()
                .max_by_key(|(url, _)| {
                    let digest = sha2::Sha256::digest(format!("{url}\n{room}").as_bytes());
                    u64::from_be_bytes(digest[..8].try_into().unwrap())
                })
                .unwrap()
                .0
                .clone()
        };
        let spread: std::collections::HashSet<String> = (0..60).map(|i| pick(&format!("s/{i}/c"))).collect();
        assert_eq!(spread.len(), 3, "rooms spread over every part");
        assert_eq!(pick("d/abc"), pick("d/abc"));
    }
}
