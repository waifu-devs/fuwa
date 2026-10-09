//! One stream per app (`LiveService`, docs/live.md): the connections open
//! here, what each has in focus, and which server events it gets whole.
//!
//! Messages, reactions, polls, pins and thread changes come whole only for
//! the channels in focus and for messages that mention the caller; the rest
//! only move the channel's head, sent at most every [`HEADS_EVERY`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::pb;
use crate::servers::Payload;

/// How often moved channel heads go out, at most.
pub const HEADS_EVERY: Duration = Duration::from_secs(2);
/// Most channels in focus at once.
pub const MAX_FOCUS_CHANNELS: usize = 8;
/// Most people in focus at once.
pub const MAX_FOCUS_PEOPLE: usize = 500;

/// What a connection has in focus, as each part of it reads it.
pub type FocusRx = watch::Receiver<Arc<pb::Focus>>;

/// The connections open in this process, by id.
#[derive(Default)]
pub struct Connections {
    open: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    account_id: String,
    /// The session that opened it: only it may change its focus.
    session: String,
    focus: watch::Sender<Arc<pb::Focus>>,
}

/// An open connection's place, given back when dropped.
pub struct Opened {
    pub id: String,
    pub focus: FocusRx,
    connections: Arc<Connections>,
}

impl Drop for Opened {
    fn drop(&mut self) {
        self.connections.lock().remove(&self.id);
    }
}

impl Connections {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.open.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A new connection for `session` of `account_id`, with `focus`.
    pub fn open(self: &Arc<Self>, account_id: &str, session: &str, focus: pb::Focus) -> Result<Opened> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| Error::internal("no randomness"))?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let (focus, rx) = watch::channel(Arc::new(focus));
        let entry = Entry { account_id: account_id.to_string(), session: session.to_string(), focus };
        self.lock().insert(id.clone(), entry);
        Ok(Opened { id, focus: rx, connections: self.clone() })
    }

    /// Puts `focus` in effect on connection `id`, if `session` of
    /// `account_id` opened it. Anything else is the same NotFound.
    pub fn focus(&self, id: &str, account_id: &str, session: &str, focus: pb::Focus) -> Result<()> {
        let open = self.lock();
        match open.get(id) {
            Some(entry) if entry.account_id == account_id && entry.session == session => {
                entry.focus.send_replace(Arc::new(focus));
                Ok(())
            }
            _ => Err(Error::NotFound("connection")),
        }
    }
}

/// Checks a focus from a client: ids only, each once, within the bounds.
pub fn check_focus(focus: pb::Focus) -> Result<pb::Focus> {
    let ids = |field: &str, list: Vec<String>, max: usize| -> Result<Vec<String>> {
        if list.len() > max {
            return Err(Error::invalid(format!("at most {max} {field}")));
        }
        let mut out: Vec<String> = Vec::with_capacity(list.len());
        for id in list {
            if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err(Error::invalid(format!("{field} must be ids")));
            }
            if !out.contains(&id) {
                out.push(id);
            }
        }
        Ok(out)
    };
    Ok(pb::Focus {
        channel_ids: ids("channel_ids", focus.channel_ids, MAX_FOCUS_CHANNELS)?,
        user_ids: ids("user_ids", focus.user_ids, MAX_FOCUS_PEOPLE)?,
    })
}

/// Which messages a connection gets whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    All,
    Focused,
    Mentions,
}

impl Intent {
    /// What the caller asked for, or their kind's default.
    pub fn of(asked: i32, agent: bool) -> Self {
        match pb::MessageIntent::try_from(asked).unwrap_or_default() {
            pb::MessageIntent::All => Self::All,
            pb::MessageIntent::Focused => Self::Focused,
            pb::MessageIntent::Mentions => Self::Mentions,
            pb::MessageIntent::Unspecified if agent => Self::All,
            pb::MessageIntent::Unspecified => Self::Focused,
        }
    }
}

