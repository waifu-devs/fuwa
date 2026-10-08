//! The Voice & video page, as the web's `settings/app/Voice.tsx`: the
//! microphone and speakers calls use (picked from this computer's devices,
//! through `core::sounds`), a live mic test against where voice activity
//! opens, the input mode, sensitivity, and call sounds. Desktop calls are
//! sound only and send the microphone as it is, so the camera and the
//! browser's sound processing say so instead of pretending.

use crate::core::voice::access;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::config::{InputMode, Prefs, Sounds};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::sounds::{self, MicTest, Sound};
use crate::ui::settings::SettingsView;
use crate::ui::settings_app::pref;
use crate::ui::settings_controls::{At, Badge, Look, Opt, button, choice, toggle};
use crate::ui::settings_menu::Item;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::icon;

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

#[derive(Default)]
pub(crate) struct VoiceForm {
    /// This computer's microphones and speakers, read when the page opens.
    devices: Option<(Vec<String>, Vec<String>)>,
    pub mic: Option<MicTest>,
    mic_failed: bool,
    /// The mic test failed because the system won't let the app use the microphone.
    mic_blocked: bool,
}

/// -100..0 dB as a share of a meter.
fn meter_at(db: f32) -> f32 {
    ((db + 100.0) / 100.0).clamp(0.0, 1.0)
}

