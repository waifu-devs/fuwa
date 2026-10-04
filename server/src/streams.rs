//! Live event streams held open, counted so one account, or everyone at once,
//! can't hold open more than the instance carries.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};

/// Streams one account may hold open at once unless an admin or
/// FUWA_STREAMS_PER_ACCOUNT says otherwise: every tab and app someone keeps
/// open is one. A protective default, the agreed exception to caps being
/// unlimited by default: it stops one runaway client or script from opening
/// streams until the instance runs out of memory (each costs about 0.25 MB
/// on one process, docs/capacity.md).
pub const PER_ACCOUNT: usize = 32;

/// What a stream carries. An app or tab holds one of each, and each kind is
/// counted against the per-account limit on its own, so the limit counts
/// apps and tabs wherever the streams are served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Servers' events (`EventService.Subscribe`).
    Events,
    /// Direct messages (`DmService.Watch`).
    Dms,
}

const TOO_MANY: &str = "this account has too many apps or tabs open at once; close one and try again";

const FULL: &str = "this instance is full right now; try again in a moment";

#[derive(Debug)]
pub struct Streams {
    total: Option<usize>,
    open: AtomicUsize,
    by_account: Mutex<HashMap<(Kind, String), usize>>,
}

/// One open stream, given back on drop.
#[derive(Debug)]
pub struct Ticket {
    streams: Arc<Streams>,
    key: (Kind, String),
}

impl Streams {
    /// `None` is no limit.
    pub fn new(total: Option<usize>) -> Arc<Self> {
        Arc::new(Self { total, open: AtomicUsize::new(0), by_account: Mutex::new(HashMap::new()) })
    }

    /// Room for one more of the account's streams of this kind, up to
    /// `per_account` of them (`None` for no limit), or the reason there isn't.
    /// The instance's total counts every kind.
    pub fn open(self: &Arc<Self>, kind: Kind, account: &str, per_account: Option<usize>) -> Result<Ticket> {
        let mut by_account = self.by_account.lock().unwrap_or_else(|p| p.into_inner());
        if self.total.is_some_and(|total| self.open.load(Ordering::Acquire) >= total) {
            crate::reports::server_error("instance_full", None);
            return Err(Error::ResourceExhausted(FULL.into()));
        }
        let key = (kind, account.to_string());
        let held = by_account.entry(key.clone()).or_default();
        if per_account.is_some_and(|limit| *held >= limit) {
            if *held == 0 {
                by_account.remove(&key);
            }
            return Err(Error::ResourceExhausted(TOO_MANY.into()));
        }
        *held += 1;
        self.open.fetch_add(1, Ordering::AcqRel);
        Ok(Ticket { streams: self.clone(), key })
    }

    /// Streams open now.
    pub fn count(&self) -> usize {
        self.open.load(Ordering::Acquire)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut by_account = self.streams.by_account.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(held) = by_account.get_mut(&self.key) {
            *held -= 1;
            if *held == 0 {
                by_account.remove(&self.key);
            }
        }
        self.streams.open.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_each_account_and_everyone() {
        let streams = Streams::new(Some(3));
        let a1 = streams.open(Kind::Events, "a", Some(2)).unwrap();
        let _a2 = streams.open(Kind::Events, "a", Some(2)).unwrap();
        assert!(streams.open(Kind::Events, "a", Some(2)).is_err(), "a third for one account");
        let _b1 = streams.open(Kind::Events, "b", Some(2)).unwrap();
        assert!(streams.open(Kind::Events, "c", Some(2)).is_err(), "a fourth for the instance");
        drop(a1);
        assert_eq!(streams.count(), 2);
        let _c1 = streams.open(Kind::Events, "c", Some(2)).unwrap();
        assert_eq!(streams.by_account.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_tab_is_one_of_each_kind() {
        let streams = Streams::new(None);
        let _events = streams.open(Kind::Events, "a", Some(1)).unwrap();
        let _dms = streams.open(Kind::Dms, "a", Some(1)).expect("a tab's DM stream counts on its own");
        assert!(streams.open(Kind::Events, "a", Some(1)).is_err(), "a second tab");
        assert!(streams.open(Kind::Dms, "a", Some(1)).is_err(), "a second tab");
        assert_eq!(streams.count(), 2);
    }

    #[test]
    fn unlimited_by_choice() {
        let streams = Streams::new(None);
        let held: Vec<_> = (0..100).map(|_| streams.open(Kind::Events, "a", None).unwrap()).collect();
        assert_eq!(streams.count(), 100);
        drop(held);
        assert_eq!(streams.count(), 0);
        assert!(streams.by_account.lock().unwrap().is_empty());
    }
}
