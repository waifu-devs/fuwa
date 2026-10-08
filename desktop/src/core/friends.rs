//! Friends on one instance (docs/friends.md): the list kept in step over
//! `WatchFriends` (which is also what shows you online to your friends), and
//! what people do with it. A port of `web/src/fuwa/friends.ts` and
//! `web/src/lib/friends.ts`.
//!
//! A friend list is its owner's alone: nothing about it goes in a log or a
//! report beyond that a feature was used.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tonic::Code;

use crate::core::api::{Api, Problem};
use crate::core::store::{self, InstanceState};
use crate::core::{Core, Notice, reports};
use crate::pb;
use crate::rpc;

pub const FRIEND: i32 = pb::FriendState::Friend as i32;
pub const OUTGOING: i32 = pb::FriendState::Outgoing as i32;
pub const INCOMING: i32 = pb::FriendState::Incoming as i32;
pub const BLOCKED: i32 = pb::FriendState::Blocked as i32;

/// Heartbeats come every 25 seconds; this long without one means the stream is gone.
const SILENCE: Duration = Duration::from_secs(70);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FriendsStatus {
    /// Not signed in, or not followed yet.
    #[default]
    Off,
    /// The first list is on its way.
    Loading,
    Ready,
    /// An instance from before friends.
    Unsupported,
}

#[derive(Debug, Clone, Default)]
pub struct FriendsState {
    pub status: FriendsStatus,
    /// Everyone in your list, by name.
    pub list: Vec<pb::Friend>,
    /// Your settings, once loaded.
    pub settings: Option<pb::FriendSettings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Online,
    All,
    Pending,
    Blocked,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Online, Tab::All, Tab::Pending, Tab::Blocked];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Online => "Online",
            Tab::All => "All",
            Tab::Pending => "Pending",
            Tab::Blocked => "Blocked",
        }
    }
}

fn name_of(f: &pb::Friend) -> String {
    f.user.as_ref().map(store::user_name).unwrap_or_default().to_lowercase()
}

fn id_of(f: &pb::Friend) -> &str {
    f.user.as_ref().map(|u| u.id.as_str()).unwrap_or("")
}

/// By name, then id, so the list doesn't shuffle.
pub fn sort(list: &mut [pb::Friend]) {
    list.sort_by(|a, b| name_of(a).cmp(&name_of(b)).then_with(|| id_of(a).cmp(id_of(b))));
}

fn millis(ts: Option<&prost_types::Timestamp>) -> Option<i64> {
    ts.map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

/// A request that ran out is as good as gone; the instance sweeps it soon.
pub fn expired(f: &pb::Friend, now_ms: i64) -> bool {
    millis(f.expires_at.as_ref()).is_some_and(|at| at <= now_ms)
}

/// Your list after an event from the instance. Events can arrive twice, so
/// each one is idempotent. Says whether anything changed.
pub fn apply(list: &mut Vec<pb::Friend>, event: &pb::FriendEvent) -> bool {
    use pb::friend_event::Payload;
    match &event.payload {
        Some(Payload::Changed(friend)) => {
            let Some(id) = friend.user.as_ref().map(|u| u.id.clone()) else { return false };
            list.retain(|f| id_of(f) != id);
            list.push(friend.clone());
            sort(list);
            true
        }
        Some(Payload::Removed(id)) => {
            let before = list.len();
            list.retain(|f| id_of(f) != id);
            list.len() != before
        }
        Some(Payload::Presence(p)) => {
            let mut changed = false;
            for f in list.iter_mut().filter(|f| id_of(f) == p.user_id && f.state == FRIEND && f.online != p.online) {
                f.online = p.online;
                changed = true;
            }
            changed
        }
        _ => false,
    }
}

/// A search as typed: a leading @ and spaces don't count.
fn query(typed: &str) -> String {
    typed.trim().trim_start_matches('@').to_lowercase()
}

/// Who goes under each tab, filtered by a search.
pub fn in_tab<'a>(list: &'a [pb::Friend], tab: Tab, typed: &str, now_ms: i64) -> Vec<&'a pb::Friend> {
    let q = query(typed);
    list.iter()
        .filter(|f| {
            if expired(f, now_ms) {
                return false;
            }
            let fits = match tab {
                Tab::Online => f.state == FRIEND && f.online,
                Tab::All => f.state == FRIEND,
                Tab::Pending => f.state == INCOMING || f.state == OUTGOING,
                Tab::Blocked => f.state == BLOCKED,
            };
            fits && (q.is_empty()
                || name_of(f).contains(&q)
                || f.user.as_ref().is_some_and(|u| u.username.to_lowercase().contains(&q)))
        })
        .collect()
}