/// The channel a message, reaction, poll, pin or thread change is in: the
/// events focus decides on. Everything else comes to every connection.
fn messageish(payload: &Payload) -> Option<&str> {
    match payload {
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }) => Some(&m.channel_id),
        Payload::MessageDeleted(d) => Some(&d.channel_id),
        Payload::PollUpdated(p) => Some(&p.channel_id),
        Payload::ThreadUpdated(t) => Some(&t.channel_id),
        Payload::MessagePinned(p) => Some(&p.channel_id),
        Payload::ReactionUpdated(r) => Some(&r.channel_id),
        Payload::ReactionsCleared(c) => Some(&c.channel_id),
        _ => None,
    }
}

/// Whether a message mentions `account_id`: by name, everyone, or a role in
/// `roles`. Roles unknown (`None`, where the member's roles aren't at hand)
/// counts any role mentioned, so a mention is never missed.
fn mentions(message: &pb::Message, account_id: &str, roles: Option<&[String]>) -> bool {
    message.mentions_everyone
        || message.mention_user_ids.iter().any(|id| id == account_id)
        || match roles {
            Some(roles) => message.mention_role_ids.iter().any(|id| roles.contains(id)),
            None => !message.mention_role_ids.is_empty(),
        }
}

/// One connection's choice of server events, and the heads it owes.
pub struct Interest {
    intent: Intent,
    account_id: String,
    focus: FocusRx,
    /// Server id to where it stands among events held back.
    heads: BTreeMap<String, Heads>,
}

#[derive(Default)]
struct Heads {
    sequence: i64,
    channels: BTreeMap<String, String>,
}

impl Interest {
    pub fn new(intent: Intent, account_id: &str, focus: FocusRx) -> Self {
        Self { intent, account_id: account_id.to_string(), focus, heads: BTreeMap::new() }
    }

