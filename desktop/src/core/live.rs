//! What a live connection (docs/live.md) is told and what's done with what
//! it says, beside the stream itself in `sync.rs`: the focus (the channel
//! on screen and the people in sight), kept up to date as the screen
//! changes, and the heads that stand in for the messages out of focus.
//!
//! Out of focus, a channel's messages arrive only as its head, so what was
//! loaded of it goes stale: when a channel comes into focus, and after each
//! reconnect, its newest page is read again and merged in. Mentions still
//! come whole, so they notify as ever; a channel set to notify for every
//! message reads what its head says it missed, a page at a time and a few
//! reads at most at once.
//!
//! Everything this starts runs in the connection's own set of tasks, so it
//! stops with the connection (and with signing out, or the instance going).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tonic::Code;

use crate::core::api::Api;
use crate::core::config::Prefs;
use crate::core::store::{self, ChannelMessages, InstanceState, Store};
use crate::core::{Core, dms, notifications, threads};
use crate::pb;
use crate::rpc;

/// The most channels a focus names, as the instance takes it.
pub const MAX_CHANNELS: usize = 8;
/// The most people a focus names.
pub const MAX_PEOPLE: usize = 500;
/// The screen settles this long before the focus goes out, so scrolling sends one.
const SETTLE: Duration = Duration::from_millis(150);
/// A channel's newest page, as it's read again when it comes into focus.
const PAGE: i32 = 50;
/// The most messages read for a channel that notifies for each, per read.
const NOTIFY_PAGE: i32 = 20;
/// Reads for notifying at once, per instance.
const NOTIFY_READS: usize = 2;
/// A channel's reads for notifying are at least this far apart; heads in between wait for the next.
const NOTIFY_EVERY: Duration = Duration::from_secs(10);

/// What a task a live connection started says when it ends.
pub(super) enum Done {
    Nothing,
    /// A read for notifying, in this channel, finished.
    Notified(String),
    /// Direct messages caught up after the feed said it's ready.
    DmsCaughtUp,
}

/// Whether an id is one the instance takes in a focus: letters and digits,
/// at most 32. Anything else (someone from another instance) is left out
/// rather than have the instance turn the whole focus down.
fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// What's on screen on one instance: the channel open (a thread open beside
/// it comes with it), the member list's rows in sight and the authors of
/// the messages loaded there, newest first. In a conversation, the people in
/// it. You aren't named: your own presence isn't news. The people are picked
/// in that order (up to the most the instance takes) and sent sorted.
pub fn choose_focus(store: &Store, key: &str) -> pb::Focus {
    let mut focus = pb::Focus::default();
    let (Some(f), Some(i)) = (store.focus.as_ref().filter(|f| f.instance == key), store.instance(key)) else {
        return focus;
    };
    let me = i.me.as_ref().map(|m| m.id.as_str());
    let mut seen = HashSet::new();
    let mut add = |id: &str| {
        if focus.user_ids.len() < MAX_PEOPLE && Some(id) != me && plain_id(id) && seen.insert(id.to_owned()) {
            focus.user_ids.push(id.to_owned());
        }
    };
    let server = i.channels.iter().find(|(_, list)| list.iter().any(|c| c.id == f.channel)).map(|(sid, _)| sid);
    match server {
        Some(sid) => {
            if let Some(view) = store.in_view.as_ref().filter(|v| v.instance == key && v.server == *sid) {
                for id in &view.people {
                    add(id);
                }
            }
            let thread = f.thread.as_ref().and_then(|t| i.messages.get(&threads::thread_key(t)));
            for list in thread.into_iter().chain(i.messages.get(&f.channel)) {
                for m in list.items.iter().rev() {
                    add(&m.author_id);
                }
            }
            if plain_id(&f.channel) {
                focus.channel_ids.push(f.channel.clone());
            }
        }
        None => {
            if let Some(c) = i.dms.conversations.iter().find(|c| c.id == f.channel) {
                for user in &c.users {
                    add(&user.id);
                }
            }
        }
    }
    focus.channel_ids.truncate(MAX_CHANNELS);
    // In a steady order, so someone already named writing again isn't a new focus.
    focus.user_ids.sort();
    focus
}

