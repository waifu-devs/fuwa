//! `CallService`: joining and leaving voice channels and direct-message
//! calls. Places live in `App::voice` (see `voice.rs`); the sound goes
//! through the media part (`App::media_link`). Voice channels' calls run on
//! the shard holding their server, and their changes reach members as events
//! with sequence 0; direct-message calls run on the directory and reach both
//! people through `DirectMessageService.Watch`.

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::Stream;
use hmac::{Hmac, Mac};
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use super::friends::blocks_any;
use super::{Api, Seat, respond};
use crate::app::App;
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::node::Account;
use crate::pb::{self, call_service_server::CallService};
use crate::servers::{self as store, Payload, ServerDb, VoiceModeration};
use crate::voice::{self, LEASE, Place};

/// How long a TURN credential works. Apps ask for new ones each time they
/// (re)connect a call, so this only has to outlast one connection setting up.
const TURN_CREDENTIAL: Duration = Duration::from_secs(60 * 60);

/// Checks for places nobody kept this often.
const SWEEP: Duration = Duration::from_secs(1);

/// How often a ListenVoice stream keeps its place.
const LISTEN_KEEP: Duration = Duration::from_secs(5);
/// Messages a ListenVoice stream holds for a program that's behind (about
/// two seconds of five people talking); past that it misses sound.
const LISTEN_BUFFER: usize = 512;
/// How long a ListenVoice stream goes without sending anything before it
/// sends a keepalive, so a program can tell a quiet call from a dead stream.
const LISTEN_QUIET: Duration = Duration::from_secs(15);
/// How long a ListenVoice stream tries to reach a media part again after
/// one went away, before it gives up and the program has to listen again.
const LISTEN_RECONNECT: Duration = Duration::from_secs(30);

type DownloadStream = Pin<Box<dyn Stream<Item = std::result::Result<pb::DownloadRecordingResponse, Status>> + Send>>;
type ListenStream = Pin<Box<dyn Stream<Item = std::result::Result<pb::ListenVoiceResponse, Status>> + Send>>;
type Listener = tokio::sync::mpsc::Sender<std::result::Result<pb::ListenVoiceResponse, Status>>;

fn listened(event: pb::listen_voice_response::Event) -> pb::ListenVoiceResponse {
    pb::ListenVoiceResponse { event: Some(event) }
}

/// Carries a ListenVoice stream: passes on what the bridge hears, keeps the
/// place while the program listens, opens the bridge again on whichever
/// media part takes over after one restarts, and leaves when the program
/// goes.
async fn listen(app: Arc<App>, server_id: String, mut place: Place, mut events: voice::BridgeEvents, tx: Listener) {
    use crate::rtc::{Bridged, Ending};
    use tokio_stream::StreamExt;
    let user_id = place.state.user_id.clone();
    let mut keep = tokio::time::interval(LISTEN_KEEP);
    keep.tick().await;
    // Moved on only when it fires, not with every frame: until then it just
    // waits out whatever's left of the quiet since the last thing sent.
    let mut last_sent = Instant::now();
    let quiet = tokio::time::sleep(LISTEN_QUIET);
    tokio::pin!(quiet);
    let ended = loop {
        tokio::select! {
            _ = tx.closed() => {
                if let Some(place) = app.voice.remove(&server_id, &user_id, Some(&place.session_id)) {
                    app.media_link.close(&place.room, Some(&user_id), Some(&place.session_id)).await;
                    gone(&app, &server_id, &place).await;
                }
                return;
            }
            _ = app.shutdown.cancelled() => break Status::unavailable("this part of the instance is restarting; listen again"),
            _ = keep.tick() => {
                let session = place.session_id.clone();
                match app.voice.update(&server_id, &user_id, |p| if p.session_id == session { p.expires = lease() }) {
                    Some(kept) if kept.session_id == session && app.settings().calls => place = kept,
                    _ => break moved_away().into(),
                }
            }
            _ = &mut quiet => {
                let due = last_sent + LISTEN_QUIET;
                if Instant::now() >= due {
                    let keepalive = listened(pb::listen_voice_response::Event::Keepalive(pb::VoiceKeepalive {}));
                    // A full buffer has plenty on its way already.
                    let _ = tx.try_send(Ok(keepalive));
                    last_sent = Instant::now();
                }
                quiet.as_mut().reset((last_sent + LISTEN_QUIET).into());
            }
            event = events.next() => match event {
                Some(Bridged::Frame(heard)) => {
                    let frame = pb::VoiceFrame { user_id: heard.participant, opus: heard.frame, timestamp: heard.timestamp };
                    // A program that doesn't keep up misses sound, rather than holding the call up.
                    if tx.try_send(Ok(listened(pb::listen_voice_response::Event::Frame(frame)))).is_ok() {
                        last_sent = Instant::now();
                    }
                }
                Some(Bridged::Ended(Ending::Replaced)) => {
                    break Status::failed_precondition("you joined this call from somewhere else");
                }
                Some(Bridged::Ended(Ending::Closed)) => break moved_away().into(),
                Some(Bridged::Ended(Ending::Restarting)) | None => match rebridge(&app, &server_id, &place, &tx).await {
                    Some(again) => events = again,
                    None if tx.is_closed() => continue,
                    None => break Status::unavailable("calls are restarting; listen again"),
                },
            }
        }
    };
    let _ = tx.send(Err(ended)).await;
}

