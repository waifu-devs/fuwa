//! Ephemeral messages (docs/ephemeral-messages.md): what an agent sent for
//! one person's eyes, for a while, like an answer to a command.
//!
//! They come as `EphemeralMessageCreated` server events (sequence 0, never
//! stored or replayed, so they move no cursor) and are kept here, in memory,
//! per instance and channel, until they run out or you dismiss them. They
//! count as nothing unread, never notify, move no read mark and are never
//! written to disk, so they're gone when the app closes.

use std::collections::HashMap;
use std::time::Duration;

use crate::core::Area;
use crate::pb;

/// The longest an instance lets one show (`expires_in_seconds` up to 3600).
pub const LONGEST_MS: i64 = 60 * 60 * 1000;
/// How long one without an end shows (the instance's own default).
pub const DEFAULT_MS: i64 = 15 * 60 * 1000;

/// When a message stops showing, in unix milliseconds (0 when it isn't ephemeral).
pub fn expires_ms(m: &pb::Message) -> i64 {
    m.ephemeral
        .as_ref()
        .and_then(|e| e.expires_at.as_ref())
        .map_or(0, |t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

/// What the line under one says about the time it has left: whole minutes,
/// rounded up, or None when it's under a minute.
pub fn minutes_left(expires: i64, now: i64) -> Option<i64> {
    let left = expires - now;
    (left >= 60_000).then(|| (left + 59_999) / 60_000)
}

/// How long until what [`minutes_left`] says changes (or it runs out).
pub fn next_change_ms(expires: i64, now: i64) -> i64 {
    let left = (expires - now).max(0);
    match minutes_left(expires, now) {
        None => left,
        // Exactly a minute: under one next.
        Some(1) => 1,
        Some(n) => left - (n - 1) * 60_000,
    }
}

/// A channel's messages and the ephemeral ones shown in it, by time: each
/// ephemeral one after the messages from its moment or before, so it sits at
/// the bottom unless older ones arrived since. The messages keep their order.
pub fn merge<'a>(items: &'a [pb::Message], extra: &'a [pb::Message]) -> Vec<&'a pb::Message> {
    let at = |m: &pb::Message| m.created_at.as_ref().map_or((0, 0), |t| (t.seconds, t.nanos));
    let mut extra: Vec<&pb::Message> = extra.iter().collect();
    extra.sort_by_key(|m| at(m));
    let mut out = Vec::with_capacity(items.len() + extra.len());
    let mut rest = extra.into_iter().peekable();
    for m in items {
        while let Some(e) = rest.next_if(|e| at(e) < at(m)) {
            out.push(e);
        }
        out.push(m);
    }
    out.extend(rest);
    out
}

/// The ephemeral messages for you, per instance and channel.
#[derive(Debug, Default)]
pub struct Ephemeral {
    shown: HashMap<(String, String), Vec<pb::Message>>,
}

impl Ephemeral {
    /// One arrived for you. Its end is kept to the hour an instance allows
    /// (and 15 minutes when it has none). Its end, or None when it isn't
    /// one or has already run out.
    pub fn add(&mut self, instance: &str, mut m: pb::Message, now: i64) -> Option<i64> {
        if m.ephemeral.is_none() || m.channel_id.is_empty() {
            return None;
        }
        let asked = expires_ms(&m);
        let until = if asked == 0 { now + DEFAULT_MS } else { asked.min(now + LONGEST_MS) };
        if until <= now {
            return None;
        }
        m.ephemeral.as_mut()?.expires_at = Some(prost_types::Timestamp {
            seconds: until.div_euclid(1000),
            nanos: (until.rem_euclid(1000) * 1_000_000) as i32,
        });
        let list = self.shown.entry((instance.to_owned(), m.channel_id.clone())).or_default();
        list.retain(|x| x.id != m.id);
        list.push(m);
        Some(until)
    }

    /// You dismissed one. True when it was there.
    pub fn dismiss(&mut self, instance: &str, channel: &str, id: &str) -> bool {
        let place = (instance.to_owned(), channel.to_owned());
        let Some(list) = self.shown.get_mut(&place) else { return false };
        let before = list.len();
        list.retain(|m| m.id != id);
        let gone = list.len() != before;
        if list.is_empty() {
            self.shown.remove(&place);
        }
        gone
    }

    /// Forgets the ones that ran out; the channels (instance, channel) that lost one.
    pub fn prune(&mut self, now: i64) -> Vec<(String, String)> {
        let mut lost = Vec::new();
        self.shown.retain(|place, list| {
            let before = list.len();
            list.retain(|m| expires_ms(m) > now);
            if list.len() != before {
                lost.push(place.clone());
            }
            !list.is_empty()
        });
        lost
    }

    /// The ones still showing in a channel for `me`, oldest first.
    pub fn live(&self, instance: &str, channel: &str, me: &str, now: i64) -> Vec<pb::Message> {
        let Some(list) = self.shown.get(&(instance.to_owned(), channel.to_owned())) else { return Vec::new() };
        list.iter()
            .filter(|m| m.ephemeral.as_ref().is_some_and(|e| e.user_id == me) && expires_ms(m) > now)
            .cloned()
            .collect()
    }
}

impl crate::core::Shared {
    /// An ephemeral message arrived: kept when it's for you, until it runs out.
    pub(crate) fn ephemeral_arrived(&self, instance: &str, m: pb::Message) {
        let mine = self.read(|s| {
            s.instance(instance)
                .and_then(|i| i.me.as_ref())
                .is_some_and(|me| m.ephemeral.as_ref().is_some_and(|e| !me.id.is_empty() && e.user_id == me.id))
        });
        if !mine {
            return;
        }
        let now = crate::core::dms::now_ms();
        let channel = m.channel_id.clone();
        let Some(until) = self.ephemeral.lock().add(instance, m, now) else { return };
        self.poke(&[Area::Messages { instance, channel: &channel }]);
        // Gone once it runs out.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let shared = self.clone();
            runtime.spawn(async move {
                let left = (until - crate::core::dms::now_ms()).max(0) as u64;
                tokio::time::sleep(Duration::from_millis(left + 50)).await;
                shared.ephemeral_prune();
            });
        }
    }

    /// Forgets the ephemeral messages that ran out, redrawing their channels.
    fn ephemeral_prune(&self) {
        let lost = self.ephemeral.lock().prune(crate::core::dms::now_ms());
        for (instance, channel) in &lost {
            self.poke(&[Area::Messages { instance, channel }]);
        }
    }

    /// You dismissed an ephemeral message: it goes, here only.
    pub fn ephemeral_dismiss(&self, instance: &str, channel: &str, id: &str) {
        if self.ephemeral.lock().dismiss(instance, channel, id) {
            self.poke(&[Area::Messages { instance, channel }]);
        }
    }

    /// The ephemeral messages showing in a channel for `me`, oldest first.
    pub fn ephemeral_in(&self, instance: &str, channel: &str, me: &str) -> Vec<pb::Message> {
        self.ephemeral.lock().live(instance, channel, me, crate::core::dms::now_ms())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, at_ms: i64, expires: Option<i64>) -> pb::Message {
        let ts = |ms: i64| prost_types::Timestamp { seconds: ms / 1000, nanos: ((ms % 1000) * 1_000_000) as i32 };
        pb::Message {
            id: id.into(),
            channel_id: "c".into(),
            created_at: Some(ts(at_ms)),
            ephemeral: expires.map(|e| pb::EphemeralInfo { user_id: "me".into(), expires_at: (e > 0).then(|| ts(e)) }),
            ..Default::default()
        }
    }

    fn ids(list: &[&pb::Message]) -> Vec<String> {
        list.iter().map(|m| m.id.clone()).collect()
    }

    #[test]
    fn minutes_left_round_up_until_the_last_minute() {
        let now = 1_000_000;
        assert_eq!(minutes_left(now + 15 * 60_000, now), Some(15));
        assert_eq!(minutes_left(now + 14 * 60_000 + 1, now), Some(15));
        assert_eq!(minutes_left(now + 60_001, now), Some(2));
        assert_eq!(minutes_left(now + 60_000, now), Some(1));
        assert_eq!(minutes_left(now + 59_999, now), None);
        assert_eq!(minutes_left(now - 5, now), None);
    }

    #[test]
    fn the_next_change_is_when_the_minutes_step_down() {
        let now = 1_000_000;
        assert_eq!(next_change_ms(now + 15 * 60_000, now), 60_000);
        assert_eq!(next_change_ms(now + 90_000, now), 30_000);
        // At a minute exactly, under one next; under one, its end.
        assert_eq!(next_change_ms(now + 60_000, now), 1);
        assert_eq!(next_change_ms(now + 20_000, now), 20_000);
        assert_eq!(next_change_ms(now - 20_000, now), 0);
        // Each step lands on a new label.
        let exp = now + 150_000;
        let mut at = now;
        let mut seen = vec![minutes_left(exp, at)];
        while at < exp {
            at += next_change_ms(exp, at).max(1);
            seen.push(minutes_left(exp, at));
        }
        assert_eq!(seen, vec![Some(3), Some(2), Some(1), None, None]);
    }

    #[test]
    fn ephemeral_ones_go_in_by_time_after_their_moment() {
        let items = vec![msg("a", 1_000, None), msg("b", 2_000, None), msg("c", 3_000, None)];
        let extra = vec![msg("late", 9_000, Some(1)), msg("same", 2_000, Some(1)), msg("first", 500, Some(1))];
        assert_eq!(ids(&merge(&items, &extra)), ["first", "a", "b", "same", "c", "late"]);
        assert_eq!(ids(&merge(&items, &[])), ["a", "b", "c"]);
        assert_eq!(ids(&merge(&[], &extra)), ["first", "same", "late"]);
    }

    #[test]
    fn they_show_for_their_person_until_they_run_out_or_are_dismissed() {
        let now = 100_000;
        let mut e = Ephemeral::default();
        assert_eq!(e.add("i", msg("m1", now, Some(now + 60_000)), now), Some(now + 60_000));
        assert_eq!(e.add("i", msg("m2", now, Some(now + 120_000)), now), Some(now + 120_000));
        // Not ephemeral, already over, or no channel: not kept.
        assert_eq!(e.add("i", msg("x", now, None), now), None);
        assert_eq!(e.add("i", msg("y", now, Some(now - 1)), now), None);
        let mut nowhere = msg("z", now, Some(now + 1_000));
        nowhere.channel_id.clear();
        assert_eq!(e.add("i", nowhere, now), None);
        let live = |e: &Ephemeral, me: &str, at: i64| -> Vec<String> {
            e.live("i", "c", me, at).into_iter().map(|m| m.id).collect()
        };
        assert_eq!(live(&e, "me", now), ["m1", "m2"]);
        assert!(live(&e, "someone else", now).is_empty());
        assert!(e.live("i", "other channel", "me", now).is_empty());
        // The first runs out; pruning forgets it and names its channel.
        assert_eq!(live(&e, "me", now + 60_000), ["m2"]);
        assert_eq!(e.prune(now + 60_000), vec![("i".to_owned(), "c".to_owned())]);
        assert!(e.prune(now + 60_000).is_empty());
        assert!(e.dismiss("i", "c", "m2"));
        assert!(!e.dismiss("i", "c", "m2"));
        assert!(live(&e, "me", now).is_empty());
    }

    #[test]
    fn its_end_is_kept_within_the_hour_and_defaults_to_fifteen_minutes() {
        let now = 100_000;
        let mut e = Ephemeral::default();
        e.add("i", msg("long", now, Some(now + 5 * LONGEST_MS)), now);
        e.add("i", msg("none", now, Some(0)), now);
        let ends: Vec<i64> = e.live("i", "c", "me", now).iter().map(expires_ms).collect();
        assert_eq!(ends, vec![now + LONGEST_MS, now + DEFAULT_MS]);
        // The same one again replaces it.
        e.add("i", msg("none", now, Some(now + 1_000)), now);
        let ends: Vec<i64> = e.live("i", "c", "me", now).iter().map(expires_ms).collect();
        assert_eq!(ends, vec![now + LONGEST_MS, now + 1_000]);
    }

    #[test]
    fn the_english_words_read_as_the_web_has_them() {
        use crate::core::i18n::{catalog, fill, template};
        let en = catalog("en");
        let say = |key: &str, count: Option<i64>| {
            fill(&template("one-other", &en, &en, key, count), |name| {
                (name == "count").then(|| format!("{}", count.unwrap_or(0)))
            })
        };
        assert_eq!(say("chat.ephemeral.onlyYou", None), "Only you can see this");
        assert_eq!(say("chat.ephemeral.minutesLeft", Some(1)), "Disappears in 1 minute");
        assert_eq!(say("chat.ephemeral.minutesLeft", Some(12)), "Disappears in 12 minutes");
        assert_eq!(say("chat.ephemeral.underAMinute", None), "Disappears in under a minute");
        assert_eq!(say("chat.ephemeral.dismiss", None), "Dismiss message");
    }
}
