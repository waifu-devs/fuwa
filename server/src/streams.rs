//! Live event streams held open, counted so one account, or everyone at once,
//! can't hold open more than the instance carries.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::{Instant, Interval};

use crate::app::App;
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

/// How often a live stream asks the database whether its session is still
/// signed in. Sessions that end reach their streams at once through
/// [`App::ended_sessions`]; this catches one that ran out, or an end that
/// was missed, and is spread out so streams opened together don't ask
/// together.
pub const SESSION_RECHECK: Duration = Duration::from_secs(10 * 60);

/// A random point in `[every / 2, every)`, so timers started together spread out.
fn spread(every: Duration) -> Duration {
    let mut bytes = [0u8; 4];
    let fraction = match getrandom::fill(&mut bytes) {
        Ok(()) => f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0),
        Err(_) => 0.5,
    };
    every.mul_f64(0.5 + fraction / 2.0)
}

/// A stream's heartbeat timer. The first tick comes at a random point within
/// one period, so thousands of streams opened together (say, everyone coming
/// back after a restart) don't all tick at once from then on.
pub fn heartbeat(every: Duration) -> Interval {
    tokio::time::interval_at(Instant::now() + spread(every), every)
}

/// Whether a live stream's session is still signed in, asked of the database
/// only every [`SESSION_RECHECK`] or so rather than at every heartbeat.
pub struct SessionCheck {
    token_hash: String,
    next: Instant,
    epoch: u64,
}

impl SessionCheck {
    /// The first heartbeat asks, which covers a session that ended between
    /// the stream's sign-in check and it listening for ends.
    pub fn new(app: &App, token_hash: &str) -> Self {
        Self { token_hash: token_hash.to_string(), next: Instant::now(), epoch: app.session_epoch() }
    }

    /// Asks at the next heartbeat. For a stream that fell behind on ended
    /// sessions: every stream on the instance falls behind together, and each
    /// asking at its own heartbeat keeps them from asking all at once.
    pub fn due(&mut self) {
        self.next = Instant::now();
    }

    /// For a heartbeat: false once the session is gone. A failed check counts
    /// as still signed in, as before, and is asked again next time.
    pub async fn still_live(&mut self, app: &App) -> bool {
        let epoch = app.session_epoch();
        if Instant::now() < self.next && epoch == self.epoch {
            return true;
        }
        match app.session_live(&self.token_hash).await {
            Ok(live) => {
                self.next = Instant::now() + spread(SESSION_RECHECK);
                self.epoch = epoch;
                live
            }
            Err(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeats_start_spread_within_one_period() {
        let every = Duration::from_secs(25);
        let starts: Vec<_> = (0..200).map(|_| spread(every)).collect();
        assert!(starts.iter().all(|d| *d >= every / 2 && *d < every));
        let distinct: std::collections::HashSet<_> = starts.iter().map(|d| d.as_millis()).collect();
        assert!(distinct.len() > 100, "spread out, not all at one moment");
    }

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
