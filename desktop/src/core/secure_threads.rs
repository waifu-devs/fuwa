//! Threads inside a secure channel, worked out on the device (the web's
//! `e2ee/threads.ts`). The server can't read the channel, so it can't sum
//! threads up the way it does for ordinary channels (docs/threads.md): a
//! reply carries the record of the message it's under inside the
//! encryption, and every device sorts the lines it opened into the channel
//! and its threads itself. Nothing here asks the server for anything: what
//! isn't on this device stays unknown, so the server never learns which
//! records make up a thread.

use std::collections::{BTreeMap, HashMap};

use crate::core::vault::{Item, ItemKind, Note};

/// A line once it's deleted: no text or files, and no signed copies either,
/// since those hold the plaintext too.
pub fn emptied(i: &Item) -> Item {
    let mut gone = i.clone();
    gone.deleted = true;
    gone.content.clear();
    gone.signed = None;
    gone.edit_signed = None;
    gone.files.clear();
    gone
}

/// What a thread is, as this device can tell from the lines it has.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecureThread {
    /// The record of the message it's under.
    pub parent: i64,
    pub replies: u32,
    /// When the last reply was sent (Unix ms), and its record; 0 with none.
    pub last_at: i64,
    pub last_seq: i64,
    /// The five latest people to reply, the latest first.
    pub participants: Vec<String>,
    pub locked: bool,
}

/// A secure channel's lines, sorted.
#[derive(Debug, Clone, Default)]
pub struct Organized {
    /// What the channel itself shows: everything but replies kept to their threads, and lock changes.
    pub channel: Vec<Item>,
    /// Threads by the record of their message.
    pub threads: BTreeMap<i64, SecureThread>,
    /// Each thread's replies and lock changes, oldest first.
    pub in_thread: HashMap<i64, Vec<Item>>,
}

const PARTICIPANTS: usize = 5;

fn thread(threads: &mut BTreeMap<i64, SecureThread>, parent: i64) -> &mut SecureThread {
    threads.entry(parent).or_insert_with(|| SecureThread { parent, ..Default::default() })
}

/// A line that can have a thread under it: text someone wrote, still there, not itself a reply.
pub fn can_have_thread(i: Option<&Item>) -> bool {
    i.is_some_and(|i| i.kind == ItemKind::Text && !i.deleted && i.thread == 0)
}

/// The thread a line replies in, or 0 if it shows as an ordinary line: no
/// thread named, or the line it names is here and can't have one (a device
/// line, another reply, or a later record). A thread whose message isn't on
/// this device still counts: its message may come later, or never did.
pub fn thread_of(i: &Item, by_seq: &HashMap<i64, &Item>) -> i64 {
    let parent = i.thread;
    if parent <= 0 || parent >= i.seq || !matches!(i.kind, ItemKind::Text | ItemKind::Thread) {
        return 0;
    }
    if let Some(p) = by_seq.get(&parent)
        && (p.kind != ItemKind::Text || p.thread != 0)
    {
        return 0;
    }
    parent
}

/// Each line by its record.
pub fn by_seq(items: &[Item]) -> HashMap<i64, &Item> {
    items.iter().map(|i| (i.seq, i)).collect()
}

/// Sorts a secure channel's lines into the channel and its threads. A lock
/// change counts only from someone `moderates` says has Manage Messages in
/// the channel (the device's own view of its permissions); the latest wins.
pub fn organize(items: &[Item], moderates: &dyn Fn(&str) -> bool) -> Organized {
    let seqs = by_seq(items);
    let mut out = Organized::default();
    for i in items {
        let parent = thread_of(i, &seqs);
        if i.kind == ItemKind::Thread {
            // A lock on a message that's gone, or that this device never had, makes no thread of its own.
            let Some(p) = seqs.get(&parent) else { continue };
            if parent == 0 || !moderates(&i.sender_id) || p.deleted {
                continue;
            }
            thread(&mut out.threads, parent).locked = i.content == "locked";
            out.in_thread.entry(parent).or_default().push(i.clone());
            continue;
        }
        if parent == 0 {
            out.channel.push(i.clone());
            continue;
        }
        // A thread goes with its message.
        if seqs.get(&parent).is_some_and(|p| p.deleted) {
            continue;
        }
        out.in_thread.entry(parent).or_default().push(i.clone());
        if i.in_channel {
            out.channel.push(i.clone());
        }
        if i.deleted {
            continue;
        }
        let t = thread(&mut out.threads, parent);
        t.replies += 1;
        t.last_at = i.at;
        t.last_seq = i.seq;
        t.participants.retain(|p| *p != i.sender_id);
        t.participants.insert(0, i.sender_id.clone());
        t.participants.truncate(PARTICIPANTS);
    }
    out.threads.retain(|parent, t| t.replies > 0 || (t.locked && can_have_thread(seqs.get(parent).copied())));
    out
}

