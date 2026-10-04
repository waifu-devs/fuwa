use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_stream::StreamMap;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use tonic::{Request, Response, Status};

use super::{Api, Seat, respond};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, event_service_server::EventService};
use crate::permissions::Access;
use crate::servers::{self as store, Payload, ServerDb};

/// How often an idle stream gets a heartbeat, so proxies don't close it.
const HEARTBEAT: Duration = Duration::from_secs(25);
/// Most servers one stream can follow.
const MAX_SERVERS: usize = 200;
/// Events read from disk at a time while replaying.
const REPLAY_PAGE: i64 = 500;
/// What an open stream gets when the instance stops.
const RESTARTING: &str = "this instance is restarting; subscribe again from your last sequence";

type EventStream = Pin<Box<dyn Stream<Item = Result<pb::SubscribeResponse, Status>> + Send>>;

/// The channel an event is about, if it's about one.
fn channel_of(payload: &Payload) -> Option<&str> {
    match payload {
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }) => Some(&m.channel_id),
        Payload::MessageDeleted(d) => Some(&d.channel_id),
        Payload::ChannelCreated(pb::ChannelCreated { channel: Some(c) })
        | Payload::ChannelUpdated(pb::ChannelUpdated { channel: Some(c) }) => Some(&c.id),
        Payload::ChannelDeleted(d) => Some(&d.channel_id),
        Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(s) }) => Some(&s.channel_id),
        Payload::VoiceStateRemoved(r) => Some(&r.channel_id),
        Payload::SecureRecordAdded(pb::SecureRecordAdded { record: Some(r) }) => Some(&r.channel_id),
        Payload::SecureRecordDeleted(d) => Some(&d.channel_id),
        _ => None,
    }
}

/// Whether a member with `access` gets an event: not one about a channel they
/// can't see, nor applications unless they can review them.
fn shown_to(access: &Access, payload: &Payload) -> bool {
    match payload {
        Payload::ApplicationUpdated(_) => access.has(pb::Permission::KickMembers),
        payload => channel_of(payload).is_none_or(|channel_id| access.can_see(channel_id)),
    }
}

/// Whether an event can change what `account_id` can see or do in its server.
fn changes_access(payload: &Payload, account_id: &str) -> bool {
    match payload {
        Payload::RoleCreated(_)
        | Payload::RoleUpdated(_)
        | Payload::RoleDeleted(_)
        | Payload::ChannelCreated(_)
        | Payload::ChannelUpdated(_)
        | Payload::ChannelDeleted(_)
        | Payload::ServerUpdated(_) => true,
        Payload::MemberUpdated(pb::MemberUpdated { member: Some(m) }) => {
            m.user.as_ref().is_some_and(|u| u.id == account_id)
        }
        _ => false,
    }
}

/// One member's view of one server's events: what they can see, worked out
/// again whenever an event changes it.
struct View {
    sdb: Arc<ServerDb>,
    account_id: String,
    access: Access,
    /// Catching up: what the member can see is what they can see now, not
    /// what they could when each event happened.
    replaying: bool,
}

impl View {
    /// What the member gets for `event`: nothing if it's about a channel they
    /// can't see; when it changes what they can see, the channels that appear
    /// for them (ChannelCreated) and go (ChannelDeleted), not stored, so
    /// sequence 0.
    async fn pass(&mut self, event: &pb::Event) -> Vec<pb::Event> {
        let mut out = self.pass_unscrubbed(event).await;
        let manager = self.access.has(pb::Permission::ManageServer);
        for event in &mut out {
            if let Some(
                Payload::MemberJoined(pb::MemberJoined { member: Some(member) })
                | Payload::MemberUpdated(pb::MemberUpdated { member: Some(member) }),
            ) = &mut event.payload
            {
                store::scrub_sso(member, &self.account_id, manager);
            }
        }
        out
    }

