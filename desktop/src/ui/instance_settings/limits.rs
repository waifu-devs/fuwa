//! Limits: the caps every server starts with, and picture uploads.

use gpui_kit::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use crate::core::instance_admin::{count_label, size_label};
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
        page.into_any_element()
    }
}

/// Every cap a page shows: (path, a size), for the boxes they're typed in.
pub(super) const CAPS: [(&str, bool); 10] = [
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
];