/// A channel that notifies for every message moved out of focus: what came
/// after its mark (`InstanceState::notified`) is read to notify for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missed {
    pub server_id: String,
    pub channel_id: String,
}

/// Puts in what heads say moved out of focus: a channel's newest message
/// moves on and it counts as unread (one for each head that moved it, since
/// heads don't say how many came). Gives the channels whose settings ask
/// for every message, once per head: their messages are read to notify.
pub fn apply_heads(
    i: &mut InstanceState,
    heads: &pb::ChannelHeads,
    focus: Option<&str>,
    prefs: &Prefs,
    now_ms: i64,
) -> Vec<Missed> {
    let mut missed = Vec::new();
    for server in &heads.servers {
        for head in &server.channels {
            let known = store::newest_known(i, &head.channel_id).map(str::to_owned);
            if known.as_deref().is_some_and(|known| head.last_message_id.as_str() <= known) {
                continue;
            }
            i.newest.insert(head.channel_id.clone(), head.last_message_id.clone());
            if focus == Some(head.channel_id.as_str()) {
                continue;
            }
            *i.unread.entry(head.channel_id.clone()).or_default() += 1;
            let settings = i.effective_notifications(&server.server_id, &head.channel_id, now_ms);
            if notifications::should_notify(settings, false, prefs) {
                // Notified up to what was known before this head, unless an earlier one left a mark.
                if let Some(known) = known {
                    i.notified.entry(head.channel_id.clone()).or_insert(known);
                }
                missed.push(Missed { server_id: server.server_id.clone(), channel_id: head.channel_id.clone() });
            }
        }
    }
    missed
}

/// What changed in what's loaded of a channel while its page was being read:
/// messages that came, or changed, live (`changed`) and those deleted live
/// (`deleted`). Live news is newer than the page, so it wins.
#[derive(Debug, Default)]
pub struct Meanwhile {
    pub changed: HashSet<String>,
    pub deleted: HashSet<String>,
}

/// Each loaded message, by id, in short, to tell later what changed meanwhile.
fn marks(entry: &ChannelMessages) -> HashMap<String, u64> {
    entry.items.iter().map(|m| (m.id.clone(), mark(m))).collect()
}

fn mark(m: &pb::Message) -> u64 {
    use prost::Message as _;
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.encode_to_vec().hash(&mut h);
    h.finish()
}

impl Meanwhile {
    /// What changed between `before` (from [`marks`]) and what's loaded now.
    fn since(before: &HashMap<String, u64>, entry: &ChannelMessages) -> Self {
        let now: HashSet<&str> = entry.items.iter().map(|m| m.id.as_str()).collect();
        Self {
            changed: entry.items.iter().filter(|m| before.get(&m.id) != Some(&mark(m))).map(|m| m.id.clone()).collect(),
            deleted: before.keys().filter(|id| !now.contains(id.as_str())).cloned().collect(),
        }
    }
}

/// Merges a channel's newest page, read again, into what was loaded of it.
/// Where they meet, the page is what's there now from its oldest message on
/// (anything loaded that it lacks was deleted meanwhile); where they don't,
/// there's a gap of unknown size, so the page replaces what was loaded.
/// Either way what came live while it was read stays as it came: messages
/// newer than the page, and those that came, changed or went meanwhile.
pub fn merge_newest(entry: &mut ChannelMessages, page: Vec<pb::Message>, has_more: bool, meanwhile: &Meanwhile) {
    let newest = page.last().map(|m| m.id.clone());
    let oldest = page.first().map(|m| m.id.clone());
    let live = |m: &pb::Message| {
        meanwhile.changed.contains(&m.id) || newest.as_deref().is_some_and(|newest| m.id.as_str() > newest)
    };
    let page: Vec<pb::Message> =
        page.into_iter().filter(|m| !meanwhile.deleted.contains(&m.id) && !meanwhile.changed.contains(&m.id)).collect();
    let meets = !has_more
        || match (&oldest, entry.items.iter().rfind(|m| newest.as_deref().is_none_or(|n| m.id.as_str() <= n))) {
            (Some(first), Some(last)) => *first <= last.id,
            _ => false,
        };
    if !meets {
        entry.items.retain(|m| live(m));
        entry.has_more = has_more;
    } else {
        // The whole channel, when there's no more: nothing older is left either.
        let from = if has_more { oldest.unwrap_or_default() } else { String::new() };
        let ids: HashSet<&str> = page.iter().map(|m| m.id.as_str()).collect();
        entry.items.retain(|m| (!from.is_empty() && m.id < from) || ids.contains(m.id.as_str()) || live(m));
        if !has_more {
            entry.has_more = false;
        }
    }
    for m in page {
        store::put_message(&mut entry.items, m);
    }
}

