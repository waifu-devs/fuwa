//! Limits: the caps every server starts with, uploads (pictures, files and
//! voice messages) and poll votes. The web's Limits tab.

use gpui_kit::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use super::controls::{count_text, per_minute_text, size_text};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::theme::Palette;

/// The default caps, saved and reset together: (path, a size).
const DEFAULT_CAPS: [(&str, bool); 6] = [
    ("default_limits.members", false),
    ("default_limits.channels", false),
    ("default_limits.storage_bytes", true),
    ("default_limits.attachment_bytes", true),
    ("default_limits.emojis", false),
    ("default_limits.recording_bytes", true),
];

/// A default cap's name, as the web's Limits tab says it.
fn cap_label(path: &str) -> String {
    match path {
        "default_limits.members" => t("serversettings.nav.members"),
        "default_limits.channels" => t("serversettings.nav.channels"),
        "default_limits.storage_bytes" => t("serversettings.usage.storage"),
        "default_limits.attachment_bytes" => t("serversettings.limits.files"),
        "default_limits.emojis" => t("serversettings.nav.emoji"),
        _ => t("serversettings.nav.recordings"),
    }
}

const DEFAULT_PATHS: [&str; 6] = [
    "default_limits.members",
    "default_limits.channels",
    "default_limits.storage_bytes",
    "default_limits.attachment_bytes",
    "default_limits.emojis",
    "default_limits.recording_bytes",
];

/// A cap's setting: id, title, hint, its path, whether it's a size, and the default in words.
type Row = (&'static str, String, String, &'static [&'static str], bool, String);

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
            t("desktop.instance.noLimits")
        } else {
            t_with(
                "instancesettings.limits.defaults",
                &[
                    ("members", Arg::Str(&count_text(l.members))),
                    ("channels", Arg::Str(&count_text(l.channels))),
                    ("storage", Arg::Str(&size_text(l.storage_bytes))),
                    ("files", Arg::Str(&size_text(l.attachment_bytes))),
                    ("emoji", Arg::Str(&count_text(l.emojis))),
                    ("recordings", Arg::Str(&size_text(l.recording_bytes))),
                ],
            )
        };
        let mut caps = div().flex().flex_col().gap(px(12.0));
        for (path, bytes) in DEFAULT_CAPS {
            caps = caps.child(self.cap(path, &cap_label(path), bytes, p, window, cx));
        }
        let up_to = t("instancesettings.shared.upTo");
        let mut page = div().flex().flex_col().child(self.setting(
            "default-limits",
            &t("instancesettings.nav.defaultLimits"),
            Some(&t("instancesettings.limits.defaultHint")),
            &DEFAULT_PATHS,
            &default,
            0,
            caps,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "picture-uploads",
            &t("instancesettings.nav.pictureUploads"),
            Some(&t("instancesettings.limits.pictureHint")),
            &["picture_upload_bytes"],
            &size_text(defaults.picture_upload_bytes),
            1,
            self.cap("picture_upload_bytes", &up_to, true, p, window, cx),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "picture-uploads-per-day",
            &t("instancesettings.limits.picturesPerDay"),
            Some(&t("instancesettings.limits.picturesPerDayHint")),
            &["picture_upload_bytes_per_day"],
            &size_text(defaults.picture_upload_bytes_per_day),
            2,
            self.cap("picture_upload_bytes_per_day", &up_to, true, p, window, cx),
            p,
            cx,
        ));
        // The rest as the web has them.
        let seconds = defaults
            .voice_message_seconds
            .map(|s| t_with("instancesettings.limits.seconds", &[("count", Arg::Num(s))]));
        let rest: [Row; 6] = [
            (
                "attachment-uploads",
                t("desktop.instance.largestFile"),
                t("desktop.instance.largestFileHint"),
                &["attachment_upload_bytes"],
                true,
                size_text(defaults.attachment_upload_bytes),
            ),
            (
                "attachment-uploads-per-day",
                t("instancesettings.limits.attachmentPerDay"),
                t("instancesettings.limits.attachmentPerDayHint"),
                &["attachment_upload_bytes_per_day"],
                true,
                size_text(defaults.attachment_upload_bytes_per_day),
            ),
            (
                "voice-message-seconds",
                t("instancesettings.limits.voiceSeconds"),
                t("instancesettings.limits.voiceSecondsHint"),
                &["voice_message_seconds"],
                false,
                seconds.unwrap_or_else(|| t("instancesettings.shared.noLimit")),
            ),
            (
                "voice-message-bytes",
                t("instancesettings.limits.voiceBytes"),
                t("instancesettings.limits.voiceBytesHint"),
                &["voice_message_bytes"],
                true,
                size_text(defaults.voice_message_bytes),
            ),
            (
                "voice-message-bytes-per-day",
                t("instancesettings.limits.voicePerDay"),
                t("instancesettings.limits.voicePerDayHint"),
                &["voice_message_bytes_per_day"],
                true,
                size_text(defaults.voice_message_bytes_per_day),
            ),
            (
                "poll-votes-per-minute",
                t("instancesettings.limits.pollVotes"),
                t("instancesettings.limits.pollVotesHint"),
                &["poll_votes_per_minute"],
                false,
                per_minute_text(defaults.poll_votes_per_minute),
            ),
        ];
        for (n, (id, title, hint, paths, bytes, default)) in rest.into_iter().enumerate() {
            let path = paths[0];
            let label =
                if path == "voice_message_seconds" { t("instancesettings.limits.secondsLabel") } else { up_to.clone() };
            page = page.child(self.setting(
                id,
                &title,
                Some(&hint),
                paths,
                &default,
                3 + n,
                self.cap(path, &label, bytes, p, window, cx),
                p,
                cx,
            ));
        }
        // Agents' commands, on instances that run them.
        if self.instance_has("agent-commands") {
            page = page.child(self.setting(
                "commands-per-minute",
                &t("instancesettings.limits.commands"),
                Some(&t("instancesettings.limits.commandsHint")),
                &["commands_per_minute"],
                &per_minute_text(defaults.commands_per_minute),
                9,
                self.cap("commands_per_minute", &up_to, false, p, window, cx),
                p,
                cx,
            ));
        }
        page.into_any_element()
    }
}

/// Every cap a page shows: (path, a size), for the boxes they're typed in.
pub(super) const CAPS: [(&str, bool); 22] = [
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
    ("commands_per_minute", false),
    ("shared_remote_sends_per_minute", false),
    ("shared_remote_people", false),
    ("shared_remote_file_bytes_per_day", true),
    ("shared_file_fetches_in_flight", false),
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
