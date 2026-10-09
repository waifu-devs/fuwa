//! Who's online and what they're doing (docs/presence.md), kept in memory
//! where accounts are and never written down or logged.
//!
//! Each signed-in app holds a lease (`update`): whether its person is away
//! from it and what they're doing there. A person with no lease is offline.
//! What others see is worked out from that, their saved settings (status,
//! "Show what I'm doing", servers they hid it from) and the instance's switch,
//! and goes to the live streams (`watch`) of the people who share a server
//! with them, at most `BURST` times per `WINDOW`; changes past that are merged
//! and sent when the window allows.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::cluster::index::Index;
use crate::error::{Error, Result};
use crate::pb;

/// How long an app's word lasts without another update.
pub const LEASE: Duration = Duration::from_secs(150);
/// How often apps are asked to update.
pub const RENEW: Duration = Duration::from_secs(60);
/// At most this many changes of one person go out per `WINDOW`.
const BURST: usize = 5;
const WINDOW: Duration = Duration::from_secs(20);
/// How far a stream may fall behind before it's cut off.
pub const STREAM_BUFFER: usize = 1024;
/// Streams one account may hold open (a tab or app each); a new one past
/// this ends the oldest. An internal bound against piling up streams, not a
/// usage cap.
pub const MAX_STREAMS: usize = 16;
/// New picture links one account may have the instance sign per hour; past
/// it, pictures are dropped from its activities (the app's name and default
/// icon show instead). Keeps the picture fetcher from being anyone's proxy.
const MAX_NEW_PICTURES: usize = 30;
const PICTURE_WINDOW: Duration = Duration::from_secs(3600);

pub const MAX_ACTIVITIES: usize = 5;
const MAX_TEXT: usize = 128;
const MAX_LABEL: usize = 32;
const MAX_BUTTONS: usize = 2;
const MAX_BUTTON_URL: usize = 512;
const MAX_PICTURE_URL: usize = 2048;
const MAX_PARTY: u32 = 1_000_000;
const MAX_APPLICATION_ID: usize = 64;
/// More hidden servers than anyone could be in: an internal bound on what one
/// request may carry, not a usage cap.
pub const MAX_HIDDEN_SERVERS: usize = 1000;
/// Servers with at least this many members are large unless the instance
/// says otherwise (FUWA_LARGE_SERVER_MEMBERS): an on-screen stream gets
/// presence there only for the people it has in focus.
pub const LARGE_SERVER_MEMBERS: usize = 2500;
/// Members of smaller servers one on-screen stream follows at most, unless
/// the instance says otherwise (FUWA_ON_SCREEN_MEMBERS): its smallest servers
/// first, the rest treated as large. Bounds what one stream holds and is
/// sent, however many servers its person is in.
pub const ON_SCREEN_MEMBERS: usize = 5000;

#[derive(Default)]
pub struct Presence {
    inner: Mutex<Inner>,
    /// Account id to the pictures it had signed this hour (SHA-256 of each
    /// link) and when that hour began.
    pictures: Mutex<HashMap<String, Signed>>,
}

/// When an account's picture hour began, and the links signed in it.
type Signed = (Instant, HashSet<[u8; 32]>);

struct Inner {
    people: HashMap<String, Person>,
    /// Account id to its open streams.
    watchers: HashMap<String, Vec<Watcher>>,
    /// People with a change waiting for their window.
    pending: HashSet<String>,
    next_watcher: u64,
    /// Servers this big are large (0: none is).
    large: usize,
    /// Members of smaller servers an on-screen stream follows at most.
    budget: usize,
    /// Accounts with a stream that sees everyone (WatchPresence), the ones
    /// looked for in large servers besides people on screen.
    everyone: HashSet<String>,
    /// On-screen stream id to the people it has in focus.
    focus: HashMap<u64, HashSet<String>>,
    /// Person to the on-screen streams (owner, id) with them in focus.
    focused_by: HashMap<String, HashSet<(String, u64)>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            people: HashMap::new(),
            watchers: HashMap::new(),
            pending: HashSet::new(),
            next_watcher: 0,
            large: LARGE_SERVER_MEMBERS,
            budget: ON_SCREEN_MEMBERS,
            everyone: HashSet::new(),
            focus: HashMap::new(),
            focused_by: HashMap::new(),
        }
    }
}

struct Watcher {
    id: u64,
    tx: mpsc::Sender<pb::Presence>,
    /// Sees only what's on screen (LiveService.Open): people in `shown`,
    /// and those in focus.
    on_screen: bool,
    /// On screen: the servers whose members it follows (smaller than large,
    /// up to `budget` members in all) with the members each took, and room
    /// left for more. A followed server that grows past large stays
    /// followed, but its members are no longer walked for it.
    shown: HashMap<String, usize>,
    room: usize,
}

impl Watcher {
    /// Whether it sees someone it shares `shared` with, focus aside.
    fn sees_through(&self, shared: &[String]) -> bool {
        !self.on_screen || shared.iter().any(|id| self.shown.contains_key(id))
    }

    /// Stops following a server, giving back the room it took.
    fn unfollow(&mut self, server_id: &str) {
        if let Some(size) = self.shown.remove(server_id) {
            self.room += size;
        }
    }
}

struct Person {
    settings: pb::PresenceSettings,
    /// Session (token hash) to what that app said.
    apps: HashMap<String, App>,
    /// When their last changes went out.
    sent: VecDeque<Instant>,
    /// Their presence as last sent, as they see it themselves.
    last: Option<pb::Presence>,
    /// What others were last sent, so nothing goes out when what someone
    /// sees doesn't change (an invisible person's apps coming and going
    /// must never show).
    shown: Shown,
    /// When their settings last arrived, so a stale copy loaded from disk
    /// can't replace newer ones while they're kept here.
    touched: Instant,
}

/// The two versions of someone others were last sent, and the settings that
/// picked between them.
struct Shown {
    with: pb::Presence,
    without: pb::Presence,
    settings: pb::PresenceSettings,
}

impl Shown {
    fn for_servers(&self, servers: &[String]) -> &pb::Presence {
        if shows_activity(&self.settings, servers) { &self.with } else { &self.without }
    }
}

struct App {
    kind: String,
    idle: bool,
    activities: Vec<pb::Activity>,
    updated: Instant,
    expires: Instant,
}

impl Person {
    fn new(user_id: &str, settings: pb::PresenceSettings, now: Instant) -> Self {
        let shown = Shown { with: offline(user_id), without: offline(user_id), settings: settings.clone() };
        Self { settings, apps: HashMap::new(), sent: VecDeque::new(), last: None, shown, touched: now }
    }