    async fn pass_unscrubbed(&mut self, event: &pb::Event) -> Vec<pb::Event> {
        let Some(payload) = &event.payload else { return vec![event.clone()] };
        if !changes_access(payload, &self.account_id) {
            return if shown_to(&self.access, payload) { vec![event.clone()] } else { vec![] };
        }
        let before = self.access.visible();
        match self.load().await {
            Ok(Some(access)) => self.access = access,
            Ok(None) => {}
            Err(err) => tracing::warn!(server = %self.sdb.id, error = %err, "couldn't work out a member's permissions"),
        }
        let after = self.access.visible();
        let own = channel_of(payload);
        let (created, deleted) =
            (matches!(payload, Payload::ChannelCreated(_)), matches!(payload, Payload::ChannelDeleted(_)));
        let shown = match own {
            // The member may have had a channel deleted while they were away,
            // so a replay always says it went; its id is all it gives away.
            Some(id) if deleted => self.replaying || before.contains(id),
            Some(id) if created => after.contains(id),
            // A channel that appears or goes because of this change comes as
            // ChannelCreated or ChannelDeleted below instead.
            Some(id) => before.contains(id) && after.contains(id),
            None => true,
        };
        let mut out = Vec::new();
        if shown {
            out.push(event.clone());
        }
        let unstored = |payload| pb::Event {
            id: new_id(),
            server_id: event.server_id.clone(),
            sequence: 0,
            actor_id: event.actor_id.clone(),
            created_at: Some(timestamp(now_ms())),
            payload: Some(payload),
        };
        let mut appeared: Vec<&String> =
            after.difference(&before).filter(|id| !(created && Some(id.as_str()) == own)).collect();
        appeared.sort();
        if !appeared.is_empty()
            && let Ok(conn) = self.sdb.read()
        {
            for id in appeared {
                if let Ok(Some(channel)) = store::load_channel(&conn, &self.sdb.id, id).await {
                    out.push(unstored(Payload::ChannelCreated(pb::ChannelCreated { channel: Some(channel) })));
                }
            }
        }
        let mut gone: Vec<&String> =
            before.difference(&after).filter(|id| !(deleted && Some(id.as_str()) == own)).collect();
        gone.sort();
        for id in gone {
            out.push(unstored(Payload::ChannelDeleted(pb::ChannelDeleted { channel_id: id.clone() })));
        }
        out
    }

    async fn load(&self) -> Result<Option<Access>> {
        let conn = self.sdb.read()?;
        Ok(store::member_access(&conn, &self.sdb.id, &self.account_id).await?.map(|(_, access)| access))
    }
}

#[tonic::async_trait]
impl EventService for Api {
    type SubscribeStream = EventStream;