    /// Whether `event` goes out whole. If not, it's held back: its channel's
    /// head moves (`visible`: the member can see that channel; otherwise only
    /// the sequence does). `roles` are the member's, when known.
    pub fn take(&mut self, event: &pb::Event, roles: Option<&[String]>, visible: bool) -> bool {
        let Some(payload) = &event.payload else { return true };
        let Some(channel_id) = messageish(payload) else { return true };
        let whole = match self.intent {
            Intent::All => true,
            Intent::Focused | Intent::Mentions => {
                let mentioned = match payload {
                    Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
                    | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }) => {
                        mentions(m, &self.account_id, roles)
                    }
                    _ => false,
                };
                mentioned
                    || (self.intent == Intent::Focused
                        && self.focus.borrow().channel_ids.iter().any(|id| id == channel_id))
            }
        };
        if whole {
            return true;
        }
        let heads = self.heads.entry(event.server_id.clone()).or_default();
        heads.sequence = heads.sequence.max(event.sequence);
        if visible && let Payload::MessageCreated(pb::MessageCreated { message: Some(m) }) = payload {
            heads.channels.insert(channel_id.to_string(), m.id.clone());
        }
        false
    }

    /// The heads moved since last time, if any.
    pub fn take_heads(&mut self) -> Option<pb::ChannelHeads> {
        if self.heads.is_empty() {
            return None;
        }
        let servers = std::mem::take(&mut self.heads)
            .into_iter()
            .map(|(server_id, heads)| pb::ServerHeads {
                server_id,
                sequence: heads.sequence,
                channels: heads
                    .channels
                    .into_iter()
                    .map(|(channel_id, last_message_id)| pb::ChannelHead { channel_id, last_message_id })
                    .collect(),
            })
            .collect();
        Some(pb::ChannelHeads { servers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created(channel: &str, id: &str, sequence: i64) -> pb::Event {
        let message = pb::Message { id: id.into(), channel_id: channel.into(), ..Default::default() };
        pb::Event {
            server_id: "s".into(),
            sequence,
            payload: Some(Payload::MessageCreated(pb::MessageCreated { message: Some(message) })),
            ..Default::default()
        }
    }

    #[test]
    fn focus_picks_whole_messages_and_the_rest_move_heads() {
        let (tx, rx) = watch::channel(Arc::new(pb::Focus { channel_ids: vec!["a".into()], ..Default::default() }));
        let mut interest = Interest::new(Intent::Focused, "me", rx);
        assert!(interest.take(&created("a", "m1", 1), None, true));
        assert!(!interest.take(&created("b", "m2", 2), None, true));
        assert!(!interest.take(&created("b", "m3", 3), None, true));
        // A channel they can't see moves only the sequence.
        assert!(!interest.take(&created("c", "m4", 4), None, false));
        let heads = interest.take_heads().unwrap();
        assert_eq!(heads.servers.len(), 1);
        assert_eq!(heads.servers[0].sequence, 4);
        let channels: Vec<(&str, &str)> =
            heads.servers[0].channels.iter().map(|h| (h.channel_id.as_str(), h.last_message_id.as_str())).collect();
        assert_eq!(channels, [("b", "m3")]);
        assert!(interest.take_heads().is_none());

        tx.send_replace(Arc::new(pb::Focus { channel_ids: vec!["b".into()], ..Default::default() }));
        assert!(interest.take(&created("b", "m5", 5), None, true));
        assert!(!interest.take(&created("a", "m6", 6), None, true));
        // Server structure always comes.
        let structure = pb::Event { payload: Some(Payload::ServerUpdated(Default::default())), ..Default::default() };
        assert!(interest.take(&structure, None, true));
    }

    #[test]
    fn mentions_come_whole_and_agents_get_everything() {
        let (_tx, rx) = watch::channel(Arc::new(pb::Focus::default()));
        let mut interest = Interest::new(Intent::Mentions, "me", rx.clone());
        let mut by_name = created("b", "m1", 1);
        let mut by_role = created("b", "m2", 2);
        let mut everyone = created("b", "m3", 3);
        if let Some(Payload::MessageCreated(pb::MessageCreated { message: Some(m) })) = &mut by_name.payload {
            m.mention_user_ids = vec!["me".into()];
        }
        if let Some(Payload::MessageCreated(pb::MessageCreated { message: Some(m) })) = &mut by_role.payload {
            m.mention_role_ids = vec!["mods".into()];
        }
        if let Some(Payload::MessageCreated(pb::MessageCreated { message: Some(m) })) = &mut everyone.payload {
            m.mentions_everyone = true;
        }
        assert!(interest.take(&by_name, Some(&[]), true));
        assert!(interest.take(&everyone, Some(&[]), true));
        assert!(!interest.take(&by_role, Some(&["other".into()]), true));
        assert!(interest.take(&by_role, Some(&["mods".into()]), true));
        // Roles unknown: any role mention comes, so none is missed.
        assert!(interest.take(&by_role, None, true));

        assert_eq!(Intent::of(0, true), Intent::All);
        assert_eq!(Intent::of(0, false), Intent::Focused);
        let mut all = Interest::new(Intent::All, "me", rx);
        assert!(all.take(&created("b", "m4", 4), None, true));
    }

    #[test]
    fn only_the_session_that_opened_a_connection_focuses_it() {
        let connections = Arc::new(Connections::default());
        let opened = connections.open("me", "session", pb::Focus::default()).unwrap();
        let focus = pb::Focus { channel_ids: vec!["a".into()], ..Default::default() };
        let not_found = |r: Result<()>| matches!(r, Err(Error::NotFound(_)));
        assert!(not_found(connections.focus(&opened.id, "me", "other session", focus.clone())));
        assert!(not_found(connections.focus(&opened.id, "someone", "session", focus.clone())));
        assert!(not_found(connections.focus("unknown", "me", "session", focus.clone())));
        connections.focus(&opened.id, "me", "session", focus.clone()).unwrap();
        assert_eq!(*opened.focus.borrow().as_ref(), focus);
        let id = opened.id.clone();
        assert_eq!(id.len(), 32);
        drop(opened);
        assert!(not_found(connections.focus(&id, "me", "session", focus)));
    }
}
