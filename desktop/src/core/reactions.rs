//! Reactions to messages, as the web app's: in a server's channels and
//! threads the instance counts them (`Message.reactions`, kept in step by
//! `ReactionUpdated` and `ReactionsCleared`); in direct messages and secure
//! channels each device tallies them itself from the encrypted
//! `DirectMessageReaction`s it reads, each person's latest for each emoji
//! winning.
//!
//! Events that carry a whole message never carry its reactions, so a message
//! put in again from one keeps the reactions it had (`store::upsert_message`).

use crate::core::api::Problem;
use crate::core::dms::{Content, DmError};
use crate::core::store::InstanceState;
use crate::core::threads::thread_key;
use crate::core::vault::ReactionMark;
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// People asked for at a time when showing who reacted.
pub const REACTORS: i32 = 25;

/// Which emoji a reaction is: a standard one by its characters, or a server's own by id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct EmojiKey {
    pub emoji: String,
    pub emoji_id: String,
}

impl EmojiKey {
    pub fn of(r: &pb::Reaction) -> Self {
        Self { emoji: r.emoji.clone(), emoji_id: r.emoji_id.clone() }
    }

    pub fn standard(emoji: &str) -> Self {
        Self { emoji: emoji.to_owned(), emoji_id: String::new() }
    }

    pub fn custom(id: &str) -> Self {
        Self { emoji: String::new(), emoji_id: id.to_owned() }
    }

    /// Whether `r` is this emoji.
    pub fn is(&self, r: &pb::Reaction) -> bool {
        if self.emoji_id.is_empty() {
            r.emoji_id.is_empty() && same(&r.emoji, &self.emoji)
        } else {
            r.emoji_id == self.emoji_id
        }
    }

    /// One string for it, to key things by.
    pub fn id(&self) -> String {
        if self.emoji_id.is_empty() { emoji_key(&self.emoji) } else { format!(":{}", self.emoji_id) }
    }
}

/// Whether text from someone else's device could be one standard emoji: short,
/// with no letters, digits, spaces or controls (the instance checks for real
/// where it can see them; in encrypted places this keeps the junk out).
pub fn valid_emoji(emoji: &str) -> bool {
    !emoji.is_empty()
        && emoji.len() <= 32
        && !emoji.chars().any(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || c.is_control())
}

/// Puts an emoji's count as it is now on a message's reactions: a new one
/// goes last, one at 0 goes. `me`, when known, says whether you're one of them.
pub fn set(list: &mut Vec<pb::Reaction>, r: &pb::Reaction, me: Option<bool>) {
    let key = EmojiKey::of(r);
    match list.iter().position(|x| key.is(x)) {
        Some(at) if r.count == 0 => {
            list.remove(at);
        }
        Some(at) => {
            let had = &mut list[at];
            had.count = r.count;
            if !r.emoji_name.is_empty() {
                had.emoji_name = r.emoji_name.clone();
                had.animated = r.animated;
            }
            if let Some(me) = me {
                had.me = me;
            }
        }
        None if r.count > 0 => list.push(pb::Reaction { me: me.unwrap_or(false), ..r.clone() }),
        None => {}
    }
}

/// Takes one emoji's reactions off, or (`None`) every one.
pub fn clear(list: &mut Vec<pb::Reaction>, key: Option<&EmojiKey>) {
    match key {
        Some(key) => list.retain(|r| !key.is(r)),
        None => list.clear(),
    }
}

/// Your reaction with `r`'s emoji put on (`on`) or taken off, as it would be
/// once the instance has it. False when it's already that way.
pub fn toggle(list: &mut Vec<pb::Reaction>, r: &pb::Reaction, on: bool) -> bool {
    let key = EmojiKey::of(r);
    let had = list.iter().find(|x| key.is(x));
    if had.is_some_and(|x| x.me) == on {
        return false;
    }
    let count = had.map_or(0, |x| x.count);
    let count = if on { count + 1 } else { count.saturating_sub(1) };
    set(list, &pb::Reaction { count, ..r.clone() }, Some(on));
    true
}