    async fn subscribe(&self, request: Request<pb::SubscribeRequest>) -> Result<Response<EventStream>, Status> {
        let started = std::time::Instant::now();
        let caller = self.caller(request.metadata()).await?;
        let account = caller.account;
        let pb::SubscribeRequest { servers: cursors, follow_new_servers } = request.into_inner();
        if cursors.len() > MAX_SERVERS || (cursors.is_empty() && !follow_new_servers) {
            return Err(Error::invalid(format!("follow 1 to {MAX_SERVERS} servers per stream")).into());
        }
        // A split instance's gateway follows new servers itself, from the
        // directory, and asks shards only for the ones it knows of.
        let follow_new = follow_new_servers && !self.app.config.cluster.is_split();
        // Listening before anything else, so nothing said meanwhile is missed.
        let mut ended = self.app.ended_sessions();
        let mut joined = self.app.joined_servers();

        // Start listening before replaying, so nothing committed in between is missed.
        // A server deleted, or left (or been removed from) while the client was
        // away gets the event that says so, so the client lets it go.
        let mut followed = Vec::with_capacity(cursors.len());
        let mut gone = Vec::new();
        for cursor in cursors {
            let payload = match self.membership(&account, &cursor.server_id).await {
                Ok(Seat { sdb, access, .. }) => {
                    let live = self.app.hub.subscribe(&sdb.id);
                    let view = View { sdb: sdb.clone(), account_id: account.id.clone(), access, replaying: true };
                    followed.push((sdb, cursor.after_sequence, live, view));
                    continue;
                }
                Err(Error::NotFound(_)) => Payload::ServerDeleted(pb::ServerDeleted {}),
                Err(Error::PermissionDenied(_)) => {
                    Payload::MemberLeft(pb::MemberLeft { user_id: account.id.clone(), ..Default::default() })
                }
                Err(err) => return Err(err.into()),
            };
            gone.push(pb::Event {
                id: new_id(),
                server_id: cursor.server_id,
                sequence: 0,
                actor_id: String::new(),
                created_at: Some(timestamp(now_ms())),
                payload: Some(payload),
            });
        }

        let (tx, rx) = mpsc::channel::<Result<pb::SubscribeResponse, Status>>(256);
        let shutdown = self.app.shutdown.clone();
        let app = self.app.clone();
        let token_hash = caller.token_hash;
        let account_id = account.id.clone();
        let api = self.clone();
        tokio::spawn(async move {
            let send = async |item| tx.send(item).await.is_ok();
            for event in gone {
                if !send(Ok(pb::SubscribeResponse { event: Some(event), ..Default::default() })).await {
                    return;
                }
            }
            let mut live = StreamMap::new();
            let mut last_sent: HashMap<String, i64> = HashMap::new();
            let mut views: HashMap<String, View> = HashMap::new();

            let mut heads = Vec::with_capacity(followed.len());
            for (sdb, after, receiver, mut view) in followed {
                let mut sequence = match after {
                    Some(after) => after.max(0),
                    // Live only: anything committed up to now is already in the
                    // state a client loads after `ready`.
                    None => match sdb.head_sequence().await {
                        Ok(head) => head,
                        Err(err) => {
                            send(Err(err.into())).await;
                            return;
                        }
                    },
                };
                if after.is_some() {
                    loop {
                        let page = match sdb.events_after(sequence, REPLAY_PAGE).await {
                            Ok(page) => page,
                            Err(err) => {
                                send(Err(err.into())).await;
                                return;
                            }
                        };
                        let Some(last) = page.last() else { break };
                        sequence = last.sequence;
                        for event in page {
                            for event in view.pass(&event).await {
                                if !send(Ok(pb::SubscribeResponse { event: Some(event), ..Default::default() })).await {
                                    return;
                                }
                            }
                        }
                    }
                }
                view.replaying = false;
                last_sent.insert(sdb.id.clone(), sequence);
                heads.push(pb::ServerHead { server_id: sdb.id.clone(), sequence });
                live.insert(sdb.id.clone(), BroadcastStream::new(receiver));
                views.insert(sdb.id.clone(), view);
            }
            let ready = pb::SubscribeReady { servers: heads };
            crate::reports::server_timing("subscribe.ready", started.elapsed());
            if !send(Ok(pb::SubscribeResponse { ready: Some(ready), ..Default::default() })).await {
                return;
            }
            if live.is_empty() && !follow_new {
                return; // nothing left to follow
            }

            let mut heartbeat = tokio::time::interval(HEARTBEAT);
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        // Stopping, say for a deploy: tell the client to follow again
                        // rather than end the stream as if it were done.
                        let _ = tx.try_send(Err(Status::unavailable(RESTARTING)));
                        return;
                    }
                    _ = tx.closed() => return,
                    _ = heartbeat.tick() => {
                        // A session signed out from another device ends its streams too.
                        if matches!(app.session_live(&token_hash).await, Ok(false)) {
                            send(Err(Status::unauthenticated("this device was signed out"))).await;
                            return;
                        }
                        if !send(Ok(pb::SubscribeResponse::default())).await {
                            return;
                        }
                    }
                    // Some session of the caller's just ended: if it's this one,
                    // the stream ends now rather than at the next heartbeat.
                    ended = ended.recv() => {
                        let ours = match ended {
                            Ok(id) => *id == *account_id,
                            Err(RecvError::Lagged(_)) => true,
                            Err(RecvError::Closed) => return,
                        };
                        if ours && matches!(app.session_live(&token_hash).await, Ok(false)) {
                            send(Err(Status::unauthenticated("this device was signed out"))).await;
                            return;
                        }
                    }
                    // Joined somewhere (an agent added to a server, say): follow
                    // it from now on, and say from where.
                    next = joined.recv(), if follow_new => {
                        let server_id = match next {
                            Ok((who, server_id)) if *who == *account_id => server_id,
                            Ok(_) | Err(RecvError::Lagged(_)) => continue,
                            Err(RecvError::Closed) => return,
                        };
                        if views.contains_key(&*server_id) || views.len() >= MAX_SERVERS {
                            continue;
                        }
                        let Ok(Seat { sdb, access, .. }) = api.membership(&account, &server_id).await else {
                            continue; // gone again already
                        };
                        let receiver = app.hub.subscribe(&sdb.id);
                        let sequence = match sdb.head_sequence().await {
                            Ok(head) => head,
                            Err(err) => {
                                send(Err(err.into())).await;
                                return;
                            }
                        };
                        let view = View { sdb: sdb.clone(), account_id: account_id.clone(), access, replaying: false };
                        last_sent.insert(sdb.id.clone(), sequence);
                        live.insert(sdb.id.clone(), BroadcastStream::new(receiver));
                        views.insert(sdb.id.clone(), view);
                        let head = pb::ServerHead { server_id: sdb.id.clone(), sequence };
                        if !send(Ok(pb::SubscribeResponse { followed: Some(head), ..Default::default() })).await {
                            return;
                        }
                    }
                    item = live.next(), if !live.is_empty() => match item {
                        None => return,
                        Some((server_id, Err(BroadcastStreamRecvError::Lagged(_)))) => {
                            let message = format!("fell behind on server {server_id}; subscribe again from your last sequence");
                            send(Err(Status::aborted(message))).await;
                            return;
                        }
                        Some((_, Ok(event))) if crate::hub::is_moved(&event) => {
                            send(Err(crate::error::Error::Misrouted.into())).await;
                            return;
                        }
                        Some((server_id, Ok(event))) => {
                            let last = last_sent.entry(server_id.clone()).or_default();
                            if event.sequence != 0 && event.sequence <= *last {
                                continue; // already sent while replaying
                            }
                            *last = event.sequence.max(*last);
                            let ends = match &event.payload {
                                Some(Payload::ServerDeleted(_)) => true,
                                Some(Payload::MemberLeft(left)) => left.user_id == account_id,
                                _ => false,
                            };
                            let out = match views.get_mut(&server_id) {
                                Some(view) if !ends => view.pass(&event).await,
                                _ => vec![(*event).clone()],
                            };
                            for event in out {
                                if !send(Ok(pb::SubscribeResponse { event: Some(event), ..Default::default() })).await {
                                    return;
                                }
                            }
                            if ends {
                                live.remove(&server_id);
                                views.remove(&server_id);
                                if live.is_empty() && !follow_new {
                                    return;
                                }
                            }
                        }
                    },
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn list_events(
        &self,
        request: Request<pb::ListEventsRequest>,
    ) -> Result<Response<pb::ListEventsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                let limit = if req.limit <= 0 { 100 } else { req.limit.min(500) } as i64;
                let mut events = sdb.events_after(req.after_sequence.max(0), limit + 1).await?;
                let has_more = events.len() as i64 > limit;
                events.truncate(limit as usize);
                // What they can't see now is left out, as a stream leaves it out.
                events.retain(|e| match &e.payload {
                    Some(Payload::ChannelDeleted(_)) | None => true,
                    Some(payload) => shown_to(&access, payload),
                });
                Ok(pb::ListEventsResponse { events, has_more })
            }
            .await,
        )
    }
}