/// Opens a place's bridge again, on the media part that took over, for as
/// long as the place is still the program's.
async fn rebridge(app: &App, server_id: &str, place: &Place, tx: &Listener) -> Option<voice::BridgeEvents> {
    let until = Instant::now() + LISTEN_RECONNECT;
    let mut wait = Duration::from_millis(150 + u64::from(rand_byte()) * 2);
    loop {
        tokio::select! {
            _ = tx.closed() => return None,
            _ = tokio::time::sleep(wait) => {}
        }
        let current = app.voice.get(server_id, &place.state.user_id);
        let place = current.filter(|p| p.session_id == place.session_id)?;
        match app.media_link.bridge(&place).await {
            Ok(events) => return Some(events),
            Err(err) if Instant::now() < until => {
                tracing::debug!(error = %err, "a voice bridge couldn't open again yet");
                wait = (wait * 2).min(Duration::from_secs(4));
            }
            Err(_) => return None,
        }
    }
}

fn rand_byte() -> u8 {
    let mut byte = [0u8; 1];
    let _ = getrandom::fill(&mut byte);
    byte[0]
}

/// Lets go of the places nobody kept, telling everyone, for as long as the
/// instance runs.
pub fn spawn_voice_sweeper(app: Arc<App>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(SWEEP);
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = every.tick() => {
                    for (scope, place) in app.voice.expire(Instant::now()) {
                        tracing::debug!(room = %place.room, "a place in a call ran out");
                        app.media_link.close(&place.room, Some(&place.state.user_id), Some(&place.session_id)).await;
                        gone(&app, &scope, &place).await;
                    }
                }
            }
        }
    });
}

/// Hangs up whoever can't be in a voice channel any more the moment it
/// happens: kicked, banned, left, timed out, or a role or channel change
/// took CONNECT away (and tells the media part about a change to SPEAK).
/// KeepVoice checks again every few seconds too, but nobody waits for it.
pub fn spawn_voice_guard(app: Arc<App>) {
    let mut events = app.hub.tap();
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                _ = app.shutdown.cancelled() => return,
                event = events.recv() => match event {
                    Some(event) => event,
                    None => return,
                },
            };
            let relevant = matches!(
                event.payload,
                Some(
                    Payload::MemberLeft(_)
                        | Payload::MemberUpdated(_)
                        | Payload::RoleUpdated(_)
                        | Payload::RoleDeleted(_)
                        | Payload::ChannelUpdated(_)
                        | Payload::ChannelDeleted(_)
                        | Payload::ServerDeleted(_)
                )
            );
            if relevant && !app.voice.list(&event.server_id).is_empty() {
                recheck_voice(&app, &event.server_id).await;
            }
        }
    });
}

/// Hangs up everyone in a server's voice channels here, for a server that's
/// leaving this shard: their apps join again where it's going.
pub async fn hang_up_server(app: &App, server_id: &str) {
    for place in app.voice.list(server_id) {
        let user_id = place.state.user_id.clone();
        if let Some(place) = app.voice.remove(server_id, &user_id, Some(&place.session_id)) {
            app.media_link.close(&place.room, Some(&user_id), Some(&place.session_id)).await;
            gone(app, server_id, &place).await;
        }
    }
}

/// Who may still be where they are in a server's voice channels.
async fn recheck_voice(app: &App, server_id: &str) {
    let sdb = app.servers.get(server_id).await.ok();
    for place in app.voice.list(server_id) {
        let user_id = place.state.user_id.clone();
        let quiet = match &sdb {
            Some(sdb) => may_be_in(sdb, &user_id, &place.state.channel_id).await,
            None => None,
        };
        match quiet {
            None => {
                if let Some(place) = app.voice.remove(server_id, &user_id, Some(&place.session_id)) {
                    app.media_link.close(&place.room, Some(&user_id), Some(&place.session_id)).await;
                    gone(app, server_id, &place).await;
                }
            }
            Some(quiet) if quiet != Quiet::of(&place.state) => {
                if let Some(place) = app.voice.update(server_id, &user_id, |p| quiet.apply(&mut p.state)) {
                    app.media_link.update(&place).await;
                    let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
                    publish_voice(app, server_id, &user_id, update);
                }
            }
            Some(_) => {}
        }
    }
}

/// What someone in a voice channel can't do there for want of a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Quiet {
    /// No SPEAK: nobody hears them.
    suppress: bool,
    /// No VIDEO: their camera stays off.
    video_suppress: bool,
    /// No RECORD: they can't say they're recording.
    record_suppress: bool,
}

impl Quiet {
    fn new(access: &crate::permissions::Access, channel_id: &str) -> Self {
        Self {
            suppress: !access.has_in(channel_id, pb::Permission::Speak),
            video_suppress: !access.has_in(channel_id, pb::Permission::Video),
            record_suppress: !access.has_in(channel_id, pb::Permission::Record),
        }
    }

    fn of(state: &pb::VoiceState) -> Self {
        Self { suppress: state.suppress, video_suppress: state.video_suppress, record_suppress: state.record_suppress }
    }

    fn apply(self, state: &mut pb::VoiceState) {
        state.suppress = self.suppress;
        state.video_suppress = self.video_suppress;
        state.record_suppress = self.record_suppress;
        state.self_record &= !self.record_suppress;
        state.server_record &= !self.record_suppress;
    }
}

