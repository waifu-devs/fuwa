//! Calls: whether they're on, recordings kept on the server, and how apps get
//! through firewalls to the instance's media server.

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, FontWeight, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use super::general::hidden;
use crate::ui::motion;
use crate::ui::server_settings::amber;
use crate::ui::theme::{Palette, corner};
use crate::ui::widgets::icon;

impl InstanceSettingsView {
    pub(super) fn calls_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        let defaults = config.defaults.clone().unwrap_or_default();
        let startup = config.startup.clone().unwrap_or_default();
        let hide = self.core.prefs().streamer_mode;
        let media = match (startup.media_port, startup.media_addresses.is_empty()) {
            (0, true) => None,
            (0, false) => Some(startup.media_addresses.join(", ")),
            (port, true) => Some(format!("port {port}")),
            (port, false) => Some(format!("port {port} on {}", startup.media_addresses.join(", "))),
        };
        let media_card = motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .border_dashed()
                .border_color(if media.is_some() { p.border.into() } else { amber(p).opacity(0.5) })
                .text_sm()
                .child(icon("server").size(px(16.0)).mt(px(2.0)).text_color(p.muted_foreground))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(4.0))
                                .child(div().font_weight(FontWeight::BOLD).child("Media server:"))
                                .map(|el| match &media {
                                    Some(_) if hide => {
                                        el.child(div().text_color(p.muted_foreground).child("address hidden"))
                                    }
                                    Some(media) => el.child(div().text_color(p.muted_foreground).child(media.clone())),
                                    None => el.child(div().text_color(amber(p)).child(
                                        "not running, so calls can't connect. Whoever runs this instance sets it up \
                                     (FUWA_MEDIA_PORT, or FUWA_MEDIA_URL for a split instance).",
                                    )),
                                }),
                        )
                        .child(div().mt(px(2.0)).text_xs().text_color(p.muted_foreground).child(
                            "Set when the instance starts. Calls in direct messages are end-to-end encrypted: it only \
                             ever forwards sound it can't read.",
                        )),
                ),
            "icalls-media",
            std::time::Duration::from_millis(100),
            8.0,
        );
        let mut page = div().flex().flex_col().child(
            self.setting(
                "calls-on",
                "Calls",
                None,
                &["calls"],
                if defaults.calls { "on" } else { "off" },
                0,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(self.toggle(
                        "calls",
                        draft.calls,
                        false,
                        "Let people talk in voice channels and call in direct messages",
                        "Turning it off hangs up every call within a few seconds.",
                        p,
                        cx,
                        |d, on| d.calls = on,
                    ))
                    .child(media_card),
                p,
                cx,
            ),
        );
        page = page.child(self.setting(
            "call-recordings",
            "Recording on the server",
            None,
            &["call_recordings"],
            if defaults.call_recordings { "on" } else { "off" },
            1,
            self.toggle(
                "call-recordings",
                draft.call_recordings,
                false,
                "Let people with Record keep voice channels' recordings on this instance",
                "One Ogg Opus track per person, kept with the server's files (sealed when the instance encrypts its \
                 files). Nobody has Record until a server's admins grant it. Turning this off stops recordings going \
                 on now; the ones kept stay. How much each server may keep is a cap under Limits.",
                p,
                cx,
                |d, on| d.call_recordings = on,
            ),
            p,
            cx,
        ));
        let keep_default =
            defaults.call_recordings_keep_days.map_or_else(|| "until deleted".to_owned(), |n| format!("{n} days"));
        page = page.child(self.setting(
            "call-recordings-keep",
            "Keep recordings for",
            Some(
                "Finished recordings older than this delete themselves, files and copies included. Off keeps them \
                 until someone deletes them.",
            ),
            &["call_recordings_keep_days"],
            &keep_default,
            2,
            self.cap("call_recordings_keep_days", "Days", false, p, window, cx),
            p,
            cx,
        ));
        let ice = self.areas.get("ice_urls").cloned();
        let ice_default = if defaults.ice_urls.is_empty() { "none".to_owned() } else { defaults.ice_urls.join(", ") };
        page = page.child(self.setting(
            "ice-urls",
            "STUN and TURN servers",
            Some(
                "For people behind strict firewalls: a TURN server relays their sound when nothing else gets \
                 through. One per line, as stun:, turn: or turns: addresses.",
            ),
            &["ice_urls"],
            if hide { "address hidden" } else { &ice_default },
            3,
            div().map(|el| match (ice, hide) {
                (_, true) => el.child(hidden(p)),
                (Some(ice), false) => el.child(Textarea::new(&ice)),
                (None, false) => el,
            }),
            p,
            cx,
        ));
        if let Some(secret) = self.texts.get("turn_secret") {
            page = page.child(self.setting(
                "turn-secret",
                "TURN secret",
                Some(
                    "The shared secret your TURN server (such as coturn with use-auth-secret) checks. Apps get a new \
                     password from it for each call, good for an hour, that names no account.",
                ),
                &["turn_secret"],
                if defaults.turn_secret_set { "set" } else { "none" },
                4,
                Input::new(secret).prefix(icon("key-round").size(px(15.0)).text_color(p.muted_foreground)),
                p,
                cx,
            ));
        }
        page.into_any_element()
    }
}
