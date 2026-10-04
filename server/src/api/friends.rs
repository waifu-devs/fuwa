//! `FriendService`: friends, friend requests and blocks between people on
//! this instance (docs/friends.md). Everything here is the caller's own and
//! reaches only the two people a change is about; a block reaches nobody.
//! The store is `crate::friends`; who may message whom is [`Api::may_message`],
//! which direct messages ask too.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::app::App;
use crate::error::{Error, Result};
use crate::friends::{Link, MAX_MUTUAL, Sent};
use crate::id::{now_ms, timestamp};
use crate::node::Account;
use crate::pb::{self, FriendState, friend_event::Payload, friend_service_server::FriendService};

/// How often an idle stream gets a heartbeat, so proxies don't close it.
const HEARTBEAT: Duration = Duration::from_secs(25);
/// What an open stream gets when the instance stops.
const RESTARTING: &str = "this instance is restarting; watch again";
/// Friends are people's: agents have none, and can't be asked.
const AGENTS_HAVE_NO_FRIENDS: &str = "agents can't have friends";
/// What someone gets when they can't message a person, whether it's that
/// person's settings or a block: the same words, so a block never shows.
pub(super) const NOT_TAKING_MESSAGES: &str = "they aren't taking direct messages from you";

type WatchStream = Pin<Box<dyn Stream<Item = Result<pb::WatchFriendsResponse, Status>> + Send>>;

fn changed(friend: pb::Friend) -> pb::FriendEvent {
    pb::FriendEvent { payload: Some(Payload::Changed(friend)) }
}

fn removed(user_id: &str) -> pb::FriendEvent {
    pb::FriendEvent { payload: Some(Payload::Removed(user_id.to_string())) }
}

/// Whether `settings` take a friend request from someone, given whether
/// they share a server.
fn takes_requests(settings: &pb::FriendSettings, share_a_server: bool) -> bool {
    match settings.requests_from() {
        pb::FriendRequestsFrom::Unspecified => true,
        pb::FriendRequestsFrom::SharedServers => share_a_server,
        pb::FriendRequestsFrom::Nobody => false,
    }
}

/// Whether `settings` let someone start a conversation.
fn takes_conversations(settings: &pb::FriendSettings, friends: bool, share_a_server: bool) -> bool {
    match settings.direct_messages_from() {
        pb::DirectMessagesFrom::Unspecified => friends || share_a_server,
        pb::DirectMessagesFrom::Friends => friends,
        pb::DirectMessagesFrom::Nobody => false,
    }
}

/// An open `WatchFriends` stream: its account is online while one is held,
/// and the last one going, however the stream ends, takes it offline.
struct Online {
    app: Arc<App>,
    account_id: String,
}

impl Online {
    fn new(app: &Arc<App>, account_id: &str) -> Self {
        if app.friends().is_ok_and(|friends| friends.opened(account_id)) {
            announce(app.clone(), account_id.to_string());
        }
        Self { app: app.clone(), account_id: account_id.to_string() }
    }
}

impl Drop for Online {
    fn drop(&mut self) {
        if self.app.friends().is_ok_and(|friends| friends.closed(&self.account_id)) {
            announce(self.app.clone(), std::mem::take(&mut self.account_id));
        }
    }
}

/// Tells someone's friends whether they're online, as they are when it
/// runs (so a quick on-and-off settles right), unless they hide it.
fn announce(app: Arc<App>, account_id: String) {
    tokio::spawn(async move {
        let told = async {
            let friends = app.friends()?;
            let ids = friends.friend_ids(&account_id).await?;
            if ids.is_empty() {
                return Ok(());
            }
            let online = friends.is_online(&account_id) && !friends.settings(&account_id).await?.hide_online;
            let presence = pb::FriendPresence { user_id: account_id.clone(), online };
            friends.publish(
                ids.iter().map(String::as_str),
                pb::FriendEvent { payload: Some(Payload::Presence(presence)) },
            );
            Ok::<_, Error>(())
        }
        .await;
        if told.is_err() {
            crate::reports::server_error("friends_presence_failed", Some("friends.presence"));
            tracing::warn!("couldn't tell friends someone came or went");
        }
    });
}