    /// Their presence as they see it themselves.
    fn own(&self, user_id: &str) -> pb::Presence {
        if self.apps.is_empty() {
            return offline(user_id);
        }
        let chosen = pb::PresenceStatus::try_from(self.settings.status).unwrap_or(pb::PresenceStatus::Online);
        let status = match chosen {
            pb::PresenceStatus::Online | pb::PresenceStatus::Unspecified | pb::PresenceStatus::Offline
                if self.apps.values().all(|app| app.idle) =>
            {
                pb::PresenceStatus::Idle
            }
            pb::PresenceStatus::Unspecified | pb::PresenceStatus::Offline => pb::PresenceStatus::Online,
            other => other,
        };
        // The most recent app that's doing something speaks for them.
        let activities = self
            .apps
            .values()
            .filter(|app| !app.activities.is_empty())
            .max_by_key(|app| app.updated)
            .map(|app| app.activities.clone())
            .unwrap_or_default();
        let mut apps: Vec<String> = self.apps.values().map(|app| app.kind.clone()).collect();
        apps.sort();
        apps.dedup();
        pb::Presence { user_id: user_id.to_string(), status: status as i32, activities, apps }
    }

    /// Whether what they're doing shows to someone they share `servers` with.
    fn shows_activity_in(&self, servers: &[String]) -> bool {
        shows_activity(&self.settings, servers)
    }

    /// Whether others see them online (an app open, and not invisible).
    fn looks_online(&self, user_id: &str) -> bool {
        seen(&self.own(user_id), false).status != pb::PresenceStatus::Offline as i32
    }

    /// What `servers` (the ones shared with a viewer) let that viewer see.
    fn seen_in(&self, user_id: &str, servers: &[String]) -> pb::Presence {
        seen(&self.own(user_id), self.shows_activity_in(servers))
    }
}

fn shows_activity(settings: &pb::PresenceSettings, servers: &[String]) -> bool {
    settings.show_activity && servers.iter().any(|id| !settings.hidden_server_ids.contains(id))
}

/// What everyone else sees of someone, given what they see themselves.
fn seen(own: &pb::Presence, with_activity: bool) -> pb::Presence {
    if own.status == pb::PresenceStatus::Invisible as i32 {
        return offline(&own.user_id);
    }
    let mut seen = own.clone();
    if !with_activity {
        seen.activities.clear();
    }
    seen
}

fn offline(user_id: &str) -> pb::Presence {
    pb::Presence { user_id: user_id.to_string(), status: pb::PresenceStatus::Offline as i32, ..Default::default() }
}

impl Inner {
    /// Sends someone's presence to everyone who may see it, and to their own
    /// streams: to each only when what they see of it changed.
    fn broadcast(&mut self, index: &Index, user_id: &str, now: Instant) {
        let Some(person) = self.people.get_mut(user_id) else { return };
        let own = person.own(user_id);
        person.sent.push_back(now);
        while person.sent.len() > BURST {
            person.sent.pop_front();
        }
        let with = seen(&own, true);
        let without = seen(&own, false);
        let watchers = &self.watchers;
        let audience = index.audience(user_id, self.large, |id| watchers.contains_key(id), &self.everyone);
        let mut behind = Vec::new();
        for (watcher, shared) in &audience {
            let presence = if person.shows_activity_in(shared) { &with } else { &without };
            if person.shown.for_servers(shared) != presence {
                send_where(&self.watchers[watcher], presence, &mut behind, |w| w.sees_through(shared));
            }
        }
        // On-screen streams with them in focus that didn't see them through a server.
        for (watcher, id) in self.focused_by.get(user_id).into_iter().flatten() {
            let Some(stream) = self.watchers.get(watcher).and_then(|s| s.iter().find(|w| w.id == *id)) else {
                continue;
            };
            let shared = match audience.get(watcher) {
                Some(shared) if stream.sees_through(shared) => continue,
                Some(shared) => shared.clone(),
                None => index.shared_servers(user_id, watcher),
            };
            if shared.is_empty() {
                continue;
            }
            let presence = if person.shows_activity_in(&shared) { &with } else { &without };
            if person.shown.for_servers(&shared) != presence {
                send_where(std::slice::from_ref(stream), presence, &mut behind, |_| true);
            }
        }
        if person.last.as_ref() != Some(&own)
            && let Some(own_streams) = self.watchers.get(user_id)
        {
            send(own_streams, &own, &mut behind);
        }
        person.shown = Shown { with, without, settings: person.settings.clone() };
        person.last = Some(own);
        self.drop_streams(&behind);
    }

    /// Tells `a` and `b` how they now see each other, after a server they
    /// shared went away for one of them: offline if they share no other.
    /// On-screen streams that showed them through `server_id` hear too.
    fn part(&self, index: &Index, a: &str, b: &str, server_id: &str, behind: &mut Vec<u64>) {
        let shared = index.shared_servers(a, b);
        for (from, to) in [(a, b), (b, a)] {
            let (Some(person), Some(streams)) = (self.people.get(from), self.watchers.get(to)) else { continue };
            // Only someone `to` was shown as online goes offline for them:
            // nothing at all for people offline or invisible.
            if !person.looks_online(from) {
                continue;
            }
            let presence = if shared.is_empty() { offline(from) } else { person.seen_in(from, &shared) };
            send_where(streams, &presence, behind, |w| !w.on_screen);
            // On screen: still shown through a server it follows, or for being
            // in focus; offline if it showed them and no longer does.
            for stream in streams.iter().filter(|w| w.on_screen) {
                let focused = self.focus.get(&stream.id).is_some_and(|people| people.contains(from));
                let shown = !shared.is_empty() && (stream.sees_through(&shared) || focused);
                if shown || stream.shown.contains_key(server_id) || focused {
                    let presence = if shown { presence.clone() } else { offline(from) };
                    send_where(std::slice::from_ref(stream), &presence, behind, |_| true);
                }
            }
        }
    }

    /// Ends streams that fell behind: they see their channel close and tell
    /// the app to watch again.
    fn drop_streams(&mut self, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        crate::reports::server_error("presence_stream_behind", Some("presence"));
        let owners: Vec<String> = self
            .watchers
            .iter()
            .filter(|(_, streams)| streams.iter().any(|stream| ids.contains(&stream.id)))
            .map(|(owner, _)| owner.clone())
            .collect();
        for owner in owners {
            for id in ids {
                self.forget_stream(&owner, *id);
            }
        }
    }

