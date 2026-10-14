//! Typing indicators (docs/typing.md): someone typing in a channel, a
//! thread's reply box or a direct-message conversation. Nothing is stored:
//! each signal goes out live once, and apps show the person as typing for
//! [`SHOWN_FOR_MS`] or until their message arrives.
//!
//! Apps send one at most every 6 seconds while someone types; this keeps a
//! burst from fanning out more than once per person and place every
//! [`PASS_EVERY_MS`], the rest dropped without an error.

use std::collections::HashMap;
use std::sync::Mutex;

/// How long apps show someone as typing after a signal.
pub const SHOWN_FOR_MS: i64 = 10_000;
/// Least time between two signals passed on for one person in one place.
pub const PASS_EVERY_MS: i64 = 3_000;

/// When each person's last signal in each place went out, by
/// `place/account`, kept in memory only.
static PASSED: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);

/// Whether `account_id`'s signal in `place` at `now` goes out, noting it
/// if so.
pub fn pass(place: &str, account_id: &str, now: i64) -> bool {
    let mut guard = PASSED.lock().unwrap_or_else(|e| e.into_inner());
    pass_in(guard.get_or_insert_with(HashMap::new), place, account_id, now)
}

/// [`pass`] against the times given.
fn pass_in(passed: &mut HashMap<String, i64>, place: &str, account_id: &str, now: i64) -> bool {
    // Entries older than a signal's life say nothing any more.
    if passed.len() > 10_000 {
        passed.retain(|_, at| now - *at < SHOWN_FOR_MS);
    }
    let key = format!("{place}/{account_id}");
    match passed.get(&key) {
        Some(at) if now - *at < PASS_EVERY_MS && now >= *at => false,
        _ => {
            passed.insert(key, now);
            true
        }
    }
}

/// Whether `id` looks like one of the instance's ids, for a thread named in
/// a signal that's passed on without being looked up.
pub fn id_like(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_signal_per_person_and_place_every_few_seconds() {
        let mut passed = HashMap::new();
        assert!(pass_in(&mut passed, "s/c", "ana", 1_000));
        assert!(!pass_in(&mut passed, "s/c", "ana", 2_000), "a burst is folded");
        assert!(pass_in(&mut passed, "s/c", "ben", 2_000), "someone else is their own");
        assert!(pass_in(&mut passed, "s/c/t", "ana", 2_000), "a thread is its own place");
        assert!(pass_in(&mut passed, "s/c", "ana", 1_000 + PASS_EVERY_MS), "and again once it's due");
    }

    #[test]
    fn a_clock_going_back_never_holds_signals_back() {
        let mut passed = HashMap::new();
        assert!(pass_in(&mut passed, "d", "ana", 50_000));
        assert!(pass_in(&mut passed, "d", "ana", 10_000));
    }

    #[test]
    fn old_entries_are_swept_once_there_are_many() {
        let mut passed = HashMap::new();
        for i in 0..=10_000 {
            pass_in(&mut passed, "p", &i.to_string(), 0);
        }
        assert!(pass_in(&mut passed, "p", "new", SHOWN_FOR_MS));
        assert_eq!(passed.len(), 1);
    }

    #[test]
    fn thread_ids_must_look_like_ids() {
        assert!(id_like("01JABCDEF"));
        assert!(!id_like(""));
        assert!(!id_like("a/b"));
        assert!(!id_like(&"a".repeat(33)));
    }
}
