//! Everything the app shows, for every instance at once, and how events from
//! an instance change it. A port of the web app's `web/src/fuwa/store.ts`:
//! the same shapes and the same reducers, so both apps agree on what an
//! event means. Applying an event twice changes nothing.

use std::collections::{HashMap, HashSet};

use crate::core::calls;
use crate::core::dms::DmState;
use crate::pb::{self, event::Payload};

/// How an instance's connection is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Live,
    Reconnecting,
    Offline,
    SignedOut,
}

/// A channel's messages, for channels someone opened.
#[derive(Debug, Clone, Default)]
pub struct ChannelMessages {
    /// Oldest first.
    pub items: Vec<pb::Message>,
    /// Older messages exist on the server.
    pub has_more: bool,
    pub loading: bool,
}

/// A message this app sent that the instance hasn't confirmed yet.
#[derive(Debug, Clone)]
pub struct PendingMessage {
    pub nonce: u64,
    pub content: String,
    pub created_at_ms: i64,
    pub failed: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstanceState {
    pub key: String,
    pub url: String,
    pub connection: Connection,
    pub problem: Option<String>,
    pub node: Option<pb::Node>,
    pub me: Option<pb::User>,
    pub admin: bool,
    /// Joined servers, in the order they were joined.
    pub servers: Vec<pb::Server>,
    /// Per server, sorted for the sidebar.
    pub channels: HashMap<String, Vec<pb::Channel>>,
    /// Per server, by name.
    pub members: HashMap<String, Vec<pb::Member>>,
    /// Per server, highest first.
    pub roles: HashMap<String, Vec<pb::Role>>,
    /// Per server, its own emoji.
    pub emojis: HashMap<String, Vec<pb::Emoji>>,
    /// Everyone this instance has shown us, by id, so authors resolve even after they leave.
    pub users: HashMap<String, pb::User>,
    /// Per channel, only for channels someone opened.
    pub messages: HashMap<String, ChannelMessages>,
    pub pending: HashMap<String, Vec<PendingMessage>>,
    /// Per channel: messages from others that arrived while it wasn't open.
    pub unread: HashMap<String, u32>,
    /// Servers whose channels and members are loaded.
    pub synced: HashSet<String>,
    pub dms: DmState,
    /// Your notification settings, by `notifications::key`. Only servers and channels that have some.
    pub notifications: HashMap<String, pb::NotificationSettings>,
    /// Per server: who's in its voice channels, in the order they joined.
    pub voice: HashMap<String, Vec<pb::VoiceState>>,
}

impl InstanceState {
    pub fn new(key: &str, url: &str) -> Self {
        Self {
            key: key.to_owned(),
            url: url.to_owned(),
            connection: Connection::Connecting,
            problem: None,
            node: None,
            me: None,
            admin: false,
            servers: Vec::new(),
            channels: HashMap::new(),
            members: HashMap::new(),
            roles: HashMap::new(),
            emojis: HashMap::new(),
            users: HashMap::new(),
            messages: HashMap::new(),
            pending: HashMap::new(),
            unread: HashMap::new(),
            synced: HashSet::new(),
            dms: DmState::default(),
            notifications: HashMap::new(),
            voice: HashMap::new(),
        }
    }

    /// The instance's name, or its address until it said.
    pub fn name(&self) -> String {
        self.node.as_ref().map(|n| n.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| self.key.clone())
    }

    pub fn server(&self, id: &str) -> Option<&pb::Server> {
        self.servers.iter().find(|s| s.id == id)
    }

    pub fn channel(&self, server_id: &str, channel_id: &str) -> Option<&pb::Channel> {
        self.channels.get(server_id)?.iter().find(|c| c.id == channel_id)
    }

    /// The member's name in a server: their nickname, else their display name, else their username.
    pub fn display_name(&self, server_id: Option<&str>, user_id: &str) -> String {
        if let Some(sid) = server_id
            && let Some(member) =
                self.members.get(sid).and_then(|m| m.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)))
            && !member.nickname.is_empty()
        {
            return member.nickname.clone();
        }
        match self.users.get(user_id) {
            Some(user) => user_name(user),
            None => "Someone".to_owned(),
        }
    }

    /// Unread messages across a server's channels; muted ones don't count.
    pub fn server_unread(&self, server_id: &str) -> u32 {
        let now = crate::core::dms::now_ms();
        self.channels
            .get(server_id)
            .map(|list| {
                list.iter()
                    .filter(|c| !self.is_muted(server_id, &c.id, now))
                    .map(|c| self.unread.get(&c.id).copied().unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0)
    }

    /// The color of a member's highest colored role, as 0xRRGGBB.
    pub fn name_color(&self, server_id: &str, user_id: &str) -> Option<u32> {
        let member = self.members.get(server_id)?.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id))?;
        let roles = self.roles.get(server_id)?;
        roles.iter().filter(|r| member.role_ids.contains(&r.id)).find_map(|r| r.color).map(|c| c as u32)
    }
}