/// Whether you follow a thread: as you set it by hand, or else if you wrote its message or replied in it.
pub fn following(note: Option<&Note>, parent: i64, items: &[Item], me: &str) -> bool {
    if let Some(set) = note.and_then(|n| n.follows.get(&parent)) {
        return *set;
    }
    items.iter().any(|i| i.sender_id == me && i.kind == ItemKind::Text && (i.seq == parent || i.thread == parent))
}

/// Replies in a thread you haven't seen yet: others', still there, after the last one you saw.
pub fn unread_in(note: Option<&Note>, parent: i64, replies: &[Item], me: &str) -> u32 {
    let seen = note.and_then(|n| n.thread_read.get(&parent)).copied().unwrap_or(0);
    replies.iter().filter(|i| i.kind == ItemKind::Text && !i.deleted && i.sender_id != me && i.seq > seen).count()
        as u32
}

/// Whether nobody has replied for longer than the server keeps threads open (0 hours: never archived).
pub fn archived(t: Option<&SecureThread>, hours: i32, now: i64) -> bool {
    t.is_some_and(|t| hours > 0 && t.last_at > 0 && now - t.last_at > i64::from(hours) * 3_600_000)
}

/// Replies to drop from this device because the message they're under was
/// deleted: the thread goes with its message. Nothing is deleted on the
/// server (a burst of deletes would tell it which records were replies); the
/// replies' ciphertext stays there, unreadable, and is never passed on.
pub fn orphaned<'a>(items: impl IntoIterator<Item = &'a Item>, by_seq: &HashMap<i64, &Item>) -> Vec<Item> {
    items
        .into_iter()
        .filter(|i| i.kind == ItemKind::Text && !i.deleted && i.thread != 0)
        .filter(|i| by_seq.get(&i.thread).is_some_and(|p| p.deleted))
        .map(emptied)
        .collect()
}

/// Threads whose message or replies say `query`, newest reply first.
pub fn search<'a>(org: &'a Organized, by_seq: &HashMap<i64, &Item>, query: &str) -> Vec<&'a SecureThread> {
    let q = query.trim().to_lowercase();
    let mut all: Vec<&SecureThread> = org.threads.values().collect();
    all.sort_by(|a, b| b.last_seq.cmp(&a.last_seq).then(b.parent.cmp(&a.parent)));
    if q.is_empty() {
        return all;
    }
    let says = |i: Option<&Item>| i.is_some_and(|i| !i.deleted && i.content.to_lowercase().contains(&q));
    all.into_iter()
        .filter(|t| {
            says(by_seq.get(&t.parent).copied())
                || org.in_thread.get(&t.parent).is_some_and(|lines| lines.iter().any(|i| says(Some(i))))
        })
        .collect()
}

/// Lines that count toward the channel's unread: replies kept to their threads count in their threads.
pub fn in_channel(i: &Item, by_seq: &HashMap<i64, &Item>) -> bool {
    thread_of(i, by_seq) == 0 || i.in_channel
}

