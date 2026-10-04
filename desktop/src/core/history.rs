//! Which entries of a secure channel's shared history a device takes: the
//! web app's `web/src/e2ee/history.ts`. An entry's signature has already been
//! checked against the key it carries; this decides whether that key, and
//! where the sharer placed the entry, can be trusted.

use std::collections::{BTreeMap, HashSet};

/// One shared entry, as its signed payload says, placed by the sharer at `seq`.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub seq: i64,
    pub sender_id: String,
    /// The device whose key signed it.
    pub device_id: String,
    /// An edit names the message it changes, inside what was signed.
    pub edit_of: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Message,
    Settings,
    Other,
}

/// What the channel's log says about a record, read from its header: nothing decrypted.
#[derive(Debug, Clone)]
pub struct Logged {
    pub kind: LogKind,
    pub sender_id: String,
    pub device_id: String,
    pub deleted: bool,
    /// For a settings record: whether it turned sharing on.
    pub on: bool,
}

pub struct Context<'a> {
    /// The sequence this device joined at: only what came before it is taken.
    pub joined: i64,
    /// Who is in the channel.
    pub allowed: &'a [String],
    /// `user/device` for every device registered to its owner now.
    pub devices: &'a HashSet<String>,
    /// The log between the earliest entry and `joined`.
    pub log: &'a BTreeMap<i64, Logged>,
}