/// Moves what heads say on into the store, and queues reads for the
/// channels that notify for every message.
pub(super) fn take_heads(core: &Arc<Core>, key: &str, heads: &pb::ChannelHeads, notifier: &mut Notifier) {
    let prefs = core.prefs();
    let missed = core.shared.update(|s| {
        let focus = s.focus_channel(key).map(str::to_owned);
        let Some(i) = s.instances.get_mut(key) else { return Vec::new() };
        apply_heads(i, heads, focus.as_deref(), &prefs, dms::now_ms())
    });
    notifier.want(missed);
}

/// The reads for notifying one instance's connection owes: a few at once,
/// each channel's far enough apart, heads in between gathered into one.
#[derive(Default)]
pub(super) struct Notifier {
    /// Channels waiting for a read, with their server.
    waiting: BTreeMap<String, String>,
    running: HashSet<String>,
    /// When each channel's last read started.
    last: HashMap<String, Instant>,
}

impl Notifier {
    pub fn want(&mut self, missed: Vec<Missed>) {
        for m in missed {
            self.waiting.insert(m.channel_id, m.server_id);
        }
    }

    /// The channels (and their servers) whose reads may start at `now`.
    fn ready(&self, now: Instant) -> Vec<(String, String)> {
        let room = NOTIFY_READS.saturating_sub(self.running.len());
        self.waiting
            .iter()
            .filter(|(c, _)| !self.running.contains(*c))
            .filter(|(c, _)| self.last.get(*c).is_none_or(|at| now >= *at + NOTIFY_EVERY))
            .take(room)
            .map(|(c, s)| (c.clone(), s.clone()))
            .collect()
    }

    /// When a waiting read that has to wait its turn could start.
    pub fn due(&self) -> Option<Instant> {
        if self.running.len() >= NOTIFY_READS {
            return None;
        }
        self.waiting
            .keys()
            .filter(|c| !self.running.contains(*c))
            .filter_map(|c| self.last.get(c).map(|at| *at + NOTIFY_EVERY))
            .min()
    }

    /// Takes the reads that may start now, as started.
    fn start(&mut self, now: Instant) -> Vec<(String, String)> {
        let ready = self.ready(now);
        for (channel_id, _) in &ready {
            self.waiting.remove(channel_id);
            self.running.insert(channel_id.clone());
            self.last.insert(channel_id.clone(), now);
        }
        ready
    }

    /// Starts the reads that may start.
    pub fn pump(&mut self, core: &Arc<Core>, key: &str, api: &Api, tasks: &mut JoinSet<Done>) {
        for (channel_id, server_id) in self.start(Instant::now()) {
            tasks.spawn(notify_missed(core.clone(), key.to_owned(), api.clone(), server_id, channel_id));
        }
    }

    /// A channel's read ended.
    pub fn finished(&mut self, channel_id: &str) {
        self.running.remove(channel_id);
    }
}

