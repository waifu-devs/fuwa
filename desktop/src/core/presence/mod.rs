//! Rich presence from games: the app listens where Discord's local RPC
//! does, so games and apps that report to Discord show what you're doing in
//! fuwa too, unchanged. Each one is asked about once ("celeste wants to show
//! what you're playing"); what allowed ones report goes to every instance
//! you're signed in to with this app's presence (`PresenceService`), where
//! your own presence settings decide who sees it.

pub mod ipc;
mod listen;
pub mod people;

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;

use crate::core::api::Api;
use crate::pb;
use crate::rpc;

pub use listen::listen;
pub use people::Idle;

/// An instance takes at most this many activities from one app.
const MAX_ACTIVITIES: usize = 5;
/// Changes within this long go out together.
const SETTLE: Duration = Duration::from_secs(2);

/// A program that connected, as the person is asked about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// What its answer is remembered by: the program's file name, or (where
    /// the system doesn't say which program it is) the application id it gave.
    pub key: String,
    /// What the prompt and the activity call it.
    pub name: String,
}

impl Program {
    pub fn new(process: Option<String>, application_id: &str) -> Self {
        match process.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()) {
            Some(name) => Self { key: format!("program:{}", name.to_lowercase()), name },
            None => Self { key: format!("discord:{application_id}"), name: "A game".into() },
        }
    }
}

struct Connected {
    program: Program,
    activity: Option<pb::Activity>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    /// Connected programs, oldest first.
    live: BTreeMap<u64, Connected>,
    /// Answers: true allowed, false not.
    answers: BTreeMap<String, bool>,
    /// Programs waiting for an answer, in the order they connected.
    asking: VecDeque<Program>,
    /// Programs whose question was closed without an answer: not asked
    /// again until the app starts again.
    later: Vec<String>,
}

/// What games are reporting, and which of them may.
pub struct Games {
    inner: Mutex<Inner>,
    /// Bumps when what goes out changes.
    changed: watch::Sender<u64>,
    /// Bumps when there's something new to ask; the window watches the store
    /// instead, so this goes through `Shared`.
    asked: Box<dyn Fn() + Send + Sync>,
}

impl Games {
    pub fn new(answers: BTreeMap<String, bool>, asked: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner { answers, ..Default::default() }),
            changed: watch::channel(0).0,
            asked: Box::new(asked),
        })
    }

    fn bump(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// A program connected; it's asked about when it hasn't been yet.
    pub fn connect(&self, program: Program) -> u64 {
        let ask = {
            let mut inner = self.inner.lock();
            inner.next += 1;
            let id = inner.next;
            let ask = !inner.answers.contains_key(&program.key)
                && !inner.asking.iter().any(|p| p.key == program.key)
                && !inner.later.contains(&program.key);
            if ask {
                inner.asking.push_back(program.clone());
            }
            inner.live.insert(id, Connected { program, activity: None });
            (id, ask)
        };
        if ask.1 {
            (self.asked)();
        }
        ask.0
    }

    pub fn set(&self, id: u64, activity: Option<pb::Activity>) {
        let shown = {
            let mut inner = self.inner.lock();
            let Some(c) = inner.live.get_mut(&id) else { return };
            c.activity = activity;
            let key = c.program.key.clone();
            inner.answers.get(&key) == Some(&true)
        };
        if shown {
            self.bump();
        }
    }

    pub fn disconnect(&self, id: u64) {
        let gone = self.inner.lock().live.remove(&id);
        if gone.is_some_and(|c| c.activity.is_some()) {
            self.bump();
        }
    }

    /// Forgets every connection (the listener stopped).
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.live.clear();
        inner.asking.clear();
        drop(inner);
        self.bump();
    }

    /// The program to ask about next, if any.
    pub fn asking(&self) -> Option<Program> {
        self.inner.lock().asking.front().cloned()
    }

    /// The person's answer; None closes the question until the app starts again.
    pub fn answer(&self, key: &str, allow: Option<bool>) {
        {
            let mut inner = self.inner.lock();
            inner.asking.retain(|p| p.key != key);
            match allow {
                Some(allow) => {
                    inner.answers.insert(key.to_owned(), allow);
                    inner.later.retain(|k| k != key);
                }
                None => inner.later.push(key.to_owned()),
            }
        }
        self.bump();
        (self.asked)();
    }

    /// Answers changed in settings; programs no longer answered are asked again.
    pub fn set_answers(&self, answers: BTreeMap<String, bool>) {
        let mut inner = self.inner.lock();
        if inner.answers == answers {
            return;
        }
        inner.later.retain(|k| answers.contains_key(k));
        inner.answers = answers;
        drop(inner);
        self.bump();
    }

    pub fn answers(&self) -> BTreeMap<String, bool> {
        self.inner.lock().answers.clone()
    }

    /// What allowed programs report, newest connection first.
    pub fn activities(&self) -> Vec<pb::Activity> {
        let inner = self.inner.lock();
        // One activity per game, as people see them.
        let mut seen = std::collections::HashSet::new();
        inner
            .live
            .values()
            .rev()
            .filter(|c| inner.answers.get(&c.program.key) == Some(&true))
            .filter_map(|c| c.activity.clone())
            .filter(|a| seen.insert((a.name.clone(), a.application_id.clone())))
            .take(MAX_ACTIVITIES)
            .collect()
    }

    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }
}

