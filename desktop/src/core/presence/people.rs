//! Who's online and what they're doing, as the web app's `fuwa/presence.ts`:
//! one `WatchPresence` stream per instance (or the presence its live
//! connection carries, `sync.rs`, which is only who's on screen: people out
//! of sight in a large server show as having none), read into
//! [`InstanceState::people`], and whether you've stepped away, which goes out
//! with this app's presence.
//!
//! Presence is kept in memory only, and changes are put in a few times a
//! second at most, so a busy server doesn't redraw the window for each one.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tonic::Code;

use crate::core::Core;
use crate::core::api::{Api, Problem};
use crate::core::store::InstanceState;
use crate::pb;

/// No input for this long and the app says you're away.
pub const IDLE_AFTER: Duration = Duration::from_secs(10 * 60);
/// How often being away is looked at.
pub const IDLE_CHECK: Duration = Duration::from_secs(30);
/// Changes within this long go in together.
const BATCH: Duration = Duration::from_millis(400);
/// Heartbeats come every 25 seconds; this long without one means the stream is gone.
const SILENCE: Duration = Duration::from_secs(70);

/// Whether you've stepped away from this computer: no keys, clicks or
/// pointer for [`IDLE_AFTER`].
pub struct Idle {
    start: Instant,
    /// When there was input last, in ms since `start`.
    last: AtomicU64,
    away: watch::Sender<bool>,
}

impl Idle {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { start: Instant::now(), last: AtomicU64::new(0), away: watch::channel(false).0 })
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// There was input: you're back, if you were away.
    pub fn seen(&self) {
        self.seen_at(self.now());
    }

    fn seen_at(&self, now: u64) {
        self.last.store(now, Ordering::Relaxed);
        if *self.away.borrow() {
            self.away.send_replace(false);
        }
    }

    /// Says you're away once there's been no input for long enough.
    pub fn check(&self) {
        self.check_at(self.now());
    }

    fn check_at(&self, now: u64) {
        let quiet = now.saturating_sub(self.last.load(Ordering::Relaxed));
        if !*self.away.borrow() && quiet >= IDLE_AFTER.as_millis() as u64 {
            self.away.send_replace(true);
        }
    }

    pub fn away(&self) -> watch::Receiver<bool> {
        self.away.subscribe()
    }
}

/// Online, idle or busy; offline (or hidden) people aren't kept.
pub fn online(presence: &pb::Presence) -> bool {
    !matches!(presence.status(), pb::PresenceStatus::Offline | pb::PresenceStatus::Unspecified)
}

/// Puts changes in: someone online is kept, someone going offline dropped.
pub fn put(people: &mut HashMap<String, pb::Presence>, changes: Vec<pb::Presence>) {
    for presence in changes {
        if online(&presence) {
            people.insert(presence.user_id.clone(), presence);
        } else {
            people.remove(&presence.user_id);
        }
    }
}

impl InstanceState {
    /// Someone's presence here, while they're online.
    pub fn presence_of(&self, user_id: &str) -> Option<&pb::Presence> {
        self.people.as_ref()?.get(user_id)
    }
}

/// Follows everyone you can see on an instance for as long as it's synced,
/// listing again after each reconnect so nothing is missed. An instance
/// without presence is left alone, and shows no one as offline.
pub(crate) async fn follow(core: Arc<Core>, key: String, api: Api) {
    let mut wait = Duration::from_millis(500);
    loop {
        match watch_once(&core, &key, &api, &mut wait).await {
            Err(err) if err.code == Code::Unimplemented => {
                core.shared.instance(&key, |i| i.people = None);
                return;
            }
            Err(err) if err.signed_out() => return,
            _ => {}
        }
        let mut b = [0u8; 1];
        let _ = getrandom::fill(&mut b);
        tokio::time::sleep(wait.mul_f64(1.0 + f64::from(b[0]) / 255.0)).await;
        wait = (wait * 2).min(Duration::from_secs(20));
    }
}

