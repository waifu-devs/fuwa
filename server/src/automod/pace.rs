//! How fast fuwa asks each provider. Every check goes through its provider's
//! lane, which sends as many at once as the provider keeps up with: one more
//! after each quick answer, half as many after a timeout, a refusal or an
//! error (the way TCP finds a link's speed). Checks past that wait their
//! turn; past [`MAX_WAITING`] waiting, a check isn't asked at all, and the
//! message goes through the Smart filter unchecked, as when the provider is
//! down. Each lane also learns how long its provider usually takes, which is
//! how long a message waits for an answer before it's sent (see [`budget`]).

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

/// Checks one provider may have in flight at first, and at most.
const START: f64 = 4.0;
const MOST: f64 = 64.0;
/// Checks that may wait for a turn with one provider.
pub const MAX_WAITING: usize = 256;
/// An answer slower than this counts against the provider, like a timeout.
const SLOW: Duration = Duration::from_secs(2);
/// How long a message waits for its answer before it's sent: what the
/// provider usually takes, plus a little, within these.
const LEAST_WAIT: Duration = Duration::from_millis(150);
const MOST_WAIT: Duration = Duration::from_millis(1000);

#[derive(Debug)]
struct State {
    /// How many checks may be in flight; whole numbers count.
    limit: f64,
    in_flight: usize,
    waiting: usize,
    /// What answers usually take, in milliseconds; `None` before the first.
    typical_ms: Option<f64>,
}

#[derive(Debug)]
struct Lane {
    state: Mutex<State>,
    freed: Notify,
}

static LANES: LazyLock<Mutex<HashMap<String, Arc<Lane>>>> = LazyLock::new(Default::default);

fn lane(provider: &str) -> Arc<Lane> {
    let mut lanes = LANES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    lanes
        .entry(provider.to_string())
        .or_insert_with(|| {
            Arc::new(Lane {
                state: Mutex::new(State { limit: START, in_flight: 0, waiting: 0, typical_ms: None }),
                freed: Notify::new(),
            })
        })
        .clone()
}

impl Lane {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Why a check didn't get its turn.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// [`MAX_WAITING`] checks are already waiting.
    Busy,
    /// Its turn didn't come within the wait it was given.
    TimedOut,
}

/// A check's turn with its provider; dropping it frees the place.
pub struct Turn {
    lane: Arc<Lane>,
    started: Instant,
    done: bool,
}

/// Waits, at most `wait`, for a turn to ask `provider`. Run it to the end
/// (in its own task when the caller may go away), or the lane's count of
/// waiting checks drifts.
pub async fn turn(provider: &str, wait: Duration) -> Result<Turn, Refused> {
    let lane = lane(provider);
    // Whether this check is in the lane's waiting count, to take it off
    // again if it gives up.
    let counted = std::sync::atomic::AtomicBool::new(false);
    let waited = tokio::time::timeout(wait, async {
        loop {
            let freed = lane.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            {
                let mut state = lane.state();
                if state.in_flight < state.limit as usize {
                    state.in_flight += 1;
                    if counted.swap(false, Relaxed) {
                        state.waiting -= 1;
                    }
                    return Ok(());
                }
                if !counted.load(Relaxed) {
                    if state.waiting >= MAX_WAITING {
                        return Err(Refused::Busy);
                    }
                    state.waiting += 1;
                    counted.store(true, Relaxed);
                }
            }
            freed.await;
        }
    })
    .await;
    match waited {
        Ok(Ok(())) => Ok(Turn { lane, started: Instant::now(), done: false }),
        Ok(Err(refused)) => Err(refused),
        Err(_) => {
            if counted.load(Relaxed) {
                lane.state().waiting -= 1;
            }
            // It may have been the one woken for a freed place: pass it on.
            lane.freed.notify_one();
            Err(Refused::TimedOut)
        }
    }
}

impl Turn {
    /// The provider answered (`ok`) or didn't; the lane grows or shrinks.
    pub fn finish(mut self, ok: bool) {
        self.done = true;
        let took = self.started.elapsed();
        let mut state = self.lane.state();
        if ok && took <= SLOW {
            state.limit = (state.limit + 1.0 / state.limit).min(MOST);
            let ms = took.as_secs_f64() * 1000.0;
            state.typical_ms = Some(state.typical_ms.map_or(ms, |typical| typical * 0.8 + ms * 0.2));
        } else {
            state.limit = (state.limit / 2.0).max(1.0);
        }
        state.in_flight -= 1;
        drop(state);
        self.lane.freed.notify_one();
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if !self.done {
            self.lane.state().in_flight -= 1;
            self.lane.freed.notify_one();
        }
    }
}

/// How long a message waits for `provider`'s answer before it's sent
/// without one (then it's checked right after): what the provider usually
/// takes and a quarter more, from [`LEAST_WAIT`] to [`MOST_WAIT`].
pub fn budget(provider: &str) -> Duration {
    let typical = lane(provider).state().typical_ms;
    typical.map_or(MOST_WAIT, |ms| Duration::from_secs_f64(ms * 1.25 / 1000.0).clamp(LEAST_WAIT, MOST_WAIT))
}

/// How many checks `provider` may have in flight now, for tests.
#[cfg(test)]
fn limit(provider: &str) -> usize {
    lane(provider).state().limit as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lanes_grow_with_quick_answers_and_halve_on_trouble() {
        let p = "test-grow";
        assert_eq!(limit(p), 4);
        for _ in 0..40 {
            turn(p, Duration::from_secs(1)).await.unwrap().finish(true);
        }
        let grown = limit(p);
        assert!(grown > 8, "{grown}");
        turn(p, Duration::from_secs(1)).await.unwrap().finish(false);
        assert_eq!(limit(p), grown / 2);
        for _ in 0..10 {
            turn(p, Duration::from_secs(1)).await.unwrap().finish(false);
        }
        assert_eq!(limit(p), 1);
        assert!(budget(p) >= LEAST_WAIT && budget(p) <= MOST_WAIT);
    }

    #[tokio::test]
    async fn checks_past_the_limit_wait_their_turn() {
        let p = "test-wait";
        let held: Vec<Turn> = futures::future::join_all((0..4).map(|_| turn(p, Duration::from_secs(1))))
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(turn(p, Duration::from_millis(50)).await.err(), Some(Refused::TimedOut));
        assert_eq!(lane(p).state().waiting, 0);
        let next = tokio::spawn(async move { turn(p, Duration::from_secs(2)).await.map(|t| t.finish(true)) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(lane(p).state().waiting, 1);
        drop(held);
        next.await.unwrap().unwrap();
        let lane = lane(p);
        let state = lane.state();
        assert_eq!((state.in_flight, state.waiting), (0, 0));
    }

    #[test]
    fn a_new_provider_gets_the_longest_wait() {
        assert_eq!(budget("test-new"), MOST_WAIT);
    }
}