/// What someone says they're doing in a voice channel.
#[derive(Debug, Clone, Copy, Default)]
struct Selves {
    self_mute: bool,
    self_deaf: bool,
    self_video: bool,
    self_stream: bool,
    self_record: bool,
    server_record: bool,
}

/// Whether someone may be in a voice channel, and if so what they can't do there.
async fn may_be_in(sdb: &store::ServerDb, user_id: &str, channel_id: &str) -> Option<Quiet> {
    let conn = sdb.read().ok()?;
    let (member, access) = store::member_access(&conn, &sdb.id, user_id).await.ok()??;
    let channel = store::load_channel(&conn, &sdb.id, channel_id).await.ok()??;
    let may = channel.r#type == pb::ChannelType::Voice as i32
        && access.can_see(&channel.id)
        && access.require_in(&channel.id, pb::Permission::Connect).is_ok()
        && super::messages::check_not_timed_out(&member).is_ok();
    may.then(|| Quiet::new(&access, &channel.id))
}

/// Tells everyone someone left a call.
async fn gone(app: &App, scope: &str, place: &Place) {
    match scope.strip_prefix("dm:") {
        Some(conversation_id) => {
            // A conversation that's gone has nobody left to tell.
            if let Ok(dms) = app.dms()
                && let Ok(Some(conversation)) = dms.conversation(conversation_id).await
            {
                publish_dm_call(app, conversation_id, &conversation.participants).await;
            }
        }
        None => publish_voice(
            app,
            scope,
            &place.state.user_id,
            Payload::VoiceStateRemoved(pb::VoiceStateRemoved {
                user_id: place.state.user_id.clone(),
                channel_id: place.state.channel_id.clone(),
            }),
        ),
    }
}

/// Sends a voice change to the server's members, not stored.
fn publish_voice(app: &App, server_id: &str, actor_id: &str, payload: Payload) {
    // A recording starts or stops with who's in the call and what they say.
    app.recordings.nudge();
    app.hub.publish([pb::Event {
        id: new_id(),
        server_id: server_id.to_string(),
        sequence: 0,
        actor_id: actor_id.to_string(),
        created_at: Some(timestamp(now_ms())),
        payload: Some(payload),
    }]);
}

/// Sends a conversation's call as it is now to both people in it.
/// Tells a conversation's people about its call, except anyone who blocked
/// someone in it: a blocked person's call never rings.
async fn publish_dm_call(app: &App, conversation_id: &str, participants: &[String]) {
    let call = voice::dm_call(&app.voice, conversation_id);
    let event = pb::DirectMessageEvent { payload: Some(pb::direct_message_event::Payload::CallUpdated(call)) };
    let Ok(dms) = app.dms() else { return };
    let mut told = Vec::with_capacity(participants.len());
    for id in participants {
        // Told nothing rather than too much when it can't be read.
        if !blocks_any(app, id, participants).await.unwrap_or(true) {
            told.push(id.clone());
        }
    }
    dms.publish(&told, event);
}

