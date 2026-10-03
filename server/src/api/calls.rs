//! `CallService`: joining and leaving voice channels and direct-message
//! calls. Places live in `App::voice` (see `voice.rs`); the sound goes
//! through the media part (`App::media_link`). Voice channels' calls run on
//! the shard holding their server, and their changes reach members as events
//! with sequence 0; direct-message calls run on the directory and reach both
//! people through `DirectMessageService.Watch`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use super::{Api, Seat, respond};
use crate::app::App;
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::node::Account;
use crate::pb::{self, call_service_server::CallService};
use crate::servers::{self as store, Payload};
use crate::voice::{self, LEASE, Place};

/// How long a TURN credential works.
const TURN_CREDENTIAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Checks for places nobody kept this often.
const SWEEP: Duration = Duration::from_secs(1);

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

/// Tells everyone someone left a call.
async fn gone(app: &App, scope: &str, place: &Place) {
    match scope.strip_prefix("dm:") {
        Some(conversation_id) => {
            // A conversation that's gone has nobody left to tell.
            if let Ok(dms) = app.dms()
                && let Ok(Some(conversation)) = dms.conversation(conversation_id).await
            {
                publish_dm_call(app, conversation_id, &conversation.participants);
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
fn publish_dm_call(app: &App, conversation_id: &str, participants: &[String]) {
    let call = voice::dm_call(&app.voice, conversation_id);
    let event = pb::DirectMessageEvent { payload: Some(pb::direct_message_event::Payload::CallUpdated(call)) };
    if let Ok(dms) = app.dms() {
        dms.publish(participants, event);
    }
}

/// A coturn-style credential (its REST API): the username says until when
/// and for whom, the password is the HMAC of it under the shared secret.
fn turn_credential(secret: &str, account_id: &str, now: i64) -> (String, String) {
    let expires = now / 1000 + TURN_CREDENTIAL.as_secs() as i64;
    let username = format!("{expires}:{account_id}");
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key");
    mac.update(username.as_bytes());
    (username, base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

fn ice_servers(urls: &[String], secret: &str, account_id: &str) -> Vec<pb::IceServer> {
    let (stun, turn): (Vec<&String>, Vec<&String>) = urls.iter().partition(|u| u.starts_with("stun:"));
    let mut servers = Vec::new();
    if !stun.is_empty() {
        servers.push(pb::IceServer { urls: stun.into_iter().cloned().collect(), ..Default::default() });
    }
    if !turn.is_empty() {
        let (username, credential) =
            if secret.is_empty() { Default::default() } else { turn_credential(secret, account_id, now_ms()) };
        servers.push(pb::IceServer { urls: turn.into_iter().cloned().collect(), username, credential });
    }
    servers
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

    async fn join_voice(&self, metadata: &MetadataMap, req: pb::JoinVoiceRequest) -> Result<pb::JoinVoiceResponse> {
        let account = self.account(metadata).await?;
        self.calls_on()?;
        check_session(&req.session_id)?;
        let (seat, channel) = self.voice_channel(&account, &req.server_id, &req.channel_id).await?;
        seat.access.require_in(&channel.id, pb::Permission::Connect)?;
        super::messages::check_not_timed_out(&seat.member)?;
        let server_id = seat.sdb.id.clone();
        let before = self.app.voice.get(&server_id, &account.id);
        let session_id = match &before {
            _ if req.session_id.is_empty() => new_id(),
            Some(place) if place.session_id != req.session_id => return Err(moved_away()),
            _ => req.session_id.clone(),
        };
        let same_channel = before.as_ref().is_some_and(|p| p.state.channel_id == channel.id);
        let state = pb::VoiceState {
            user_id: account.id.clone(),
            channel_id: channel.id.clone(),
            self_mute: req.self_mute,
            self_deaf: req.self_deaf,
            server_mute: before.as_ref().is_some_and(|p| p.state.server_mute),
            server_deaf: before.as_ref().is_some_and(|p| p.state.server_deaf),
            joined_at: match &before {
                Some(p) if same_channel => p.state.joined_at,
                _ => Some(timestamp(now_ms())),
            },
            suppress: !seat.access.has_in(&channel.id, pb::Permission::Speak),
            ..Default::default()
        };
        let place = Place { session_id, room: voice::channel_room(&server_id, &channel.id), state, expires: lease() };
        let answer = self.app.media_link.open(&place, &req.offer).await?;
        if let Some(before) = self.app.voice.put(&server_id, place.clone())
            && before.room != place.room
        {
            self.app.media_link.close(&before.room, Some(&account.id), Some(&before.session_id)).await;
        }
        let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
        publish_voice(&self.app, &server_id, &account.id, update);
        Ok(pb::JoinVoiceResponse { answer, session_id: place.session_id, state: Some(place.state) })
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
        let server_id = self.app.servers.get(&req.server_id).await?.id.clone();
        let before = self.app.voice.get(&server_id, &account.id);
        if before.as_ref().is_some_and(|p| p.session_id != req.session_id) {
            return Err(moved_away());
        }
        // What they may do now: permissions change while people talk.
        let channel_id = before.as_ref().map(|p| p.state.channel_id.clone()).unwrap_or(req.channel_id.clone());
        let allowed = match self.voice_channel(&account, &server_id, &channel_id).await {
            Ok((seat, channel)) => {
                let may = seat.access.require_in(&channel.id, pb::Permission::Connect).is_ok()
                    && super::messages::check_not_timed_out(&seat.member).is_ok();
                may.then(|| !seat.access.has_in(&channel.id, pb::Permission::Speak))
            }
            Err(_) => None,
        };
        let (Some(suppress), true) = (allowed, self.app.settings().calls) else {
            self.disconnect(&server_id, &account.id).await;
            return Err(moved_away());
        };
        let place = match before {
            Some(mut place) => {
                let changed = place.state.self_mute != req.self_mute
                    || place.state.self_deaf != req.self_deaf
                    || place.state.suppress != suppress;
                let may_changed = place.state.suppress != suppress;
                place.state.self_mute = req.self_mute;
                place.state.self_deaf = req.self_deaf;
                place.state.suppress = suppress;
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
                let state = pb::VoiceState {
                    user_id: account.id.clone(),
                    channel_id: channel_id.clone(),
                    self_mute: req.self_mute,
                    self_deaf: req.self_deaf,
                    joined_at: Some(timestamp(now_ms())),
                    suppress,
                    ..Default::default()
                };
                let place = Place {
                    session_id: req.session_id.clone(),
                    room: voice::channel_room(&server_id, &channel_id),
                    state,
                    expires: lease(),
                };
                self.app.voice.put(&server_id, place.clone());
                self.app.media_link.update(&place).await;
                let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
                publish_voice(&self.app, &server_id, &account.id, update);
                place
            }
        };
        Ok(pb::KeepVoiceResponse { state: Some(place.state) })
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
        if req.server_mute.is_some() || req.server_deaf.is_some() {
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
        let Some(place) = self.app.voice.update(&server_id, &req.user_id, |p| {
            if let Some(mute) = req.server_mute {
                p.state.server_mute = mute;
            }
            if let Some(deaf) = req.server_deaf {
                p.state.server_deaf = deaf;
            }
        }) else {
            return Err(Error::NotFound("voice state"));
        };
        self.app.media_link.update(&place).await;
        let update = Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(place.state.clone()) });
        publish_voice(&self.app, &server_id, &account.id, update);
        Ok(pb::ModerateVoiceResponse {})
    }

    // ───────────────────────── Direct-message calls ─────────────────────────

    async fn join_dm_call(&self, metadata: &MetadataMap, req: pb::JoinDmCallRequest) -> Result<pb::JoinDmCallResponse> {
        let account = self.account(metadata).await?;
        self.calls_on()?;
        check_session(&req.session_id)?;
        let conversation = self.app.dms()?.conversation_of(&account.id, &req.conversation_id).await?;
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
            joined_at: before.as_ref().and_then(|p| p.state.joined_at).or(Some(timestamp(now_ms()))),
            ..Default::default()
        };
        let place = Place { session_id, room: voice::dm_room(&conversation.id), state, expires: lease() };
        let answer = self.app.media_link.open(&place, &req.offer).await?;
        self.app.voice.put(&scope, place.clone());
        publish_dm_call(&self.app, &conversation.id, &conversation.participants);
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
            publish_dm_call(&self.app, &conversation.id, &conversation.participants);
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
        if !self.app.settings().calls {
            if let Some(place) = self.app.voice.remove(&scope, &account.id, None) {
                self.app.media_link.close(&place.room, Some(&account.id), Some(&place.session_id)).await;
                publish_dm_call(&self.app, &conversation.id, &conversation.participants);
            }
            return Err(moved_away());
        }
        let before = self.app.voice.get(&scope, &account.id);
        if before.as_ref().is_some_and(|p| p.session_id != req.session_id) {
            return Err(moved_away());
        }
        let changed =
            before.as_ref().is_none_or(|p| p.state.self_mute != req.self_mute || p.state.self_deaf != req.self_deaf);
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
        place.expires = lease();
        self.app.voice.put(&scope, place.clone());
        if changed {
            publish_dm_call(&self.app, &conversation.id, &conversation.participants);
        }
        Ok(pb::KeepDmCallResponse { state: Some(place.state) })
    }

    async fn list_dm_calls(&self, metadata: &MetadataMap) -> Result<pb::ListDmCallsResponse> {
        let account = self.account(metadata).await?;
        let dms = self.app.dms()?;
        let mut calls = Vec::new();
        for scope in self.app.voice.scopes("dm:") {
            let conversation_id = scope.trim_start_matches("dm:");
            if dms.conversation_of(&account.id, conversation_id).await.is_ok() {
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
                let account = self.account(request.metadata()).await?;
                let settings = self.app.settings();
                let enabled = settings.calls && self.app.media_link.is_on();
                let ice_servers = match enabled {
                    true => ice_servers(&settings.ice_urls, &settings.turn_secret, &account.id),
                    false => vec![],
                };
                Ok(pb::GetCallSettingsResponse { enabled, ice_servers })
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
        assert_eq!(username, format!("{}:acc", 1000 + 24 * 60 * 60));
        assert_eq!(credential.len(), 28);
        let servers = ice_servers(&["stun:a:3478".into(), "turn:b:3478".into()], "north", "acc");
        assert_eq!(servers.len(), 2);
        assert!(servers[0].username.is_empty());
        assert!(!servers[1].credential.is_empty());
    }
}