async fn watch_once(core: &Arc<Core>, key: &str, api: &Api, wait: &mut Duration) -> Result<(), Problem> {
    let mut stream =
        api.presence().watch_presence(pb::WatchPresenceRequest {}).await.map_err(Problem::from)?.into_inner();
    let mut feed = Feed::default();
    loop {
        let limit = feed.due.unwrap_or_else(|| tokio::time::Instant::now() + SILENCE);
        let next = match tokio::time::timeout_at(limit, stream.message()).await {
            Err(_) if feed.due.is_some() => {
                feed.flush(core, key);
                continue;
            }
            Err(_) => return Err(Problem::new(Code::Unavailable, "Lost the connection.")),
            Ok(next) => next.map_err(Problem::from)?,
        };
        let Some(res) = next else { return Ok(()) };
        if feed.take(core, key, res) {
            *wait = Duration::from_millis(500);
        }
    }
}

/// Where one presence stream is: its own, or a live connection's
/// (`sync.rs`), which hands it the same responses.
#[derive(Default)]
pub(crate) struct Feed {
    gathered: Vec<pb::Presence>,
    ready: bool,
    /// Changes not put in yet go in then.
    pub due: Option<tokio::time::Instant>,
}

impl Feed {
    /// Takes one response. True when it's the stream saying it's ready.
    pub fn take(&mut self, core: &Core, key: &str, res: pb::WatchPresenceResponse) -> bool {
        if let Some(presence) = res.presence {
            self.gathered.push(presence);
            if self.ready && self.due.is_none() {
                self.due = Some(tokio::time::Instant::now() + BATCH);
            }
        }
        if !res.ready || self.ready {
            return false;
        }
        self.ready = true;
        // Everyone at once: whoever went offline while away is gone too.
        let list = std::mem::take(&mut self.gathered);
        core.shared.instance(key, |i| {
            let mut people = HashMap::new();
            put(&mut people, list);
            i.people = Some(people);
        });
        true
    }

    /// Puts in the changes gathered since the last time.
    pub fn flush(&mut self, core: &Core, key: &str) {
        self.due = None;
        let changes = std::mem::take(&mut self.gathered);
        core.shared.instance(key, |i| put(i.people.get_or_insert_with(HashMap::new), changes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(user: &str, status: pb::PresenceStatus) -> pb::Presence {
        pb::Presence { user_id: user.into(), status: status as i32, ..Default::default() }
    }

    #[test]
    fn away_after_quiet_and_back_on_input() {
        let idle = Idle::new();
        let away = idle.away();
        let after = IDLE_AFTER.as_millis() as u64;
        idle.seen_at(1_000);
        idle.check_at(1_000 + after - 1);
        assert!(!*away.borrow());
        idle.check_at(1_000 + after);
        assert!(*away.borrow());
        idle.seen_at(1_000 + after + 5);
        assert!(!*away.borrow());
        // Input since keeps it from flipping again.
        idle.check_at(1_000 + after + 10);
        assert!(!*away.borrow());
    }

    #[test]
    fn going_offline_drops_someone() {
        let mut people = HashMap::new();
        put(
            &mut people,
            vec![
                presence("a", pb::PresenceStatus::Online),
                presence("b", pb::PresenceStatus::Idle),
                presence("c", pb::PresenceStatus::DoNotDisturb),
            ],
        );
        assert_eq!(people.len(), 3);
        put(&mut people, vec![presence("b", pb::PresenceStatus::Offline), presence("a", pb::PresenceStatus::Idle)]);
        assert!(!people.contains_key("b"));
        assert_eq!(people["a"].status(), pb::PresenceStatus::Idle);
        put(&mut people, vec![presence("c", pb::PresenceStatus::Unspecified)]);
        assert_eq!(people.keys().collect::<Vec<_>>(), vec!["a"]);
    }
}