/// A coturn-style credential (its REST API): the username says until when,
/// the password is the HMAC of it under the shared secret. Each one is new
/// (`name` is random), so the TURN server's logs can't tie it to an account
/// or to another of the same person's calls.
fn turn_credential(secret: &str, name: &str, now: i64) -> (String, String) {
    let expires = now / 1000 + TURN_CREDENTIAL.as_secs() as i64;
    let username = format!("{expires}:{name}");
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key");
    mac.update(username.as_bytes());
    (username, base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

fn ice_servers(urls: &[String], secret: &str) -> Result<Vec<pb::IceServer>> {
    let (stun, turn): (Vec<&String>, Vec<&String>) = urls.iter().partition(|u| u.starts_with("stun:"));
    let mut servers = Vec::new();
    if !stun.is_empty() {
        servers.push(pb::IceServer { urls: stun.into_iter().cloned().collect(), ..Default::default() });
    }
    if !turn.is_empty() {
        let (username, credential) =
            if secret.is_empty() { Default::default() } else { turn_credential(secret, &random_name()?, now_ms()) };
        servers.push(pb::IceServer { urls: turn.into_iter().cloned().collect(), username, credential });
    }
    Ok(servers)
}

fn random_name() -> Result<String> {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).map_err(|err| Error::internal(format!("no randomness: {err}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn moved_away() -> Error {
    Error::FailedPrecondition("you're not in that call any more".into())
}

/// A place that lasts another lease from now.
fn lease() -> Instant {
    Instant::now() + LEASE
}

fn check_session(session_id: &str) -> Result<()> {
    if session_id.len() > 64 {
        return Err(Error::invalid("that isn't a call session"));
    }
    Ok(())
}

impl Api {
    fn calls_on(&self) -> Result<()> {
        if !self.app.settings().calls {
            return Err(Error::FailedPrecondition("this instance's admins turned calls off".into()));
        }
        Ok(())
    }

    /// A voice channel the caller is in the server of, and what they may do in it.
    async fn voice_channel(&self, account: &Account, server_id: &str, channel_id: &str) -> Result<(Seat, pb::Channel)> {
        let seat = self.membership(account, server_id).await?;
        let conn = seat.sdb.read()?;
        let channel = store::load_channel(&conn, &seat.sdb.id, channel_id)
            .await?
            .filter(|c| seat.access.can_see(&c.id))
            .ok_or(Error::NotFound("channel"))?;
        if channel.r#type != pb::ChannelType::Voice as i32 {
            return Err(Error::invalid("that isn't a voice channel"));
        }
        Ok((seat, channel))
    }

    /// The place someone joining a voice channel gets: checked, not taken yet.
    async fn voice_place(
        &self,
        account: &Account,
        server_id: &str,
        channel_id: &str,
        Selves { self_mute, self_deaf, self_video, self_stream, self_record, server_record }: Selves,
        session_id: &str,
    ) -> Result<(String, Place, bool)> {
        self.calls_on()?;
        check_session(session_id)?;
        let (seat, channel) = self.voice_channel(account, server_id, channel_id).await?;
        // A server on its way to another shard is joined there, once it's arrived.
        seat.sdb.writable()?;
        seat.access.require_in(&channel.id, pb::Permission::Connect)?;
        super::messages::check_not_timed_out(&seat.member)?;
        let server_id = seat.sdb.id.clone();
        let before = self.app.voice.get(&server_id, &account.id);
        let session_id = match &before {
            _ if session_id.is_empty() => new_id(),
            Some(place) if place.session_id != session_id => return Err(moved_away()),
            _ => session_id.to_string(),
        };
        let same_channel = before.as_ref().is_some_and(|p| p.state.channel_id == channel.id);
        let moderation = seat.sdb.voice_moderation(&account.id).await?;
        let (server_record, full) = self.server_record(&seat.sdb, server_record).await?;
        let mut state = pb::VoiceState {
            user_id: account.id.clone(),
            channel_id: channel.id.clone(),
            self_mute,
            self_deaf,
            joined_at: match &before {
                Some(p) if same_channel => p.state.joined_at,
                _ => Some(timestamp(now_ms())),
            },
            self_video,
            self_stream,
            self_record,
            server_record,
            ..Default::default()
        };
        Quiet::new(&seat.access, &channel.id).apply(&mut state);
        moderation.apply(&mut state);
        let full = full && !state.record_suppress;
        let place = Place { session_id, room: voice::channel_room(&server_id, &channel.id), state, expires: lease() };
        Ok((server_id, place, full))
    }

    /// Whether someone who asks to record on the server may: the instance
    /// allows it, and the server's recordings aren't at their cap. Then
    /// whether they are.
    async fn server_record(&self, sdb: &ServerDb, asked: bool) -> Result<(bool, bool)> {
        if !asked || !self.app.settings().call_recordings {
            return Ok((false, false));
        }
        let full = crate::recordings::full(&self.app, sdb).await?;
        Ok((!full, full))
    }

    /// Takes a place once its connection to the media part is up, hanging
    /// up the one it moved from, and tells everyone.
    async fn take_voice_place(&self, server_id: &str, place: &Place) {
        let user_id = &place.state.user_id;
        if let Some(before) = self.app.voice.put(server_id, place.clone())
            && before.room != place.room
        {
            self.app.media_link.close(&before.room, Some(user_id), Some(&before.session_id)).await;
        }
        let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
        publish_voice(&self.app, server_id, user_id, update);
    }

    async fn join_voice(&self, metadata: &MetadataMap, req: pb::JoinVoiceRequest) -> Result<pb::JoinVoiceResponse> {
        let account = self.account(metadata).await?;
        let selves = Selves {
            self_mute: req.self_mute,
            self_deaf: req.self_deaf,
            self_video: req.self_video,
            self_stream: req.self_stream,
            self_record: req.self_record,
            server_record: req.server_record,
        };
        let (server_id, place, recordings_full) =
            self.voice_place(&account, &req.server_id, &req.channel_id, selves, &req.session_id).await?;
        let answer = self.app.media_link.open(&place, &req.offer).await?;
        self.take_voice_place(&server_id, &place).await;
        Ok(pb::JoinVoiceResponse { answer, session_id: place.session_id, state: Some(place.state), recordings_full })
    }

    async fn listen_voice(&self, metadata: &MetadataMap, req: pb::ListenVoiceRequest) -> Result<ListenStream> {
        let account = self.account(metadata).await?;
        // Programs have no camera, and record by listening.
        let selves = Selves { self_mute: req.self_mute, self_deaf: req.self_deaf, ..Default::default() };
        let (server_id, place, _) =
            self.voice_place(&account, &req.server_id, &req.channel_id, selves, &req.session_id).await?;
        let events = self.app.media_link.bridge(&place).await?;
        self.take_voice_place(&server_id, &place).await;
        let (tx, rx) = tokio::sync::mpsc::channel(LISTEN_BUFFER);
        let joined = pb::VoiceJoined { session_id: place.session_id.clone(), state: Some(place.state.clone()) };
        let _ = tx.try_send(Ok(listened(pb::listen_voice_response::Event::Joined(joined))));
        tokio::spawn(listen(self.app.clone(), server_id, place, events, tx));
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn speak_voice(&self, metadata: &MetadataMap, req: pb::SpeakVoiceRequest) -> Result<pb::SpeakVoiceResponse> {
        let account = self.account(metadata).await?;
        check_session(&req.session_id)?;
        if req.frames.len() > crate::rtc::MAX_QUEUED {
            return Err(Error::invalid(format!("at most {} frames at a time", crate::rtc::MAX_QUEUED)));
        }
        let server_id = self.app.servers.get(&req.server_id).await?.id.clone();
        let place = self
            .app
            .voice
            .get(&server_id, &account.id)
            .filter(|p| !req.session_id.is_empty() && p.session_id == req.session_id)
            .ok_or_else(moved_away)?;
        if !place.may_speak() {
            return Err(Error::PermissionDenied("you can't speak in this voice channel".into()));
        }
        let queued = self.app.media_link.speak(&place, req.frames, req.interrupt).await?;
        Ok(pb::SpeakVoiceResponse { queued: queued as u32 })
    }

    async fn leave_voice(&self, metadata: &MetadataMap, req: pb::LeaveVoiceRequest) -> Result<pb::LeaveVoiceResponse> {
        let account = self.account(metadata).await?;
        let sdb = self.app.servers.get(&req.server_id).await?;
        if let Some(place) = self.app.voice.remove(&sdb.id, &account.id, Some(&req.session_id)) {
            self.app.media_link.close(&place.room, Some(&account.id), Some(&place.session_id)).await;
            gone(&self.app, &sdb.id, &place).await;
        }
        Ok(pb::LeaveVoiceResponse {})
    }

    /// Takes someone out of a server's voice channel, hanging them up.
    async fn disconnect(&self, server_id: &str, user_id: &str) {
        if let Some(place) = self.app.voice.remove(server_id, user_id, None) {
            self.app.media_link.close(&place.room, Some(user_id), Some(&place.session_id)).await;
            gone(&self.app, server_id, &place).await;
        }
    }

    async fn keep_voice(&self, metadata: &MetadataMap, req: pb::KeepVoiceRequest) -> Result<pb::KeepVoiceResponse> {
        let account = self.account(metadata).await?;
        check_session(&req.session_id)?;
        if req.session_id.is_empty() {
            return Err(Error::invalid("which call session?"));
        }
        let sdb = self.app.servers.get(&req.server_id).await?;
        let server_id = sdb.id.clone();
        let before = self.app.voice.get(&server_id, &account.id);
        if before.as_ref().is_some_and(|p| p.session_id != req.session_id) {
            return Err(moved_away());
        }
        // What they may do now: permissions change while people talk.
        let channel_id = before.as_ref().map(|p| p.state.channel_id.clone()).unwrap_or(req.channel_id.clone());
        let mut moderation = VoiceModeration::default();
        let allowed = match self.voice_channel(&account, &server_id, &channel_id).await {
            Ok((seat, channel)) => {
                let may = seat.access.require_in(&channel.id, pb::Permission::Connect).is_ok()
                    && super::messages::check_not_timed_out(&seat.member).is_ok();
                if before.is_none() {
                    moderation = seat.sdb.voice_moderation(&account.id).await?;
                }
                may.then(|| Quiet::new(&seat.access, &channel.id))
            }
            Err(_) => None,
        };
        let (Some(quiet), true) = (allowed, self.app.settings().calls) else {
            self.disconnect(&server_id, &account.id).await;
            return Err(moved_away());
        };
        let (server_record, full) = self.server_record(&sdb, req.server_record).await?;
        let place = match before {
            Some(mut place) => {
                let may_before = place.may();
                let changed = Quiet::of(&place.state) != quiet
                    || place.state.self_mute != req.self_mute
                    || place.state.self_deaf != req.self_deaf
                    || place.state.self_video != req.self_video
                    || place.state.self_stream != req.self_stream
                    || place.state.self_record != req.self_record
                    || place.state.server_record != server_record;
                place.state.self_mute = req.self_mute;
                place.state.self_deaf = req.self_deaf;
                place.state.self_video = req.self_video;
                place.state.self_stream = req.self_stream;
                place.state.self_record = req.self_record;
                place.state.server_record = server_record;
                quiet.apply(&mut place.state);
                VoiceModeration::of(&place.state).apply(&mut place.state);
                let may_changed = place.may() != may_before;
                place.expires = lease();
                let kept = self.app.voice.update(&server_id, &account.id, |p| *p = place.clone());
                if kept.is_none() {
                    // Let go between reading and keeping: put it back.
                    self.app.voice.put(&server_id, place.clone());
                }
                if may_changed {
                    self.app.media_link.update(&place).await;
                }
                if changed {
                    let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
                    publish_voice(&self.app, &server_id, &account.id, update);
                }
                place
            }
            // This part restarted and forgot them; their connection to the
            // media part never went through here, so the place comes back as it was.
            None => {
                let mut state = pb::VoiceState {
                    user_id: account.id.clone(),
                    channel_id: channel_id.clone(),
                    self_mute: req.self_mute,
                    self_deaf: req.self_deaf,
                    self_video: req.self_video,
                    self_stream: req.self_stream,
                    self_record: req.self_record,
                    server_record,
                    joined_at: Some(timestamp(now_ms())),
                    ..Default::default()
                };
                quiet.apply(&mut state);
                moderation.apply(&mut state);
                let place = Place {
                    session_id: req.session_id.clone(),
                    room: voice::channel_room(&server_id, &channel_id),
                    state,
                    expires: lease(),
                };
                // Only someone still connected to the media part is still
                // in the call; anyone else joins again (with their sound).
                if !self.app.media_link.update(&place).await {
                    return Err(Error::Unavailable("calls are restarting; join again".into()));
                }
                self.app.voice.put(&server_id, place.clone());
                let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
                publish_voice(&self.app, &server_id, &account.id, update);
                place
            }
        };
        let recordings_full = full && !place.state.record_suppress;
        Ok(pb::KeepVoiceResponse { state: Some(place.state), recordings_full })
    }

    async fn list_voice_states(
        &self,
        metadata: &MetadataMap,
        req: pb::ListVoiceStatesRequest,
    ) -> Result<pb::ListVoiceStatesResponse> {
        let account = self.account(metadata).await?;
        let seat = self.membership(&account, &req.server_id).await?;
        let states = self
            .app
            .voice
            .list(&seat.sdb.id)
            .into_iter()
            .filter(|p| seat.access.can_see(&p.state.channel_id))
            .map(|p| p.state)
            .collect();
        Ok(pb::ListVoiceStatesResponse { states })
    }

    async fn moderate_voice(
        &self,
        metadata: &MetadataMap,
        req: pb::ModerateVoiceRequest,
    ) -> Result<pb::ModerateVoiceResponse> {
        let account = self.account(metadata).await?;
        let seat = self.membership(&account, &req.server_id).await?;
        let server_id = seat.sdb.id.clone();
        let place = self.app.voice.get(&server_id, &req.user_id).ok_or(Error::NotFound("voice state"))?;
        let channel_id = place.state.channel_id.clone();
        if !seat.access.can_see(&channel_id) {
            return Err(Error::NotFound("voice state"));
        }
        if req.server_mute.is_some() || req.server_deaf.is_some() || req.server_video_off.is_some() {
            seat.access.require_in(&channel_id, pb::Permission::MuteMembers)?;
        }
        if req.disconnect {
            seat.access.require_in(&channel_id, pb::Permission::MoveMembers)?;
        }
        if req.user_id != account.id {
            let conn = seat.sdb.read()?;
            if let Some((_, theirs)) = store::member_access(&conn, &server_id, &req.user_id).await?
                && !seat.access.outranks(&theirs)
            {
                return Err(Error::denied("you can only do that to people ranked below you"));
            }
        }
        if req.disconnect {
            self.disconnect(&server_id, &req.user_id).await;
            return Ok(pb::ModerateVoiceResponse {});
        }
        let mut moderation = VoiceModeration::of(&place.state);
        moderation.mute = req.server_mute.unwrap_or(moderation.mute);
        moderation.deaf = req.server_deaf.unwrap_or(moderation.deaf);
        moderation.video_off = req.server_video_off.unwrap_or(moderation.video_off);
        seat.sdb.set_voice_moderation(&req.user_id, moderation).await?;
        let Some(place) = self.app.voice.update(&server_id, &req.user_id, |p| moderation.apply(&mut p.state)) else {
            return Err(Error::NotFound("voice state"));
        };
        self.app.media_link.update(&place).await;
        let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
        publish_voice(&self.app, &server_id, &account.id, update);
        Ok(pb::ModerateVoiceResponse {})
    }

    // ───────────────────────── Recordings ─────────────────────────

    /// The caller's seat in the server a recording is in.
    async fn recorder(&self, metadata: &MetadataMap, server_id: &str) -> Result<Seat> {
        let account = self.account(metadata).await?;
        self.membership(&account, server_id).await
    }

    async fn list_recordings(
        &self,
        metadata: &MetadataMap,
        req: pb::ListRecordingsRequest,
    ) -> Result<pb::ListRecordingsResponse> {
        let seat = self.recorder(metadata, &req.server_id).await?;
        seat.access.require_in(&req.channel_id, pb::Permission::Record)?;
        let recordings = crate::recordings::list(&self.app, &seat.sdb, &req.channel_id).await?;
        let (used_bytes, cap_bytes) = crate::recordings::usage(&self.app, &seat.sdb).await?;
        let keep_days = self.app.settings().call_recordings_keep_days;
        Ok(pb::ListRecordingsResponse { recordings, used_bytes, cap_bytes, keep_days })
    }

    async fn download_recording(
        &self,
        metadata: &MetadataMap,
        req: pb::DownloadRecordingRequest,
    ) -> Result<DownloadStream> {
        use tokio_stream::StreamExt;
        let seat = self.recorder(metadata, &req.server_id).await?;
        let row = crate::recordings::find(&seat.sdb, &req.recording_id).await?;
        seat.access.require_in(&row.channel_id, pb::Permission::Record)?;
        let row = crate::recordings::finished(&self.app, &seat.sdb, row).await?;
        let user_id = crate::id::parse_id("account", &req.user_id)?;
        let pieces = crate::recordings::download(&self.app, &seat.sdb, &row, &user_id).await?;
        let stream = tokio_stream::wrappers::ReceiverStream::new(pieces)
            .map(|piece| piece.map(|data| pb::DownloadRecordingResponse { data }).map_err(Status::from));
        Ok(Box::pin(stream))
    }

    async fn delete_recording(
        &self,
        metadata: &MetadataMap,
        req: pb::DeleteRecordingRequest,
    ) -> Result<pb::DeleteRecordingResponse> {
        let account = self.account(metadata).await?;
        let seat = self.membership(&account, &req.server_id).await?;
        let row = crate::recordings::find(&seat.sdb, &req.recording_id).await?;
        seat.access.require_in(&row.channel_id, pb::Permission::Record)?;
        // Whoever started it, or someone who runs the channel.
        if row.started_by != account.id && !seat.access.has_in(&row.channel_id, pb::Permission::ManageChannels) {
            return Err(Error::denied(
                "only whoever started a recording, or someone who can manage the channel, can delete it",
            ));
        }
        let row = crate::recordings::finished(&self.app, &seat.sdb, row).await?;
        crate::recordings::delete(&self.app, &seat.sdb, &row).await?;
        Ok(pb::DeleteRecordingResponse {})
    }

    // ───────────────────────── Direct-message calls ─────────────────────────

    async fn join_dm_call(&self, metadata: &MetadataMap, req: pb::JoinDmCallRequest) -> Result<pb::JoinDmCallResponse> {
        let account = self.account(metadata).await?;
        self.calls_on()?;
        check_session(&req.session_id)?;
        let conversation = self.app.dms()?.conversation_of(&account.id, &req.conversation_id).await?;
        // Your own block stops you calling; someone who blocked you never
        // hears the call ring (`publish_dm_call`).
        for other in conversation.participants.iter().filter(|id| **id != account.id) {
            self.may_message(&account.id, other, true).await?;
        }
        let scope = voice::dm_scope(&conversation.id);
        let before = self.app.voice.get(&scope, &account.id);
        let session_id = match &before {
            _ if req.session_id.is_empty() => new_id(),
            Some(place) if place.session_id != req.session_id => return Err(moved_away()),
            _ => req.session_id.clone(),
        };
        let state = pb::VoiceState {
            user_id: account.id.clone(),
            conversation_id: conversation.id.clone(),
            self_mute: req.self_mute,
            self_deaf: req.self_deaf,
            self_video: req.self_video,
            self_stream: req.self_stream,
            self_record: req.self_record,
            joined_at: before.as_ref().and_then(|p| p.state.joined_at).or(Some(timestamp(now_ms()))),
            ..Default::default()
        };
        let place = Place { session_id, room: voice::dm_room(&conversation.id), state, expires: lease() };
        let answer = self.app.media_link.open(&place, &req.offer).await?;
        self.app.voice.put(&scope, place.clone());
        publish_dm_call(&self.app, &conversation.id, &conversation.participants).await;
        Ok(pb::JoinDmCallResponse { answer, session_id: place.session_id, state: Some(place.state) })
    }

    async fn leave_dm_call(
        &self,
        metadata: &MetadataMap,
        req: pb::LeaveDmCallRequest,
    ) -> Result<pb::LeaveDmCallResponse> {
        let account = self.account(metadata).await?;
        let conversation = self.app.dms()?.conversation_of(&account.id, &req.conversation_id).await?;
        let scope = voice::dm_scope(&conversation.id);
        if let Some(place) = self.app.voice.remove(&scope, &account.id, Some(&req.session_id)) {
            self.app.media_link.close(&place.room, Some(&account.id), Some(&place.session_id)).await;
            publish_dm_call(&self.app, &conversation.id, &conversation.participants).await;
        }
        Ok(pb::LeaveDmCallResponse {})
    }

    async fn keep_dm_call(&self, metadata: &MetadataMap, req: pb::KeepDmCallRequest) -> Result<pb::KeepDmCallResponse> {
        let account = self.account(metadata).await?;
        check_session(&req.session_id)?;
        if req.session_id.is_empty() {
            return Err(Error::invalid("which call session?"));
        }
        let conversation = self.app.dms()?.conversation_of(&account.id, &req.conversation_id).await?;
        let scope = voice::dm_scope(&conversation.id);
        // Calls turned off, or you blocked someone in it since joining: out you go.
        if !self.app.settings().calls || blocks_any(&self.app, &account.id, &conversation.participants).await? {
            if let Some(place) = self.app.voice.remove(&scope, &account.id, None) {
                self.app.media_link.close(&place.room, Some(&account.id), Some(&place.session_id)).await;
                publish_dm_call(&self.app, &conversation.id, &conversation.participants).await;
            }
            return Err(moved_away());
        }
        let before = self.app.voice.get(&scope, &account.id);
        if before.as_ref().is_some_and(|p| p.session_id != req.session_id) {
            return Err(moved_away());
        }
        let changed = before.as_ref().is_none_or(|p| {
            p.state.self_mute != req.self_mute
                || p.state.self_deaf != req.self_deaf
                || p.state.self_video != req.self_video
                || p.state.self_stream != req.self_stream
                || p.state.self_record != req.self_record
        });
        let forgotten = before.is_none();
        let may_before = before.as_ref().map(Place::may);
        let mut place = before.unwrap_or_else(|| Place {
            session_id: req.session_id.clone(),
            room: voice::dm_room(&conversation.id),
            state: pb::VoiceState {
                user_id: account.id.clone(),
                conversation_id: conversation.id.clone(),
                joined_at: Some(timestamp(now_ms())),
                ..Default::default()
            },
            expires: lease(),
        });
        place.state.self_mute = req.self_mute;
        place.state.self_deaf = req.self_deaf;
        place.state.self_video = req.self_video;
        place.state.self_stream = req.self_stream;
        place.state.self_record = req.self_record;
        place.expires = lease();
        // Forgotten in a restart: only back if still connected to the media part.
        if forgotten && !self.app.media_link.update(&place).await {
            return Err(Error::Unavailable("calls are restarting; join again".into()));
        }
        // The camera turned on or off: the media part passes it on only while it's on.
        if may_before.is_some_and(|may| may != place.may()) {
            self.app.media_link.update(&place).await;
        }
        self.app.voice.put(&scope, place.clone());
        if changed {
            publish_dm_call(&self.app, &conversation.id, &conversation.participants).await;
        }
        Ok(pb::KeepDmCallResponse { state: Some(place.state) })
    }

    async fn list_dm_calls(&self, metadata: &MetadataMap) -> Result<pb::ListDmCallsResponse> {
        let account = self.account(metadata).await?;
        let dms = self.app.dms()?;
        let mut calls = Vec::new();
        for scope in self.app.voice.scopes("dm:") {
            let conversation_id = scope.trim_start_matches("dm:");
            if let Ok(conversation) = dms.conversation_of(&account.id, conversation_id).await
                && !blocks_any(&self.app, &account.id, &conversation.participants).await.unwrap_or(true)
            {
                calls.push(voice::dm_call(&self.app.voice, conversation_id));
            }
        }
        Ok(pb::ListDmCallsResponse { calls })
    }
}

#[tonic::async_trait]
impl CallService for Api {
    async fn get_call_settings(
        &self,
        request: Request<pb::GetCallSettingsRequest>,
    ) -> Result<Response<pb::GetCallSettingsResponse>, Status> {
        respond(
            async {
                self.account(request.metadata()).await?;
                let settings = self.app.settings();
                let enabled = settings.calls && self.app.media_link.is_on();
                let ice_servers = match enabled {
                    true => ice_servers(&settings.ice_urls, &settings.turn_secret)?,
                    false => vec![],
                };
                Ok(pb::GetCallSettingsResponse {
                    enabled,
                    ice_servers,
                    recordings: enabled && settings.call_recordings,
                    screen_sound: enabled,
                })
            }
            .await,
        )
    }

    async fn join_voice(
        &self,
        request: Request<pb::JoinVoiceRequest>,
    ) -> Result<Response<pb::JoinVoiceResponse>, Status> {
        respond(Api::join_voice(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn leave_voice(
        &self,
        request: Request<pb::LeaveVoiceRequest>,
    ) -> Result<Response<pb::LeaveVoiceResponse>, Status> {
        respond(Api::leave_voice(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn keep_voice(
        &self,
        request: Request<pb::KeepVoiceRequest>,
    ) -> Result<Response<pb::KeepVoiceResponse>, Status> {
        respond(Api::keep_voice(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn list_voice_states(
        &self,
        request: Request<pb::ListVoiceStatesRequest>,
    ) -> Result<Response<pb::ListVoiceStatesResponse>, Status> {
        respond(Api::list_voice_states(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn moderate_voice(
        &self,
        request: Request<pb::ModerateVoiceRequest>,
    ) -> Result<Response<pb::ModerateVoiceResponse>, Status> {
        respond(Api::moderate_voice(self, request.metadata(), request.get_ref().clone()).await)
    }

    type ListenVoiceStream = ListenStream;

    async fn listen_voice(&self, request: Request<pb::ListenVoiceRequest>) -> Result<Response<ListenStream>, Status> {
        respond(Api::listen_voice(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn speak_voice(
        &self,
        request: Request<pb::SpeakVoiceRequest>,
    ) -> Result<Response<pb::SpeakVoiceResponse>, Status> {
        respond(Api::speak_voice(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn list_recordings(
        &self,
        request: Request<pb::ListRecordingsRequest>,
    ) -> Result<Response<pb::ListRecordingsResponse>, Status> {
        respond(Api::list_recordings(self, request.metadata(), request.get_ref().clone()).await)
    }

    type DownloadRecordingStream = DownloadStream;

    async fn download_recording(
        &self,
        request: Request<pb::DownloadRecordingRequest>,
    ) -> Result<Response<DownloadStream>, Status> {
        respond(Api::download_recording(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn delete_recording(
        &self,
        request: Request<pb::DeleteRecordingRequest>,
    ) -> Result<Response<pb::DeleteRecordingResponse>, Status> {
        respond(Api::delete_recording(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn join_dm_call(
        &self,
        request: Request<pb::JoinDmCallRequest>,
    ) -> Result<Response<pb::JoinDmCallResponse>, Status> {
        respond(Api::join_dm_call(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn leave_dm_call(
        &self,
        request: Request<pb::LeaveDmCallRequest>,
    ) -> Result<Response<pb::LeaveDmCallResponse>, Status> {
        respond(Api::leave_dm_call(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn keep_dm_call(
        &self,
        request: Request<pb::KeepDmCallRequest>,
    ) -> Result<Response<pb::KeepDmCallResponse>, Status> {
        respond(Api::keep_dm_call(self, request.metadata(), request.get_ref().clone()).await)
    }

    async fn list_dm_calls(
        &self,
        request: Request<pb::ListDmCallsRequest>,
    ) -> Result<Response<pb::ListDmCallsResponse>, Status> {
        respond(Api::list_dm_calls(self, request.metadata()).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_credentials_follow_coturn() {
        // coturn checks HMAC-SHA1(secret, "<expiry>:<name>"), base64.
        let (username, credential) = turn_credential("north", "acc", 1_000_000);
        assert_eq!(username, format!("{}:acc", 1000 + 60 * 60));
        assert_eq!(credential.len(), 28);
        let servers = ice_servers(&["stun:a:3478".into(), "turn:b:3478".into()], "north").unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers[0].username.is_empty());
        assert!(!servers[1].credential.is_empty());
        assert_ne!(
            servers[1].username,
            ice_servers(&["turn:b:3478".into()], "north").unwrap()[0].username,
            "new each time"
        );
    }
}