    /// Takes a stream out, with what it had in focus.
    fn forget_stream(&mut self, owner: &str, id: u64) {
        if let Some(streams) = self.watchers.get_mut(owner) {
            streams.retain(|stream| stream.id != id);
            if !streams.iter().any(|stream| !stream.on_screen) {
                self.everyone.remove(owner);
            }
            if streams.is_empty() {
                self.watchers.remove(owner);
            }
        }
        self.set_focus(owner, id, HashSet::new());
    }

    /// Replaces what on-screen stream `id` of `owner` has in focus. Gives
    /// back the people new to it.
    fn set_focus(&mut self, owner: &str, id: u64, people: HashSet<String>) -> Vec<String> {
        let before = self.focus.remove(&id).unwrap_or_default();
        for gone in before.difference(&people) {
            if let Some(by) = self.focused_by.get_mut(gone) {
                by.remove(&(owner.to_string(), id));
                if by.is_empty() {
                    self.focused_by.remove(gone);
                }
            }
        }
        let added: Vec<String> = people.difference(&before).cloned().collect();
        for person in &added {
            self.focused_by.entry(person.clone()).or_default().insert((owner.to_string(), id));
        }
        if !people.is_empty() {
            self.focus.insert(id, people);
        }
        added
    }

    /// Sends now if their window allows, or marks it to go later.
    fn changed(&mut self, index: &Index, user_id: &str, now: Instant, force: bool) {
        let Some(person) = self.people.get(user_id) else { return };
        if !force && person.last.as_ref() == Some(&person.own(user_id)) {
            return;
        }
        let full = person.sent.len() >= BURST && person.sent.front().is_some_and(|first| now - *first < WINDOW);
        if full {
            if self.pending.insert(user_id.to_string()) {
                crate::reports::server_used("presence.coalesced", 1);
            }
        } else {
            self.pending.remove(user_id);
            self.broadcast(index, user_id, now);
        }
    }

    /// Whether someone with no app, no stream and nothing waiting can be
    /// forgotten (their settings are read again when they're back).
    fn forgettable(&self, user_id: &str, person: &Person, now: Instant) -> bool {
        person.apps.is_empty()
            && !self.watchers.contains_key(user_id)
            && !self.pending.contains(user_id)
            && now.duration_since(person.touched) >= LEASE
    }
}

fn send(streams: &[Watcher], presence: &pb::Presence, behind: &mut Vec<u64>) {
    send_where(streams, presence, behind, |_| true);
}

/// Sends to the streams that pass `to`.
fn send_where(streams: &[Watcher], presence: &pb::Presence, behind: &mut Vec<u64>, to: impl Fn(&Watcher) -> bool) {
    for stream in streams.iter().filter(|stream| to(stream)) {
        if stream.tx.try_send(presence.clone()).is_err() {
            behind.push(stream.id);
        }
    }
}

/// A new stream: what it starts with, and what follows.
pub struct Watch {
    pub id: u64,
    pub snapshot: Vec<pb::Presence>,
    pub rx: mpsc::Receiver<pb::Presence>,
}

impl Presence {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether someone's settings are already here (else load them first
    /// and hand them to `update` or `watch`).
    pub fn knows(&self, user_id: &str) -> bool {
        self.lock().people.contains_key(user_id)
    }