/// Indexes of the entries to take, in order:
/// - signed by a device registered to the person named as sender (anyone can
///   sign with a key of their own, so a key that isn't theirs proves nothing);
/// - the sender is in the channel;
/// - before this device joined, and after sharing was last turned on, so what
///   was written while it was off stays with those who were there;
/// - the log has a message at that place, from that sender (and, for new text,
///   that very device), not deleted, so a sharer can't move, repeat or bring
///   back messages;
/// - each message at most once, and an edit only of a message taken here.
pub fn accept(entries: &[Candidate], ctx: &Context) -> Vec<usize> {
    let mut since = 0;
    let mut on = true;
    for (&seq, r) in ctx.log {
        if r.kind == LogKind::Settings && seq < ctx.joined && seq > since {
            since = seq;
            on = r.on;
        }
    }
    if !on {
        return Vec::new();
    }
    let mut taken = HashSet::new();
    let mut kept = Vec::new();
    for (n, e) in entries.iter().enumerate() {
        if e.seq <= since || e.seq >= ctx.joined {
            continue;
        }
        if !ctx.allowed.contains(&e.sender_id) || !ctx.devices.contains(&format!("{}/{}", e.sender_id, e.device_id)) {
            continue;
        }
        let Some(r) = ctx.log.get(&e.seq) else { continue };
        if r.kind != LogKind::Message || r.deleted || r.sender_id != e.sender_id {
            continue;
        }
        match e.edit_of {
            None => {
                if r.device_id != e.device_id || !taken.insert(e.seq) {
                    continue;
                }
            }
            Some(target) => {
                if target != e.seq || !taken.contains(&e.seq) {
                    continue;
                }
            }
        }
        kept.push(n);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(sender: &str, device: &str, deleted: bool) -> Logged {
        Logged { kind: LogKind::Message, sender_id: sender.into(), device_id: device.into(), deleted, on: false }
    }

    fn other(kind: LogKind, sender: &str, on: bool) -> Logged {
        Logged { kind, sender_id: sender.into(), device_id: "j1".into(), deleted: false, on }
    }

    fn text(seq: i64, sender: &str, device: &str) -> Candidate {
        Candidate { seq, sender_id: sender.into(), device_id: device.into(), edit_of: None }
    }

    fn edit(seq: i64, sender: &str, device: &str, target: i64) -> Candidate {
        Candidate { seq, sender_id: sender.into(), device_id: device.into(), edit_of: Some(target) }
    }

    fn run(log: Vec<(i64, Logged)>, joined: i64, devices: &[&str], entries: &[Candidate]) -> Vec<usize> {
        let allowed: Vec<String> = ["aoi", "mika", "juan"].map(String::from).to_vec();
        let devices: HashSet<String> = devices.iter().map(|d| d.to_string()).collect();
        let log: BTreeMap<i64, Logged> = log.into_iter().collect();
        accept(entries, &Context { joined, allowed: &allowed, devices: &devices, log: &log })
    }

    const DEVICES: [&str; 4] = ["aoi/a1", "mika/m1", "mika/m2", "juan/j1"];

    #[test]
    fn takes_entries_signed_by_their_senders_own_device_at_their_place() {
        let log = vec![(3, message("aoi", "a1", false)), (4, message("mika", "m1", false))];
        let entries = [text(3, "aoi", "a1"), text(4, "mika", "m1"), edit(4, "mika", "m2", 4)];
        assert_eq!(run(log, 100, &DEVICES, &entries), [0, 1, 2]);
    }

    #[test]
    fn drops_an_entry_signed_with_someone_elses_key() {
        let log = || vec![(3, message("aoi", "a1", false))];
        assert!(run(log(), 100, &DEVICES, &[text(3, "aoi", "throwaway")]).is_empty());
        assert!(run(log(), 100, &DEVICES, &[text(3, "aoi", "j1")]).is_empty());
    }

    #[test]
    fn drops_entries_from_a_removed_device() {
        assert!(run(vec![(3, message("aoi", "a-old", false))], 100, &DEVICES, &[text(3, "aoi", "a-old")]).is_empty());
    }

    #[test]
    fn a_sharer_cant_move_repeat_or_bring_back_messages() {
        let log = || {
            vec![
                (3, message("aoi", "a1", false)),
                (4, message("mika", "m1", false)),
                (5, message("aoi", "a1", true)),
                (6, Logged { kind: LogKind::Other, ..message("aoi", "a1", false) }),
            ]
        };
        let aoi = |seq| text(seq, "aoi", "a1");
        assert!(run(log(), 100, &DEVICES, &[aoi(4)]).is_empty(), "moved onto someone else's message");
        assert_eq!(run(log(), 100, &DEVICES, &[aoi(3), aoi(3)]), [0], "repeated");
        assert!(run(log(), 100, &DEVICES, &[aoi(5)]).is_empty(), "deleted");
        assert!(run(log(), 100, &DEVICES, &[aoi(6)]).is_empty(), "not a message");
        assert!(run(log(), 100, &DEVICES, &[aoi(7)]).is_empty(), "not in the log");
        assert!(run(log(), 100, &DEVICES, &[edit(3, "aoi", "a1", 3)]).is_empty(), "edit of nothing taken");
        assert_eq!(run(log(), 100, &DEVICES, &[aoi(3), edit(3, "aoi", "a1", 9)]), [0], "edit pointing elsewhere");
    }

    #[test]
    fn takes_only_what_came_before_joining_and_after_sharing_was_turned_on() {
        let log = vec![
            (3, message("aoi", "a1", false)),
            (5, other(LogKind::Settings, "juan", true)),
            (6, message("aoi", "a1", false)),
            (12, message("aoi", "a1", false)),
        ];
        let aoi = |seq| text(seq, "aoi", "a1");
        assert_eq!(run(log, 10, &DEVICES, &[aoi(3), aoi(6), aoi(12)]), [1]);
    }

    #[test]
    fn takes_nothing_when_sharing_was_last_turned_off() {
        let log = vec![(2, other(LogKind::Settings, "juan", false)), (3, message("aoi", "a1", false))];
        assert!(run(log, 100, &DEVICES, &[text(3, "aoi", "a1")]).is_empty());
    }

    #[test]
    fn drops_senders_no_longer_in_the_channel() {
        assert!(run(vec![(3, message("rin", "r1", false))], 100, &["rin/r1"], &[text(3, "rin", "r1")]).is_empty());
    }
}
