//! Read marks (docs/read-marks.md): how far someone has read in each place,
//! said by one of their devices and passed on to the others, so a channel,
//! thread or conversation read on one stops showing unread on the rest.
//! Kept in node.db (`NodeDb::mark_read`), where accounts are; this is the
//! part that checks marks and tells each account's live connections.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::error::{Error, Result};
use crate::id::{new_id, parse_id};
use crate::pb;

/// Marks one MarkRead may carry.
pub const MAX_MARKS: usize = 100;
/// Marks a live connection is sent again when it opens, at most: past that it
/// shows a few places unread that another device read, as before marks.
pub const MAX_REPLAY: i64 = 2_000;
/// Batches of marks a connection may fall behind on before it misses some.
const BUFFER: usize = 256;

/// Checks marks from MarkRead and puts them in the form they're kept in: a
/// place's id canonical, and an empty message id with no sequence (everything
/// up to now) filled in with an id that sorts after every message sent so far.
pub fn check(marks: Vec<pb::ReadMark>) -> Result<Vec<pb::ReadMark>> {
    if marks.is_empty() {
        return Err(Error::invalid("say where you read"));
    }
    if marks.len() > MAX_MARKS {
        return Err(Error::invalid(format!("at most {MAX_MARKS} read marks at a time")));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(marks.len());
    for mark in marks {
        let place = match mark.place.split_once('/') {
            None => parse_id("place", &mark.place)?,
            // A thread in a secure channel: the channel, and the record it's under.
            Some((channel, record)) => {
                let record: i64 = record.parse().map_err(|_| Error::invalid("that place doesn't exist"))?;
                if record <= 0 {
                    return Err(Error::invalid("that place doesn't exist"));
                }
                format!("{}/{record}", parse_id("place", channel)?)
            }
        };
        if !seen.insert(place.clone()) {
            return Err(Error::invalid("each place at most once"));
        }
        if mark.sequence < 0 {
            return Err(Error::invalid("a read mark's sequence can't be negative"));
        }
        let message_id = match (mark.message_id.is_empty(), mark.sequence) {
            (true, 0) => new_id(),
            (true, _) => String::new(),
            (false, 0) => parse_id("message_id", &mark.message_id)?,
            (false, _) => return Err(Error::invalid("a read mark is a message or a sequence, not both")),
        };
        out.push(pb::ReadMark { place, message_id, sequence: mark.sequence, at_ms: 0 });
    }
    Ok(out)
}

/// Each account's live connections that asked for read marks, where accounts
/// are. Memory only: a connection that was away asks node.db for what it
/// missed (`OpenRequest.reads_after`).
#[derive(Default)]
pub struct Reads {
    watchers: Mutex<HashMap<String, broadcast::Sender<Arc<Vec<pb::ReadMark>>>>>,
}

impl Reads {
    /// The marks someone's devices make from now on.
    pub fn watch(&self, account_id: &str) -> broadcast::Receiver<Arc<Vec<pb::ReadMark>>> {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        watchers.entry(account_id.to_string()).or_insert_with(|| broadcast::channel(BUFFER).0).subscribe()
    }

    /// Tells someone's connections of marks that moved.
    pub fn publish(&self, account_id: &str, marks: Vec<pb::ReadMark>) {
        if marks.is_empty() {
            return;
        }
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        let idle = match watchers.get(account_id) {
            None => return,
            Some(sender) => sender.receiver_count() == 0 || sender.send(Arc::new(marks)).is_err(),
        };
        if idle {
            watchers.remove(account_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(place: &str, message_id: &str, sequence: i64) -> pb::ReadMark {
        pb::ReadMark { place: place.into(), message_id: message_id.into(), sequence, at_ms: 0 }
    }

    #[test]
    fn checks_places_and_fills_in_now() {
        let channel = new_id();
        let message = new_id();
        let marks = check(vec![
            mark(&channel.to_lowercase(), "", 0),
            mark(&format!("{channel}/7"), "", 7),
            mark(&message, &message, 0),
        ])
        .unwrap();
        assert_eq!(marks[0].place, channel);
        assert!(marks[0].message_id > message, "now sorts after every message so far");
        assert_eq!(marks[1].message_id, "");
        assert_eq!(marks[2].message_id, message);
    }

    #[test]
    fn refuses_what_isnt_a_place() {
        let id = new_id();
        assert!(check(vec![]).is_err());
        assert!(check(vec![mark("nope", "", 0)]).is_err());
        assert!(check(vec![mark(&format!("{id}/0"), "", 1)]).is_err());
        assert!(check(vec![mark(&format!("{id}/x"), "", 1)]).is_err());
        assert!(check(vec![mark(&id, "", -1)]).is_err());
        assert!(check(vec![mark(&id, &id, 3)]).is_err());
        assert!(check(vec![mark(&id, "", 0), mark(&id, "", 0)]).is_err());
        assert!(check((0..=MAX_MARKS).map(|_| mark(&new_id(), "", 0)).collect()).is_err());
    }
}