/// Text for previews: no Markdown marks, one line.
pub fn plain(content: &str) -> String {
    let stripped: String = content.chars().filter(|c| !matches!(c, '*' | '_' | '~' | '`' | '>' | '#')).collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(seq: i64, sender: &str, thread: i64) -> Item {
        let mut i = Item::new(seq, ItemKind::Text, seq * 1000, sender, "d");
        i.content = format!("line {seq}");
        i.thread = thread;
        i
    }

    fn lock(seq: i64, sender: &str, parent: i64, locked: bool) -> Item {
        let mut i = Item::new(seq, ItemKind::Thread, seq * 1000, sender, "d");
        i.thread = parent;
        i.content = if locked { "locked" } else { "unlocked" }.into();
        i
    }

    #[test]
    fn replies_go_to_their_thread_and_also_sent_ones_to_the_channel_too() {
        let mut also = text(4, "b", 1);
        also.in_channel = true;
        let items = vec![text(1, "a", 0), text(2, "b", 1), text(3, "c", 1), also, text(5, "a", 0)];
        let org = organize(&items, &|_| false);
        assert_eq!(org.channel.iter().map(|i| i.seq).collect::<Vec<_>>(), vec![1, 4, 5]);
        let t = &org.threads[&1];
        assert_eq!((t.replies, t.last_seq), (3, 4));
        assert_eq!(t.participants, vec!["b".to_owned(), "c".to_owned()]);
        assert_eq!(org.in_thread[&1].len(), 3);
    }

    #[test]
    fn a_reply_to_a_reply_or_a_later_line_is_an_ordinary_line() {
        let items = vec![text(1, "a", 0), text(2, "b", 1), text(3, "c", 2), text(4, "c", 9)];
        let org = organize(&items, &|_| false);
        assert_eq!(org.channel.iter().map(|i| i.seq).collect::<Vec<_>>(), vec![1, 3, 4]);
    }

    #[test]
    fn locks_count_only_from_moderators_and_the_latest_wins() {
        let items = vec![text(1, "a", 0), lock(2, "mod", 1, true), lock(3, "x", 1, false), lock(4, "mod", 1, false)];
        let org = organize(&items, &|u| u == "mod");
        // Unlocked again, with no replies: no thread.
        assert!(org.threads.is_empty());
        let org = organize(&items[..3], &|u| u == "mod");
        assert!(org.threads[&1].locked);
        assert_eq!(org.channel.len(), 1);
    }

    #[test]
    fn a_deleted_message_takes_its_thread() {
        let mut parent = text(1, "a", 0);
        parent.deleted = true;
        let items = vec![parent, text(2, "b", 1)];
        let seqs = by_seq(&items);
        assert_eq!(orphaned(&items, &seqs).len(), 1);
        assert!(orphaned(&items, &seqs)[0].deleted);
        assert!(organize(&items, &|_| false).threads.is_empty());
    }

    #[test]
    fn following_and_unread() {
        let items = vec![text(1, "me", 0), text(2, "b", 1), text(3, "b", 1)];
        assert!(following(None, 1, &items, "me"));
        let mut note = Note::default();
        note.follows.insert(1, false);
        assert!(!following(Some(&note), 1, &items, "me"));
        note.thread_read.insert(1, 2);
        assert_eq!(unread_in(Some(&note), 1, &items[1..], "me"), 1);
        assert_eq!(unread_in(None, 1, &items[1..], "me"), 2);
    }

    #[test]
    fn search_reads_messages_and_replies() {
        let mut reply = text(2, "b", 1);
        reply.content = "Sakura petals".into();
        let items = vec![text(1, "a", 0), reply, text(3, "a", 0), text(4, "b", 3)];
        let org = organize(&items, &|_| false);
        let seqs = by_seq(&items);
        assert_eq!(search(&org, &seqs, "").iter().map(|t| t.parent).collect::<Vec<_>>(), vec![3, 1]);
        assert_eq!(search(&org, &seqs, "sakura").iter().map(|t| t.parent).collect::<Vec<_>>(), vec![1]);
        assert!(archived(org.threads.get(&1), 1, 2000 + 3_600_001));
        assert!(!archived(org.threads.get(&1), 0, i64::MAX));
    }

    #[test]
    fn previews_lose_their_marks() {
        assert_eq!(plain("**hi**  _there_\n> quote"), "hi there quote");
    }
}
