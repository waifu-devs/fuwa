//! Typing indicators (docs/typing.md): who's writing a message where you're
//! looking, and saying so for you while you write one.
//!
//! What others are typing comes as `TypingStarted` server events (sequence 0,
//! never replayed, so they move no cursor and count as nothing unread) and as
//! the direct-message feed's `DmTyping`. Each one shows its person as typing
//! in that place for [`SHOWN_FOR`], or until a message from them arrives
//! there. It's kept here, apart from the store, so a typing line coming and
//! going redraws the window ([`Area::Window`]) without building the message
//! list again. Nothing about typing is logged, kept on disk or reported.
//!
//! Saying you're typing goes at most every [`SEND_EVERY`] per place, and
//! sending a message there lets the next keystroke say it again at once.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::core::Area;
use crate::core::api::Api;
use crate::pb;

/// How long someone shows as typing after the last word that they are.
pub const SHOWN_FOR: Duration = Duration::from_secs(10);
/// The least time between two "I'm typing" signals for one place.
pub const SEND_EVERY: Duration = Duration::from_secs(6);
/// The instance feature that has typing.
pub const FEATURE: &str = "typing";

/// Where someone types: a channel (or a thread's reply box in it), or a
/// conversation, on one instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    pub instance: String,
    /// The channel or conversation.
    pub room: String,
    /// The thread's parent message id, empty outside a thread.
    pub thread: String,
}

impl Place {
    pub fn channel(instance: &str, channel: &str, thread: &str) -> Self {
        Self { instance: instance.to_owned(), room: channel.to_owned(), thread: thread.to_owned() }
    }

    pub fn dm(instance: &str, conversation: &str) -> Self {
        Self::channel(instance, conversation, "")
    }
}

/// Who's typing where, and when this app last said you were.
#[derive(Debug, Default)]
pub struct Typing {
    /// Per place, each person typing and when that stops showing, in the
    /// order they started.
    seen: HashMap<Place, Vec<(String, Instant)>>,
    /// When "I'm typing" last went out, per place.
    sent: HashMap<Place, Instant>,
}

impl Typing {
    /// Someone is typing. True when the line changes (someone new).
    pub fn started(&mut self, place: Place, user: &str, now: Instant) -> bool {
        let until = now + SHOWN_FOR;
        let list = self.seen.entry(place).or_default();
        match list.iter_mut().find(|(id, _)| id == user) {
            Some((_, at)) => {
                let shown = *at > now;
                *at = until;
                !shown
            }
            None => {
                list.push((user.to_owned(), until));
                true
            }
        }
    }

    /// A message from someone arrived in a place: they're no longer typing there.
    pub fn stopped(&mut self, place: &Place, user: &str, now: Instant) -> bool {
        let Some(list) = self.seen.get_mut(place) else { return false };
        let before = list.iter().any(|(id, at)| id == user && *at > now);
        list.retain(|(id, _)| id != user);
        if list.is_empty() {
            self.seen.remove(place);
        }
        before
    }

    /// A message from someone arrived somewhere in a room (a secure
    /// channel's, whose thread can't be told): they're no longer typing in it.
    pub fn stopped_in(&mut self, instance: &str, room: &str, user: &str, now: Instant) -> bool {
        let places: Vec<Place> =
            self.seen.keys().filter(|p| p.instance == instance && p.room == room).cloned().collect();
        let mut any = false;
        for place in &places {
            any |= self.stopped(place, user, now);
        }
        any
    }

    /// Forgets whoever stopped showing. True when someone did.
    pub fn prune(&mut self, now: Instant) -> bool {
        let mut gone = false;
        self.seen.retain(|_, list| {
            let before = list.len();
            list.retain(|(_, at)| *at > now);
            gone |= list.len() != before;
            !list.is_empty()
        });
        gone
    }

    /// Who's typing in a place now, in the order they started.
    pub fn who(&self, place: &Place, now: Instant) -> Vec<String> {
        self.seen
            .get(place)
            .map(|list| list.iter().filter(|(_, at)| *at > now).map(|(id, _)| id.clone()).collect())
            .unwrap_or_default()
    }

    /// Whether "I'm typing" should go out for a place now; when it should,
    /// it counts as sent.
    pub fn should_send(&mut self, place: &Place, now: Instant) -> bool {
        if self.sent.get(place).is_some_and(|at| now.duration_since(*at) < SEND_EVERY) {
            return false;
        }
        self.sent.retain(|_, at| now.duration_since(*at) < SEND_EVERY);
        self.sent.insert(place.clone(), now);
        true
    }

    /// A message went in a place: typing again there says so at once.
    pub fn sent_message(&mut self, place: &Place) {
        self.sent.remove(place);
    }
}

