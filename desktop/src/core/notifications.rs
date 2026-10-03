//! How a message reaches you: your settings for its channel, else for its
//! server (both kept on the instance, so they follow you to every device),
//! else the server's default, else this computer's own settings. A port of
//! `web/src/lib/notifications.ts`.

use crate::core::config::{NotifyFor, Prefs};
use crate::core::store::InstanceState;
use crate::pb::{self, NotificationLevel as Level};

/// Where settings for a server, or one of its channels, are kept.
pub fn key(server_id: &str, channel_id: &str) -> String {
    format!("{server_id}/{channel_id}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    /// Unspecified when neither you nor the server's default says, so this computer decides.
    pub level: Level,
    pub muted: bool,
    pub suppress_everyone: bool,
}

/// Whether settings mute, right now.
pub fn is_muted(n: Option<&pb::NotificationSettings>, now_ms: i64) -> bool {
    let Some(n) = n.filter(|n| n.muted) else { return false };
    match &n.muted_until {
        None => true,
        Some(t) => t.seconds * 1000 + i64::from(t.nanos) / 1_000_000 > now_ms,
    }
}

impl InstanceState {
    pub fn notification_settings(&self, server_id: &str, channel_id: &str) -> Option<&pb::NotificationSettings> {
        self.notifications.get(&key(server_id, channel_id))
    }

    pub fn effective_notifications(&self, server_id: &str, channel_id: &str, now_ms: i64) -> Effective {
        let server = self.notification_settings(server_id, "");
        let channel = self.notification_settings(server_id, channel_id);
        let set = |n: Option<&pb::NotificationSettings>| n.map(|n| n.level()).filter(|l| *l != Level::Unspecified);
        let level = set(channel)
            .or(set(server))
            .or_else(|| self.server(server_id).map(|s| s.default_notifications()).filter(|l| *l != Level::Unspecified))
            .unwrap_or(Level::Unspecified);
        Effective {
            level,
            muted: is_muted(server, now_ms) || is_muted(channel, now_ms),
            suppress_everyone: server.is_some_and(|s| s.suppress_everyone),
        }
    }

    /// Whether a channel is muted, by itself or through its server.
    pub fn is_muted(&self, server_id: &str, channel_id: &str, now_ms: i64) -> bool {
        is_muted(self.notification_settings(server_id, ""), now_ms)
            || is_muted(self.notification_settings(server_id, channel_id), now_ms)
    }

    /// Whether a message pings you: by your @username, or through @everyone,
    /// @here (unless you hid those) or one of your roles. The server works out
    /// who may ping everyone and which roles a message reached.
    pub fn pings_me(&self, server_id: &str, message: &pb::Message, suppress_everyone: bool) -> bool {
        let Some(me) = &self.me else { return false };
        if message.author_id == me.id {
            return false;
        }
        if mentions(&message.content, &me.username) {
            return true;
        }
        if message.mentions_everyone && !suppress_everyone {
            return true;
        }
        let mine = self.my_member(server_id).map(|m| m.role_ids.as_slice()).unwrap_or_default();
        message.mention_role_ids.iter().any(|id| mine.contains(id))
    }
}

/// Whether a message should reach you as a notification.
pub fn should_notify(e: Effective, mention: bool, prefs: &Prefs) -> bool {
    if e.muted || e.level == Level::Nothing {
        return false;
    }
    match e.level {
        Level::Mentions => mention,
        Level::All => true,
        // Nothing set for this server: this computer's settings decide.
        _ => mention || prefs.notify_for == NotifyFor::All,
    }
}

/// Whether a message mentions someone by @username: the name after an @
/// that doesn't follow a word character or another @, and ends there.
pub fn mentions(content: &str, username: &str) -> bool {
    if username.is_empty() {
        return false;
    }
    let lower = content.to_lowercase();
    let name = username.to_lowercase();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(at) = lower[from..].find('@').map(|n| n + from) {
        let before = lower[..at].chars().next_back();
        let rest = &lower[at + 1..];
        if !before.is_some_and(|c| word(c) || c == '@')
            && rest.starts_with(&name)
            && !rest[name.len()..].chars().next().is_some_and(word)
        {
            return true;
        }
        from = at + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_need_the_whole_name() {
        assert!(mentions("hi @mika!", "mika"));
        assert!(mentions("@Mika", "mika"));
        assert!(mentions("ping @mika.s", "mika.s"));
        assert!(!mentions("hi @mikasa", "mika"));
        assert!(!mentions("mail me at x@mika", "mika"));
        assert!(!mentions("@@mika", "mika"));
        assert!(!mentions("nobody", "mika"));
    }

    #[test]
    fn levels_mutes_and_defaults() {
        let prefs = Prefs::default();
        let e = |level, muted| Effective { level, muted, suppress_everyone: false };
        assert!(!should_notify(e(Level::All, true), true, &prefs));
        assert!(!should_notify(e(Level::Nothing, false), true, &prefs));
        assert!(should_notify(e(Level::All, false), false, &prefs));
        assert!(!should_notify(e(Level::Mentions, false), false, &prefs));
        assert!(should_notify(e(Level::Unspecified, false), true, &prefs));
        assert!(!should_notify(e(Level::Unspecified, false), false, &prefs));
        let all = Prefs { notify_for: NotifyFor::All, ..Prefs::default() };
        assert!(should_notify(e(Level::Unspecified, false), false, &all));

        let until = |s| Some(prost_types::Timestamp { seconds: s, nanos: 0 });
        let muted = pb::NotificationSettings { muted: true, muted_until: until(100), ..Default::default() };
        assert!(is_muted(Some(&muted), 99_000));
        assert!(!is_muted(Some(&muted), 100_000));
        assert!(is_muted(Some(&pb::NotificationSettings { muted: true, ..Default::default() }), i64::MAX));
    }
}
