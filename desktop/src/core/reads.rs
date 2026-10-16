//! Read marks on every device (docs/read-marks.md): what you read here goes
//! to the instance, which passes it to your other devices' live connections,
//! and what you read there clears unread counts here. Channels and threads
//! are marked "up to now" (the instance fills in the id); conversations and
//! secure channels by the last record this device noted it read. A port of
//! `web/src/fuwa/reads.ts`; the pure half is `read_marks.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::core::api::Api;
use crate::core::read_marks;
use crate::core::{Area, Core, Shared};
use crate::pb;
use crate::rpc;

/// How long marks wait to go out together, so a busy channel on screen sends one now and then, not one per message.
const SEND_AFTER: Duration = Duration::from_millis(1500);
/// As many as one MarkRead takes.
const PER_CALL: usize = 100;

/// The marks waiting to go out, and whether the window is in front.
pub struct Outbox {
    /// Per instance, by place: the record read up to, or 0 for "up to now".
    queued: Mutex<HashMap<String, HashMap<String, i64>>>,
    looking: AtomicBool,
}

impl Default for Outbox {
    fn default() -> Self {
        Self { queued: Mutex::new(HashMap::new()), looking: AtomicBool::new(true) }
    }
}

impl Outbox {
    /// Adds a mark. True when it's the first for its instance since the last
    /// send, so a send has to be started.
    fn queue(&self, key: &str, place: &str, sequence: i64) -> bool {
        let mut queued = self.queued.lock();
        let start = !queued.contains_key(key);
        let at = queued.entry(key.to_owned()).or_default().entry(place.to_owned()).or_insert(-1);
        *at = (*at).max(sequence);
        start
    }

    /// The instance's marks waiting to go out, taken.
    fn take(&self, key: &str) -> Vec<pb::ReadMark> {
        let places = self.queued.lock().remove(key).unwrap_or_default();
        let mut marks: Vec<pb::ReadMark> = places
            .into_iter()
            .map(|(place, sequence)| pb::ReadMark { place, sequence, ..Default::default() })
            .collect();
        marks.sort_by(|a, b| a.place.cmp(&b.place));
        marks
    }

    /// Whether you're looking at the app (its window is in front).
    pub fn looking(&self) -> bool {
        self.looking.load(Ordering::Relaxed)
    }
}

/// Whether marks go out from this instance: signed in, and it has them.
fn marks_here(shared: &Shared, key: &str) -> bool {
    shared.read(|s| s.instance(key).is_some_and(|i| i.me.is_some() && i.has("read-marks")))
}

/// Queues a mark for `place`, up to now (`sequence` 0) or up to a record,
/// sent with the others queued in the next moment.
pub(crate) fn note(
    shared: &Shared,
    api: &Api,
    key: &str,
    place: &str,
    sequence: i64,
    runtime: &tokio::runtime::Handle,
) {
    if place.is_empty() || !marks_here(shared, key) {
        return;
    }
    if shared.reads.queue(key, place, sequence) {
        runtime.spawn(send(shared.clone(), api.clone(), key.to_owned()));
    }
}

async fn send(shared: Shared, api: Api, key: String) {
    tokio::time::sleep(SEND_AFTER).await;
    let marks = shared.reads.take(&key);
    if marks.is_empty() || !marks_here(&shared, &key) {
        return;
    }
    for chunk in marks.chunks(PER_CALL) {
        // Not passed on when it fails; the next read there will be.
        let Ok(res) = rpc!(api.account(), mark_read(pb::MarkReadRequest { marks: chunk.to_vec() })).await else {
            continue;
        };
        // As kept: the id the instance filled in, so what came before stops counting here too.
        shared.instance_if(&key, &[Area::All], |i| read_marks::with_read_marks(i, &res.marks));
    }
}

impl Core {
    /// You read a channel or a thread (by the id of the message it's under)
    /// up to now, or a conversation or secure channel up to a record.
    fn note_read(&self, key: &str, place: &str, sequence: i64) {
        if let Some(api) = self.api(key) {
            note(&self.shared, &api, key, place, sequence, self.runtime.handle());
        }
    }

    /// The channel and thread on screen, read up to now, while the window is
    /// in front: on focus, on coming back to the app, as messages arrive in them.
    pub(crate) fn note_focus_read(&self) {
        if !self.shared.reads.looking() {
            return;
        }
        let Some((key, channel, thread)) = self.shared.read(|s| {
            let f = s.focus.as_ref()?;
            let i = s.instance(&f.instance)?;
            // A server's channel (conversations and secure channels note their own).
            let c = i.channels.values().flatten().find(|c| c.id == f.channel)?;
            (c.r#type != pb::ChannelType::Secure as i32).then(|| (f.instance.clone(), c.id.clone(), f.thread.clone()))
        }) else {
            return;
        };
        self.note_read(&key, &channel, 0);
        if let Some(thread) = thread {
            self.note_read(&key, &thread, 0);
        }
    }

    /// The window came to the front or went behind others.
    pub fn window_active(&self, active: bool) {
        let was = self.shared.reads.looking.swap(active, Ordering::Relaxed);
        if active && !was {
            self.note_focus_read();
        }
    }

    /// A message came live: in the channel or thread on screen, that's read too.
    pub(crate) fn note_arrived(&self, key: &str, m: &pb::Message) {
        let on_screen = self.shared.read(|s| {
            let Some(f) = s.focus.as_ref().filter(|f| f.instance == key) else { return false };
            let mine = s.instance(key).and_then(|i| i.me.as_ref()).is_some_and(|me| me.id == m.author_id);
            !mine
                && (m.channel_id == f.channel || (!m.thread_id.is_empty() && f.thread.as_deref() == Some(&m.thread_id)))
        });
        if on_screen {
            self.note_focus_read();
        }
    }

    /// You marked channels or threads read by hand (Mark as read): up to now, wherever they are.
    pub(crate) fn note_marked_read(&self, key: &str, places: &[String]) {
        for place in places {
            self.note_read(key, place, 0);
        }
    }

    /// Marks heard on the live connection: from every device of yours, this
    /// one's too. Gives where to ask from next time.
    pub(crate) fn take_read_marks(&self, key: &str, reads: &pb::ReadMarks, since: i64) -> i64 {
        self.shared.instance_if(key, &[Area::All], |i| read_marks::with_read_marks(i, &reads.marks));
        let records: Vec<(String, i64)> = reads
            .marks
            .iter()
            .filter(|m| !read_marks::by_message(m) && m.sequence > 0)
            .map(|m| (m.place.clone(), m.sequence))
            .collect();
        if !records.is_empty()
            && let Some(engine) = self.dm_engine(key)
        {
            self.runtime.spawn(async move {
                for (place, sequence) in records {
                    engine.read_elsewhere(&place, sequence).await;
                }
            });
        }
        read_marks::newest_mark(&reads.marks, since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_wait_together_per_instance_and_keep_the_furthest() {
        let outbox = Outbox::default();
        assert!(outbox.queue("k", "c1", 0), "the first starts a send");
        assert!(!outbox.queue("k", "dm", 4));
        assert!(!outbox.queue("k", "dm", 2), "an older record changes nothing");
        assert!(outbox.queue("other", "c9", 0), "each instance sends its own");
        let marks = outbox.take("k");
        let places: Vec<(&str, i64)> = marks.iter().map(|m| (m.place.as_str(), m.sequence)).collect();
        assert_eq!(places, [("c1", 0), ("dm", 4)]);
        assert!(outbox.take("k").is_empty());
        assert!(outbox.queue("k", "c1", 0), "after a send, the next mark starts another");
    }
}