/// Runs `f` on every loaded copy of a message: in its channel, in its
/// thread, as a thread's parent, and in the pin lists.
fn each_copy(
    i: &mut InstanceState,
    channel_id: &str,
    thread_id: &str,
    message_id: &str,
    mut f: impl FnMut(&mut pb::Message),
) {
    let mut places = vec![channel_id.to_owned(), thread_key(message_id)];
    if !thread_id.is_empty() {
        places.push(thread_key(thread_id));
    }
    for at in places {
        if let Some(loaded) = i.messages.get_mut(&at) {
            loaded.items.iter_mut().filter(|m| m.id == message_id).for_each(&mut f);
        }
    }
    if let Some(parent) = i.thread_parents.get_mut(message_id) {
        f(parent);
    }
    let prefix = format!("{channel_id}|");
    for (_, list) in i.pins.iter_mut().filter(|(k, _)| k.starts_with(&prefix)) {
        list.messages.iter_mut().filter(|m| m.id == message_id).for_each(&mut f);
    }
}

/// Someone reacted, or took theirs off: the count as the event says, and
/// `me` when it was you.
pub fn updated(i: &mut InstanceState, p: &pb::ReactionUpdated) {
    let Some(r) = &p.reaction else { return };
    let me = i.me.as_ref().is_some_and(|me| me.id == p.user_id).then_some(p.added);
    each_copy(i, &p.channel_id, &p.thread_id, &p.message_id, |m| set(&mut m.reactions, r, me));
}

/// A moderator took reactions off.
pub fn cleared(i: &mut InstanceState, p: &pb::ReactionsCleared) {
    let key = EmojiKey { emoji: p.emoji.clone(), emoji_id: p.emoji_id.clone() };
    let key = (!(key.emoji.is_empty() && key.emoji_id.is_empty())).then_some(&key);
    each_copy(i, &p.channel_id, &p.thread_id, &p.message_id, |m| clear(&mut m.reactions, key));
}

/// The reactions a message had, carried over to a new copy of it that came
/// without them (events never carry them).
pub fn keep(had: &pb::Message, now: &mut pb::Message) {
    if now.reactions.is_empty() {
        now.reactions = had.reactions.clone();
    }
}

/// A standard emoji as reactions match it: without variation selectors
/// (U+FE0F), which emoji lists and keyboards add or leave out, so "👍" and
/// "👍️" are one reaction, as the instance stores them and the web matches them.
pub fn emoji_key(emoji: &str) -> String {
    emoji.replace('\u{fe0f}', "")
}

/// Whether two standard emoji are one reaction.
pub fn same(a: &str, b: &str) -> bool {
    emoji_key(a) == emoji_key(b)
}

// ───────────────────────── Encrypted places ─────────────────────────

/// Notes what a record (`seq`) from `user` said about their reaction with
/// `emoji`: the latest record wins. False when it changes nothing (an older
/// or repeated record).
pub fn mark(marks: &mut Vec<ReactionMark>, user: &str, emoji: &str, seq: i64, removed: bool) -> bool {
    match marks.iter_mut().find(|m| m.user_id == user && same(&m.emoji, emoji)) {
        Some(m) if m.seq >= seq => false,
        Some(m) => {
            m.seq = seq;
            m.removed = removed;
            true
        }
        None => {
            marks.push(ReactionMark { user_id: user.to_owned(), emoji: emoji.to_owned(), seq, removed });
            true
        }
    }
}

/// A message's reactions from its marks, as the instance would count them:
/// each emoji once with how many have it on, in the order they were first
/// put on, `me` when you're one of them.
pub fn tally(marks: &[ReactionMark], me: &str) -> Vec<pb::Reaction> {
    let mut on: Vec<&ReactionMark> = marks.iter().filter(|m| !m.removed).collect();
    on.sort_by_key(|m| m.seq);
    let mut out: Vec<pb::Reaction> = Vec::new();
    for m in on {
        match out.iter_mut().find(|r| same(&r.emoji, &m.emoji)) {
            Some(r) => {
                r.count += 1;
                r.me |= m.user_id == me;
            }
            None => {
                out.push(pb::Reaction { emoji: m.emoji.clone(), count: 1, me: m.user_id == me, ..Default::default() })
            }
        }
    }
    out
}