/// Whether `account_id` blocked anyone else among `participants`: messages
/// and calls in their conversation are kept from them.
pub(super) async fn blocks_any(app: &App, account_id: &str, participants: &[String]) -> Result<bool> {
    let friends = app.friends()?;
    for other in participants.iter().filter(|id| *id != account_id) {
        if friends.blocked(account_id, other).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the recipient of a direct message sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    Seen,
    /// They blocked the sender, who isn't told.
    Hidden,
}

impl Api {
    /// The caller, who must be a person.
    async fn befriender(&self, metadata: &MetadataMap) -> Result<Account> {
        let account = self.account(metadata).await?;
        if account.kind == pb::AccountKind::Agent {
            return Err(Error::FailedPrecondition(AGENTS_HAVE_NO_FRIENDS.into()));
        }
        Ok(account)
    }

    /// The person a request names, by id or username: someone else, a
    /// person, and not turned off.
    async fn other_person(&self, me: &Account, user_id: &str, username: &str) -> Result<Account> {
        let node = self.app.node()?;
        let (user_id, username) = (user_id.trim(), username.trim().trim_start_matches('@').to_lowercase());
        let other = match (user_id.is_empty(), username.is_empty()) {
            (false, true) => node.account(user_id).await?,
            (true, false) => node.account_by_username(&username).await?,
            _ => return Err(Error::invalid("give a user_id or a username")),
        };
        let other = other.filter(|other| !other.disabled).ok_or(Error::NotFound("user"))?;
        if other.id == me.id {
            return Err(Error::invalid("that's you"));
        }
        if other.kind == pb::AccountKind::Agent {
            return Err(Error::FailedPrecondition(AGENTS_HAVE_NO_FRIENDS.into()));
        }
        Ok(other)
    }

    /// Rows as clients see them, with each person and, for friends who show
    /// it, whether they're online.
    async fn friends_pb(&self, links: &[Link]) -> Result<Vec<pb::Friend>> {
        let friends = self.app.friends()?;
        let ids: Vec<&str> = links.iter().map(|link| link.other_id.as_str()).collect();
        let users: HashMap<String, pb::User> =
            self.app.node()?.accounts(&ids).await?.into_iter().map(|a| (a.id.clone(), a.user())).collect();
        let friend_ids: Vec<&str> =
            links.iter().filter(|l| l.state == FriendState::Friend).map(|l| l.other_id.as_str()).collect();
        let online = friends.online_of(friend_ids.iter().copied());
        let hidden = friends.settings_of(&friend_ids).await?;
        Ok(links
            .iter()
            .filter_map(|link| {
                // Someone deleted since: the sweep that forgot them is moments away.
                let user = users.get(&link.other_id)?.clone();
                let shows = !hidden.get(&link.other_id).is_some_and(|s| s.hide_online);
                Some(pb::Friend {
                    user: Some(user),
                    state: link.state as i32,
                    since: Some(timestamp(link.created_at)),
                    expires_at: link.expires_at.map(timestamp),
                    online: link.state == FriendState::Friend && shows && online.contains(&link.other_id),
                })
            })
            .collect())
    }

    async fn friend_pb(&self, link: &Link) -> Result<pb::Friend> {
        self.friends_pb(std::slice::from_ref(link)).await?.pop().ok_or(Error::NotFound("user"))
    }

    /// Whether `from` may send `to` a direct message, and whether `to` sees
    /// it. `from`'s own block stops them (they unblock to write); otherwise
    /// an existing conversation always goes on and a new one is as `to`'s
    /// settings say. Someone `to` blocked is never told: what they send is
    /// taken as usual and kept from `to` (see [`blocks_any`]), so the
    /// only refusals anyone gets are the settings' own.
    pub(super) async fn may_message(&self, from: &str, to: &str, existing: bool) -> Result<Reach> {
        let friends = self.app.friends()?;
        // Everything is read whatever the answer, so a block takes no
        // longer to answer than a setting.
        let settings = friends.settings(to).await?;
        let are_friends = friends.are_friends(from, to).await?;
        let takes = existing || takes_conversations(&settings, are_friends, self.app.index.share_a_server(from, to));
        let mine = friends.blocked(from, to).await?;
        let theirs = friends.blocked(to, from).await?;
        if mine {
            return Err(Error::FailedPrecondition("you blocked them; unblock them first".into()));
        }
        if !takes {
            return Err(Error::denied(NOT_TAKING_MESSAGES));
        }
        Ok(if theirs { Reach::Hidden } else { Reach::Seen })
    }

    async fn list_friends(&self, metadata: &MetadataMap) -> Result<pb::ListFriendsResponse> {
        let me = self.befriender(metadata).await?;
        let links = self.app.friends()?.links(&me.id, now_ms()).await?;
        Ok(pb::ListFriendsResponse { friends: self.friends_pb(&links).await? })
    }

    async fn send_friend_request(
        &self,
        metadata: &MetadataMap,
        req: pb::SendFriendRequestRequest,
    ) -> Result<pb::SendFriendRequestResponse> {
        let me = self.befriender(metadata).await?;
        let other = self.other_person(&me, &req.user_id, &req.username).await?;
        let friends = self.app.friends()?;
        let now = now_ms();
        // Their settings first, and only then (inside `request`) a block, so
        // a blocked sender gets the answer anyone else would.
        let theirs = friends.settings(&other.id).await?;
        let pending = friends.link(&me.id, &other.id, now).await?;
        let answering =
            pending.as_ref().is_some_and(|l| matches!(l.state, FriendState::Incoming | FriendState::Friend));
        if !answering && !takes_requests(&theirs, self.app.index.share_a_server(&me.id, &other.id)) {
            return Err(Error::FailedPrecondition("they aren't taking friend requests from you".into()));
        }
        // A new request takes one of the hour's, given back if none came of it.
        let reserved = pending.is_none();
        if reserved && !friends.reserve(&me.id, now) {
            crate::reports::server_used("friends.request_limited", 1);
            return Err(Error::ResourceExhausted("you've sent a lot of friend requests; try again in a while".into()));
        }
        let sent = friends.request(&me.id, &other.id, now).await;
        if reserved && !matches!(sent, Ok(Sent::Asked { .. })) {
            friends.refund(&me.id);
        }
        let mine = match sent? {
            Sent::Already(mine) => mine,
            Sent::Asked { mine, delivered } => {
                crate::reports::server_used("friends.request", 1);
                // Built whether or not it's delivered, so a block takes no
                // less time to answer.
                let theirs = Link {
                    account_id: other.id.clone(),
                    other_id: me.id.clone(),
                    state: FriendState::Incoming,
                    ..mine.clone()
                };
                let theirs = self.friend_pb(&theirs).await?;
                if delivered {
                    friends.publish([other.id.as_str()], changed(theirs));
                }
                mine
            }
            Sent::Friends { mine, theirs } => {
                crate::reports::server_used("friends.accept", 1);
                friends.publish([other.id.as_str()], changed(self.friend_pb(&theirs).await?));
                mine
            }
        };
        let friend = self.friend_pb(&mine).await?;
        friends.publish([me.id.as_str()], changed(friend.clone()));
        Ok(pb::SendFriendRequestResponse { friend: Some(friend) })
    }

    async fn accept_friend_request(
        &self,
        metadata: &MetadataMap,
        req: pb::AcceptFriendRequestRequest,
    ) -> Result<pb::AcceptFriendRequestResponse> {
        let me = self.befriender(metadata).await?;
        let other = self.other_person(&me, &req.user_id, "").await?;
        let friends = self.app.friends()?;
        let (mine, theirs) = friends.accept(&me.id, &other.id, now_ms()).await?;
        crate::reports::server_used("friends.accept", 1);
        let friend = self.friend_pb(&mine).await?;
        friends.publish([me.id.as_str()], changed(friend.clone()));
        friends.publish([other.id.as_str()], changed(self.friend_pb(&theirs).await?));
        Ok(pb::AcceptFriendRequestResponse { friend: Some(friend) })
    }

    async fn remove_friend(
        &self,
        metadata: &MetadataMap,
        req: pb::RemoveFriendRequest,
    ) -> Result<pb::RemoveFriendResponse> {
        let me = self.befriender(metadata).await?;
        // Someone whose account was deleted or turned off can still be let go.
        let other = req.user_id.trim();
        let friends = self.app.friends()?;
        let gone = friends.remove(&me.id, other, now_ms()).await?;
        friends.publish([me.id.as_str()], removed(other));
        if gone.theirs {
            friends.publish([other], removed(&me.id));
        }
        Ok(pb::RemoveFriendResponse {})
    }

    async fn block_user(&self, metadata: &MetadataMap, req: pb::BlockUserRequest) -> Result<pb::BlockUserResponse> {
        let me = self.befriender(metadata).await?;
        let other = self.other_person(&me, &req.user_id, "").await?;
        let friends = self.app.friends()?;
        let (mine, gone) = friends.block(&me.id, &other.id, now_ms()).await?;
        crate::reports::server_used("friends.block", 1);
        // To them it reads as being unfriended, or a request withdrawn.
        if gone.theirs {
            friends.publish([other.id.as_str()], removed(&me.id));
        }
        let friend = self.friend_pb(&mine).await?;
        friends.publish([me.id.as_str()], changed(friend.clone()));
        Ok(pb::BlockUserResponse { friend: Some(friend) })
    }

    async fn unblock_user(
        &self,
        metadata: &MetadataMap,
        req: pb::UnblockUserRequest,
    ) -> Result<pb::UnblockUserResponse> {
        let me = self.befriender(metadata).await?;
        let other = req.user_id.trim();
        let friends = self.app.friends()?;
        friends.unblock(&me.id, other).await?;
        friends.publish([me.id.as_str()], removed(other));
        Ok(pb::UnblockUserResponse {})
    }

    async fn get_relationship(
        &self,
        metadata: &MetadataMap,
        req: pb::GetRelationshipRequest,
    ) -> Result<pb::GetRelationshipResponse> {
        let me = self.befriender(metadata).await?;
        // An agent reads as nobody here, as a stranger does.
        let other = match self.other_person(&me, &req.user_id, "").await {
            Err(Error::FailedPrecondition(_)) => return Err(Error::NotFound("user")),
            other => other?,
        };
        let friends = self.app.friends()?;
        let now = now_ms();
        let link = friends.link(&me.id, &other.id, now).await?;
        let shared = self.app.index.share_a_server(&me.id, &other.id);
        let existing = self.app.dms()?.partners(&me.id).await?.contains(&other.id);
        // Only about people you'd see anyway: friends and requests either
        // way, people in a server with you, and people you've talked with.
        if link.is_none() && !shared && !existing && friends.link(&other.id, &me.id, now).await?.is_none() {
            return Err(Error::NotFound("user"));
        }
        let settings = friends.settings_of(&[me.id.as_str(), other.id.as_str()]).await?;
        let (mine, theirs) =
            (settings.get(&me.id).copied().unwrap_or_default(), settings.get(&other.id).copied().unwrap_or_default());
        let mut mutual_friends = Vec::new();
        if !mine.hide_mutual_friends && !theirs.hide_mutual_friends {
            let ids = friends.mutual(&me.id, &other.id).await?;
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            let hiding = friends.settings_of(&ids).await?;
            let showing: Vec<&str> = ids
                .into_iter()
                .filter(|id| !hiding.get(*id).is_some_and(|s| s.hide_mutual_friends))
                .take(MAX_MUTUAL)
                .collect();
            mutual_friends = self.app.node()?.accounts(&showing).await?.into_iter().map(|a| a.user()).collect();
        }
        let state = link.as_ref().map_or(FriendState::Unspecified, |l| l.state);
        let may_message = self.may_message(&me.id, &other.id, existing).await.is_ok();
        Ok(pb::GetRelationshipResponse {
            state: state as i32,
            expires_at: link.and_then(|l| l.expires_at).map(timestamp),
            mutual_friends,
            may_request: matches!(state, FriendState::Incoming) || takes_requests(&theirs, shared),
            may_message,
        })
    }

    async fn update_friend_settings(
        &self,
        metadata: &MetadataMap,
        req: pb::UpdateFriendSettingsRequest,
    ) -> Result<pb::UpdateFriendSettingsResponse> {
        let me = self.befriender(metadata).await?;
        let settings = req.settings.unwrap_or_default();
        if pb::FriendRequestsFrom::try_from(settings.requests_from).is_err()
            || pb::DirectMessagesFrom::try_from(settings.direct_messages_from).is_err()
        {
            return Err(Error::invalid("that isn't one of the choices"));
        }
        let friends = self.app.friends()?;
        let before = friends.settings(&me.id).await?;
        friends.save_settings(&me.id, &settings).await?;
        friends.publish([me.id.as_str()], pb::FriendEvent { payload: Some(Payload::Settings(settings)) });
        if before.hide_online != settings.hide_online && friends.is_online(&me.id) {
            announce(self.app.clone(), me.id.clone());
        }
        Ok(pb::UpdateFriendSettingsResponse { settings: Some(settings) })
    }
}

/// Tells everyone who had a deleted account in their list that it's gone.
pub(super) async fn forget_account(app: &App, account_id: &str) -> Result<()> {
    let friends = app.friends()?;
    let told = friends.forget_account(account_id).await?;
    friends.publish(told.iter().map(String::as_str), removed(account_id));
    Ok(())
}

#[tonic::async_trait]
impl FriendService for Api {
    async fn list_friends(
        &self,
        request: Request<pb::ListFriendsRequest>,
    ) -> Result<Response<pb::ListFriendsResponse>, Status> {
        respond(Api::list_friends(self, request.metadata()).await)
    }

    async fn send_friend_request(
        &self,
        request: Request<pb::SendFriendRequestRequest>,
    ) -> Result<Response<pb::SendFriendRequestResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::send_friend_request(self, &metadata, req).await)
    }

    async fn accept_friend_request(
        &self,
        request: Request<pb::AcceptFriendRequestRequest>,
    ) -> Result<Response<pb::AcceptFriendRequestResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::accept_friend_request(self, &metadata, req).await)
    }

    async fn remove_friend(
        &self,
        request: Request<pb::RemoveFriendRequest>,
    ) -> Result<Response<pb::RemoveFriendResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::remove_friend(self, &metadata, req).await)
    }

    async fn block_user(
        &self,
        request: Request<pb::BlockUserRequest>,
    ) -> Result<Response<pb::BlockUserResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::block_user(self, &metadata, req).await)
    }

    async fn unblock_user(
        &self,
        request: Request<pb::UnblockUserRequest>,
    ) -> Result<Response<pb::UnblockUserResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::unblock_user(self, &metadata, req).await)
    }

    async fn get_relationship(
        &self,
        request: Request<pb::GetRelationshipRequest>,
    ) -> Result<Response<pb::GetRelationshipResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::get_relationship(self, &metadata, req).await)
    }

    async fn get_friend_settings(
        &self,
        request: Request<pb::GetFriendSettingsRequest>,
    ) -> Result<Response<pb::GetFriendSettingsResponse>, Status> {
        respond(
            async {
                let me = self.befriender(request.metadata()).await?;
                Ok(pb::GetFriendSettingsResponse { settings: Some(self.app.friends()?.settings(&me.id).await?) })
            }
            .await,
        )
    }

    async fn update_friend_settings(
        &self,
        request: Request<pb::UpdateFriendSettingsRequest>,
    ) -> Result<Response<pb::UpdateFriendSettingsResponse>, Status> {
        let (metadata, _, req) = request.into_parts();
        respond(Api::update_friend_settings(self, &metadata, req).await)
    }

    type WatchFriendsStream = WatchStream;

    async fn watch_friends(&self, request: Request<pb::WatchFriendsRequest>) -> Result<Response<WatchStream>, Status> {
        let caller = self.caller(request.metadata()).await?;
        if caller.account.kind == pb::AccountKind::Agent {
            return Err(Error::FailedPrecondition(AGENTS_HAVE_NO_FRIENDS.into()).into());
        }
        let mut events = self.app.friends()?.watch(&caller.account.id);
        let (tx, rx) = mpsc::channel::<Result<pb::WatchFriendsResponse, Status>>(64);
        let app = self.app.clone();
        tokio::spawn(async move {
            // Online for as long as this task runs.
            let _online = Online::new(&app, &caller.account.id);
            let send = async |item| tx.send(item).await.is_ok();
            if !send(Ok(pb::WatchFriendsResponse { ready: true, event: None })).await {
                return;
            }
            let mut heartbeat = tokio::time::interval(HEARTBEAT);
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    _ = app.shutdown.cancelled() => {
                        let _ = tx.try_send(Err(Status::unavailable(RESTARTING)));
                        return;
                    }
                    _ = tx.closed() => return,
                    _ = heartbeat.tick() => {
                        // A session signed out elsewhere stops getting events.
                        let live = async { app.node()?.session_live(&caller.token_hash).await }.await;
                        if matches!(live, Ok(false)) {
                            let _ = tx.send(Err(Error::Unauthenticated.into())).await;
                            return;
                        }
                        if !send(Ok(pb::WatchFriendsResponse { ready: false, event: None })).await {
                            return;
                        }
                    }
                    event = events.recv() => match event {
                        Ok(event) => {
                            let response = pb::WatchFriendsResponse { ready: false, event: Some((*event).clone()) };
                            if !send(Ok(response)).await {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let _ = tx.send(Err(Status::aborted("fell behind; list your friends and watch again"))).await;
                            return;
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_decide_requests_and_conversations() {
        let open = pb::FriendSettings::default();
        assert!(takes_requests(&open, false));
        assert!(takes_conversations(&open, false, true));
        assert!(takes_conversations(&open, true, false));
        assert!(!takes_conversations(&open, false, false));

        let shared = pb::FriendSettings { requests_from: pb::FriendRequestsFrom::SharedServers as i32, ..open };
        assert!(takes_requests(&shared, true));
        assert!(!takes_requests(&shared, false));

        let friends_only = pb::FriendSettings { direct_messages_from: pb::DirectMessagesFrom::Friends as i32, ..open };
        assert!(takes_conversations(&friends_only, true, false));
        assert!(!takes_conversations(&friends_only, false, true));

        let closed = pb::FriendSettings {
            requests_from: pb::FriendRequestsFrom::Nobody as i32,
            direct_messages_from: pb::DirectMessagesFrom::Nobody as i32,
            ..open
        };
        assert!(!takes_requests(&closed, true));
        assert!(!takes_conversations(&closed, true, true));
    }
}
