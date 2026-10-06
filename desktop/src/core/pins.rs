//! Pinned messages in a channel or thread, as the web app's `fuwa/pins.ts`:
//! each list read when someone opens it and read again when an event says a
//! pin there changed. Messages themselves carry `pinned_at` while pinned.
//!
//! A private conversation's pins name records by their place only (never what
//! they say, nor who pinned them): this device draws each from what it opened.

use std::sync::Arc;

use crate::core::api::Problem;
use crate::core::store::{self, InstanceState};
use crate::core::threads::thread_key;
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// Pins asked for at a time.
pub const PAGE: i32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinStatus {
    Loading,
    Ready,
    Failed,
}

/// A channel's (or thread's) pins, the latest pin first.
#[derive(Debug, Clone)]
pub struct PinList {
    pub status: PinStatus,
    pub messages: Vec<pb::Message>,
    pub has_more: bool,
    /// Counts reads from the top, so an answer overtaken by a newer read is dropped.
    pub generation: u64,
}

impl PinList {
    fn new() -> Self {
        Self { status: PinStatus::Loading, messages: Vec::new(), has_more: false, generation: 0 }
    }
}

/// Where a list is kept: a channel's own (`thread` empty) or a thread's.
pub fn pins_key(channel_id: &str, thread_id: &str) -> String {
    format!("{channel_id}|{thread_id}")
}

/// A page read: the first replaces the list, a later one adds what's new to its end.
pub fn merge(had: &[pb::Message], page: Vec<pb::Message>, more: bool) -> Vec<pb::Message> {
    if !more {
        return page;
    }
    let mut out = had.to_vec();
    out.extend(page.into_iter().filter(|m| !had.iter().any(|h| h.id == m.id)));
    out
}

/// Whether an answer still fits the list: no read from the top started since
/// it was asked for, and for a later page, the list still ends where it asked from.
pub fn still_current(list: &PinList, generation: u64, more: bool, after_id: &str) -> bool {
    list.generation == generation && (!more || list.messages.last().is_some_and(|m| m.id == after_id))
}

/// Marks a message pinned or not wherever it's loaded, as the event says.
pub fn mark(i: &mut InstanceState, p: &pb::MessagePinned) {
    let mut places = vec![p.channel_id.clone()];
    if !p.thread_id.is_empty() {
        places.push(thread_key(&p.thread_id));
    }
    for at in places {
        if let Some(loaded) = i.messages.get_mut(&at) {
            for m in loaded.items.iter_mut().filter(|m| m.id == p.message_id) {
                m.pinned_at = p.pinned_at;
            }
        }
    }
    if let Some(parent) = i.thread_parents.get_mut(&p.message_id) {
        parent.pinned_at = p.pinned_at;
    }
}

/// A conversation's pins, the latest pin first.
#[derive(Debug, Clone)]
pub struct DmPinList {
    pub status: PinStatus,
    pub pins: Vec<pb::DmPin>,
    pub has_more: bool,
    /// As [`PinList::generation`].
    pub generation: u64,
    /// While a read is out: the pins and unpins seen since it started, to
    /// apply again over its answer (which may predate them).
    changes: Option<Vec<(i64, Option<pb::DmPin>)>>,
}

impl DmPinList {
    fn new() -> Self {
        Self { status: PinStatus::Loading, pins: Vec::new(), has_more: false, generation: 0, changes: None }
    }
}

/// Starts a read of a conversation's pins: the generation it answers to, and
/// (for `more`) the sequence the next page follows.
fn begin_dm(list: &mut DmPinList, more: bool) -> (u64, Option<i64>) {
    if more {
        list.changes.get_or_insert_with(Vec::new);
    } else {
        list.status = PinStatus::Loading;
        list.generation += 1;
        list.changes = Some(Vec::new());
    }
    (list.generation, if more { list.pins.last().map(|p| p.sequence) } else { None })
}