/// A person's name as shown: their display name, else their username.
pub fn user_name(user: &pb::User) -> String {
    if user.display_name.is_empty() { user.username.clone() } else { user.display_name.clone() }
}

/// What the app is looking at, so it doesn't collect unread counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Focus {
    pub instance: String,
    /// A channel id, or a conversation id for direct messages.
    pub channel: String,
}

#[derive(Debug, Clone, Default)]
pub struct Store {
    pub instances: HashMap<String, InstanceState>,
    /// Instance keys, in the order they were added.
    pub order: Vec<String>,
    pub focus: Option<Focus>,
}

impl Store {
    pub fn instance(&self, key: &str) -> Option<&InstanceState> {
        self.instances.get(key)
    }

    pub fn focus_channel(&self, key: &str) -> Option<&str> {
        self.focus.as_ref().filter(|f| f.instance == key).map(|f| f.channel.as_str())
    }
}

// ───────────────────────── Reducers ─────────────────────────

/// Sidebar order: by position, then creation (ids sort by time).
pub fn sort_channels(channels: &mut [pb::Channel]) {
    channels.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
}

fn member_name(m: &pb::Member) -> String {
    if !m.nickname.is_empty() {
        return m.nickname.to_lowercase();
    }
    m.user.as_ref().map(user_name).unwrap_or_default().to_lowercase()
}

pub fn sort_members(members: &mut [pb::Member]) {
    members.sort_by_key(member_name);
}

/// Highest first, @everyone (whose id is the server's) last.
pub fn sort_roles(roles: &mut [pb::Role]) {
    roles.sort_by(|a, b| {
        let everyone = |r: &pb::Role| r.id == r.server_id;
        everyone(a).cmp(&everyone(b)).then_with(|| b.position.cmp(&a.position)).then_with(|| a.id.cmp(&b.id))
    });
}

/// Inserts or replaces a message, keeping the list sorted by id (which is by time).
pub fn upsert_message(items: &mut Vec<pb::Message>, message: pb::Message) {
    match items.binary_search_by(|m| m.id.as_str().cmp(&message.id)) {
        Ok(at) => items[at] = message,
        Err(at) => items.insert(at, message),
    }
}

fn add_user(users: &mut HashMap<String, pb::User>, user: Option<&pb::User>) {
    if let Some(user) = user {
        users.insert(user.id.clone(), user.clone());
    }
}

/// Puts a user's new look everywhere it shows.
pub fn update_user(i: &mut InstanceState, user: &pb::User) {
    for list in i.members.values_mut() {
        for m in list.iter_mut().filter(|m| m.user.as_ref().is_some_and(|u| u.id == user.id)) {
            m.user = Some(user.clone());
        }
    }
    if i.me.as_ref().is_some_and(|me| me.id == user.id) {
        i.me = Some(user.clone());
    }
    add_user(&mut i.users, Some(user));
}

pub fn add_server(i: &mut InstanceState, server: pb::Server) {
    match i.servers.iter_mut().find(|s| s.id == server.id) {
        Some(existing) => *existing = server,
        None => i.servers.push(server),
    }
}

pub fn remove_server(i: &mut InstanceState, server_id: &str) {
    let channels: Vec<String> =
        i.channels.get(server_id).map(|l| l.iter().map(|c| c.id.clone()).collect()).unwrap_or_default();
    i.servers.retain(|s| s.id != server_id);
    i.channels.remove(server_id);
    i.members.remove(server_id);
    i.roles.remove(server_id);
    i.voice.remove(server_id);
    i.emojis.remove(server_id);
    i.synced.remove(server_id);
    for id in channels {
        i.messages.remove(&id);
        i.pending.remove(&id);
        i.unread.remove(&id);
    }
}

/// A server's channels as listed again, letting go of what was in the ones that are gone.
pub fn set_channels(i: &mut InstanceState, server_id: &str, mut channels: Vec<pb::Channel>) {
    let kept: HashSet<&str> = channels.iter().map(|c| c.id.as_str()).collect();
    if let Some(before) = i.channels.get(server_id) {
        for c in before.iter().filter(|c| !kept.contains(c.id.as_str())) {
            i.messages.remove(&c.id);
            i.unread.remove(&c.id);
        }
    }
    sort_channels(&mut channels);
    i.channels.insert(server_id.to_owned(), channels);
}