    /// What an app says about its person. `settings` (their saved ones) is
    /// used only when the person isn't known here; without them, nothing
    /// happens and this gives back false, so the caller loads them and
    /// calls again. Settings are never guessed.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        index: &Index,
        user_id: &str,
        session: &str,
        kind: &str,
        idle: bool,
        activities: Vec<pb::Activity>,
        settings: Option<pb::PresenceSettings>,
    ) -> bool {
        let now = Instant::now();
        let mut inner = self.lock();
        if !inner.people.contains_key(user_id) {
            let Some(settings) = settings else { return false };
            inner.people.insert(user_id.to_string(), Person::new(user_id, settings, now));
        }
        let person = inner.people.get_mut(user_id).expect("just added");
        person.apps.insert(
            session.to_string(),
            App { kind: kind.to_string(), idle, activities, updated: now, expires: now + LEASE },
        );
        inner.changed(index, user_id, now, false);
        true
    }

    /// Someone changed their settings: everyone may now see them differently.
    /// Kept even for someone not here yet, so a copy loaded before the change
    /// doesn't win.
    pub fn settings_changed(&self, index: &Index, user_id: &str, settings: pb::PresenceSettings) {
        let now = Instant::now();
        let mut inner = self.lock();
        let person =
            inner.people.entry(user_id.to_string()).or_insert_with(|| Person::new(user_id, settings.clone(), now));
        person.settings = settings;
        person.touched = now;
        inner.changed(index, user_id, now, true);
    }

    /// The instance stopped showing activities: those shown go now.
    pub fn clear_activities(&self, index: &Index) {
        let now = Instant::now();
        let mut inner = self.lock();
        let mut doing = Vec::new();
        for (user_id, person) in inner.people.iter_mut() {
            for app in person.apps.values_mut() {
                if !app.activities.is_empty() {
                    app.activities.clear();
                    doing.push(user_id.clone());
                }
            }
        }
        doing.dedup();
        for user_id in doing {
            inner.changed(index, &user_id, now, false);
        }
    }

    /// Whether the instance may sign another picture link for `user_id` this
    /// hour (`MAX_NEW_PICTURES` new ones; links already signed this hour are
    /// free).
    pub fn allow_picture(&self, user_id: &str, url: &str) -> bool {
        use sha2::Digest as _;
        let now = Instant::now();
        let hash: [u8; 32] = sha2::Sha256::digest(url.as_bytes()).into();
        let mut pictures = self.pictures.lock().unwrap_or_else(|p| p.into_inner());
        let (began, seen) = pictures.entry(user_id.to_string()).or_insert_with(|| (now, HashSet::new()));
        if now.duration_since(*began) >= PICTURE_WINDOW {
            *began = now;
            seen.clear();
        }
        if seen.contains(&hash) {
            return true;
        }
        if seen.len() >= MAX_NEW_PICTURES {
            crate::reports::server_used("presence.pictures_capped", 1);
            return false;
        }
        seen.insert(hash);
        true
    }

    /// Presence where servers with at least `large` members are large (0:
    /// none is), and on-screen streams follow up to `budget` members of
    /// smaller ones.
    pub fn with_large(large: usize, budget: usize) -> Self {
        let presence = Self::default();
        let mut inner = presence.lock();
        (inner.large, inner.budget) = (large, budget);
        drop(inner);
        presence
    }

    /// Opens a stream for `user_id`: everyone online they may see, then each
    /// change.
    pub fn watch(&self, index: &Index, user_id: &str, settings: Option<pb::PresenceSettings>) -> Watch {
        self.open(index, user_id, settings, false)
    }

    /// Opens an on-screen stream for `user_id`: everyone online in servers
    /// smaller than large, then each change, and people in its focus
    /// (`focus`) as it changes.
    pub fn watch_on_screen(&self, index: &Index, user_id: &str, settings: Option<pb::PresenceSettings>) -> Watch {
        self.open(index, user_id, settings, true)
    }

    fn open(&self, index: &Index, user_id: &str, settings: Option<pb::PresenceSettings>, on_screen: bool) -> Watch {
        let now = Instant::now();
        let mut inner = self.lock();
        inner.next_watcher += 1;
        let id = inner.next_watcher;
        let (tx, rx) = mpsc::channel(STREAM_BUFFER);
        // Past the bound the oldest ends (its app sees the stream close and
        // watches again if it's still there).
        if let Some(oldest) = inner.watchers.get(user_id).filter(|s| s.len() >= MAX_STREAMS).map(|s| s[0].id) {
            inner.forget_stream(user_id, oldest);
            crate::reports::server_used("presence.streams_replaced", 1);
        }
        let (shown, room) = match on_screen {
            true => index.smallest_servers(user_id, inner.large, inner.budget),
            false => (HashMap::new(), 0),
        };
        inner.watchers.entry(user_id.to_string()).or_default().push(Watcher { id, tx, on_screen, shown, room });
        if !on_screen {
            inner.everyone.insert(user_id.to_string());
        }
        if let Some(settings) = settings {
            inner.people.entry(user_id.to_string()).or_insert_with(|| Person::new(user_id, settings, now));
        }
        let people = &inner.people;
        let online = |id: &str| people.get(id).is_some_and(|p| !p.apps.is_empty());
        let mut snapshot = Vec::new();
        let stream = inner.watchers[user_id].last().expect("just added");
        let seen: Vec<(String, Vec<String>)> = match on_screen {
            false => index.neighbours(user_id, online).into_iter().collect(),
            true => index
                .audience(user_id, inner.large, online, &HashSet::new())
                .into_iter()
                .filter(|(_, shared)| stream.sees_through(shared))
                .collect(),
        };
        for (other, shared) in seen {
            let presence = people[&other].seen_in(&other, &shared);
            if presence.status != pb::PresenceStatus::Offline as i32 {
                snapshot.push(presence);
            }
        }
        if let Some(person) = people.get(user_id).filter(|p| !p.apps.is_empty()) {
            snapshot.push(person.own(user_id));
        }
        Watch { id, snapshot, rx }
    }

    /// A stream ended.
    pub fn unwatch(&self, user_id: &str, id: u64) {
        self.lock().forget_stream(user_id, id);
    }

    /// The people on-screen stream `id` of `user_id` has in focus now. Those
    /// new to it who are online and share a server with them, and weren't
    /// already shown through a smaller server, are sent at once.
    pub fn focus(&self, index: &Index, user_id: &str, id: u64, people: Vec<String>) {
        let mut inner = self.lock();
        let Some(streams) = inner.watchers.get(user_id) else { return };
        if !streams.iter().any(|stream| stream.id == id && stream.on_screen) {
            return;
        }
        let people: HashSet<String> = people.into_iter().filter(|id| id != user_id).collect();
        let added = inner.set_focus(user_id, id, people);
        let mut behind = Vec::new();
        let stream = inner.watchers[user_id].iter().find(|w| w.id == id).expect("checked above");
        for other in added {
            let Some(person) = inner.people.get(&other).filter(|p| !p.apps.is_empty()) else { continue };
            let shared = index.shared_servers(&other, user_id);
            if shared.is_empty() || stream.sees_through(&shared) {
                continue;
            }
            let presence = person.seen_in(&other, &shared);
            if presence.status != pb::PresenceStatus::Offline as i32 {
                send_where(std::slice::from_ref(stream), &presence, &mut behind, |_| true);
            }
        }
        inner.drop_streams(&behind);
    }

    /// Someone joined a server: they and its members now see each other.
    pub fn joined(&self, index: &Index, user_id: &str, server_id: &str) {
        let mut inner = self.lock();
        let online = |p: Option<&Person>| p.is_some_and(|p| !p.apps.is_empty());
        let mut behind = Vec::new();
        // The newcomer's on-screen streams follow it if it's small and there's room.
        let size = index.size(server_id);
        if inner.large == 0 || size < inner.large {
            for stream in inner.watchers.get_mut(user_id).into_iter().flatten().filter(|w| w.on_screen) {
                if size <= stream.room {
                    stream.room -= size;
                    stream.shown.insert(server_id.to_string(), size);
                }
            }
        }
        // On screen, a server not followed shows only who's in focus.
        let shows = |stream: &Watcher, other: &str| {
            !stream.on_screen
                || stream.shown.contains_key(server_id)
                || inner.focus.get(&stream.id).is_some_and(|people| people.contains(other))
        };
        // The server's members who are watching see the newcomer, as last sent.
        if let Some(person) = inner.people.get(user_id).filter(|p| !p.apps.is_empty())
            && let Some(own) = &person.last
        {
            let watchers = &inner.watchers;
            for other in index.members_where(server_id, |id| id != user_id && watchers.contains_key(id)) {
                let shared = index.shared_servers(user_id, &other);
                let presence = seen(own, person.shows_activity_in(&shared));
                // Invisible stays out of sight: nothing is sent for them.
                if presence.status != pb::PresenceStatus::Offline as i32 {
                    send_where(&watchers[&other], &presence, &mut behind, |w| shows(w, user_id));
                }
            }
        }
        // And the newcomer sees who's online there.
        if let Some(streams) = inner.watchers.get(user_id) {
            let people = &inner.people;
            for other in index.members_where(server_id, |id| id != user_id && online(people.get(id))) {
                let presence = people[&other].seen_in(&other, &index.shared_servers(&other, user_id));
                if presence.status != pb::PresenceStatus::Offline as i32 {
                    send_where(streams, &presence, &mut behind, |w| shows(w, &other));
                }
            }
        }
        inner.drop_streams(&behind);
    }

    /// Someone left a server, or was removed: whoever there they no longer
    /// share a server with sees them go offline, and they see them go.
    pub fn left(&self, index: &Index, user_id: &str, server_id: &str) {
        let mut inner = self.lock();
        let mut behind = Vec::new();
        let people = &inner.people;
        let watchers = &inner.watchers;
        let involved = |id: &str| people.contains_key(id) || watchers.contains_key(id);
        if involved(user_id) {
            for other in index.members_where(server_id, |id| id != user_id && involved(id)) {
                inner.part(index, user_id, &other, server_id, &mut behind);
            }
        }
        for stream in inner.watchers.get_mut(user_id).into_iter().flatten() {
            stream.unfollow(server_id);
        }
        inner.drop_streams(&behind);
    }

    /// A server went away; `members` were its members, now out of the index.
    pub fn server_gone(&self, index: &Index, server_id: &str, members: &[String]) {
        let mut inner = self.lock();
        let mut behind = Vec::new();
        let watching: Vec<&String> = members.iter().filter(|id| inner.watchers.contains_key(*id)).collect();
        let online: Vec<&String> =
            members.iter().filter(|id| inner.people.get(*id).is_some_and(|p| p.looks_online(id))).collect();
        for to in &watching {
            for from in &online {
                let shared = index.shared_servers(to, from);
                let focused = |w: &Watcher| inner.focus.get(&w.id).is_some_and(|people| people.contains(*from));
                // Still shown on screen through a server it follows, or for
                // being in focus, while they share one.
                let still = |w: &Watcher| !shared.is_empty() && (w.sees_through(&shared) || focused(w));
                let gone = |w: &Watcher| match w.on_screen {
                    false => shared.is_empty(),
                    true => !still(w) && (w.shown.contains_key(server_id) || focused(w)),
                };
                if to == from {
                    continue;
                }
                send_where(&inner.watchers[*to], &offline(from), &mut behind, gone);
            }
        }
        let watching: Vec<String> = watching.into_iter().cloned().collect();
        for to in watching {
            for stream in inner.watchers.get_mut(&to).into_iter().flatten() {
                stream.unfollow(server_id);
            }
        }
        inner.drop_streams(&behind);
    }

    /// Forgets apps that stopped calling, and sends changes whose window
    /// opened. Runs every second.
    pub fn tick(&self, index: &Index) {
        let now = Instant::now();
        let mut inner = self.lock();
        let mut lapsed = Vec::new();
        for (user_id, person) in inner.people.iter_mut() {
            let before = person.apps.len();
            person.apps.retain(|_, app| app.expires > now);
            if person.apps.len() != before {
                lapsed.push(user_id.clone());
            }
        }
        for user_id in &lapsed {
            inner.changed(index, user_id, now, false);
        }
        let ready: Vec<String> = inner
            .pending
            .iter()
            .filter(|id| {
                inner
                    .people
                    .get(*id)
                    .is_none_or(|p| p.sent.len() < BURST || p.sent.front().is_none_or(|first| now - *first >= WINDOW))
            })
            .cloned()
            .collect();
        for user_id in ready {
            inner.pending.remove(&user_id);
            if inner.people.contains_key(&user_id) {
                inner.broadcast(index, &user_id, now);
            }
        }
        let forget: Vec<String> = inner
            .people
            .iter()
            .filter(|(id, person)| inner.forgettable(id, person, now))
            .map(|(id, _)| id.clone())
            .collect();
        for user_id in forget {
            inner.people.remove(&user_id);
        }
        drop(inner);
        self.pictures
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, (began, _)| now.duration_since(*began) < PICTURE_WINDOW);
    }

    /// How many people have an app here now, for the usage counts.
    pub fn online_count(&self) -> usize {
        self.lock().people.values().filter(|p| !p.apps.is_empty()).count()
    }
}