/// Keeps this app's presence on one instance: every minute, soon after what
/// games report changes, and at once when you step away or come back. An
/// instance without presence is left alone.
pub(crate) async fn keep(api: Api, games: Arc<Games>, mut away: watch::Receiver<bool>) {
    let mut changes = games.changes();
    loop {
        changes.mark_unchanged();
        away.mark_unchanged();
        let idle = *away.borrow();
        let request = pb::UpdatePresenceRequest { app: "desktop".into(), idle, activities: games.activities() };
        let renew = match rpc!(api.presence(), update_presence(request)).await {
            Ok(res) => res.renew_seconds.clamp(15, 120),
            Err(problem) if problem.code == tonic::Code::Unimplemented => return,
            Err(_) => 15,
        };
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(renew.into())) => {}
            changed = away.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
                tokio::time::sleep(SETTLE).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn playing(name: &str) -> pb::Activity {
        pb::Activity { name: name.into(), ..Default::default() }
    }

    #[test]
    fn programs_are_named_by_their_process() {
        let p = Program::new(Some("Celeste".into()), "123");
        assert_eq!((p.key.as_str(), p.name.as_str()), ("program:celeste", "Celeste"));
        let p = Program::new(None, "123");
        assert_eq!((p.key.as_str(), p.name.as_str()), ("discord:123", "A game"));
    }

    #[test]
    fn only_allowed_programs_are_shown() {
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        let games = Games::new(BTreeMap::new(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let celeste = games.connect(Program::new(Some("celeste".into()), "1"));
        let other = games.connect(Program::new(Some("spy".into()), "2"));
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        games.set(celeste, Some(playing("celeste")));
        games.set(other, Some(playing("spy")));
        assert!(games.activities().is_empty(), "nothing before an answer");

        assert_eq!(games.asking().unwrap().key, "program:celeste");
        games.answer("program:celeste", Some(true));
        games.answer("program:spy", Some(false));
        assert_eq!(games.activities(), vec![playing("celeste")]);
        assert!(games.asking().is_none());

        // Asked once: connecting again doesn't ask.
        let again = games.connect(Program::new(Some("celeste".into()), "1"));
        assert_eq!(asked.load(Ordering::SeqCst), 4);
        games.disconnect(celeste);
        assert!(games.activities().is_empty());
        games.set(again, Some(playing("celeste")));
        assert_eq!(games.activities().len(), 1);
    }

    #[test]
    fn closing_the_question_waits_for_the_next_start() {
        let games = Games::new(BTreeMap::new(), || {});
        let first = games.connect(Program::new(Some("celeste".into()), "1"));
        games.answer("program:celeste", None);
        // Not asked again while the app runs...
        games.connect(Program::new(Some("celeste".into()), "1"));
        assert!(games.asking().is_none());
        games.disconnect(first);
        // ...though it can still be answered from settings.
        games.answer("program:celeste", Some(true));
        assert_eq!(games.answers().get("program:celeste"), Some(&true));
    }

    #[test]
    fn at_most_five_and_one_per_game() {
        let games = Games::new(BTreeMap::from([("program:g".to_owned(), true)]), || {});
        for _ in 0..3 {
            let id = games.connect(Program::new(Some("g".into()), "1"));
            games.set(id, Some(playing("g")));
        }
        assert_eq!(games.activities().len(), 1);
        for n in 0..8 {
            let id = games.connect(Program::new(Some("g".into()), "1"));
            games.set(id, Some(playing(&format!("g{n}"))));
        }
        assert!(games.activities().len() <= MAX_ACTIVITIES);
        assert_eq!(games.activities()[0].name, "g7");
    }
}