/// Who has `emoji` on, the earliest first.
pub fn reactors(marks: &[ReactionMark], emoji: &str) -> Vec<String> {
    let mut on: Vec<&ReactionMark> = marks.iter().filter(|m| !m.removed && same(&m.emoji, emoji)).collect();
    on.sort_by_key(|m| m.seq);
    on.into_iter().map(|m| m.user_id.clone()).collect()
}

/// A page of who reacted with one emoji.
#[derive(Debug, Clone, Default)]
pub struct Reactors {
    pub users: Vec<pb::User>,
    pub has_more: bool,
}

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

impl Core {
    /// Reacts to a message in a channel or thread with `r`'s emoji, or takes
    /// your reaction off. It shows at once, and goes back if the instance says no.
    pub async fn react(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
        r: pb::Reaction,
        on: bool,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used(if on { "message.react" } else { "message.unreact" });
        let thread = self.thread_of(key, channel_id, message_id);
        let changed = self
            .shared
            .instance(key, |i| {
                let mut changed = false;
                each_copy(i, channel_id, &thread, message_id, |m| changed |= toggle(&mut m.reactions, &r, on));
                changed
            })
            .unwrap_or(false);
        let res = rpc!(
            api.messages(),
            react(pb::ReactRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                message_id: message_id.into(),
                emoji: r.emoji.clone(),
                emoji_id: r.emoji_id.clone(),
                reacted: on,
            })
        )
        .await;
        self.shared.instance(key, |i| match &res {
            Ok(res) => {
                // As the instance has it now (its count may hold others' too).
                let now = res.reaction.clone().unwrap_or_else(|| pb::Reaction { count: 0, ..r.clone() });
                let now = pb::Reaction { emoji: r.emoji.clone(), emoji_id: r.emoji_id.clone(), ..now };
                each_copy(i, channel_id, &thread, message_id, |m| set(&mut m.reactions, &now, Some(on)));
            }
            Err(_) if changed => {
                each_copy(i, channel_id, &thread, message_id, |m| {
                    toggle(&mut m.reactions, &r, !on);
                });
            }
            Err(_) => {}
        });
        res.map(|_| ())
    }

    /// The thread a loaded message is a reply in, if it is one.
    fn thread_of(&self, key: &str, channel_id: &str, message_id: &str) -> String {
        self.shared
            .read(|s| {
                let i = s.instance(key)?;
                i.messages.iter().filter(|(at, _)| *at == channel_id || at.starts_with("t:")).find_map(|(_, loaded)| {
                    loaded
                        .items
                        .iter()
                        .find(|m| m.id == message_id && m.channel_id == channel_id)
                        .map(|m| m.thread_id.clone())
                })
            })
            .unwrap_or_default()
    }

    /// Who reacted to a message with one emoji, a page at a time (`after_id`
    /// the last one of the page before).
    pub async fn list_reactors(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
        emoji: &EmojiKey,
        after_id: &str,
    ) -> Result<Reactors, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.messages(),
            list_reactors(pb::ListReactorsRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                message_id: message_id.into(),
                emoji: emoji.emoji.clone(),
                emoji_id: emoji.emoji_id.clone(),
                limit: REACTORS,
                after_id: after_id.into(),
            })
        )
        .await?;
        self.shared.instance(key, |i| {
            for user in &res.users {
                if !i.users.contains_key(&user.id) {
                    i.users.insert(user.id.clone(), user.clone());
                }
            }
        });
        Ok(Reactors { users: res.users, has_more: res.has_more })
    }

    /// Takes one emoji's reactions off a message, or (`None`) every one. Needs Manage Messages.
    pub async fn clear_reactions(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        message_id: &str,
        emoji: Option<&EmojiKey>,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used("message.clear_reactions");
        rpc!(
            api.messages(),
            clear_reactions(pb::ClearReactionsRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                message_id: message_id.into(),
                emoji: emoji.map(|e| e.emoji.clone()).unwrap_or_default(),
                emoji_id: emoji.map(|e| e.emoji_id.clone()).unwrap_or_default(),
            })
        )
        .await?;
        let thread = self.thread_of(key, channel_id, message_id);
        self.shared
            .instance(key, |i| each_copy(i, channel_id, &thread, message_id, |m| clear(&mut m.reactions, emoji)));
        Ok(())
    }

    /// Reacts to a message in a conversation or secure channel (`room`), by its
    /// record, with a standard emoji, or takes the reaction off. It goes inside
    /// the encryption like a message, and shows once this device reads it back.
    pub async fn react_dm(&self, key: &str, room: &str, sequence: i64, emoji: &str, on: bool) -> Result<(), DmError> {
        if !valid_emoji(emoji) {
            return Err(DmError(crate::core::i18n::t("chattools.reactions.failed")));
        }
        reports::used(if on { "message.react" } else { "message.unreact" });
        self.send_dm(key, room, Content::React { sequence, emoji: emoji.to_owned(), removed: !on }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard(emoji: &str, count: u32) -> pb::Reaction {
        pb::Reaction { emoji: emoji.into(), count, ..Default::default() }
    }

    fn custom(id: &str, count: u32) -> pb::Reaction {
        pb::Reaction { emoji_id: id.into(), emoji_name: "blob".into(), count, ..Default::default() }
    }

    fn state() -> InstanceState {
        let mut i = InstanceState::new("k", "http://x");
        i.me = Some(pb::User { id: "me".into(), ..Default::default() });
        let message = pb::Message { id: "m1".into(), channel_id: "c".into(), ..Default::default() };
        i.messages.entry("c".into()).or_default().items.push(message);
        i
    }

    fn event(user: &str, r: pb::Reaction, added: bool) -> pb::ReactionUpdated {
        pb::ReactionUpdated {
            channel_id: "c".into(),
            message_id: "m1".into(),
            thread_id: String::new(),
            reaction: Some(r),
            user_id: user.into(),
            added,
        }
    }

    fn reactions(i: &InstanceState) -> Vec<pb::Reaction> {
        i.messages["c"].items[0].reactions.clone()
    }

    #[test]
    fn events_set_counts_and_are_idempotent() {
        let mut i = state();
        let e = event("ann", standard("👍", 1), true);
        updated(&mut i, &e);
        updated(&mut i, &e);
        assert_eq!(reactions(&i), vec![standard("👍", 1)]);
        // Yours: `me` from who it was.
        updated(&mut i, &event("me", standard("👍", 2), true));
        updated(&mut i, &event("me", standard("👍", 2), true));
        assert_eq!(reactions(&i), vec![pb::Reaction { me: true, ..standard("👍", 2) }]);
        // Someone else's doesn't change yours.
        updated(&mut i, &event("bob", custom("e1", 1), true));
        assert!(reactions(&i)[0].me);
        assert_eq!(reactions(&i)[1].emoji_id, "e1");
        // Taking it off; at 0 it goes.
        updated(&mut i, &event("me", standard("👍", 1), false));
        assert_eq!(reactions(&i)[0], standard("👍", 1));
        updated(&mut i, &event("ann", standard("👍", 0), false));
        assert_eq!(reactions(&i).len(), 1);
    }

    #[test]
    fn clears_one_or_all() {
        let mut i = state();
        updated(&mut i, &event("ann", standard("👍", 1), true));
        updated(&mut i, &event("ann", custom("e1", 1), true));
        let gone = |emoji: &str, id: &str| pb::ReactionsCleared {
            channel_id: "c".into(),
            message_id: "m1".into(),
            emoji: emoji.into(),
            emoji_id: id.into(),
            ..Default::default()
        };
        cleared(&mut i, &gone("", "e1"));
        assert_eq!(reactions(&i), vec![standard("👍", 1)]);
        updated(&mut i, &event("ann", custom("e1", 1), true));
        cleared(&mut i, &gone("", ""));
        assert!(reactions(&i).is_empty());
    }

    #[test]
    fn edits_keep_reactions() {
        let mut i = state();
        updated(&mut i, &event("ann", standard("🎉", 3), true));
        let edited =
            pb::Message { id: "m1".into(), channel_id: "c".into(), content: "new".into(), ..Default::default() };
        let ev = pb::Event {
            server_id: "s".into(),
            payload: Some(pb::event::Payload::MessageUpdated(pb::MessageUpdated { message: Some(edited) })),
            ..Default::default()
        };
        crate::core::store::apply_event(&mut i, &ev, None, None);
        assert_eq!(i.messages["c"].items[0].content, "new");
        assert_eq!(reactions(&i), vec![standard("🎉", 3)]);
    }

    #[test]
    fn toggling_counts_once() {
        let mut list = vec![standard("👍", 2)];
        assert!(toggle(&mut list, &standard("👍", 0), true));
        assert!(!toggle(&mut list, &standard("👍", 0), true));
        assert_eq!(list, vec![pb::Reaction { me: true, ..standard("👍", 3) }]);
        assert!(toggle(&mut list, &standard("👍", 0), false));
        assert_eq!(list, vec![standard("👍", 2)]);
        assert!(toggle(&mut list, &custom("e1", 0), true));
        assert!(toggle(&mut list, &custom("e1", 0), false));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn latest_record_wins_in_encrypted_places() {
        let mut marks = Vec::new();
        assert!(mark(&mut marks, "ann", "👍", 5, false));
        assert!(mark(&mut marks, "me", "👍", 6, false));
        assert!(mark(&mut marks, "ann", "❤️", 7, false));
        // Read again, or an older record: nothing changes.
        assert!(!mark(&mut marks, "ann", "👍", 5, false));
        assert!(!mark(&mut marks, "ann", "👍", 4, true));
        let t = tally(&marks, "me");
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].emoji.as_str(), t[0].count, t[0].me), ("👍", 2, true));
        assert_eq!((t[1].emoji.as_str(), t[1].count, t[1].me), ("❤️", 1, false));
        assert_eq!(reactors(&marks, "👍"), vec!["ann".to_owned(), "me".to_owned()]);
        // Taken off, then an older "on" arriving late can't put it back.
        assert!(mark(&mut marks, "me", "👍", 9, true));
        assert!(!mark(&mut marks, "me", "👍", 8, false));
        let t = tally(&marks, "me");
        assert_eq!((t[0].count, t[0].me), (1, false));
        // Everyone off: the emoji goes.
        assert!(mark(&mut marks, "ann", "👍", 10, true));
        assert_eq!(tally(&marks, "me").len(), 1);
    }

    #[test]
    fn variation_selectors_dont_split_a_reaction() {
        let mut marks = Vec::new();
        assert!(mark(&mut marks, "ann", "👍", 1, false));
        assert!(mark(&mut marks, "bo", "👍\u{fe0f}", 2, false));
        assert!(mark(&mut marks, "ann", "👍\u{fe0f}", 3, true));
        let t = tally(&marks, "bo");
        assert_eq!((t.len(), t[0].count, t[0].me), (1, 1, true));
        assert_eq!(reactors(&marks, "👍"), vec!["bo".to_owned()]);
        assert!(EmojiKey::standard("👍").is(&pb::Reaction { emoji: "👍\u{fe0f}".into(), ..Default::default() }));
    }

    #[test]
    fn only_emoji_like_text_counts() {
        assert!(valid_emoji("👍") && valid_emoji("👍🏽") && valid_emoji("❤️"));
        assert!(!valid_emoji("") && !valid_emoji("hi") && !valid_emoji("👍 ") && !valid_emoji(&"👍".repeat(9)));
    }
}