/// Reads one page of what a channel missed after its mark and notifies for
/// it as if it had come live, then moves the mark on to the newest read.
async fn notify_missed(core: Arc<Core>, key: String, api: Api, server_id: String, channel_id: String) -> Done {
    let done = Done::Notified(channel_id.clone());
    let after = core.shared.read(|s| s.instance(&key).and_then(|i| i.notified.get(&channel_id).cloned()));
    let req = pb::ListMessagesRequest {
        server_id: server_id.clone(),
        channel_id: channel_id.clone(),
        limit: NOTIFY_PAGE,
        after_id: after.clone().unwrap_or_default(),
        ..Default::default()
    };
    let Ok(res) = rpc!(api.messages(), list_messages(req)).await else { return done };
    let prefs = core.prefs();
    let notices = core.shared.update(|s| {
        let shown = s.focus_channel(&key) == Some(channel_id.as_str());
        let Some(i) = s.instances.get_mut(&key) else { return Vec::new() };
        if let Some(newest) = res.messages.iter().map(|m| &m.id).max()
            && i.notified.get(&channel_id).is_none_or(|mark| newest > mark)
        {
            i.notified.insert(channel_id.clone(), newest.clone());
        }
        if shown {
            return Vec::new();
        }
        for user in res.authors {
            i.users.insert(user.id.clone(), user);
        }
        store::add_shared_authors(&mut i.users, &res.messages);
        let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let suppress = i.effective_notifications(&server_id, &channel_id, dms::now_ms()).suppress_everyone;
        res.messages
            .iter()
            .filter(|m| after.as_ref().is_none_or(|after| m.id > *after))
            // Your own came whole, and so did mentions, which notified as they came.
            .filter(|m| m.author_id != me && !i.pings_me(&server_id, m, suppress))
            .filter(|m| m.thread_id.is_empty() || m.also_in_channel)
            .filter_map(|m| {
                super::sync::message_notice(i, &key, &server_id, &channel_id, None, m, m.created_at.as_ref(), &prefs)
            })
            .collect::<Vec<_>>()
    });
    for notice in notices {
        core.shared.notice(notice);
    }
    done
}

/// Reads a loaded channel's newest page again (and the thread open beside
/// it), merging it in, with its pins where they're open. Its other threads
/// that were loaded go, to be read afresh when opened, as on the web. A
/// channel never opened is left to load as ever.
async fn reread(core: Arc<Core>, key: String, api: Api, channel_id: String) -> Done {
    let found = core.shared.update(|s| {
        let open = (s.focus_channel(&key) == Some(channel_id.as_str())).then(|| s.focus_thread(&key)).flatten();
        let open = open.map(str::to_owned);
        let i = s.instances.get_mut(&key)?;
        if !i.messages.contains_key(&channel_id) {
            return None;
        }
        let sid = i.channels.iter().find(|(_, list)| list.iter().any(|c| c.id == channel_id))?.0.clone();
        let stale: Vec<String> = i
            .thread_parents
            .values()
            .filter(|p| p.channel_id == channel_id && Some(&p.id) != open.as_ref())
            .map(|p| threads::thread_key(&p.id))
            .collect();
        for at in stale {
            i.messages.remove(&at);
        }
        let thread = open.filter(|t| i.messages.contains_key(&threads::thread_key(t)));
        Some((sid, thread))
    });
    let Some((server_id, thread)) = found else { return Done::Nothing };
    for thread_id in [String::new()].into_iter().chain(thread.clone()) {
        let at = if thread_id.is_empty() { channel_id.clone() } else { threads::thread_key(&thread_id) };
        let before = core.shared.read(|s| s.instance(&key).and_then(|i| i.messages.get(&at)).map(marks));
        let Some(before) = before else { continue };
        let req = pb::ListMessagesRequest {
            server_id: server_id.clone(),
            channel_id: channel_id.clone(),
            limit: PAGE,
            thread_id,
            ..Default::default()
        };
        let Ok(res) = rpc!(api.messages(), list_messages(req)).await else { continue };
        core.shared.instance(&key, |i| {
            for user in res.authors {
                i.users.insert(user.id.clone(), user);
            }
            store::add_shared_authors(&mut i.users, &res.messages);
            if let Some(parent) = res.parent {
                store::add_shared_authors(&mut i.users, std::slice::from_ref(&parent));
                i.thread_parents.insert(parent.id.clone(), parent);
            }
            if let Some(entry) = i.messages.get_mut(&at) {
                let meanwhile = Meanwhile::since(&before, entry);
                merge_newest(entry, res.messages, res.has_more, &meanwhile);
            }
        });
    }
    // Pins moved out of focus came only as heads too.
    core.reload_pins(&key, &server_id, &channel_id, thread.as_deref().unwrap_or_default());
    Done::Nothing
}