/// Someone's settings before they change anything.
pub fn default_settings() -> pb::PresenceSettings {
    pb::PresenceSettings { status: pb::PresenceStatus::Online as i32, ..Default::default() }
}

/// Checks settings from a client.
pub fn check_settings(mut settings: pb::PresenceSettings) -> Result<pb::PresenceSettings> {
    let status = pb::PresenceStatus::try_from(settings.status).unwrap_or(pb::PresenceStatus::Unspecified);
    settings.status = match status {
        pb::PresenceStatus::Unspecified => pb::PresenceStatus::Online,
        pb::PresenceStatus::Offline => {
            return Err(Error::invalid("pick online, idle, do not disturb or invisible"));
        }
        other => other,
    } as i32;
    settings.hidden_server_ids.retain(|id| !id.is_empty());
    settings.hidden_server_ids.sort();
    settings.hidden_server_ids.dedup();
    if settings.hidden_server_ids.len() > MAX_HIDDEN_SERVERS
        || settings.hidden_server_ids.iter().any(|id| id.len() > 64)
    {
        return Err(Error::invalid("too many hidden servers"));
    }
    Ok(settings)
}

/// Checks activities from an app and makes their pictures safe to show:
/// https pictures become links the instance fetches itself (`picture_link`);
/// anything else (a Discord asset key) is dropped, so apps show the app's
/// name with a default icon.
pub fn check_activities(
    activities: Vec<pb::Activity>,
    picture_link: impl Fn(&str) -> String,
) -> Result<Vec<pb::Activity>> {
    if activities.len() > MAX_ACTIVITIES {
        return Err(Error::invalid(format!("at most {MAX_ACTIVITIES} activities")));
    }
    activities.into_iter().map(|activity| check_activity(activity, &picture_link)).collect()
}

fn check_activity(mut a: pb::Activity, picture_link: &impl Fn(&str) -> String) -> Result<pb::Activity> {
    if pb::ActivityKind::try_from(a.kind).unwrap_or(pb::ActivityKind::Unspecified) == pb::ActivityKind::Unspecified {
        a.kind = pb::ActivityKind::Playing as i32;
    }
    a.name = text("an activity's name", &a.name, 1, MAX_TEXT)?;
    a.details = text("an activity's details", &a.details, 0, MAX_TEXT)?;
    a.state = text("an activity's state", &a.state, 0, MAX_TEXT)?;
    a.large_text = text("a picture's text", &a.large_text, 0, MAX_TEXT)?;
    a.small_text = text("a picture's text", &a.small_text, 0, MAX_TEXT)?;
    if a.party_size > MAX_PARTY || a.party_max > MAX_PARTY {
        return Err(Error::invalid("a party can have at most 1,000,000 people"));
    }
    if a.party_max == 0 {
        a.party_size = 0;
    }
    a.application_id = a.application_id.trim().to_string();
    if a.application_id.len() > MAX_APPLICATION_ID || !a.application_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::invalid("an application id is up to 64 letters and digits"));
    }
    a.large_image_url = picture(&a.large_image_url, picture_link);
    a.small_image_url = picture(&a.small_image_url, picture_link);
    if a.buttons.len() > MAX_BUTTONS {
        return Err(Error::invalid(format!("at most {MAX_BUTTONS} buttons")));
    }
    for button in &mut a.buttons {
        button.label = text("a button's label", &button.label, 1, MAX_LABEL)?;
        button.url = button_url(&button.url)?;
    }
    Ok(a)
}

