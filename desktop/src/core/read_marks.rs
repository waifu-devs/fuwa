//! The pure half of read marks (docs/read-marks.md): how far you've read in a
//! channel or thread, heard from any of your devices, clears its unread count
//! here and keeps what came before it from counting again. Conversations and
//! secure channels go by sequence and are the encryption engine's (their
//! counts come from what each device notes it read). `reads.rs` sends and
//! takes the marks. A port of `web/src/fuwa/read-marks.ts`.

use crate::core::store::InstanceState;
use crate::pb;

/// Whether a mark is for a channel or thread (a message id), not a conversation (a sequence).
pub fn by_message(mark: &pb::ReadMark) -> bool {
    !mark.message_id.is_empty() && mark.sequence == 0
}

/// Channels' and threads' marks, as kept here: their unread counts go, and
/// what they cover won't count again. Whether anything changed.
pub fn with_read_marks(i: &mut InstanceState, marks: &[pb::ReadMark]) -> bool {
    let mut changed = false;
    for mark in marks.iter().filter(|m| by_message(m)) {
        if i.read_marks.get(&mark.place).is_none_or(|kept| *kept < mark.message_id) {
            i.read_marks.insert(mark.place.clone(), mark.message_id.clone());
            changed = true;
        }
        // A place is a channel or a thread (the message it's under), never both.
        changed |= i.unread.remove(&mark.place).is_some_and(|n| n > 0);
        changed |= i.thread_unread.remove(&mark.place).is_some_and(|n| n > 0);
    }
    changed
}

/// Whether a message in a channel or thread was read already, on this device or another.
pub fn read_already(i: &InstanceState, place: &str, message_id: &str) -> bool {
    i.read_marks.get(place).is_some_and(|kept| message_id <= kept.as_str())
}

/// The newest `at_ms` among marks, or `since` when none is newer: where a connection asks for marks from next time.
pub fn newest_mark(marks: &[pb::ReadMark], since: i64) -> i64 {
    marks.iter().map(|m| m.at_ms).fold(since, i64::max)
}

/// A conversation's or secure channel's place split into its id and, for a
/// thread in a secure channel ("<id>/<record it's under>"), that record; 0 otherwise.
pub fn split_place(place: &str) -> (&str, i64) {
    match place.split_once('/') {
        Some((id, under)) => (id, under.parse().unwrap_or(0)),
        None => (place, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_id(place: &str, id: &str) -> pb::ReadMark {
        pb::ReadMark { place: place.into(), message_id: id.into(), ..Default::default() }
    }

    #[test]
    fn message_marks_clear_counts_and_only_move_forward() {
        let mut i = InstanceState::new("k", "https://k");
        i.unread.insert("c1".into(), 3);
        i.unread.insert("c2".into(), 1);
        i.thread_unread.insert("t1".into(), 2);
        let record = pb::ReadMark { place: "dm".into(), sequence: 7, ..Default::default() };
        assert!(with_read_marks(&mut i, &[by_id("c1", "05"), by_id("t1", "09"), record]));
        assert_eq!(i.unread.get("c1"), None);
        assert_eq!(i.unread.get("c2"), Some(&1));
        assert!(i.thread_unread.is_empty());
        // Records are the encryption engine's.
        assert!(!i.read_marks.contains_key("dm"));
        // An older mark leaves the newer one.
        assert!(!with_read_marks(&mut i, &[by_id("c1", "03")]));
        assert_eq!(i.read_marks["c1"], "05");
        assert!(read_already(&i, "c1", "05") && read_already(&i, "c1", "04"));
        assert!(!read_already(&i, "c1", "06") && !read_already(&i, "c2", "01"));
    }

    #[test]
    fn where_to_ask_from_and_which_thread() {
        let at = |at_ms| pb::ReadMark { at_ms, ..Default::default() };
        assert_eq!(newest_mark(&[at(5), at(9), at(2)], 3), 9);
        assert_eq!(newest_mark(&[], 3), 3);
        assert_eq!(split_place("chan"), ("chan", 0));
        assert_eq!(split_place("chan/12"), ("chan", 12));
    }
}
