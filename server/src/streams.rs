//! Live event streams held open, counted so one account, or everyone at once,
//! can't hold open more than the instance carries.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};

/// Streams one account may hold open at once when FUWA_STREAMS_PER_ACCOUNT
/// isn't set: every tab and app someone keeps open is one. A protective
/// limit, on by default: each stream costs memory on every part it passes
/// through (about 0.25 MB on one process, docs/capacity.md).
pub const PER_ACCOUNT: usize = 32;

const TOO_MANY: &str = "this account has too many apps or tabs open at once; close one and try again";

const FULL: &str = "this instance is full right now; try again in a moment";

#[derive(Debug)]
pub struct Streams {
    per_account: Option<usize>,
    total: Option<usize>,
    open: AtomicUsize,
    by_account: Mutex<HashMap<String, usize>>,
}

/// One open stream, given back on drop.
#[derive(Debug)]
pub struct Ticket {
    streams: Arc<Streams>,
    account: String,
}

impl Streams {
    /// `None` is no limit.
    pub fn new(per_account: Option<usize>, total: Option<usize>) -> Arc<Self> {
        Arc::new(Self { per_account, total, open: AtomicUsize::new(0), by_account: Mutex::new(HashMap::new()) })
    }

    /// Room for one more of the account's streams, or the reason there isn't.
    pub fn open(self: &Arc<Self>, account: &str) -> Result<Ticket> {
        let mut by_account = self.by_account.lock().unwrap_or_else(|p| p.into_inner());
        if self.total.is_some_and(|total| self.open.load(Ordering::Acquire) >= total) {
            crate::reports::server_error("instance_full", None);
            return Err(Error::ResourceExhausted(FULL.into()));
        }
        let held = by_account.entry(account.to_string()).or_default();
        if self.per_account.is_some_and(|limit| *held >= limit) {
            if *held == 0 {
                by_account.remove(account);
            }
            return Err(Error::ResourceExhausted(TOO_MANY.into()));
        }
        *held += 1;
        self.open.fetch_add(1, Ordering::AcqRel);
        Ok(Ticket { streams: self.clone(), account: account.to_string() })
    }

    /// Streams open now.
    pub fn count(&self) -> usize {
        self.open.load(Ordering::Acquire)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut by_account = self.streams.by_account.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(held) = by_account.get_mut(&self.account) {
            *held -= 1;
            if *held == 0 {
                by_account.remove(&self.account);
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
        let streams = Streams::new(Some(2), Some(3));
        let a1 = streams.open("a").unwrap();
        let _a2 = streams.open("a").unwrap();
        assert!(streams.open("a").is_err(), "a third for one account");
        let _b1 = streams.open("b").unwrap();
        assert!(streams.open("c").is_err(), "a fourth for the instance");
        drop(a1);
        assert_eq!(streams.count(), 2);
        let _c1 = streams.open("c").unwrap();
        assert_eq!(streams.by_account.lock().unwrap().len(), 3);
    }

    #[test]
    fn unlimited_by_choice() {
        let streams = Streams::new(None, None);
        let held: Vec<_> = (0..100).map(|_| streams.open("a").unwrap()).collect();
        assert_eq!(streams.count(), 100);
        drop(held);
        assert_eq!(streams.count(), 0);
        assert!(streams.by_account.lock().unwrap().is_empty());
    }
}