fn text(field: &str, value: &str, min: usize, max: usize) -> Result<String> {
    // Line breaks and other control characters would only break layouts;
    // direction overrides and invisible characters could disguise the text.
    let value: String =
        value.chars().filter(|c| !hidden_char(*c)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    let value = value.trim();
    let length = value.chars().count();
    if length < min || length > max {
        return Err(Error::invalid(if min == 0 {
            format!("{field} can be at most {max} characters")
        } else {
            format!("{field} must be {min} to {max} characters")
        }));
    }
    Ok(value.to_string())
}

/// Characters that change how text around them reads without showing
/// themselves: direction marks and overrides, and zero-width ones. The
/// zero-width joiner stays, since emoji are built with it.
fn hidden_char(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{180E}' | '\u{200B}' | '\u{200C}' | '\u{200E}' | '\u{200F}' | '\u{FEFF}')
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2060}'..='\u{2069}').contains(&c)
}

fn picture(value: &str, picture_link: &impl Fn(&str) -> String) -> String {
    let value = value.trim();
    match url::Url::parse(value) {
        Ok(parsed)
            if parsed.scheme() == "https"
                && value.len() <= MAX_PICTURE_URL
                && parsed.username().is_empty()
                && parsed.password().is_none() =>
        {
            picture_link(value)
        }
        _ => String::new(),
    }
}