/// Requests waiting for your answer: the badge.
pub fn waiting_for_you(list: &[pb::Friend], now_ms: i64) -> usize {
    list.iter().filter(|f| f.state == INCOMING && !expired(f, now_ms)).count()
}

/// Where you stand with someone, from your list. 0 when there's nothing.
pub fn state_with(list: &[pb::Friend], user_id: &str, now_ms: i64) -> i32 {
    list.iter().find(|f| id_of(f) == user_id).filter(|f| !expired(f, now_ms)).map(|f| f.state).unwrap_or(0)
}

/// Ids of people you blocked, whose conversations stay out of sight.
pub fn blocked_ids(list: &[pb::Friend]) -> HashSet<String> {
    list.iter().filter(|f| f.state == BLOCKED).map(|f| id_of(f).to_owned()).collect()
}

/// Whether a conversation is with someone you blocked, so it stays out of sight.
pub fn hidden(i: &InstanceState, conversation: &pb::Conversation) -> bool {
    let me = i.me.as_ref().map(|m| m.id.as_str()).unwrap_or("");
    conversation
        .users
        .iter()
        .any(|u| u.id != me && i.friends.list.iter().any(|f| f.state == BLOCKED && id_of(f) == u.id))
}

/// A username as typed into "Add friend": a leading @ and spaces don't count.
pub fn clean_username(typed: &str) -> String {
    typed.trim().trim_start_matches('@').to_lowercase()
}

/// What a pending request's line says.
pub fn pending_line(f: &pb::Friend, now_ms: i64) -> String {
    let left = millis(f.expires_at.as_ref())
        .map(|at| {
            let days = ((at - now_ms) as f64 / 86_400_000.0).ceil().max(1.0) as i64;
            if days == 1 { " · 1 day left".to_owned() } else { format!(" · {days} days left") }
        })
        .unwrap_or_default();
    if f.state == INCOMING { format!("Wants to be friends{left}") } else { format!("Request sent{left}") }
}

/// Puts someone in your list (or changes their place in it), remembering who they are.
fn put(i: &mut InstanceState, friend: &pb::Friend) {
    if let Some(user) = &friend.user {
        store::update_user(i, user);
    }
    apply(&mut i.friends.list, &pb::FriendEvent { payload: Some(pb::friend_event::Payload::Changed(friend.clone())) });
}

fn drop_from(i: &mut InstanceState, user_id: &str) {
    apply(&mut i.friends.list, &pb::FriendEvent { payload: Some(pb::friend_event::Payload::Removed(user_id.into())) });
}

/// Lists your friends, then follows changes for as long as the instance is
/// synced, listing again after each reconnect so nothing is missed. An
/// instance from before friends simply has none.
pub(super) async fn follow(core: Arc<Core>, key: String, api: Api) {
    core.shared.instance(&key, |i| {
        if i.friends.status != FriendsStatus::Ready {
            i.friends.status = FriendsStatus::Loading;
        }
    });
    let mut wait = Duration::from_millis(500);
    loop {
        match watch_once(&core, &key, &api, &mut wait).await {
            Err(err) if err.code == Code::Unimplemented => {
                core.shared.instance(&key, |i| i.friends.status = FriendsStatus::Unsupported);
                return;
            }
            Err(err) if err.signed_out() => return,
            _ => {}
        }
        let mut b = [0u8; 1];
        let _ = getrandom::fill(&mut b);
        tokio::time::sleep(wait.mul_f64(0.75 + f64::from(b[0]) / 510.0)).await;
        wait = (wait * 2).min(Duration::from_secs(20));
    }
}