/// The typing line's words: a translation key and the names to put in it,
/// bold. None when nobody is typing.
pub fn label(names: &[String]) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    Some(match names {
        [] => return None,
        [a] => ("chat.typing.one", vec![("name", a.clone())]),
        [a, b] => ("chat.typing.two", vec![("first", a.clone()), ("second", b.clone())]),
        [a, b, c] => ("chat.typing.three", vec![("first", a.clone()), ("second", b.clone()), ("third", c.clone())]),
        _ => ("chat.typing.several", vec![]),
    })
}

/// A typing person's name in a server, as the member list shows it: their
/// nickname there, else their display name, else their username.
pub fn name_in(i: &crate::core::store::InstanceState, server: &str, user_id: &str) -> String {
    let member =
        i.members.get(server).and_then(|m| m.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)));
    if let Some(m) = member.filter(|m| !m.nickname.is_empty()) {
        return m.nickname.clone();
    }
    match i.users.get(user_id).or_else(|| member.and_then(|m| m.user.as_ref())) {
        Some(user) => crate::core::store::user_name(user),
        None => crate::core::i18n::t("chat.typing.someone"),
    }
}

impl crate::core::Shared {
    /// Someone started (or is still) typing in a place; your own signals
    /// (from another device) are left out. Their line goes once
    /// [`SHOWN_FOR`] passes without another.
    pub(crate) fn typing_started(&self, place: Place, user: &str) {
        let mine =
            self.read(|s| s.instance(&place.instance).and_then(|i| i.me.as_ref()).is_none_or(|me| me.id == user));
        if mine || user.is_empty() {
            return;
        }
        if self.typing.lock().started(place, user, Instant::now()) {
            self.poke(&[Area::Window]);
        }
        // Gone once nobody says otherwise.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let shared = self.clone();
            runtime.spawn(async move {
                tokio::time::sleep(SHOWN_FOR + Duration::from_millis(50)).await;
                if shared.typing.lock().prune(Instant::now()) {
                    shared.poke(&[Area::Window]);
                }
            });
        }
    }

    /// A message from someone arrived in a place.
    pub(crate) fn typing_stopped(&self, place: &Place, user: &str) {
        if self.typing.lock().stopped(place, user, Instant::now()) {
            self.poke(&[Area::Window]);
        }
    }

    /// A message from someone arrived somewhere in a secure channel.
    pub(crate) fn typing_stopped_in(&self, instance: &str, room: &str, user: &str) {
        if self.typing.lock().stopped_in(instance, room, user, Instant::now()) {
            self.poke(&[Area::Window]);
        }
    }

    /// Who's typing in a place now (user ids), in the order they started.
    pub fn typing_in(&self, place: &Place) -> Vec<String> {
        self.typing.lock().who(place, Instant::now())
    }
}

impl crate::core::Core {
    /// You typed in a place's box: tells the instance, at most every
    /// [`SEND_EVERY`], unless the "Show when I'm typing" setting is off or
    /// the instance doesn't have typing. A server channel is `server` with
    /// `place.room` its channel; a conversation has no server. Whether you
    /// may write there is the caller's to know. Fire and forget: a failure
    /// is dropped, never retried, said or reported.
    pub fn typed(self: &std::sync::Arc<Self>, server: Option<&str>, place: Place) {
        if !self.prefs_now().send_typing {
            return;
        }
        let has = self.shared.read(|s| s.instance(&place.instance).is_some_and(|i| i.has(FEATURE)));
        if !has || !self.shared.typing.lock().should_send(&place, Instant::now()) {
            return;
        }
        let Some(api) = self.api(&place.instance) else { return };
        let server = server.map(str::to_owned);
        // Not `rpc!`: nothing about typing is timed or counted in reports.
        drop(self.spawn(async move { send(api, server, place).await }));
    }

    /// A message went in a place: the next keystroke there says you're typing at once.
    pub(crate) fn typing_sent(&self, place: &Place) {
        self.shared.typing.lock().sent_message(place);
    }
}

