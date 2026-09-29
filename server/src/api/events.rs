use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_stream::StreamMap;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, event_service_server::EventService};
use crate::servers::Payload;

/// How often an idle stream gets a heartbeat, so proxies don't close it.
const HEARTBEAT: Duration = Duration::from_secs(25);
/// Most servers one stream can follow.
const MAX_SERVERS: usize = 200;
/// Events read from disk at a time while replaying.
const REPLAY_PAGE: i64 = 500;

type EventStream = Pin<Box<dyn Stream<Item = Result<pb::SubscribeResponse, Status>> + Send>>;

#[tonic::async_trait]
impl EventService for Api {
    type SubscribeStream = EventStream;

    async fn subscribe(&self, request: Request<pb::SubscribeRequest>) -> Result<Response<EventStream>, Status> {
        let caller = self.caller(request.metadata()).await?;
        let account = caller.account;
        let cursors = request.into_inner().servers;
        if cursors.is_empty() || cursors.len() > MAX_SERVERS {
            return Err(Error::invalid(format!("follow 1 to {MAX_SERVERS} servers per stream")).into());
        }

        // Start listening before replaying, so nothing committed in between is missed.
        // A server deleted, or left (or been removed from) while the client was
        // away gets the event that says so, so the client lets it go.
        let mut followed = Vec::with_capacity(cursors.len());
        let mut gone = Vec::new();
        for cursor in cursors {
            let payload = match self.membership(&account, &cursor.server_id).await {
                Ok((sdb, _)) => {
                    let live = self.app.hub.subscribe(&sdb.id);
                    followed.push((sdb, cursor.after_sequence, live));
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
        tokio::spawn(async move {
            let send = async |item| tx.send(item).await.is_ok();
            for event in gone {
                if !send(Ok(pb::SubscribeResponse { event: Some(event), ready: None })).await {
                    return;
                }
            }
            let mut live = StreamMap::new();
            let mut last_sent: HashMap<String, i64> = HashMap::new();

            let mut heads = Vec::with_capacity(followed.len());
            for (sdb, after, receiver) in followed {
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
                            if !send(Ok(pb::SubscribeResponse { event: Some(event), ready: None })).await {
                                return;
                            }
                        }
                    }
                }
                last_sent.insert(sdb.id.clone(), sequence);
                heads.push(pb::ServerHead { server_id: sdb.id.clone(), sequence });
                live.insert(sdb.id.clone(), BroadcastStream::new(receiver));
            }
            let ready = pb::SubscribeReady { servers: heads };
            if !send(Ok(pb::SubscribeResponse { event: None, ready: Some(ready) })).await {
                return;
            }

            let mut heartbeat = tokio::time::interval(HEARTBEAT);
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = tx.closed() => return,
                    _ = heartbeat.tick() => {
                        // A session signed out from another device ends its streams too.
                        if matches!(app.node.session_live(&token_hash).await, Ok(false)) {
                            send(Err(Status::unauthenticated("this device was signed out"))).await;
                            return;
                        }
                        if !send(Ok(pb::SubscribeResponse { event: None, ready: None })).await {
                            return;
                        }
                    }
                    item = live.next() => match item {
                        None => return,
                        Some((server_id, Err(BroadcastStreamRecvError::Lagged(_)))) => {
                            let message = format!("fell behind on server {server_id}; subscribe again from your last sequence");
                            send(Err(Status::aborted(message))).await;
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
                            if !send(Ok(pb::SubscribeResponse { event: Some((*event).clone()), ready: None })).await {
                                return;
                            }
                            if ends {
                                live.remove(&server_id);
                                if live.is_empty() {
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
                let (sdb, _) = self.membership(&account, &req.server_id).await?;
                let limit = if req.limit <= 0 { 100 } else { req.limit.min(500) } as i64;
                let mut events = sdb.events_after(req.after_sequence.max(0), limit + 1).await?;
                let has_more = events.len() as i64 > limit;
                events.truncate(limit as usize);
                Ok(pb::ListEventsResponse { events, has_more })
            }
            .await,
        )
    }
}