/// Keeps one live connection's focus on what's on screen: worked out again
/// a moment after the store changes, sent only when it did, one call at a
/// time and never waited on. When the stream says a channel came into
/// focus, what was loaded of it is read again.
pub(super) struct Focuser {
    core: Arc<Core>,
    key: String,
    /// What the stream was opened with.
    opened: pb::Focus,
    /// What was asked for last.
    asked: pb::Focus,
    /// What the stream last said is in effect.
    in_effect: pb::Focus,
    /// The store changing: the focus is worked out again at `due`.
    pub changes: watch::Receiver<u64>,
    pub due: Option<tokio::time::Instant>,
    /// Hands focuses to the task that sends them, once the stream has an id.
    sender: Option<(watch::Sender<pb::Focus>, tokio::task::JoinHandle<()>)>,
}

impl Focuser {
    pub fn new(core: &Arc<Core>, key: &str, opened: pb::Focus) -> Self {
        let mut changes = core.changes();
        changes.mark_unchanged();
        Self {
            core: core.clone(),
            key: key.to_owned(),
            asked: opened.clone(),
            in_effect: opened.clone(),
            opened,
            changes,
            due: None,
            sender: None,
        }
    }

    /// The stream's id: focuses can go out now.
    pub fn connected(&mut self, api: &Api, id: String) {
        let live = store::LiveConnection { id: id.clone(), focus: self.opened.clone() };
        self.core.shared.instance(&self.key, |i| i.live = Some(live));
        let (tx, mut rx) = watch::channel(self.opened.clone());
        let api = api.clone();
        let task = tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let focus = rx.borrow_and_update().clone();
                let req = pb::FocusRequest { connection_id: id.clone(), focus: Some(focus) };
                match rpc!(api.live(), focus(req)).await {
                    // The stream ended: the next one opens with the focus.
                    Err(err) if err.code == Code::NotFound => {}
                    Err(_) => tracing::debug!("couldn't change a live connection's focus"),
                    Ok(_) => {}
                }
            }
        });
        if self.asked != self.opened {
            tx.send_replace(self.asked.clone());
        }
        if let Some((_, old)) = self.sender.replace((tx, task)) {
            old.abort();
        }
    }

    /// The store changed: the focus is looked at again shortly.
    pub fn changed(&mut self) {
        self.due.get_or_insert_with(|| tokio::time::Instant::now() + SETTLE);
    }

    /// Works the focus out again, and sends it if it's new.
    pub fn settle(&mut self) {
        self.due = None;
        let now = self.core.shared.read(|s| choose_focus(s, &self.key));
        if now == self.asked {
            return;
        }
        self.asked = now.clone();
        if let Some((tx, _)) = &self.sender {
            tx.send_replace(now);
        }
    }

    /// The stream says what's in focus now: channels new to it are read again,
    /// since what they said out of focus came only as heads.
    pub fn echoed(&mut self, api: &Api, now: pb::Focus, tasks: &mut JoinSet<Done>) {
        for id in now.channel_ids.iter().filter(|id| !self.in_effect.channel_ids.contains(id)) {
            tasks.spawn(reread(self.core.clone(), self.key.clone(), api.clone(), id.clone()));
        }
        self.core.shared.instance(&self.key, |i| {
            if let Some(live) = &mut i.live {
                live.focus = now.clone();
            }
        });
        self.in_effect = now;
    }

    /// Server events are ready (again): the channel on screen is read again,
    /// for whatever it missed while the stream was away.
    pub fn ready(&self, api: &Api, tasks: &mut JoinSet<Done>) {
        let shown = self.core.shared.read(|s| s.focus_channel(&self.key).map(str::to_owned));
        if let Some(id) = shown {
            tasks.spawn(reread(self.core.clone(), self.key.clone(), api.clone(), id));
        }
    }
}

