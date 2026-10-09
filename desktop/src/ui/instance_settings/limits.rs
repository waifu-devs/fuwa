//! Limits: the caps every server starts with, uploads (pictures, files and
//! voice messages), poll votes, agents' commands, pins and live tiles. The
//! web's `LimitSettings`.

use gpui_kit::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{count_label, size_label};
use crate::ui::theme::Palette;

/// The default caps, saved and reset together: (path, label key, a size).
const DEFAULT_CAPS: [(&str, &str, bool); 6] = [
    ("default_limits.members", "serversettings.nav.members", false),
    ("default_limits.channels", "serversettings.nav.channels", false),
    ("default_limits.storage_bytes", "serversettings.usage.storage", true),
    ("default_limits.attachment_bytes", "serversettings.limits.files", true),
    ("default_limits.emojis", "serversettings.nav.emoji", false),
    ("default_limits.recording_bytes", "serversettings.nav.recordings", true),
];

const DEFAULT_PATHS: [&str; 6] = [
    "default_limits.members",
    "default_limits.channels",
    "default_limits.storage_bytes",
    "default_limits.attachment_bytes",
    "default_limits.emojis",
    "default_limits.recording_bytes",
];

/// "no limit", or the number in the app's language.
fn count(n: Option<i64>) -> String {
    n.map_or_else(|| t("instancesettings.shared.noLimit"), |n| count_label(Some(n)))
}

/// "no limit", or the size.
fn size(n: Option<i64>) -> String {
    n.map_or_else(|| t("instancesettings.shared.noLimit"), |n| size_label(Some(n)))
}

/// "30 a minute", or "no limit".
fn per_minute(n: Option<i64>) -> String {
    n.map_or_else(
        || t("instancesettings.shared.noLimit"),
        |n| t_with("instancesettings.shared.perMinute", &[("count", Arg::Num(n))]),
    )
}

/// One cap's setting: (id, title key, hint key, path, a size, the label beside its switch, what
/// it says while it's off, the default in words).
struct Row {
    id: &'static str,
    title: &'static str,
    hint: &'static str,
    path: &'static [&'static str],
    bytes: bool,
    label: String,
    off: Option<&'static str>,
    default: String,
}

