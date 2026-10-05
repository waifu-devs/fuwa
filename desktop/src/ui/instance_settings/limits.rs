//! Limits: the caps every server starts with, uploads (pictures, files and
//! voice messages) and poll votes. The web's Limits tab.

use gpui_kit::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use crate::core::instance_admin::{count_label, per_minute_label, size_label};
use crate::ui::theme::Palette;

/// The default caps, saved and reset together: (path, label, a size).
const DEFAULT_CAPS: [(&str, &str, bool); 6] = [
    ("default_limits.members", "Members", false),
    ("default_limits.channels", "Channels", false),
    ("default_limits.storage_bytes", "Storage", true),
    ("default_limits.attachment_bytes", "Files", true),
    ("default_limits.emojis", "Emoji", false),
    ("default_limits.recording_bytes", "Recordings", true),
];

const DEFAULT_PATHS: [&str; 6] = [
    "default_limits.members",
    "default_limits.channels",
    "default_limits.storage_bytes",
    "default_limits.attachment_bytes",
    "default_limits.emojis",
    "default_limits.recording_bytes",
];

/// A cap's setting: id, title, hint, its path, whether it's a size, and the default in words.
type Row = (&'static str, &'static str, &'static str, &'static [&'static str], bool, String);

impl InstanceSettingsView {
    pub(super) fn limits_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(defaults) = self.config.as_ref().and_then(|c| c.defaults.clone()) else {
            return div().into_any_element();
        };
        let l = defaults.default_limits.unwrap_or_default();
        let none = [l.members, l.channels, l.storage_bytes, l.attachment_bytes, l.emojis, l.recording_bytes]
            .iter()
            .all(Option::is_none);
        let default = if none {
            "no limits".to_owned()
        } else {
            [
                format!("{} members", count_label(l.members)),
                format!("{} channels", count_label(l.channels)),
                format!("{} storage", size_label(l.storage_bytes)),
                format!("{} files", size_label(l.attachment_bytes)),
                format!("{} emoji", count_label(l.emojis)),
                format!("{} recordings", size_label(l.recording_bytes)),
            ]
            .join(", ")
        };
        let mut caps = div().flex().flex_col().gap(px(12.0));
        for (path, label, bytes) in DEFAULT_CAPS {
            caps = caps.child(self.cap(path, label, bytes, p, window, cx));
        }
        let mut page = div().flex().flex_col().child(self.setting(
            "default-limits",
            "Default caps for every server",
            Some("A server can get its own caps from its settings. With a cap off, it's unlimited."),
            &DEFAULT_PATHS,
            &default,
            0,
            caps,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "picture-uploads",
            "Largest picture upload",
            Some(
                "Avatars, banners and server icons. The app crops pictures and saves them small, so only GIFs, which \
                 go up as they are, get near a few megabytes.",
            ),
            &["picture_upload_bytes"],
            &size_label(defaults.picture_upload_bytes),
            1,
            self.cap("picture_upload_bytes", "Up to", true, p, window, cx),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "picture-uploads-per-day",
            "Pictures per day",
            Some("How much one account may upload in a day (UTC), so nobody can fill this instance's disk."),
            &["picture_upload_bytes_per_day"],
            &size_label(defaults.picture_upload_bytes_per_day),
            2,
            self.cap("picture_upload_bytes_per_day", "Up to", true, p, window, cx),
            p,
            cx,
        ));
        // The rest as the web has them.
        let seconds = defaults.voice_message_seconds.map(|s| format!("{} seconds", count_label(Some(s))));
        let rest: [Row; 6] = [
            (
                "attachment-uploads",
                "Largest file",
                "The biggest file one attachment may be, apart from pictures.",
                &["attachment_upload_bytes"],
                true,
                size_label(defaults.attachment_upload_bytes),
            ),
            (
                "attachment-uploads-per-day",
                "Files per day",
                "How much one account may send in files in a day (UTC), apart from pictures.",
                &["attachment_upload_bytes_per_day"],
                true,
                size_label(defaults.attachment_upload_bytes_per_day),
            ),
            (
                "voice-message-seconds",
                "Longest voice message",
                "In direct messages. Apps stop recording here; voice messages are end-to-end encrypted, so this \
                 instance can't check their length itself.",
                &["voice_message_seconds"],
                false,
                seconds.unwrap_or_else(|| "no limit".to_owned()),
            ),
            (
                "voice-message-bytes",
                "Biggest voice message",
                "Its encrypted file, which this instance does see. A minute of voice is about 240 KB.",
                &["voice_message_bytes"],
                true,
                size_label(defaults.voice_message_bytes),
            ),
            (
                "voice-message-bytes-per-day",
                "Voice messages a day",
                "What one account may send in a day (UTC), counted apart from pictures and files.",
                &["voice_message_bytes_per_day"],
                true,
                size_label(defaults.voice_message_bytes_per_day),
            ),
            (
                "poll-votes-per-minute",
                "Poll votes per minute",
                "How many times one account may vote, change or take back a vote in polls in a minute. Every vote \
                 is a live update to everyone in the channel.",
                &["poll_votes_per_minute"],
                false,
                per_minute_label(defaults.poll_votes_per_minute),
            ),
        ];
        for (n, (id, title, hint, paths, bytes, default)) in rest.into_iter().enumerate() {
            let path = paths[0];
            let label = if path == "voice_message_seconds" { "Seconds" } else { "Up to" };
            page = page.child(self.setting(
                id,
                title,
                Some(hint),
                paths,
                &default,
                3 + n,
                self.cap(path, label, bytes, p, window, cx),
                p,
                cx,
            ));
        }
        page.into_any_element()
    }
}

/// Every cap a page shows: (path, a size), for the boxes they're typed in.
pub(super) const CAPS: [(&str, bool); 19] = [
    ("servers_per_account", false),
    ("default_limits.members", false),
    ("default_limits.channels", false),
    ("default_limits.storage_bytes", true),
    ("default_limits.attachment_bytes", true),
    ("default_limits.emojis", false),
    ("default_limits.recording_bytes", true),
    ("picture_upload_bytes", true),
    ("picture_upload_bytes_per_day", true),
    ("call_recordings_keep_days", false),
    ("automod_checks_per_day", false),
    ("attachment_upload_bytes", true),
    ("attachment_upload_bytes_per_day", true),
    ("voice_message_seconds", false),
    ("voice_message_bytes", true),
    ("voice_message_bytes_per_day", true),
    ("poll_votes_per_minute", false),
    ("shared_remote_sends_per_minute", false),
    ("shared_remote_people", false),
];

#[cfg(test)]
mod tests {
    use crate::core::instance_admin::{PATHS, cap, changed, copy_field, set_cap};
    use crate::pb;

    #[test]
    fn every_cap_a_page_shows_is_saved_and_copied() {
        for (path, _) in super::CAPS {
            assert!(PATHS.contains(&path), "{path} isn't saved");
            let mut draft = pb::InstanceSettings::default();
            set_cap(&mut draft, path, Some(7));
            assert_eq!(cap(&draft, path), Some(7), "{path}");
            assert_eq!(changed(&draft, &pb::InstanceSettings::default()), vec![path.to_owned()]);
            let mut into = pb::InstanceSettings::default();
            copy_field(&mut into, &draft, path);
            assert_eq!(cap(&into, path), Some(7), "{path}");
        }
    }
}