impl Drop for Focuser {
    fn drop(&mut self) {
        if let Some((_, task)) = self.sender.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::store::{Focus, InView};

    fn message(id: &str, author: &str) -> pb::Message {
        pb::Message { id: id.into(), channel_id: "c1".into(), author_id: author.into(), ..Default::default() }
    }

    fn instance() -> InstanceState {
        let mut i = InstanceState::new("k", "https://k");
        i.me = Some(pb::User { id: "me".into(), ..Default::default() });
        let channel = |id: &str| pb::Channel { id: id.into(), server_id: "s".into(), ..Default::default() };
        i.channels.insert("s".into(), vec![channel("c1"), channel("c2")]);
        i
    }

    fn heads(channels: &[(&str, &str)]) -> pb::ChannelHeads {
        pb::ChannelHeads {
            servers: vec![pb::ServerHeads {
                server_id: "s".into(),
                sequence: 9,
                channels: channels
                    .iter()
                    .map(|(c, m)| pb::ChannelHead { channel_id: (*c).into(), last_message_id: (*m).into() })
                    .collect(),
            }],
        }
    }

    #[test]
    fn the_focus_is_the_channel_open_and_the_people_on_screen() {
        let mut store = Store::default();
        let mut i = instance();
        i.messages.insert(
            "c1".into(),
            ChannelMessages {
                items: vec![message("01", "ann"), message("02", "me"), message("03", "bo"), message("04", "ann")],
                ..Default::default()
            },
        );
        i.messages.insert(
            threads::thread_key("01"),
            ChannelMessages { items: vec![message("05", "cy")], ..Default::default() },
        );
        store.instances.insert("k".into(), i);
        // Nothing open here: nothing in focus.
        assert_eq!(choose_focus(&store, "k"), pb::Focus::default());

        store.focus = Some(Focus { instance: "k".into(), channel: "c1".into(), thread: Some("01".into()) });
        store.in_view = Some(InView {
            instance: "k".into(),
            server: "s".into(),
            people: vec!["dee".into(), "me".into(), "bad id!".into()],
        });
        let focus = choose_focus(&store, "k");
        assert_eq!(focus.channel_ids, ["c1"]);
        // The rows in sight and the authors of what's loaded, sorted; never you.
        assert_eq!(focus.user_ids, ["ann", "bo", "cy", "dee"]);
        // Someone already named writing again changes nothing.
        let again = message("06", "bo");
        store.instances.get_mut("k").unwrap().messages.get_mut("c1").unwrap().items.push(again);
        assert_eq!(choose_focus(&store, "k"), focus);

        // Another server's member list isn't on screen with this channel.
        store.in_view.as_mut().unwrap().server = "elsewhere".into();
        assert!(!choose_focus(&store, "k").user_ids.contains(&"dee".to_owned()));

        // Never more people than the instance takes.
        let crowd = (0..MAX_PEOPLE + 50).map(|n| format!("u{n}")).collect();
        store.in_view = Some(InView { instance: "k".into(), server: "s".into(), people: crowd });
        assert_eq!(choose_focus(&store, "k").user_ids.len(), MAX_PEOPLE);
    }

    #[test]
    fn heads_move_channels_on_and_count_them_unread() {
        let mut i = instance();
        i.messages.insert("c1".into(), ChannelMessages { items: vec![message("05", "ann")], ..Default::default() });
        let prefs = Prefs::default();
        // c1 has news; c2 is on screen; an old head changes nothing.
        let missed = apply_heads(&mut i, &heads(&[("c1", "07"), ("c2", "03")]), Some("c2"), &prefs, 0);
        assert!(missed.is_empty(), "only mentions notify by default");
        assert_eq!(i.unread.get("c1"), Some(&1));
        assert_eq!(i.unread.get("c2"), None);
        assert_eq!(store::newest_known(&i, "c1"), Some("07"));
        assert_eq!(store::newest_known(&i, "c2"), Some("03"));
        apply_heads(&mut i, &heads(&[("c1", "06")]), None, &prefs, 0);
        assert_eq!(i.unread.get("c1"), Some(&1));
        apply_heads(&mut i, &heads(&[("c1", "08")]), None, &prefs, 0);
        assert_eq!(i.unread.get("c1"), Some(&2));
    }

    #[test]
    fn channels_that_notify_for_everything_read_what_they_missed_once_per_head() {
        let mut i = instance();
        let all = pb::NotificationSettings { level: pb::NotificationLevel::All as i32, ..Default::default() };
        i.notifications.insert(notifications::key("s", "c1"), all);
        i.newest.insert("c1".into(), "04".into());
        let prefs = Prefs::default();
        let missed = apply_heads(&mut i, &heads(&[("c1", "07"), ("c2", "02")]), None, &prefs, 0);
        assert_eq!(
            missed,
            [Missed { server_id: "s".into(), channel_id: "c1".into() }],
            "c2 notifies only for mentions"
        );
        // Read from what was known before the head.
        assert_eq!(i.notified.get("c1").map(String::as_str), Some("04"));
        // The same head again reads nothing; a newer one reads from the mark still.
        assert!(apply_heads(&mut i, &heads(&[("c1", "07")]), None, &prefs, 0).is_empty());
        assert_eq!(apply_heads(&mut i, &heads(&[("c1", "09")]), None, &prefs, 0).len(), 1);
        assert_eq!(i.notified.get("c1").map(String::as_str), Some("04"));
        // Muted, or on screen: nothing to read.
        let muted = pb::NotificationSettings { muted: true, ..i.notifications[&notifications::key("s", "c1")].clone() };
        i.notifications.insert(notifications::key("s", "c1"), muted);
        assert!(apply_heads(&mut i, &heads(&[("c1", "10")]), None, &prefs, 0).is_empty());
        i.notifications.clear();
        let everything = Prefs { notify_for: crate::core::config::NotifyFor::All, ..Prefs::default() };
        assert!(apply_heads(&mut i, &heads(&[("c1", "11")]), Some("c1"), &everything, 0).is_empty());
        assert_eq!(apply_heads(&mut i, &heads(&[("c1", "12")]), None, &everything, 0).len(), 1);
    }

    #[test]
    fn a_whole_message_moves_the_mark_only_when_nothing_waits_to_be_read() {
        let mut i = instance();
        i.notified.insert("c1".into(), "04".into());
        i.newest.insert("c1".into(), "04".into());
        // Nothing held back: a whole message (one of yours, a mention) moves the mark with it.
        store::heard_of(&mut i, "c1", "05");
        assert_eq!(i.notified["c1"], "05");
        // A head moved past the mark: the held-back message still waits to be
        // read, so a whole mention after it leaves the mark where it was.
        let all = pb::NotificationSettings { level: pb::NotificationLevel::All as i32, ..Default::default() };
        i.notifications.insert(notifications::key("s", "c1"), all);
        apply_heads(&mut i, &heads(&[("c1", "06")]), None, &Prefs::default(), 0);
        store::heard_of(&mut i, "c1", "07");
        assert_eq!(i.notified["c1"], "05");
        assert_eq!(store::newest_known(&i, "c1"), Some("07"));
    }

    #[test]
    fn notifying_reads_go_a_few_at_once_and_far_enough_apart() {
        let missed = |c: &str| Missed { server_id: "s".into(), channel_id: c.into() };
        let mut n = Notifier::default();
        let now = Instant::now();
        n.want(vec![missed("a"), missed("b"), missed("c")]);
        let first: Vec<String> = n.start(now).into_iter().map(|(c, _)| c).collect();
        assert_eq!(first, ["a", "b"], "two at once");
        assert!(n.start(now).is_empty());
        assert_eq!(n.due(), None, "nothing can start while two run");
        // More heads for a channel being read gather into one later read.
        n.want(vec![missed("a"), missed("a")]);
        n.finished("a");
        let next: Vec<String> = n.start(now).into_iter().map(|(c, _)| c).collect();
        assert_eq!(next, ["c"], "a waits its turn, c doesn't");
        n.finished("b");
        n.finished("c");
        assert!(n.start(now + Duration::from_secs(9)).is_empty());
        assert_eq!(n.due(), Some(now + NOTIFY_EVERY));
        let later: Vec<String> = n.start(now + NOTIFY_EVERY).into_iter().map(|(c, _)| c).collect();
        assert_eq!(later, ["a"]);
        assert_eq!(n.due(), None);
    }

    #[test]
    fn a_page_read_again_merges_where_it_meets_and_replaces_across_a_gap() {
        let ids = |e: &ChannelMessages| e.items.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        let loaded = |list: &[&str]| ChannelMessages {
            items: list.iter().map(|id| message(id, "ann")).collect(),
            has_more: true,
            loading: false,
        };
        let page = |list: &[&str]| list.iter().map(|id| message(id, "bo")).collect::<Vec<_>>();
        let quiet = Meanwhile::default();

        // It meets what's loaded: what's new joins, and what's gone from the page's span goes.
        let mut entry = loaded(&["01", "02", "03", "04"]);
        merge_newest(&mut entry, page(&["03", "05", "06"]), true, &quiet);
        assert_eq!(ids(&entry), ["01", "02", "03", "05", "06"]);
        assert!(entry.has_more);

        // A gap: what was loaded can't be joined to it, so the page takes its place.
        let mut entry = loaded(&["01", "02"]);
        merge_newest(&mut entry, page(&["07", "08"]), true, &quiet);
        assert_eq!(ids(&entry), ["07", "08"]);
        assert!(entry.has_more);

        // The whole channel: it's all there is.
        let mut entry = loaded(&["01", "02", "03"]);
        merge_newest(&mut entry, page(&["02", "09"]), false, &quiet);
        assert_eq!(ids(&entry), ["02", "09"]);
        assert!(!entry.has_more);
    }

    #[test]
    fn what_came_live_during_the_read_wins_over_the_page() {
        let ids = |e: &ChannelMessages| e.items.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        let page = |list: &[&str]| list.iter().map(|id| message(id, "bo")).collect::<Vec<_>>();
        let set = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        let before = ChannelMessages {
            items: vec![message("01", "ann"), message("02", "ann"), message("03", "ann")],
            has_more: true,
            loading: false,
        };
        let marked = marks(&before);
        // While the page was read: 09 came, 02 was edited and 03 deleted.
        let mut entry = before.clone();
        entry.items.retain(|m| m.id != "03");
        entry.items[1].content = "edited".into();
        entry.items.push(message("09", "ann"));
        let meanwhile = Meanwhile::since(&marked, &entry);
        assert_eq!(meanwhile.changed, set(&["02", "09"]));
        assert_eq!(meanwhile.deleted, set(&["03"]));

        // Where they meet: the page's old copies don't undo the edit or the delete, and 09 stays.
        let mut merged = entry.clone();
        merge_newest(&mut merged, page(&["02", "03", "04"]), true, &meanwhile);
        assert_eq!(ids(&merged), ["01", "02", "04", "09"]);
        assert_eq!(merged.items[1].content, "edited");

        // Across a gap: the page takes the place of what was loaded, but not of what came live.
        let mut merged = entry.clone();
        merge_newest(&mut merged, page(&["05", "06"]), true, &meanwhile);
        assert_eq!(ids(&merged), ["02", "05", "06", "09"]);

        // An empty channel, as read: only what came live is left.
        let mut merged = entry.clone();
        merge_newest(&mut merged, Vec::new(), false, &meanwhile);
        assert_eq!(ids(&merged), ["02", "09"]);

        // Nothing came live, but something newer than the page is loaded: it stays.
        let mut merged = ChannelMessages { items: page(&["01", "08"]), has_more: true, loading: false };
        merge_newest(&mut merged, page(&["06", "07"]), true, &Meanwhile::default());
        assert_eq!(ids(&merged), ["06", "07", "08"]);
    }
}
