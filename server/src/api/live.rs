//! `LiveService`: one stream per app (docs/live.md), made of the event,
//! direct-message, friend and presence streams, with what's in focus
//! (`crate::live`) choosing which messages come whole.

use std::pin::Pin;
use std::time::Duration;

use futures::stream::SelectAll;
use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::error::Error;
use crate::live::{Intent, Interest, check_focus};
use crate::pb::open_response::Item;
use crate::pb::{self, live_service_server::LiveService};

/// How often an idle stream gets a heartbeat, so proxies don't close it.
const HEARTBEAT: Duration = Duration::from_secs(25);
/// What an open stream gets when the instance stops.
const RESTARTING: &str = "this instance is restarting; connect again from your last sequences";

type OpenStream = Pin<Box<dyn Stream<Item = Result<pb::OpenResponse, Status>> + Send>>;
type Feed = Pin<Box<dyn Stream<Item = Result<pb::OpenResponse, Status>> + Send>>;

/// Whether a feed's item is only its heartbeat (the connection sends its own).
fn is_heartbeat(item: &Item) -> bool {
    match item {
        Item::Events(r) => **r == pb::SubscribeResponse::default(),
        Item::DirectMessages(r) => *r == pb::WatchResponse::default(),
        Item::Friends(r) => *r == pb::WatchFriendsResponse::default(),
        Item::Presence(r) => *r == pb::WatchPresenceResponse::default(),
        _ => false,
    }
}

fn feed<T: Send + 'static>(
    stream: impl Stream<Item = Result<T, Status>> + Send + 'static,
    item: fn(T) -> Item,
) -> Feed {
    Box::pin(stream.map(move |next| next.map(|r| pb::OpenResponse { item: Some(item(r)) })))
}

#[tonic::async_trait]
impl LiveService for Api {
    type OpenStream = OpenStream;

    async fn open(&self, request: Request<pb::OpenRequest>) -> Result<Response<OpenStream>, Status> {
        let metadata = request.metadata().clone();
        let caller = self.caller(&metadata).await?;
        let req = request.into_inner();
        let agent = caller.account.kind == pb::AccountKind::Agent;
        if req.presence && agent {
            return Err(Error::denied("agents can't watch presence").into());
        }
        // Counted like any other stream, so connections can't pile up. On a
        // split instance this is the directory's half: one per connection too.
        let per_account = self.app.settings().streams_per_account();
        let ticket = self.app.streams.open(crate::streams::Kind::Live, &caller.account.id, per_account)?;
        let focus = check_focus(req.focus.unwrap_or_default())?;
        // Listening before anything else, so an end said meanwhile isn't missed.
        let mut ended = self.app.ended_sessions();
        let opened = self.app.connections.open(&caller.account.id, &caller.token_hash, focus)?;
        let mut feeds: SelectAll<Feed> = SelectAll::new();
        if !req.servers.is_empty() || req.follow_new_servers {
            let interest = Interest::new(Intent::of(req.messages, agent), &caller.account.id, opened.focus.clone());
            let subscribe = pb::SubscribeRequest { servers: req.servers, follow_new_servers: req.follow_new_servers };
            let rx = self.events(caller.clone(), subscribe, Some(interest)).await?;
            feeds.push(Box::pin(ReceiverStream::new(rx)));
        }
        if req.direct_messages {
            let request = Request::from_parts(metadata.clone(), Default::default(), pb::WatchRequest {});
            let stream = pb::direct_message_service_server::DirectMessageService::watch(self, request);
            feeds.push(feed(stream.await?.into_inner(), Item::DirectMessages));
        }
        if req.friends {
            let request = Request::from_parts(metadata.clone(), Default::default(), pb::WatchFriendsRequest {});
            let stream = pb::friend_service_server::FriendService::watch_friends(self, request);
            feeds.push(feed(stream.await?.into_inner(), Item::Friends));
        }
        if req.presence {
            let rx = self.presence_feed(caller.clone(), Some(opened.focus.clone())).await?;
            feeds.push(feed(ReceiverStream::new(rx), Item::Presence));
        }

        let (tx, rx) = mpsc::channel::<Result<pb::OpenResponse, Status>>(256);
        let app = self.app.clone();
        tokio::spawn(async move {
            let mut focus = opened.focus.clone();
            let hello = pb::OpenResponse { item: Some(Item::ConnectionId(opened.id.clone())) };
            if tx.send(Ok(hello)).await.is_err() {
                return;
            }
            // Holds the connection open for Focus until this task ends.
            let (_opened, _ticket) = (opened, ticket);
            let mut heartbeat = crate::streams::heartbeat(HEARTBEAT);
            let mut session = crate::streams::SessionCheck::new(&app, &caller.token_hash);
            loop {
                tokio::select! {
                    _ = app.shutdown.cancelled() => {
                        let _ = tx.try_send(Err(Status::unavailable(RESTARTING)));
                        return;
                    }
                    _ = tx.closed() => return,
                    _ = heartbeat.tick() => {
                        // A session signed out elsewhere stops hearing.
                        if !session.still_live(&app).await {
                            let _ = tx.send(Err(Status::unauthenticated("this device was signed out"))).await;
                            return;
                        }
                        if tx.send(Ok(pb::OpenResponse::default())).await.is_err() {
                            return;
                        }
                    }
                    // Signed out: the stream ends now, and its id stops resolving.
                    ended = ended.recv() => match ended {
                        Ok(id) if *id == *caller.account.id => {
                            if matches!(app.session_live(&caller.token_hash).await, Ok(false)) {
                                let _ = tx.send(Err(Status::unauthenticated("this device was signed out"))).await;
                                return;
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => session.due(),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    },
                    Ok(()) = focus.changed() => {
                        let now = focus.borrow_and_update().as_ref().clone();
                        if tx.send(Ok(pb::OpenResponse { item: Some(Item::Focus(now)) })).await.is_err() {
                            return;
                        }
                    }
                    next = feeds.next(), if !feeds.is_empty() => match next {
                        // Every feed ended (each followed server went, say):
                        // the connection stays for what's left to say.
                        None => {}
                        Some(Err(status)) => {
                            let _ = tx.send(Err(status)).await;
                            return;
                        }
                        Some(Ok(pb::OpenResponse { item: Some(item) })) if is_heartbeat(&item) => {}
                        Some(Ok(response)) => {
                            if tx.send(Ok(response)).await.is_err() {
                                return;
                            }
                        }
                    },
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn focus(&self, request: Request<pb::FocusRequest>) -> Result<Response<pb::FocusResponse>, Status> {
        respond(
            async {
                let caller = self.caller(request.metadata()).await?;
                let req = request.into_inner();
                // Only the latest matters: each replaces the last, and what
                // follows it reads whatever is there by then.
                let focus = check_focus(req.focus.unwrap_or_default())?;
                self.app.connections.focus(&req.connection_id, &caller.account.id, &caller.token_hash, focus)?;
                Ok(pb::FocusResponse {})
            }
            .await,
        )
    }
}