async fn send(api: Api, server: Option<String>, place: Place) {
    let _ = match server {
        Some(server_id) => {
            let req = pb::SendTypingRequest { server_id, channel_id: place.room, thread_id: place.thread };
            crate::core::api::call(async move { api.messages().send_typing(req).await }).await.map(drop)
        }
        None => {
            let req = pb::SendConversationTypingRequest { conversation_id: place.room };
            crate::core::api::call(async move { api.dms().send_conversation_typing(req).await }).await.map(drop)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here() -> Place {
        Place::channel("i", "c", "")
    }

    #[test]
    fn someone_shows_for_ten_seconds_after_their_last_signal() {
        let mut t = Typing::default();
        let now = Instant::now();
        assert!(t.started(here(), "ana", now));
        // Said again: still showing, nothing new to draw.
        assert!(!t.started(here(), "ana", now + Duration::from_secs(5)));
        assert_eq!(t.who(&here(), now + Duration::from_secs(14)), vec!["ana"]);
        assert!(t.who(&here(), now + Duration::from_secs(15)).is_empty());
        assert!(t.prune(now + Duration::from_secs(15)));
        assert!(!t.prune(now + Duration::from_secs(16)));
        // Back after going: shows again.
        assert!(t.started(here(), "ana", now + Duration::from_secs(20)));
    }

    #[test]
    fn a_message_from_them_ends_their_line_in_that_place_only() {
        let mut t = Typing::default();
        let now = Instant::now();
        let thread = Place::channel("i", "c", "m1");
        t.started(here(), "ana", now);
        t.started(here(), "ben", now);
        t.started(thread.clone(), "ana", now);
        assert!(t.stopped(&here(), "ana", now));
        assert!(!t.stopped(&here(), "ana", now));
        assert_eq!(t.who(&here(), now), vec!["ben"]);
        assert_eq!(t.who(&thread, now), vec!["ana"]);
        assert!(t.who(&Place::dm("i", "c2"), now).is_empty());
        // A sealed message ends their line anywhere in its channel.
        t.started(here(), "ana", now);
        assert!(t.stopped_in("i", "c", "ana", now));
        assert_eq!(t.who(&here(), now), vec!["ben"]);
        assert!(t.who(&thread, now).is_empty());
        assert!(!t.stopped_in("i", "c", "ana", now));
    }

    #[test]
    fn people_are_listed_in_the_order_they_started() {
        let mut t = Typing::default();
        let now = Instant::now();
        for (n, who) in ["cy", "ana", "ben"].iter().enumerate() {
            t.started(here(), who, now + Duration::from_millis(n as u64));
        }
        t.started(here(), "cy", now + Duration::from_secs(3));
        assert_eq!(t.who(&here(), now + Duration::from_secs(4)), vec!["cy", "ana", "ben"]);
    }

    #[test]
    fn typing_goes_out_at_most_every_six_seconds_until_a_message_is_sent() {
        let mut t = Typing::default();
        let now = Instant::now();
        let dm = Place::dm("i", "d");
        assert!(t.should_send(&here(), now));
        assert!(!t.should_send(&here(), now + Duration::from_secs(5)));
        // Each place on its own.
        assert!(t.should_send(&dm, now + Duration::from_secs(1)));
        assert!(t.should_send(&here(), now + Duration::from_secs(6)));
        assert!(!t.should_send(&here(), now + Duration::from_secs(7)));
        t.sent_message(&here());
        assert!(t.should_send(&here(), now + Duration::from_secs(7)));
    }

    #[test]
    fn the_line_names_up_to_three_people() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(label(&[]), None);
        assert_eq!(label(&names(&["Ana"])), Some(("chat.typing.one", vec![("name", "Ana".into())])));
        assert_eq!(
            label(&names(&["Ana", "Ben"])),
            Some(("chat.typing.two", vec![("first", "Ana".into()), ("second", "Ben".into())]))
        );
        assert_eq!(
            label(&names(&["Ana", "Ben", "Cy"])),
            Some((
                "chat.typing.three",
                vec![("first", "Ana".into()), ("second", "Ben".into()), ("third", "Cy".into())]
            ))
        );
        assert_eq!(label(&names(&["Ana", "Ben", "Cy", "Dee"])), Some(("chat.typing.several", vec![])));
    }

    #[test]
    fn the_english_words_read_as_the_web_has_them() {
        use crate::core::i18n::{catalog, fill, template};
        let en = catalog("en");
        let say = |key: &str, values: &[(&str, &str)]| {
            fill(&template("one-other", &en, &en, key, None), |name| {
                values.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
            })
        };
        assert_eq!(say("chat.typing.one", &[("name", "Ana")]), "Ana is typing");
        assert_eq!(say("chat.typing.two", &[("first", "Ana"), ("second", "Ben")]), "Ana and Ben are typing");
        assert_eq!(
            say("chat.typing.three", &[("first", "Ana"), ("second", "Ben"), ("third", "Cy")]),
            "Ana, Ben and Cy are typing"
        );
        assert_eq!(say("chat.typing.several", &[]), "Several people are typing");
        // Spanish has them too.
        let es = catalog("es");
        for key in ["chat.typing.one", "chat.typing.two", "chat.typing.three", "chat.typing.several"] {
            assert!(es.contains_key(key), "{key}");
        }
    }
}