/// Takes a read's answer, then what changed while it was out.
fn settle_dm(list: &mut DmPinList, pins: Vec<pb::DmPin>, has_more: bool, more: bool) {
    if more {
        let new: Vec<_> = pins.into_iter().filter(|p| !list.pins.iter().any(|h| h.sequence == p.sequence)).collect();
        list.pins.extend(new);
    } else {
        list.pins = pins;
    }
    for (sequence, pin) in list.changes.take().unwrap_or_default() {
        list.pins.retain(|p| p.sequence != sequence);
        if let Some(pin) = pin {
            list.pins.insert(0, pin);
        }
    }
    list.has_more = has_more;
    list.status = PinStatus::Ready;
}

/// As [`still_current`], for a conversation's list.
pub fn dm_still_current(list: &DmPinList, generation: u64, after: Option<i64>) -> bool {
    list.generation == generation && after.is_none_or(|seq| list.pins.last().is_some_and(|p| p.sequence == seq))
}

/// A record pinned (to the top) or unpinned (gone from the list).
pub fn change_dm(list: &mut DmPinList, sequence: i64, pin: Option<pb::DmPin>) {
    if let Some(changes) = &mut list.changes {
        changes.push((sequence, pin.clone()));
    }
    list.pins.retain(|p| p.sequence != sequence);
    if let Some(pin) = pin {
        list.pins.insert(0, pin);
    }
}

/// A deleted message leaves the pin lists it was in.
pub fn forget(i: &mut InstanceState, channel_id: &str, message_id: &str) {
    let prefix = format!("{channel_id}|");
    for (_, list) in i.pins.iter_mut().filter(|(k, _)| k.starts_with(&prefix)) {
        list.messages.retain(|m| m.id != message_id);
    }
}

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