fn button_url(value: &str) -> Result<String> {
    let value = value.trim();
    let parsed = url::Url::parse(value).map_err(|_| Error::invalid("a button's link must be an https link"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none_or(str::is_empty) {
        return Err(Error::invalid("a button's link must be an https link"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(Error::invalid("a button's link can't hold a user name or password"));
    }
    if value.len() > MAX_BUTTON_URL {
        return Err(Error::invalid(format!("a button's link can be at most {MAX_BUTTON_URL} characters")));
    }
    Ok(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: &str) -> pb::Server {
        pb::Server { id: id.into(), name: id.into(), ..Default::default() }
    }

    fn index() -> Index {
        let index = Index::default();
        index.insert(server("s1"), vec!["ann".into(), "bo".into()], vec![], None);
        index.insert(server("s2"), vec!["ann".into(), "cy".into()], vec![], None);
        index
    }

    fn playing(name: &str) -> pb::Activity {
        pb::Activity { kind: pb::ActivityKind::Playing as i32, name: name.into(), ..Default::default() }
    }

    fn sharing(hidden: &[&str]) -> pb::PresenceSettings {
        pb::PresenceSettings {
            status: pb::PresenceStatus::Online as i32,
            show_activity: true,
            hidden_server_ids: hidden.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn drain(rx: &mut mpsc::Receiver<pb::Presence>) -> Vec<pb::Presence> {
        let mut out = vec![];
        while let Ok(p) = rx.try_recv() {
            out.push(p);
        }
        out
    }

    #[test]
    fn only_people_sharing_a_server_hear_and_hidden_servers_hide_activity() {
        let index = index();
        let presence = Presence::default();
        let mut bo = presence.watch(&index, "bo", Some(default_settings()));
        let mut cy = presence.watch(&index, "cy", Some(default_settings()));
        let mut dee = presence.watch(&index, "dee", Some(default_settings()));
        presence.update(&index, "ann", "t1", "desktop", false, vec![playing("Celeste")], Some(sharing(&["s2"])));
        let to_bo = drain(&mut bo.rx);
        assert_eq!(to_bo.len(), 1);
        assert_eq!(to_bo[0].status, pb::PresenceStatus::Online as i32);
        assert_eq!(to_bo[0].activities[0].name, "Celeste");
        // cy shares only s2, where ann hid what she's doing.
        let to_cy = drain(&mut cy.rx);
        assert_eq!(to_cy.len(), 1);
        assert!(to_cy[0].activities.is_empty());
        assert!(drain(&mut dee.rx).is_empty());
    }

    #[test]
    fn activity_needs_show_activity_and_invisible_looks_offline() {
        let index = index();
        let presence = Presence::default();
        let mut bo = presence.watch(&index, "bo", Some(default_settings()));
        presence.update(&index, "ann", "t1", "desktop", false, vec![playing("Celeste")], Some(default_settings()));
        assert!(drain(&mut bo.rx)[0].activities.is_empty());
        let invisible = pb::PresenceSettings { status: pb::PresenceStatus::Invisible as i32, ..sharing(&[]) };
        presence.settings_changed(&index, "ann", invisible);
        let seen = drain(&mut bo.rx);
        assert_eq!(seen[0].status, pb::PresenceStatus::Offline as i32);
        assert!(seen[0].activities.is_empty() && seen[0].apps.is_empty());
        // ann's own stream sees herself as invisible.
        let ann = presence.watch(&index, "ann", None);
        let own = ann.snapshot.iter().find(|p| p.user_id == "ann").unwrap();
        assert_eq!(own.status, pb::PresenceStatus::Invisible as i32);
    }

    #[test]
    fn idle_only_when_every_app_is() {
        let index = index();
        let presence = Presence::default();
        presence.update(&index, "ann", "t1", "web", true, vec![], Some(default_settings()));
        presence.update(&index, "ann", "t2", "desktop", false, vec![], None);
        let bo = presence.watch(&index, "bo", Some(default_settings()));
        assert_eq!(bo.snapshot[0].status, pb::PresenceStatus::Online as i32);
        assert_eq!(bo.snapshot[0].apps, ["desktop", "web"]);
        presence.update(&index, "ann", "t2", "desktop", true, vec![], None);
        let bo = presence.watch(&index, "bo", None);
        assert_eq!(bo.snapshot[0].status, pb::PresenceStatus::Idle as i32);
    }

    #[test]
    fn bursts_are_merged() {
        let index = index();
        let presence = Presence::default();
        let mut bo = presence.watch(&index, "bo", Some(default_settings()));
        for n in 0..12 {
            presence.update(
                &index,
                "ann",
                "t1",
                "desktop",
                false,
                vec![playing(&format!("Game {n}"))],
                Some(sharing(&[])),
            );
        }
        let sent = drain(&mut bo.rx);
        assert_eq!(sent.len(), BURST);
        assert_eq!(sent.last().unwrap().activities[0].name, "Game 4");
        // Waiting for the window: the latest goes out, once.
        let mut inner = presence.lock();
        let past = Instant::now() - WINDOW;
        inner.people.get_mut("ann").unwrap().sent.iter_mut().for_each(|at| *at = past);
        drop(inner);
        presence.tick(&index);
        let sent = drain(&mut bo.rx);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].activities[0].name, "Game 11");
        presence.tick(&index);
        assert!(drain(&mut bo.rx).is_empty());
    }

    #[test]
    fn lapsed_apps_go_offline_and_joins_introduce() {
        let index = index();
        let presence = Presence::default();
        presence.update(&index, "ann", "t1", "web", false, vec![], Some(default_settings()));
        let mut bo = presence.watch(&index, "bo", Some(default_settings()));
        presence.lock().people.get_mut("ann").unwrap().apps.get_mut("t1").unwrap().expires = Instant::now();
        presence.lock().people.get_mut("ann").unwrap().touched = Instant::now() - LEASE;
        presence.tick(&index);
        assert_eq!(drain(&mut bo.rx)[0].status, pb::PresenceStatus::Offline as i32);
        assert!(!presence.knows("ann"));

        // cy joins s1, where bo is online: each now sees the other.
        presence.update(&index, "bo", "t2", "web", false, vec![], None);
        let mut cy = presence.watch(&index, "cy", Some(default_settings()));
        drain(&mut bo.rx);
        presence.update(&index, "cy", "t3", "web", false, vec![], None);
        index.join("cy", "s1");
        presence.joined(&index, "cy", "s1");
        assert!(drain(&mut cy.rx).iter().any(|p| p.user_id == "bo"));
        assert!(drain(&mut bo.rx).iter().any(|p| p.user_id == "cy"));
    }

    #[test]
    fn activities_are_checked() {
        let link = |url: &str| format!("https://fuwa.test/media/outside/sig?url={url}");
        let mut a = playing("  Celeste\n");
        a.large_image_url = "https://img.example/a.png".into();
        a.small_image_url = "celeste_logo".into();
        a.buttons = vec![pb::ActivityButton { label: "Store".into(), url: "https://store.example/celeste".into() }];
        let checked = check_activities(vec![a.clone()], link).unwrap();
        assert_eq!(checked[0].name, "Celeste");
        assert!(checked[0].large_image_url.starts_with("https://fuwa.test/media/outside/"));
        assert_eq!(checked[0].small_image_url, "");

        for bad in ["http://store.example", "javascript:alert(1)", "https://user:pw@store.example", "store.example"] {
            let mut b = a.clone();
            b.buttons[0].url = bad.into();
            assert!(check_activities(vec![b], link).is_err(), "{bad}");
        }
        let mut long = a.clone();
        long.details = "x".repeat(129);
        assert!(check_activities(vec![long], link).is_err());
        assert!(check_activities(vec![playing("")], link).is_err());
        assert!(check_activities(vec![playing("x"); 6], link).is_err());
        let mut id = a;
        id.application_id = "../etc".into();
        assert!(check_activities(vec![id], link).is_err());
    }

    #[test]
    fn invisible_people_send_nothing_when_their_apps_change() {
        let index = index();
        let presence = Presence::default();
        let mut bo = presence.watch(&index, "bo", Some(default_settings()));
        let invisible = pb::PresenceSettings { status: pb::PresenceStatus::Invisible as i32, ..sharing(&[]) };
        presence.update(&index, "ann", "t1", "web", false, vec![], Some(invisible));
        presence.update(&index, "ann", "t1", "web", true, vec![], None);
        presence.update(&index, "ann", "t2", "desktop", false, vec![playing("Celeste")], None);
        presence.update(&index, "ann", "t2", "desktop", false, vec![], None);
        presence.lock().people.get_mut("ann").unwrap().apps.values_mut().for_each(|app| app.expires = Instant::now());
        presence.tick(&index);
        assert!(drain(&mut bo.rx).is_empty(), "an invisible person's comings and goings never show");
        // Hidden in the only shared server: activity changes send nothing either.
        presence.update(&index, "cy", "t3", "desktop", false, vec![playing("A")], Some(sharing(&["s2"])));
        let mut ann = presence.watch(&index, "ann", None);
        drain(&mut ann.rx);
        presence.update(&index, "cy", "t3", "desktop", false, vec![playing("B")], None);
        presence.update(&index, "cy", "t3", "desktop", false, vec![], None);
        assert!(drain(&mut ann.rx).is_empty());
    }

    #[test]
    fn streams_per_account_are_bounded() {
        let index = index();
        let presence = Presence::default();
        let mut first = presence.watch(&index, "bo", Some(default_settings()));
        for _ in 1..MAX_STREAMS {
            presence.watch(&index, "bo", None);
        }
        assert!(first.rx.try_recv().is_err_and(|e| e == mpsc::error::TryRecvError::Empty));
        let last = presence.watch(&index, "bo", None);
        assert_eq!(presence.lock().watchers["bo"].len(), MAX_STREAMS);
        assert!(first.rx.try_recv().is_err_and(|e| e == mpsc::error::TryRecvError::Disconnected));
        presence.unwatch("bo", last.id);
        assert_eq!(presence.lock().watchers["bo"].len(), MAX_STREAMS - 1);
    }

    #[test]
    fn leaving_shows_offline_both_ways() {
        let index = index();
        let presence = Presence::default();
        presence.update(&index, "ann", "t1", "web", false, vec![], Some(default_settings()));
        presence.update(&index, "bo", "t2", "web", false, vec![], Some(default_settings()));
        // dee is in s1 too, online but invisible.
        index.join("dee", "s1");
        let invisible = pb::PresenceSettings { status: pb::PresenceStatus::Invisible as i32, ..sharing(&[]) };
        presence.update(&index, "dee", "t4", "web", false, vec![], Some(invisible));
        let mut ann = presence.watch(&index, "ann", None);
        let mut bo = presence.watch(&index, "bo", None);
        let mut dee = presence.watch(&index, "dee", None);
        drain(&mut dee.rx);
        index.leave("bo", "s1");
        presence.left(&index, "bo", "s1");
        let to_ann = drain(&mut ann.rx);
        assert_eq!((to_ann[0].user_id.as_str(), to_ann[0].status), ("bo", pb::PresenceStatus::Offline as i32));
        let to_bo = drain(&mut bo.rx);
        assert_eq!(to_bo.len(), 1, "nothing about dee, who bo never saw online: {to_bo:?}");
        assert_eq!((to_bo[0].user_id.as_str(), to_bo[0].status), ("ann", pb::PresenceStatus::Offline as i32));
        assert_eq!(drain(&mut dee.rx).len(), 1);

        // The server goes: nothing about dee either.
        let mut ann = presence.watch(&index, "ann", None);
        drain(&mut ann.rx);
        let members = index.members_where("s1", |_| true);
        index.remove("s1");
        presence.server_gone(&index, "s1", &members);
        assert!(drain(&mut ann.rx).iter().all(|p| p.user_id != "dee"));
    }

    #[test]
    fn on_screen_streams_see_large_servers_only_through_focus() {
        let index = Index::default();
        index.insert(server("big"), vec!["ann".into(), "bo".into(), "cy".into()], vec![], None);
        index.insert(server("small"), vec!["ann".into(), "dee".into()], vec![], None);
        let presence = Presence::with_large(3, ON_SCREEN_MEMBERS);
        for (who, session) in [("bo", "t2"), ("dee", "t4"), ("eve", "t5")] {
            presence.update(&index, who, session, "web", false, vec![], Some(default_settings()));
        }
        let mut everyone = presence.watch(&index, "ann", Some(default_settings()));
        let mut on_screen = presence.watch_on_screen(&index, "ann", None);
        let ids = |list: Vec<pb::Presence>| -> Vec<(String, i32)> {
            let mut ids: Vec<(String, i32)> = list.into_iter().map(|p| (p.user_id, p.status)).collect();
            ids.sort();
            ids
        };
        let (online, offline) = (pb::PresenceStatus::Online as i32, pb::PresenceStatus::Offline as i32);
        assert_eq!(ids(everyone.snapshot.clone()), [("bo".into(), online), ("dee".into(), online)]);
        assert_eq!(ids(on_screen.snapshot.clone()), [("dee".into(), online)]);

        // Joining the large server: only the stream that sees everyone hears of eve.
        index.join("eve", "big");
        presence.joined(&index, "eve", "big");
        assert_eq!(ids(drain(&mut everyone.rx)), [("eve".into(), online)]);
        assert!(drain(&mut on_screen.rx).is_empty());

        // In focus, she comes at once and her changes follow; bo's don't.
        presence.focus(&index, "ann", on_screen.id, vec!["eve".into()]);
        assert_eq!(ids(drain(&mut on_screen.rx)), [("eve".into(), online)]);
        presence.update(&index, "bo", "t2", "web", true, vec![], None);
        presence.update(&index, "eve", "t5", "web", true, vec![], None);
        let idle = pb::PresenceStatus::Idle as i32;
        assert_eq!(ids(drain(&mut on_screen.rx)), [("eve".into(), idle)]);
        assert_eq!(ids(drain(&mut everyone.rx)), [("bo".into(), idle), ("eve".into(), idle)]);

        // Leaving: whoever the stream showed goes offline for it.
        index.leave("eve", "big");
        presence.left(&index, "eve", "big");
        index.leave("dee", "small");
        presence.left(&index, "dee", "small");
        assert_eq!(ids(drain(&mut on_screen.rx)), [("dee".into(), offline), ("eve".into(), offline)]);
        index.leave("bo", "big");
        presence.left(&index, "bo", "big");
        assert!(drain(&mut on_screen.rx).is_empty(), "bo was never on screen");
        assert_eq!(ids(drain(&mut everyone.rx)).len(), 3);
    }

    #[test]
    fn on_screen_streams_follow_their_smallest_servers_up_to_a_bound() {
        let index = Index::default();
        let ids = |n: usize, tag: &str| -> Vec<String> {
            std::iter::once("ann".to_string()).chain((1..n).map(|i| format!("{tag}{i}"))).collect()
        };
        // 3 + 4 + 5 members; a bound of 8 follows the two smallest.
        index.insert(server("three"), ids(3, "a"), vec![], None);
        index.insert(server("four"), ids(4, "b"), vec![], None);
        index.insert(server("five"), ids(5, "c"), vec![], None);
        let presence = Presence::with_large(0, 8);
        let mut on_screen = presence.watch_on_screen(&index, "ann", Some(default_settings()));
        for who in ["a1", "b1", "c1"] {
            presence.update(&index, who, who, "web", false, vec![], Some(default_settings()));
        }
        let heard: Vec<String> = drain(&mut on_screen.rx).into_iter().map(|p| p.user_id).collect();
        assert_eq!(heard, ["a1", "b1"], "the third server counts as large");
        presence.focus(&index, "ann", on_screen.id, vec!["c1".into()]);
        assert_eq!(drain(&mut on_screen.rx)[0].user_id, "c1");
        // Leaving a followed server gives its room back for the next one.
        index.leave("ann", "three");
        presence.left(&index, "ann", "three");
        index.insert(server("other"), vec!["d1".into(), "d2".into(), "d3".into()], vec![], None);
        index.join("ann", "other");
        presence.joined(&index, "ann", "other");
        drain(&mut on_screen.rx);
        presence.update(&index, "d1", "d1", "web", false, vec![], Some(default_settings()));
        let heard: Vec<String> = drain(&mut on_screen.rx).into_iter().map(|p| p.user_id).collect();
        assert_eq!(heard, ["d1"]);
    }

    #[test]
    fn settings_are_never_guessed_and_newer_ones_win() {
        let index = index();
        let presence = Presence::default();
        assert!(!presence.update(&index, "ann", "t1", "web", false, vec![], None));
        assert!(!presence.knows("ann"));
        let invisible = pb::PresenceSettings { status: pb::PresenceStatus::Invisible as i32, ..sharing(&[]) };
        presence.settings_changed(&index, "ann", invisible);
        // A copy read from disk before that change arrives late: it loses.
        assert!(presence.update(&index, "ann", "t1", "web", false, vec![], Some(default_settings())));
        let bo = presence.watch(&index, "bo", Some(default_settings()));
        assert!(bo.snapshot.is_empty());
    }

    #[test]
    fn new_pictures_per_account_are_bounded() {
        let presence = Presence::default();
        for n in 0..MAX_NEW_PICTURES {
            assert!(presence.allow_picture("ann", &format!("https://img.example/{n}.png")));
        }
        assert!(presence.allow_picture("ann", "https://img.example/0.png"), "already signed this hour");
        assert!(!presence.allow_picture("ann", "https://img.example/new.png"));
        assert!(presence.allow_picture("bo", "https://img.example/new.png"));
    }

    #[test]
    fn hidden_characters_are_stripped() {
        let link = |url: &str| url.to_string();
        let checked = check_activities(vec![playing("Ce\u{202E}les\u{200B}te\u{2066}")], link).unwrap();
        assert_eq!(checked[0].name, "Celeste");
        let family = "\u{1F468}\u{200D}\u{1F469}";
        assert_eq!(check_activities(vec![playing(family)], link).unwrap()[0].name, family);
    }
}
