//! Calls: whether they're on, recordings kept on the server, and how apps get
//! through firewalls to the instance's media server (the web's `instance/Calls.tsx`).

use crate::ui::instance_home::{focus_ring, has_focus};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, FontWeight, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::controls::{area_box, input_box};
use super::general::hidden;
use super::{HIDDEN_ADDRESS, InstanceSettingsView};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::motion;
use crate::ui::server_settings::amber;
use crate::ui::theme::{Palette, radius_2xl};
use crate::ui::widgets::icon;

/// "on" or "off", as a setting's default.
fn on_off(on: bool) -> String {
    t(if on { "instancesettings.shared.on" } else { "instancesettings.shared.off" })
}

impl InstanceSettingsView {
    pub(super) fn calls_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        let defaults = config.defaults.clone().unwrap_or_default();
        let startup = config.startup.clone().unwrap_or_default();
        let hide = self.core.prefs().streamer_mode;
        let addresses = startup.media_addresses.join(", ");
        let port = startup.media_port.to_string();
        let media = match (startup.media_port, startup.media_addresses.is_empty()) {
            (0, true) => None,
            (0, false) => Some(addresses.clone()),
            (_, true) => Some(t_with("instancesettings.calls.port", &[("port", Arg::Str(&port))])),
            (_, false) => Some(t_with(
                "instancesettings.calls.portOn",
                &[("port", Arg::Str(&port)), ("addresses", Arg::Str(&addresses))],
            )),
        };
        // `flex items-start gap-2.5 rounded-2xl border border-dashed p-3 text-sm`.
        let media_card = motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_dashed()
                .border_color(if media.is_some() { p.border.into() } else { amber(p).opacity(0.5) })
                .text_sm()
                .line_height(px(20.0))
                .child(icon("server").size(px(16.0)).flex_none().mt(px(2.0)).text_color(p.muted_foreground))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_x(px(4.0))
                                .child(
                                    div().font_weight(FontWeight::BOLD).child(t("instancesettings.calls.mediaServer")),
                                )
                                .map(|el| match &media {
                                    Some(_) if hide => {
                                        el.child(div().text_color(p.muted_foreground).child(HIDDEN_ADDRESS))
                                    }
                                    Some(media) => el.child(div().text_color(p.muted_foreground).child(media.clone())),
                                    None => el.child(
                                        div().text_color(amber(p)).child(t("instancesettings.calls.notRunning")),
                                    ),
                                }),
                        )
                        .child(
                            div()
                                .mt(px(2.0))
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(t("instancesettings.calls.setAtStart")),
                        ),
                ),
            "icalls-media",
            std::time::Duration::from_millis(100),
            8.0,
        );
        let mut page = div().flex().flex_col().child(
            self.setting(
                "calls-on",
                &t("instancesettings.nav.calls"),
                None,
                &["calls"],
                &on_off(defaults.calls),
                0,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(self.toggle(
                        "calls",
                        draft.calls,
                        false,
                        &t("instancesettings.calls.label"),
                        &t("instancesettings.calls.hint"),
                        p,
                        window,
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
            &t("instancesettings.nav.callRecordings"),
            None,
            &["call_recordings"],
            &on_off(defaults.call_recordings),
            1,
            self.toggle(
                "call-recordings",
                draft.call_recordings,
                false,
                &t("instancesettings.calls.recordLabel"),
                &t("instancesettings.calls.recordHint"),
                p,
                window,
                cx,
                |d, on| d.call_recordings = on,
            ),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "call-recording-video",
            &t("instancesettings.calls.video"),
            None,
            &["call_recording_video"],
            &on_off(defaults.call_recording_video),
            1,
            self.toggle(
                "call-recording-video",
                draft.call_recording_video,
                false,
                &t("instancesettings.calls.videoLabel"),
                &t("instancesettings.calls.videoHint"),
                p,
                window,
                cx,
                |d, on| d.call_recording_video = on,
            ),
            p,
            cx,
        ));
        let keep_default = defaults.call_recordings_keep_days.map_or_else(
            || t("instancesettings.calls.untilDeleted"),
            |n| t_with("instancesettings.calls.days", &[("count", Arg::Num(n))]),
        );
        page = page.child(self.setting(
            "call-recordings-keep",
            &t("instancesettings.nav.callRecordingsKeep"),
            Some(&t("instancesettings.calls.keepHint")),
            &["call_recordings_keep_days"],
            &keep_default,
            2,
            self.cap("call_recordings_keep_days", &t("instancesettings.calls.daysLabel"), false, p, window, cx),
            p,
            cx,
        ));
        let ice = self.areas.get("ice_urls").cloned();
        let ice_default =
            if defaults.ice_urls.is_empty() { t("instancesettings.shared.none") } else { defaults.ice_urls.join(", ") };
        page = page.child(self.setting(
            "ice-urls",
            &t("instancesettings.nav.iceUrls"),
            Some(&t("instancesettings.calls.iceHint")),
            &["ice_urls"],
            if hide { HIDDEN_ADDRESS } else { &ice_default },
            3,
            div().map(|el| {
                match (ice, hide) {
                    (_, true) => el.child(hidden(p)),
                    (Some(ice), false) => el.child(
                        focus_ring(
                            area_box(Textarea::new(&ice).appearance(false), Some("radio-tower"), p),
                            has_focus(&ice, window, cx),
                            p,
                        )
                        .font_family("monospace"),
                    ),
                    (None, false) => el,
                }
            }),
            p,
            cx,
        ));
        if let Some(secret) = self.texts.get("turn_secret") {
            page = page.child(self.setting(
                "turn-secret",
                &t("instancesettings.nav.turnSecret"),
                Some(&t("instancesettings.calls.turnHint")),
                &["turn_secret"],
                &t(if defaults.turn_secret_set {
                    "instancesettings.shared.set"
                } else {
                    "instancesettings.shared.none"
                }),
                4,
                focus_ring(
                    input_box(Input::new(secret).appearance(false), Some("key-round"), p),
                    has_focus(secret, window, cx),
                    p,
                ),
                p,
                cx,
            ));
        }
        page.into_any_element()
    }
}
