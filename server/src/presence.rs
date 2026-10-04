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

pub const MAX_ACTIVITIES: usize = 5;
const MAX_TEXT: usize = 128;
const MAX_LABEL: usize = 32;
const MAX_BUTTONS: usize = 2;
const MAX_BUTTON_URL: usize = 512;
const MAX_PICTURE_URL: usize = 2048;
const MAX_PARTY: u32 = 1_000_000;
const MAX_APPLICATION_ID: usize = 64;
/// More hidden servers than anyone could be in.
pub const MAX_HIDDEN_SERVERS: usize = 1000;

#[derive(Default)]
pub struct Presence {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    people: HashMap<String, Person>,
    /// Account id to its open streams.
    watchers: HashMap<String, Vec<Watcher>>,
    /// People with a change waiting for their window.
    pending: HashSet<String>,
    next_watcher: u64,
}

struct Watcher {
    id: u64,
    tx: mpsc::Sender<pb::Presence>,
}

struct Person {
    settings: pb::PresenceSettings,
    /// Session (token hash) to what that app said.
    apps: HashMap<String, App>,
    /// When their last changes went out.
    sent: VecDeque<Instant>,
    /// Their presence as last sent, as they see it themselves.
    last: Option<pb::Presence>,
}

struct App {
    kind: String,
    idle: bool,
    activities: Vec<pb::Activity>,
    updated: Instant,
    expires: Instant,
}

impl Person {
    fn new(settings: pb::PresenceSettings) -> Self {
        Self { settings, apps: HashMap::new(), sent: VecDeque::new(), last: None }
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
        self.settings.show_activity && servers.iter().any(|id| !self.settings.hidden_server_ids.contains(id))
    }
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
    /// streams.
    fn broadcast(&mut self, index: &Index, user_id: &str, now: Instant) {
        let Some(person) = self.people.get_mut(user_id) else { return };
        let own = person.own(user_id);
        person.sent.push_back(now);
        while person.sent.len() > BURST {
            person.sent.pop_front();
        }
        person.last = Some(own.clone());
        let with = seen(&own, true);
        let without = seen(&own, false);
        let watchers = &self.watchers;
        let audience = index.neighbours(user_id, |id| watchers.contains_key(id));
        let mut behind = Vec::new();
        for (watcher, shared) in audience {
            let presence = if person.shows_activity_in(&shared) { &with } else { &without };
            send(&self.watchers[&watcher], presence, &mut behind);
        }
        if let Some(own_streams) = self.watchers.get(user_id) {
            send(own_streams, &own, &mut behind);
        }
        self.drop_streams(&behind);
    }

    /// Ends streams that fell behind: they see their channel close and tell
    /// the app to watch again.
    fn drop_streams(&mut self, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        crate::reports::server_error("presence_stream_behind", Some("presence"));
        self.watchers.retain(|_, streams| {
            streams.retain(|stream| !ids.contains(&stream.id));
            !streams.is_empty()
        });
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

    /// Forgets someone with no app and no stream.
    fn tidy(&mut self, user_id: &str) {
        let idle = self.people.get(user_id).is_some_and(|p| p.apps.is_empty())
            && !self.watchers.contains_key(user_id)
            && !self.pending.contains(user_id);
        if idle {
            self.people.remove(user_id);
        }
    }
}

fn send(streams: &[Watcher], presence: &pb::Presence, behind: &mut Vec<u64>) {
    for stream in streams {
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

    /// What an app says about its person. `settings` is used only when the
    /// person isn't known here yet.
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
    ) {
        let now = Instant::now();
        let mut inner = self.lock();
        let person = inner
            .people
            .entry(user_id.to_string())
            .or_insert_with(|| Person::new(settings.unwrap_or_else(default_settings)));
        person.apps.insert(
            session.to_string(),
            App { kind: kind.to_string(), idle, activities, updated: now, expires: now + LEASE },
        );
        inner.changed(index, user_id, now, false);
    }

    /// Someone changed their settings: everyone may now see them differently.
    pub fn settings_changed(&self, index: &Index, user_id: &str, settings: pb::PresenceSettings) {
        let now = Instant::now();
        let mut inner = self.lock();
        match inner.people.get_mut(user_id) {
            Some(person) => person.settings = settings,
            None => return,
        }
        inner.changed(index, user_id, now, true);
    }

    /// Opens a stream for `user_id`: everyone online they may see, then each
    /// change.
    pub fn watch(&self, index: &Index, user_id: &str, settings: Option<pb::PresenceSettings>) -> Watch {
        let mut inner = self.lock();
        inner.next_watcher += 1;
        let id = inner.next_watcher;
        let (tx, rx) = mpsc::channel(STREAM_BUFFER);
        inner.watchers.entry(user_id.to_string()).or_default().push(Watcher { id, tx });
        if let Some(settings) = settings {
            inner.people.entry(user_id.to_string()).or_insert_with(|| Person::new(settings));
        }
        let people = &inner.people;
        let mut snapshot = Vec::new();
        for (other, shared) in index.neighbours(user_id, |id| people.get(id).is_some_and(|p| !p.apps.is_empty())) {
            let person = &people[&other];
            let presence = seen(&person.own(&other), person.shows_activity_in(&shared));
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
        let mut inner = self.lock();
        if let Some(streams) = inner.watchers.get_mut(user_id) {
            streams.retain(|stream| stream.id != id);
            if streams.is_empty() {
                inner.watchers.remove(user_id);
            }
        }
        inner.tidy(user_id);
    }

    /// Someone joined a server: they and its members now see each other.
    pub fn joined(&self, index: &Index, user_id: &str, server_id: &str) {
        let mut inner = self.lock();
        let online = |p: Option<&Person>| p.is_some_and(|p| !p.apps.is_empty());
        let mut behind = Vec::new();
        // The server's members who are watching see the newcomer, as last sent.
        if let Some(person) = inner.people.get(user_id).filter(|p| !p.apps.is_empty())
            && let Some(own) = &person.last
        {
            let watchers = &inner.watchers;
            for other in index.members_where(server_id, |id| id != user_id && watchers.contains_key(id)) {
                let shared = index.shared_servers(user_id, &other);
                send(&watchers[&other], &seen(own, person.shows_activity_in(&shared)), &mut behind);
            }
        }
        // And the newcomer sees who's online there.
        if let Some(streams) = inner.watchers.get(user_id) {
            let people = &inner.people;
            for other in index.members_where(server_id, |id| id != user_id && online(people.get(id))) {
                let person = &people[&other];
                let shared = index.shared_servers(&other, user_id);
                let presence = seen(&person.own(&other), person.shows_activity_in(&shared));
                if presence.status != pb::PresenceStatus::Offline as i32 {
                    send(streams, &presence, &mut behind);
                }
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
        for user_id in lapsed {
            inner.tidy(&user_id);
        }
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
    // Line breaks and other control characters would only break layouts.
    let value: String = value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
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
}
