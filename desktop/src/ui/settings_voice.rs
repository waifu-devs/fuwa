//! The Voice & video page, as the web's `settings/app/Voice.tsx`.

use gpui_kit::{AnyElement, Context, IntoElement, ParentElement as _, Styled as _, Window, div};

use crate::core::config::Prefs;
use crate::core::i18n::t;
use crate::ui::settings::SettingsView;
use crate::ui::theme::Palette;

/// Where the page's settings sit for search (the web's `voiceSettings`).
pub(crate) fn voice_settings() -> Vec<(&'static str, String, &'static str)> {
    vec![
        ("devices", t("appsettings.voice.devices"), "input output device headset"),
        ("mic-test", t("appsettings.voice.micTest"), "check level meter"),
        ("input-mode", t("appsettings.voice.inputMode"), "voice activity push to talk ptt"),
        ("sensitivity", t("appsettings.voice.sensitivity"), "threshold gate noise"),
        ("processing", t("appsettings.voice.processing"), "echo noise suppression gain"),
        ("camera", t("appsettings.voice.camera"), "video webcam mirror preview"),
        ("call-sounds", t("appsettings.voice.callSounds"), "ring ringtone join leave"),
    ]
}

impl SettingsView {
    pub(crate) fn voice_page(
        &mut self,
        _prefs: &Prefs,
        p: &Palette,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        div().text_color(p.muted_foreground).child("…").into_any_element()
    }
}