impl SettingsView {
    #[allow(clippy::too_many_arguments)]
    fn device_picker(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: String,
        value: &str,
        devices: &[String],
        p: &Palette,
        cx: &mut Context<Self>,
        set: fn(&mut Prefs, String),
    ) -> AnyElement {
        let current = if value.is_empty() || !devices.iter().any(|d| d == value) {
            t("appsettings.voice.systemDefault")
        } else {
            value.to_owned()
        };
        let mut items = Vec::new();
        for (n, name) in std::iter::once(String::new()).chain(devices.iter().cloned()).enumerate() {
            let shown = if n == 0 { t("appsettings.voice.systemDefault") } else { name.clone() };
            let on = name == value;
            items.push(Item::action(shown, on.then_some("check"), move |this, cx| {
                let name = name.clone();
                this.set(cx, move |pr| set(pr, name));
            }));
        }
        let hover = alpha(p.primary, 0.5);
        let trigger = div()
            .id(SharedString::from(format!("{id}-trigger")))
            .h(px(40.0))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(p.background)
            .px(px(12.0))
            .text_sm()
            .cursor_pointer()
            .hover(move |s| s.border_color(hover))
            .child(div().flex_1().min_w_0().truncate().child(current))
            .child(icon("chevron-down").size(px(16.0)).text_color(p.muted_foreground));
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(icon(glyph).size(px(14.0)))
                    .child(tracked(label.to_uppercase(), WIDE)),
            )
            .child(div().w_full().child(self.dropdown(id.to_owned(), trigger.w_full(), items, false, 280.0, p, cx)))
            .into_any_element()
    }

    /// A volume from 0 to 200%, its name and value above it.
    #[allow(clippy::too_many_arguments)]
    fn volume(
        &mut self,
        id: &'static str,
        label: String,
        value: u16,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
        set: fn(&mut Prefs, f32),
    ) -> AnyElement {
        let slider = self.slider(
            id,
            (0.0, 200.0, 5.0),
            f32::from(value),
            vec![(100.0, "100%".into())],
            |v| format!("{}%", v.round() as i64),
            false,
            p,
            window,
            cx,
            set,
        );
        div()
            .child(
                div()
                    .mb(px(-20.0))
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .text_sm()
                    .child(div().font_weight(FontWeight::BOLD).child(label))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(format!("{value}%")),
                    ),
            )
            .child(slider)
            .into_any_element()
    }

    pub(crate) fn voice_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.voice.devices.is_none() {
            self.voice.devices = Some((sounds::device_names(true), sounds::device_names(false)));
        }
        let (inputs, outputs) = self.voice.devices.clone().unwrap_or_default();
        let half = (self.column - 16.0) / 2.0;
        let mic_vol = self.volume(
            "input-volume",
            t("appsettings.voice.micVolume"),
            prefs.input_volume,
            p,
            window,
            cx,
            |pr, v| pr.input_volume = v.round() as u16,
        );
        let call_vol = self.volume(
            "output-volume",
            t("appsettings.voice.callVolume"),
            prefs.output_volume,
            p,
            window,
            cx,
            |pr, v| pr.output_volume = v.round() as u16,
        );
        let devices = div()
            .flex()
            .gap(px(16.0))
            .child(
                div()
                    .w(px(half))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(self.device_picker(
                        "mic-device",
                        "mic",
                        t("appsettings.voice.mic"),
                        &prefs.input_device,
                        &inputs,
                        p,
                        cx,
                        |pr, v| pr.input_device = v,
                    ))
                    .child(mic_vol),
            )
            .child(
                div()
                    .w(px(half))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(self.device_picker(
                        "speaker-device",
                        "headphones",
                        t("appsettings.voice.speakers"),
                        &prefs.output_device,
                        &outputs,
                        p,
                        cx,
                        |pr, v| pr.output_device = v,
                    ))
                    .child(call_vol),
            );

        // The mic test.
        let testing = self.voice.mic.is_some();
        if let Some(mic) = &self.voice.mic {
            if mic.failed.load(std::sync::atomic::Ordering::Relaxed) {
                self.voice.mic_blocked = mic.blocked.load(std::sync::atomic::Ordering::Relaxed);
                self.voice.mic = None;
                self.voice.mic_failed = true;
            } else {
                window.request_animation_frame();
            }
        }
        let level = self.voice.mic.as_ref().map_or(-100.0, MicTest::level);
        let threshold = if prefs.auto_sensitivity { -50.0 } else { f32::from(prefs.sensitivity) };
        let open = testing && level > threshold;
        let shown = motion_level(level, window, cx);
        let meter = div()
            .relative()
            .h(px(12.0))
            .w_full()
            .overflow_hidden()
            .rounded_full()
            .bg(p.muted)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(gpui_kit::relative(meter_at(shown)))
                    .rounded_full()
                    .bg(if open { rgb(0x3ba55d).into() } else { alpha(p.primary, 0.6) }),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui_kit::relative(meter_at(threshold)))
                    .w(px(2.0))
                    .bg(alpha(p.foreground, 0.7)),
            );
        let device = prefs.input_device.clone();
        let mic_test = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div().flex().child(
                    button(
                        "mic-test",
                        if testing { t("appsettings.voice.stop") } else { t("appsettings.voice.micTestStart") },
                        Some(if testing { "square" } else { "play" }),
                        if testing { Look::Outline } else { Look::Primary },
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .font_weight(FontWeight::BOLD)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.voice.mic = if this.voice.mic.is_some() { None } else { Some(MicTest::start(&device)) };
                        this.voice.mic_failed = false;
                        this.voice.mic_blocked = false;
                        cx.notify();
                    })),
                ),
            )
            .child(meter)
            .when(self.voice.mic_failed && !self.voice.mic_blocked, |el| {
                el.child(div().text_sm().text_color(p.destructive).child(t("desktop.voice.micFailed")))
            })
            .when(self.voice.mic_blocked, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(8.0))
                        .child(div().text_sm().text_color(p.destructive).child(t("desktop.voice.micBlocked")))
                        .when(access::has_settings(), |el| {
                            el.child(
                                button(
                                    "mic-allow",
                                    t("desktop.voice.openPrivacy"),
                                    Some("settings"),
                                    Look::Outline,
                                    false,
                                    p,
                                )
                                .rounded(radius_xl())
                                .on_click(|_, _, _| access::open_settings(access::Device::Microphone)),
                            )
                        }),
                )
            });

        let mode_at = if prefs.input_mode == InputMode::Voice { 0 } else { 1 };
        let mut ptt = Opt::new(t("appsettings.voice.pushToTalk"), t("appsettings.voice.pushToTalkHint"), "keyboard");
        ptt.disabled = Some(t("desktop.voice.pttLater"));
        let mode = choice(
            "input-mode",
            Some(mode_at),
            vec![
                Opt::new(t("appsettings.voice.voiceActivity"), t("appsettings.voice.voiceActivityHint"), "audio-lines"),
                ptt,
            ],
            self.column,
            p,
            window,
            cx,
            |this, n, cx| {
                let m = if n == 0 { InputMode::Voice } else { InputMode::Ptt };
                this.set(cx, |pr| pr.input_mode = m)
            },
        );
        let sensitivity_slider = (!prefs.auto_sensitivity).then(|| {
            self.slider(
                "sensitivity",
                (-100.0, 0.0, 1.0),
                f32::from(prefs.sensitivity),
                Vec::new(),
                |v| t_with("appsettings.voice.decibels", &[("value", Arg::Num(v.round() as i64))]),
                false,
                p,
                window,
                cx,
                |pr, v| pr.sensitivity = v.round() as i8,
            )
        });
        let sensitivity = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle(
                "auto-sensitivity",
                &t("appsettings.voice.autoSensitivity"),
                Some(&t("appsettings.voice.autoSensitivityHint")),
                prefs.auto_sensitivity,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.auto_sensitivity = on),
            ))
            .children(sensitivity_slider);
        let processing = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle(
                "echo",
                &t("appsettings.voice.echo"),
                Some(&t("appsettings.voice.echoHint")),
                prefs.echo_cancellation,
                true,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.echo_cancellation = on),
            ))
            .child(toggle(
                "noise",
                &t("appsettings.voice.noise"),
                Some(&t("appsettings.voice.noiseHint")),
                prefs.noise_suppression,
                true,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.noise_suppression = on),
            ))
            .child(toggle(
                "gain",
                &t("appsettings.voice.gain"),
                Some(&t("appsettings.voice.gainHint")),
                prefs.auto_gain_control,
                true,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.auto_gain_control = on),
            ))
            .child(div().text_xs().text_color(p.muted_foreground).child(t("desktop.voice.noProcessing")));
        let camera = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(448.0))
                    .h(px(252.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.muted)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .text_color(p.muted_foreground)
                    .child(icon("video-off").size(px(32.0)))
                    .child(div().max_w(px(320.0)).text_center().text_sm().child(t("desktop.voice.noCamera"))),
            )
            .child(toggle(
                "mirror",
                &t("appsettings.voice.mirror"),
                Some(&t("appsettings.voice.mirrorHint")),
                prefs.mirror_video,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.mirror_video = on),
            ));
        let volume = prefs.volume;
        let out = prefs.output_device.clone();
        let sound_row = |id: &'static str,
                         label: String,
                         hint: String,
                         on: bool,
                         sound: Sound,
                         put: fn(&mut Sounds, bool),
                         _this: &mut Self,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
            let (hover_bg, hover_fg) = (p.primary, p.primary_foreground);
            let out = out.clone();
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .id(SharedString::from(format!("play-{id}")))
                        .size(px(36.0))
                        .flex_none()
                        .rounded_full()
                        .bg(p.muted)
                        .text_color(p.muted_foreground)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                        .on_click(move |_, _, _| sounds::play(sound, volume, &out))
                        .child(icon("volume-2").size(px(16.0))),
                )
                .child(div().flex_1().min_w_0().child(toggle(
                    id,
                    &label,
                    Some(&hint),
                    on,
                    false,
                    p,
                    window,
                    cx,
                    move |this, v, cx| this.set(cx, |pr| put(&mut pr.sounds, v)),
                )))
                .into_any_element()
        };
        let _ = &mut *self;
        let cues = sound_row(
            "call-cues",
            t("appsettings.voice.cues"),
            t("appsettings.voice.cuesHint"),
            prefs.sounds.call,
            Sound::Call,
            |s, v| s.call = v,
            self,
            window,
            cx,
        );
        let ring = sound_row(
            "ringtone",
            t("appsettings.voice.ringtone"),
            t("appsettings.voice.ringtoneHint"),
            prefs.sounds.ring,
            Sound::Ring,
            |s, v| s.ring = v,
            self,
            window,
            cx,
        );
        let call_sounds = div().flex().flex_col().gap(px(12.0)).child(cues).child(ring);
        let d = Prefs::default();
        let devices_changed = prefs.input_device != d.input_device
            || prefs.output_device != d.output_device
            || prefs.input_volume != d.input_volume
            || prefs.output_volume != d.output_volume;
        let mut rows: Vec<(&'static str, String, Option<String>, Badge, AnyElement)> = vec![
            (
                "devices",
                t("appsettings.voice.devices"),
                None,
                Badge::pref(devices_changed, |pr| {
                    let d = Prefs::default();
                    pr.input_device = d.input_device;
                    pr.output_device = d.output_device;
                    pr.input_volume = d.input_volume;
                    pr.output_volume = d.output_volume;
                }),
                devices.into_any_element(),
            ),
            (
                "mic-test",
                t("appsettings.voice.micTest"),
                Some(t("appsettings.voice.micTestHint")),
                Badge::pref(false, |_| {}),
                mic_test.into_any_element(),
            ),
            ("input-mode", t("appsettings.voice.inputMode"), None, pref!(prefs, input_mode), mode),
        ];
        if prefs.input_mode == InputMode::Voice {
            rows.push((
                "sensitivity",
                t("appsettings.voice.sensitivity"),
                None,
                Badge::pref(prefs.auto_sensitivity != d.auto_sensitivity || prefs.sensitivity != d.sensitivity, |pr| {
                    pr.auto_sensitivity = true;
                    pr.sensitivity = Prefs::default().sensitivity;
                }),
                sensitivity.into_any_element(),
            ));
        }
        rows.extend([
            (
                "processing",
                t("appsettings.voice.processing"),
                None,
                Badge::pref(
                    prefs.echo_cancellation != d.echo_cancellation
                        || prefs.noise_suppression != d.noise_suppression
                        || prefs.auto_gain_control != d.auto_gain_control,
                    |pr| {
                        pr.echo_cancellation = true;
                        pr.noise_suppression = true;
                        pr.auto_gain_control = true;
                    },
                ),
                processing.into_any_element(),
            ),
            ("camera", t("appsettings.voice.camera"), None, pref!(prefs, mirror_video), camera.into_any_element()),
            (
                "call-sounds",
                t("appsettings.voice.callSounds"),
                None,
                Badge::pref(prefs.sounds.call != d.sounds.call || prefs.sounds.ring != d.sounds.ring, |pr| {
                    let d = Sounds::default();
                    pr.sounds.call = d.call;
                    pr.sounds.ring = d.ring;
                }),
                call_sounds.into_any_element(),
            ),
        ]);
        let count = rows.len();
        let mut list = div().flex().flex_col();
        for (n, (id, title, hint, badge, body)) in rows.into_iter().enumerate() {
            list = list.child(self.setting(
                id,
                &title,
                hint.map(|h| crate::ui::settings_controls::hint(h, p)),
                badge,
                At::of(n, count),
                body,
                p,
                cx,
            ));
        }
        let _ = Duration::ZERO;
        list.into_any_element()
    }
}

/// The meter's level, eased so it glides between readings.
fn motion_level(db: f32, window: &mut Window, cx: &mut Context<SettingsView>) -> f32 {
    crate::ui::motion::follow("mic-level", db, window, cx)
}