impl Core {
    /// Reads a channel's (or thread's) pins; `more` adds the next page.
    pub async fn load_pins(&self, key: &str, server_id: &str, channel_id: &str, thread_id: &str, more: bool) {
        let Some(api) = self.api(key) else { return };
        let at = pins_key(channel_id, thread_id);
        let Some((generation, after_id)) = self.shared.instance(key, |i| {
            let list = i.pins.entry(at.clone()).or_insert_with(PinList::new);
            if !more {
                list.status = PinStatus::Loading;
                list.generation += 1;
            }
            let after_id =
                if more { list.messages.last().map(|m| m.id.clone()).unwrap_or_default() } else { String::new() };
            (list.generation, after_id)
        }) else {
            return;
        };
        if more && after_id.is_empty() {
            return;
        }
        let res = rpc!(
            api.messages(),
            list_pins(pb::ListPinsRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                thread_id: thread_id.into(),
                limit: PAGE,
                after_id: after_id.clone(),
            })
        )
        .await;
        self.shared.instance(key, |i| {
            let Some(list) = i.pins.get_mut(&at) else { return };
            if !still_current(list, generation, more, &after_id) {
                return;
            }
            match res {
                Ok(res) => {
                    list.messages = merge(&list.messages, res.messages, more);
                    list.has_more = res.has_more;
                    list.status = PinStatus::Ready;
                    let messages = list.messages.clone();
                    for user in &res.authors {
                        if !i.users.contains_key(&user.id) {
                            i.users.insert(user.id.clone(), user.clone());
                        }
                    }
                    store::add_shared_authors(&mut i.users, &messages);
                }
                Err(_) => {
                    list.status = PinStatus::Failed;
                    if !more {
                        list.has_more = false;
                    }
                }
            }
        });
    }

    /// Reads again the lists a pin change shows in, where they've been opened.
    pub fn reload_pins(self: &Arc<Self>, key: &str, server_id: &str, channel_id: &str, thread_id: &str) {
        let mut threads = vec![String::new()];
        if !thread_id.is_empty() {
            threads.push(thread_id.to_owned());
        }
        for thread in threads {
            let loaded = self
                .shared
                .read(|s| s.instance(key).is_some_and(|i| i.pins.contains_key(&pins_key(channel_id, &thread))));
            if loaded {
                let (core, key, server, channel) =
                    (self.clone(), key.to_owned(), server_id.to_owned(), channel_id.to_owned());
                drop(self.spawn(async move { core.load_pins(&key, &server, &channel, &thread, false).await }));
            }
        }
    }

    /// Reads a conversation's pins; `more` adds the next page.
    pub async fn load_dm_pins(&self, key: &str, conversation: &str, more: bool) {
        let Some(api) = self.api(key) else { return };
        let Some((generation, after)) = self.shared.instance(key, |i| {
            begin_dm(i.dms.pins.entry(conversation.to_owned()).or_insert_with(DmPinList::new), more)
        }) else {
            return;
        };
        if more && after.is_none() {
            return;
        }
        let res = rpc!(
            api.dms(),
            list_record_pins(pb::ListRecordPinsRequest {
                conversation_id: conversation.into(),
                limit: PAGE,
                after_sequence: after,
            })
        )
        .await;
        self.shared.instance(key, |i| {
            let Some(list) = i.dms.pins.get_mut(conversation) else { return };
            if !dm_still_current(list, generation, after) {
                return;
            }
            match res {
                Ok(res) => settle_dm(list, res.pins, res.has_more, more),
                Err(_) => {
                    list.changes = None;
                    list.status = PinStatus::Failed;
                }
            }
        });
    }

    /// Pins a private message by its place in the conversation, or unpins it.
    pub async fn pin_dm(&self, key: &str, conversation: &str, sequence: i64, pinned: bool) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used(if pinned { "message.pin" } else { "message.unpin" });
        let res = rpc!(
            api.dms(),
            pin_record(pb::PinRecordRequest { conversation_id: conversation.into(), sequence, pinned })
        )
        .await?;
        self.shared.instance(key, |i| {
            if let Some(list) = i.dms.pins.get_mut(conversation) {
                change_dm(list, sequence, res.pin);
            }
        });
        Ok(())
    }

    /// Pins a message in a channel or thread, or unpins it; it's marked at once.
    pub async fn pin_message(
        self: &Arc<Self>,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
        pinned: bool,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used(if pinned { "message.pin" } else { "message.unpin" });
        let res = rpc!(
            api.messages(),
            pin_message(pb::PinMessageRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                message_id: message_id.into(),
                pinned,
            })
        )
        .await?;
        if let Some(message) = res.message {
            let change = pb::MessagePinned {
                channel_id: message.channel_id.clone(),
                message_id: message.id.clone(),
                thread_id: message.thread_id.clone(),
                pinned_at: message.pinned_at,
            };
            self.shared.instance(key, |i| mark(i, &change));
            self.reload_pins(key, server_id, &change.channel_id, &change.thread_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str) -> pb::Message {
        pb::Message { id: id.into(), channel_id: "c".into(), ..Default::default() }
    }

    fn ids(list: &[pb::Message]) -> Vec<&str> {
        list.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn a_later_page_adds_only_whats_new() {
        let had = vec![message("a"), message("b")];
        // A pin made while reading moves the page along by one.
        assert_eq!(ids(&merge(&had, vec![message("b"), message("c")], true)), ["a", "b", "c"]);
        // Reading from the top again replaces the list.
        assert_eq!(ids(&merge(&had, vec![message("z")], false)), ["z"]);
    }

    #[test]
    fn pins_show_on_loaded_messages_and_leave_with_them() {
        let mut i = InstanceState::new("k", "https://k");
        let mut reply = message("r");
        reply.thread_id = "t".into();
        i.messages.insert(
            "c".into(),
            store::ChannelMessages { items: vec![message("m"), reply.clone()], ..Default::default() },
        );
        i.messages.insert(thread_key("t"), store::ChannelMessages { items: vec![reply], ..Default::default() });
        let at = Some(prost_types::Timestamp { seconds: 9, nanos: 0 });
        let change = |id: &str, thread: &str, pinned_at| pb::MessagePinned {
            channel_id: "c".into(),
            message_id: id.into(),
            thread_id: thread.into(),
            pinned_at,
        };
        mark(&mut i, &change("r", "t", at));
        assert!(i.messages["c"].items[1].pinned_at.is_some());
        assert!(i.messages[&thread_key("t")].items[0].pinned_at.is_some());
        assert!(i.messages["c"].items[0].pinned_at.is_none());
        mark(&mut i, &change("r", "t", None));
        assert!(i.messages[&thread_key("t")].items[0].pinned_at.is_none());

        i.pins.insert(
            pins_key("c", ""),
            PinList {
                status: PinStatus::Ready,
                messages: vec![message("m"), message("n")],
                has_more: false,
                generation: 0,
            },
        );
        i.pins.insert(
            pins_key("other", ""),
            PinList { status: PinStatus::Ready, messages: vec![message("m")], has_more: false, generation: 0 },
        );
        forget(&mut i, "c", "m");
        assert_eq!(ids(&i.pins[&pins_key("c", "")].messages), ["n"]);
        assert_eq!(ids(&i.pins[&pins_key("other", "")].messages), ["m"]);
    }

    #[test]
    fn answers_overtaken_by_a_newer_read_are_dropped() {
        let list = PinList {
            status: PinStatus::Loading,
            messages: vec![message("a"), message("b")],
            has_more: true,
            generation: 3,
        };
        // The newest read from the top applies; an older one that came back late doesn't.
        assert!(still_current(&list, 3, false, ""));
        assert!(!still_current(&list, 2, false, ""));
        // A later page applies where the list still ends where it asked from…
        assert!(still_current(&list, 3, true, "b"));
        // …but not after a read from the top, nor when the list now ends elsewhere.
        assert!(!still_current(&list, 2, true, "b"));
        assert!(!still_current(&list, 3, true, "a"));
    }

    fn dm_pin(seq: i64) -> pb::DmPin {
        pb::DmPin { conversation_id: "d".into(), sequence: seq, pinned_at: None }
    }

    #[test]
    fn a_conversations_pins_change_in_place() {
        let mut list = DmPinList::new();
        list.pins = vec![dm_pin(4), dm_pin(2)];
        // Pinned again: once, at the top.
        change_dm(&mut list, 2, Some(dm_pin(2)));
        assert_eq!(list.pins.iter().map(|p| p.sequence).collect::<Vec<_>>(), [2, 4]);
        // Unpinned, or its record deleted: gone.
        change_dm(&mut list, 4, None);
        assert_eq!(list.pins.iter().map(|p| p.sequence).collect::<Vec<_>>(), [2]);
        // A later page counts only where the list still ends where it asked from.
        list.generation = 5;
        assert!(dm_still_current(&list, 5, Some(2)));
        assert!(!dm_still_current(&list, 5, Some(4)));
        assert!(!dm_still_current(&list, 4, None));
    }

    #[test]
    fn changes_during_a_read_outlast_its_older_answer() {
        let mut list = DmPinList::new();
        list.pins = vec![dm_pin(3), dm_pin(1)];
        let (generation, after) = begin_dm(&mut list, false);
        // While the read is out: 7 is pinned and 3 unpinned.
        change_dm(&mut list, 7, Some(dm_pin(7)));
        change_dm(&mut list, 3, None);
        // The answer was worked out before either.
        assert!(dm_still_current(&list, generation, after));
        settle_dm(&mut list, vec![dm_pin(3), dm_pin(1)], false, false);
        assert_eq!(list.pins.iter().map(|p| p.sequence).collect::<Vec<_>>(), [7, 1]);
        // Once taken, later reads start clean.
        begin_dm(&mut list, false);
        settle_dm(&mut list, vec![dm_pin(1)], false, false);
        assert_eq!(list.pins.iter().map(|p| p.sequence).collect::<Vec<_>>(), [1]);
    }
}