async fn watch_once(core: &Arc<Core>, key: &str, api: &Api, wait: &mut Duration) -> Result<(), Problem> {
    let mut stream = api.friends().watch_friends(pb::WatchFriendsRequest {}).await.map_err(Problem::from)?.into_inner();
    loop {
        let next = tokio::time::timeout(SILENCE, stream.message())
            .await
            .map_err(|_| Problem::new(Code::Unavailable, "Lost the connection."))?
            .map_err(Problem::from)?;
        let Some(res) = next else { return Ok(()) };
        if res.ready {
            *wait = Duration::from_millis(500);
            // Listening: read the whole list, so whatever happened while away is in.
            let list = rpc!(api.friends(), list_friends(pb::ListFriendsRequest {})).await?;
            let settings = rpc!(api.friends(), get_friend_settings(pb::GetFriendSettingsRequest {})).await?;
            core.shared.instance(key, |i| {
                let mut friends = list.friends;
                for user in friends.iter().filter_map(|f| f.user.as_ref()) {
                    store::update_user(i, user);
                }
                sort(&mut friends);
                i.friends = FriendsState { status: FriendsStatus::Ready, list: friends, settings: settings.settings };
            });
            continue;
        }
        let Some(event) = res.event else { continue };
        if let Some(pb::friend_event::Payload::Settings(settings)) = &event.payload {
            core.shared.instance(key, |i| i.friends.settings = Some(*settings));
            continue;
        }
        let news = core.shared.instance(key, |i| {
            let now = crate::core::dms::now_ms();
            let news = match &event.payload {
                Some(pb::friend_event::Payload::Changed(friend)) => {
                    let id = friend.user.as_ref().map(|u| u.id.as_str()).unwrap_or("");
                    let was = state_with(&i.friends.list, id, now);
                    let name = friend.user.as_ref().map(store::user_name).unwrap_or_else(|| "Someone".into());
                    if friend.state == INCOMING && was != INCOMING {
                        Some(format!("{name} wants to be friends"))
                    } else if friend.state == FRIEND && was == OUTGOING {
                        Some(format!("{name} accepted your friend request"))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            match &event.payload {
                Some(pb::friend_event::Payload::Changed(friend)) => put(i, friend),
                _ => {
                    apply(&mut i.friends.list, &event);
                }
            }
            news
        });
        if let Some(Some(title)) = news {
            core.shared.notice(Notice::Friend { instance: key.to_owned(), title });
        }
    }
}

impl Core {
    /// Asks someone to be friends, by id or by username. Gives where you
    /// stand now: a request sent, or friends when they had asked you first.
    pub async fn send_friend_request(&self, key: &str, user_id: &str, username: &str) -> Result<pb::Friend, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.friends(),
            send_friend_request(pb::SendFriendRequestRequest {
                user_id: user_id.into(),
                username: clean_username(username)
            })
        )
        .await?;
        reports::used("friends.request");
        let friend = res.friend.unwrap_or_default();
        self.shared.instance(key, |i| put(i, &friend));
        Ok(friend)
    }

    pub async fn accept_friend(&self, key: &str, user_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.friends(), accept_friend_request(pb::AcceptFriendRequestRequest { user_id: user_id.into() }))
                .await?;
        reports::used("friends.accept");
        if let Some(friend) = res.friend {
            self.shared.instance(key, |i| put(i, &friend));
        }
        Ok(())
    }

    /// Unfriends someone, declines their request or cancels yours.
    pub async fn remove_friend(&self, key: &str, user_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.friends(), remove_friend(pb::RemoveFriendRequest { user_id: user_id.into() })).await?;
        self.shared.instance(key, |i| drop_from(i, user_id));
        Ok(())
    }

    /// Blocks someone. They aren't told.
    pub async fn block_user(&self, key: &str, user_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.friends(), block_user(pb::BlockUserRequest { user_id: user_id.into() })).await?;
        reports::used("friends.block");
        if let Some(friend) = res.friend {
            self.shared.instance(key, |i| put(i, &friend));
        }
        Ok(())
    }

    pub async fn unblock_user(&self, key: &str, user_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.friends(), unblock_user(pb::UnblockUserRequest { user_id: user_id.into() })).await?;
        self.shared.instance(key, |i| drop_from(i, user_id));
        Ok(())
    }

    /// Where you stand with someone, with mutual friends, for their profile card.
    pub async fn relationship(&self, key: &str, user_id: &str) -> Result<pb::GetRelationshipResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.friends(), get_relationship(pb::GetRelationshipRequest { user_id: user_id.into() })).await?;
        self.shared.instance(key, |i| {
            for user in &res.mutual_friends {
                store::update_user(i, user);
            }
        });
        Ok(res)
    }

    /// Saves your privacy settings: shown at once, and put back if this fails.
    pub async fn save_friend_settings(&self, key: &str, settings: pb::FriendSettings) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let before = self.shared.instance(key, |i| i.friends.settings.replace(settings)).flatten();
        match rpc!(api.friends(), update_friend_settings(pb::UpdateFriendSettingsRequest { settings: Some(settings) }))
            .await
        {
            Ok(res) => {
                self.shared.instance(key, |i| i.friends.settings = res.settings);
                Ok(())
            }
            Err(err) => {
                self.shared.instance(key, |i| i.friends.settings = before);
                Err(err)
            }
        }
    }
}

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(id: &str, name: &str, state: i32) -> pb::Friend {
        pb::Friend {
            user: Some(pb::User { id: id.into(), username: name.into(), ..Default::default() }),
            state,
            ..Default::default()
        }
    }

    fn changed(f: pb::Friend) -> pb::FriendEvent {
        pb::FriendEvent { payload: Some(pb::friend_event::Payload::Changed(f)) }
    }

    #[test]
    fn events_apply_once_however_often_they_come() {
        let mut list = vec![friend("b", "bea", FRIEND)];
        assert!(apply(&mut list, &changed(friend("a", "ana", INCOMING))));
        assert!(apply(&mut list, &changed(friend("a", "ana", INCOMING))));
        assert_eq!(list.len(), 2);
        assert_eq!(id_of(&list[0]), "a", "kept by name");
        let online = pb::FriendEvent {
            payload: Some(pb::friend_event::Payload::Presence(pb::FriendPresence {
                user_id: "b".into(),
                online: true,
            })),
        };
        assert!(apply(&mut list, &online));
        assert!(!apply(&mut list, &online), "already online");
        // Presence for someone who only asked changes nothing.
        let asked = pb::FriendEvent {
            payload: Some(pb::friend_event::Payload::Presence(pb::FriendPresence {
                user_id: "a".into(),
                online: true,
            })),
        };
        assert!(!apply(&mut list, &asked));
        let gone = pb::FriendEvent { payload: Some(pb::friend_event::Payload::Removed("a".into())) };
        assert!(apply(&mut list, &gone));
        assert!(!apply(&mut list, &gone));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn tabs_counts_and_expiry() {
        let now = 1_000_000_000;
        let mut late = friend("c", "cy", OUTGOING);
        late.expires_at = Some(prost_types::Timestamp { seconds: now / 1000 - 1, nanos: 0 });
        let mut on = friend("a", "ana", FRIEND);
        on.online = true;
        let list =
            vec![on, friend("b", "bea", INCOMING), late, friend("d", "dot", BLOCKED), friend("e", "eli", FRIEND)];
        let ids = |tab, q| in_tab(&list, tab, q, now).iter().map(|f| id_of(f).to_owned()).collect::<Vec<_>>();
        assert_eq!(ids(Tab::Online, ""), ["a"]);
        assert_eq!(ids(Tab::All, ""), ["a", "e"]);
        assert_eq!(ids(Tab::All, " @EL"), ["e"]);
        assert_eq!(ids(Tab::Pending, ""), ["b"], "a request that ran out is gone");
        assert_eq!(ids(Tab::Blocked, ""), ["d"]);
        assert_eq!(waiting_for_you(&list, now), 1);
        assert_eq!(state_with(&list, "c", now), 0);
        assert_eq!(state_with(&list, "d", now), BLOCKED);
        assert!(blocked_ids(&list).contains("d"));
    }

    #[test]
    fn lines_and_usernames() {
        let now = 0;
        let mut f = friend("a", "ana", INCOMING);
        f.expires_at = Some(prost_types::Timestamp { seconds: 86_400 * 3 - 10, nanos: 0 });
        assert_eq!(pending_line(&f, now), "Wants to be friends · 3 days left");
        f.state = OUTGOING;
        f.expires_at = Some(prost_types::Timestamp { seconds: 60, nanos: 0 });
        assert_eq!(pending_line(&f, now), "Request sent · 1 day left");
        assert_eq!(clean_username("  @@Mika "), "mika");
    }
}