/// A server's state as loaded in one go, after the event stream said where it stands.
pub fn apply_snapshot(
    i: &mut InstanceState,
    server: pb::Server,
    channels: Vec<pb::Channel>,
    mut members: Vec<pb::Member>,
    mut roles: Vec<pb::Role>,
    emojis: Vec<pb::Emoji>,
) {
    let id = server.id.clone();
    add_server(i, server);
    set_channels(i, &id, channels);
    for m in &members {
        add_user(&mut i.users, m.user.as_ref());
    }
    sort_members(&mut members);
    sort_roles(&mut roles);
    i.members.insert(id.clone(), members);
    i.roles.insert(id.clone(), roles);
    i.emojis.insert(id.clone(), emojis);
    i.synced.insert(id);
}

/// What an event did that the app may want to say out loud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    /// A new message from someone else, in a channel that isn't on screen.
    Unread {
        channel_id: String,
    },
    /// You left the server, or were removed from it, or it was deleted.
    Gone,
}

/// Applies one event from a server's log.
pub fn apply_event(i: &mut InstanceState, event: &pb::Event, focus: Option<&str>) -> Outcome {
    let sid = event.server_id.as_str();
    let Some(payload) = &event.payload else { return Outcome::Nothing };
    match payload {
        Payload::ServerUpdated(p) => {
            if let Some(server) = &p.server
                && i.servers.iter().any(|s| s.id == server.id)
            {
                add_server(i, server.clone());
            }
        }
        Payload::ServerDeleted(_) => {
            remove_server(i, sid);
            return Outcome::Gone;
        }
        Payload::ChannelCreated(pb::ChannelCreated { channel: Some(channel) })
        | Payload::ChannelUpdated(pb::ChannelUpdated { channel: Some(channel) }) => {
            let list = i.channels.entry(sid.to_owned()).or_default();
            list.retain(|c| c.id != channel.id);
            list.push(channel.clone());
            sort_channels(list);
        }
        Payload::ChannelDeleted(p) => {
            if let Some(list) = i.channels.get_mut(sid) {
                list.retain(|c| c.id != p.channel_id);
            }
            i.messages.remove(&p.channel_id);
            i.unread.remove(&p.channel_id);
            if let Some(list) = i.voice.get_mut(sid) {
                list.retain(|v| v.channel_id != p.channel_id);
            }
        }
        Payload::MessageCreated(pb::MessageCreated { message: Some(message) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(message) }) => {
            // Thread replies live in their thread, not the channel, unless also sent to it.
            if !message.thread_id.is_empty() && !message.also_in_channel {
                return Outcome::Nothing;
            }
            let created = matches!(payload, Payload::MessageCreated(_));
            let mut known = false;
            if let Some(loaded) = i.messages.get_mut(&message.channel_id) {
                known = loaded.items.iter().any(|m| m.id == message.id);
                upsert_message(&mut loaded.items, message.clone());
            }
            let mine = i.me.as_ref().is_some_and(|me| me.id == message.author_id);
            if created && !known && !mine && focus != Some(message.channel_id.as_str()) {
                *i.unread.entry(message.channel_id.clone()).or_default() += 1;
                return Outcome::Unread { channel_id: message.channel_id.clone() };
            }
        }
        Payload::MessageDeleted(p) => {
            if let Some(loaded) = i.messages.get_mut(&p.channel_id) {
                loaded.items.retain(|m| m.id != p.message_id);
            }
        }
        Payload::UserUpdated(pb::UserUpdated { user: Some(user) }) => update_user(i, user),
        Payload::MemberJoined(pb::MemberJoined { member: Some(member) })
        | Payload::MemberUpdated(pb::MemberUpdated { member: Some(member) }) => {
            let Some(user) = member.user.clone() else { return Outcome::Nothing };
            let list = i.members.entry(sid.to_owned()).or_default();
            let new = !list.iter().any(|m| m.user.as_ref().is_some_and(|u| u.id == user.id));
            list.retain(|m| !m.user.as_ref().is_some_and(|u| u.id == user.id));
            list.push(member.clone());
            sort_members(list);
            update_user(i, &user);
            if new
                && matches!(payload, Payload::MemberJoined(_))
                && let Some(server) = i.servers.iter_mut().find(|s| s.id == sid)
            {
                server.member_count += 1;
            }
        }
        Payload::MemberLeft(p) => {
            if i.me.as_ref().is_some_and(|me| me.id == p.user_id) {
                remove_server(i, sid);
                return Outcome::Gone;
            }
            if let Some(list) = i.voice.get_mut(sid) {
                list.retain(|v| v.user_id != p.user_id);
            }
            if let Some(list) = i.members.get_mut(sid) {
                let before = list.len();
                list.retain(|m| !m.user.as_ref().is_some_and(|u| u.id == p.user_id));
                if list.len() != before
                    && let Some(server) = i.servers.iter_mut().find(|s| s.id == sid)
                {
                    server.member_count -= 1;
                }
            }
        }
        Payload::RoleCreated(pb::RoleCreated { role: Some(role) })
        | Payload::RoleUpdated(pb::RoleUpdated { role: Some(role) }) => {
            let list = i.roles.entry(sid.to_owned()).or_default();
            list.retain(|r| r.id != role.id);
            list.push(role.clone());
            sort_roles(list);
        }
        Payload::EmojisUpdated(p) => {
            i.emojis.insert(sid.to_owned(), p.emojis.clone());
        }
        Payload::RoleDeleted(p) => {
            // The server takes it from everyone and every channel without saying so for each.
            if let Some(list) = i.roles.get_mut(sid) {
                list.retain(|r| r.id != p.role_id);
            }
            if let Some(list) = i.members.get_mut(sid) {
                for m in list {
                    m.role_ids.retain(|r| *r != p.role_id);
                }
            }
            if let Some(list) = i.channels.get_mut(sid) {
                for c in list {
                    c.permission_overwrites.retain(|o| o.target_id != p.role_id);
                }
            }
        }
        Payload::VoiceStateUpdated(pb::VoiceStateUpdated { state: Some(state) }) => {
            calls::put(i.voice.entry(sid.to_owned()).or_default(), state.clone());
        }
        Payload::VoiceStateRemoved(p) => {
            if let Some(list) = i.voice.get_mut(sid) {
                list.retain(|v| v.user_id != p.user_id);
            }
        }
        _ => {}
    }
    Outcome::Nothing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(server: &str, payload: Payload) -> pb::Event {
        pb::Event { server_id: server.into(), sequence: 1, payload: Some(payload), ..Default::default() }
    }

    fn message(id: &str, channel: &str, author: &str) -> pb::Message {
        pb::Message { id: id.into(), channel_id: channel.into(), author_id: author.into(), ..Default::default() }
    }

    #[test]
    fn messages_stay_sorted_and_count_once() {
        let mut i = InstanceState::new("k", "https://k");
        i.me = Some(pb::User { id: "me".into(), ..Default::default() });
        i.messages.insert("c".into(), ChannelMessages::default());
        let created = |m| event("s", Payload::MessageCreated(pb::MessageCreated { message: Some(m) }));
        assert!(matches!(apply_event(&mut i, &created(message("02", "c", "them")), None), Outcome::Unread { .. }));
        // The same event again changes nothing.
        assert_eq!(apply_event(&mut i, &created(message("02", "c", "them")), None), Outcome::Nothing);
        apply_event(&mut i, &created(message("01", "c", "me")), None);
        apply_event(&mut i, &created(message("03", "c", "them")), Some("c"));
        let ids: Vec<_> = i.messages["c"].items.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["01", "02", "03"]);
        assert_eq!(i.unread["c"], 1);
    }

    #[test]
    fn leaving_forgets_the_server() {
        let mut i = InstanceState::new("k", "https://k");
        i.me = Some(pb::User { id: "me".into(), ..Default::default() });
        let server = pb::Server { id: "s".into(), member_count: 2, ..Default::default() };
        let channel = pb::Channel { id: "c".into(), server_id: "s".into(), ..Default::default() };
        apply_snapshot(&mut i, server, vec![channel], vec![], vec![], vec![]);
        i.messages.insert("c".into(), ChannelMessages::default());
        let left = event("s", Payload::MemberLeft(pb::MemberLeft { user_id: "me".into(), reason: 1 }));
        assert_eq!(apply_event(&mut i, &left, None), Outcome::Gone);
        assert!(i.servers.is_empty() && i.messages.is_empty() && !i.synced.contains("s"));
    }

    #[test]
    fn roles_sort_highest_first_with_everyone_last() {
        let role =
            |id: &str, position| pb::Role { id: id.into(), server_id: "s".into(), position, ..Default::default() };
        let mut roles = vec![role("s", 0), role("a", 1), role("b", 5)];
        sort_roles(&mut roles);
        let ids: Vec<_> = roles.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "s"]);
    }
}