impl InstanceSettingsView {
    pub(super) fn limits_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(defaults) = self.config.as_ref().and_then(|c| c.defaults.clone()) else {
            return div().into_any_element();
        };
        let l = defaults.default_limits.unwrap_or_default();
        let default = t_with(
            "instancesettings.limits.defaults",
            &[
                ("members", Arg::Str(&count(l.members))),
                ("channels", Arg::Str(&count(l.channels))),
                ("storage", Arg::Str(&size(l.storage_bytes))),
                ("files", Arg::Str(&size(l.attachment_bytes))),
                ("emoji", Arg::Str(&count(l.emojis))),
                ("recordings", Arg::Str(&size(l.recording_bytes))),
            ],
        );
        let mut caps = div().flex().flex_col().gap(px(12.0));
        for (path, label, bytes) in DEFAULT_CAPS {
            caps = caps.child(self.cap(path, &t(label), bytes, p, window, cx));
        }
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
        let up_to = t("instancesettings.shared.upTo");
        let row = |id, title, hint, path, bytes, default| Row {
            id,
            title,
            hint,
            path,
            bytes,
            label: up_to.clone(),
            off: None,
            default,
        };
        let mut rows = vec![
            row(
                "picture-uploads",
                "instancesettings.nav.pictureUploads",
                "instancesettings.limits.pictureHint",
                &["picture_upload_bytes"],
                true,
                size(defaults.picture_upload_bytes),
            ),
            row(
                "picture-uploads-per-day",
                "instancesettings.limits.picturesPerDay",
                "instancesettings.limits.picturesPerDayHint",
                &["picture_upload_bytes_per_day"],
                true,
                size(defaults.picture_upload_bytes_per_day),
            ),
            row(
                "attachment-uploads",
                "instancesettings.limits.attachment",
                "instancesettings.limits.attachmentHint",
                &["attachment_upload_bytes"],
                true,
                size(defaults.attachment_upload_bytes),
            ),
            row(
                "attachment-uploads-per-day",
                "instancesettings.limits.attachmentPerDay",
                "instancesettings.limits.attachmentPerDayHint",
                &["attachment_upload_bytes_per_day"],
                true,
                size(defaults.attachment_upload_bytes_per_day),
            ),
            Row {
                label: t("instancesettings.limits.secondsLabel"),
                ..row(
                    "voice-message-seconds",
                    "instancesettings.limits.voiceSeconds",
                    "instancesettings.limits.voiceSecondsHint",
                    &["voice_message_seconds"],
                    false,
                    defaults.voice_message_seconds.map_or_else(
                        || t("instancesettings.shared.noLimit"),
                        |n| t_with("instancesettings.limits.seconds", &[("count", Arg::Num(n))]),
                    ),
                )
            },
            row(
                "voice-message-bytes",
                "instancesettings.limits.voiceBytes",
                "instancesettings.limits.voiceBytesHint",
                &["voice_message_bytes"],
                true,
                size(defaults.voice_message_bytes),
            ),
            row(
                "voice-message-bytes-per-day",
                "instancesettings.limits.voicePerDay",
                "instancesettings.limits.voicePerDayHint",
                &["voice_message_bytes_per_day"],
                true,
                size(defaults.voice_message_bytes_per_day),
            ),
            Row {
                off: Some("30"),
                ..row(
                    "poll-votes-per-minute",
                    "instancesettings.limits.pollVotes",
                    "instancesettings.limits.pollVotesHint",
                    &["poll_votes_per_minute"],
                    false,
                    per_minute(defaults.poll_votes_per_minute),
                )
            },
            Row {
                off: Some("20"),
                ..row(
                    "commands-per-minute",
                    "instancesettings.limits.commands",
                    "instancesettings.limits.commandsHint",
                    &["commands_per_minute"],
                    false,
                    per_minute(defaults.commands_per_minute),
                )
            },
            Row {
                off: Some("50"),
                ..row(
                    "pins-per-channel",
                    "instancesettings.limits.pinsPerChannel",
                    "instancesettings.limits.pinsPerChannelHint",
                    &["pins_per_channel"],
                    false,
                    count(defaults.pins_per_channel),
                )
            },
            Row {
                off: Some("50"),
                ..row(
                    "pins-per-conversation",
                    "instancesettings.limits.pinsPerConversation",
                    "instancesettings.limits.pinsPerConversationHint",
                    &["pins_per_conversation"],
                    false,
                    count(defaults.pins_per_conversation),
                )
            },
            Row {
                off: Some("100"),
                ..row(
                    "reactions-per-message",
                    "instancesettings.limits.reactionsPerMessage",
                    "instancesettings.limits.reactionsPerMessageHint",
                    &["reactions_per_message"],
                    false,
                    count(defaults.reactions_per_message),
                )
            },
            Row {
                off: Some("60"),
                ..row(
                    "reactions-per-minute",
                    "instancesettings.limits.reactionsPerMinute",
                    "instancesettings.limits.reactionsPerMinuteHint",
                    &["reactions_per_minute"],
                    false,
                    per_minute(defaults.reactions_per_minute),
                )
            },
            Row {
                off: Some("10"),
                ..row(
                    "live-tiles-per-channel",
                    "instancesettings.limits.liveTilesPerChannel",
                    "instancesettings.limits.liveTilesPerChannelHint",
                    &["live_tiles_per_channel"],
                    false,
                    count(defaults.live_tiles_per_channel),
                )
            },
            Row {
                off: Some("120"),
                ..row(
                    "live-tile-updates-per-minute",
                    "instancesettings.limits.liveTileUpdates",
                    "instancesettings.limits.liveTileUpdatesHint",
                    &["live_tile_updates_per_minute"],
                    false,
                    per_minute(defaults.live_tile_updates_per_minute),
                )
            },
            Row {
                label: t("instancesettings.limits.liveTilePublishLabel"),
                off: Some("1000"),
                ..row(
                    "live-tile-publish-ms",
                    "instancesettings.limits.liveTilePublish",
                    "instancesettings.limits.liveTilePublishHint",
                    &["live_tile_publish_ms"],
                    false,
                    defaults.live_tile_publish_ms.map_or_else(
                        || t("instancesettings.shared.noLimit"),
                        |n| t_with("instancesettings.limits.liveTilePublishEvery", &[("count", Arg::Num(n))]),
                    ),
                )
            },
        ];
        // Agents' commands, only on instances that run them.
        if !self.instance_has("agent-commands") {
            rows.retain(|r| r.id != "commands-per-minute");
        }
        for (n, r) in rows.into_iter().enumerate() {
            let cap = self.cap_with(r.path[0], &r.label, r.bytes, r.off, p, window, cx);
            page = page.child(self.setting(r.id, &t(r.title), Some(&t(r.hint)), r.path, &r.default, 1 + n, cap, p, cx));
        }
        page.into_any_element()
    }
}

/// Every cap a page shows: (path, a size), for the boxes they're typed in.
pub(super) const CAPS: [(&str, bool); 32] = [
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
    ("pins_per_channel", false),
    ("pins_per_conversation", false),
    ("reactions_per_message", false),
    ("reactions_per_minute", false),
    ("live_tiles_per_channel", false),
    ("live_tile_updates_per_minute", false),
    ("live_tile_publish_ms", false),
    ("shared_remote_sends_per_minute", false),
    ("shared_remote_people", false),
    ("shared_remote_file_bytes_per_day", true),
    ("shared_file_fetches_in_flight", false),
    ("gifs.gif_bytes", true),
    ("gifs.searches_per_minute", false),
    ("gifs.provider_calls_per_day", false),
];

#[cfg(test)]
mod tests {
    use crate::core::instance_admin::{PATHS, cap, changed, copy_field, saved_as, set_cap};
    use crate::pb;

    #[test]
    fn every_cap_a_page_shows_is_saved_and_copied() {
        for (path, _) in super::CAPS {
            let saved = saved_as(path);
            assert!(PATHS.contains(&saved), "{path} isn't saved");
            let mut draft = pb::InstanceSettings::default();
            set_cap(&mut draft, path, Some(7));
            assert_eq!(cap(&draft, path), Some(7), "{path}");
            assert_eq!(changed(&draft, &pb::InstanceSettings::default()), vec![saved.to_owned()]);
            let mut into = pb::InstanceSettings::default();
            copy_field(&mut into, &draft, saved);
            assert_eq!(cap(&into, path), Some(7), "{path}");
        }
    }
}
