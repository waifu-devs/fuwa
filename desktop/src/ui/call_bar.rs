//! The call you're in, above who you are at the bottom of the sidebar (the
//! web's components/calls/CallPanel.tsx): how the connection is, where the
//! call is and how long it's run, hanging up, and the camera and record
//! buttons. And the people in each voice channel under its row
//! (VoiceUsers.tsx), lit up while they speak.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::calls::clock;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::voice::access;
use crate::core::voice::devices::Trouble;
use crate::core::voice::{CallView, Status};
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::call_parts::{
    CallPop, Side, Size, amber, green, hang_up_button, ping_color, signal, voice_avatar, voice_flags,
};
use crate::ui::motion;
use crate::ui::theme::{alpha, radius_lg, radius_md};
use crate::ui::widgets::{app_badge, icon, is_agent, pal};

/// How tall one person under a voice channel is: `py-1` around a 24 px avatar.
pub const PERSON: f32 = 32.0;

impl FuwaApp {
    /// The call panel, while you're in a call; and the notice when one ended on its own.
    pub(crate) fn call_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(ended) = self.core.take_call_ended() {
            self.toast("phone-off", t("desktop.voice.left"), ended.message(), None, None, cx);
        }
        self.call_notices(cx);
        let Some(call) = self.core.call() else {
            self.calls.ticking = None;
            self.calls.mic_missing = false;
            if matches!(self.calls.pop, Some(CallPop::Connection | CallPop::Record { .. })) {
                self.calls.pop = None;
            }
            return None;
        };
        self.keep_ticking(&call, cx);
        self.apply_call_settings();
        let p = pal(cx);
        let (where_, dm) = self.call_place(&call);
        let (label, tint) = match call.status {
            Status::Connected => (t("dms-calls.calls.status.connected"), green().into()),
            Status::Connecting => (t("dms-calls.calls.status.connecting"), p.muted_foreground.into()),
            Status::Reconnecting => (t("dms-calls.calls.status.reconnecting"), gpui_kit::Hsla::from(amber())),
        };
        let connected = call.status == Status::Connected;
        let seconds = call.since.map(|s| s.elapsed().as_secs()).unwrap_or(0);
        let open = self.calls.pop == Some(CallPop::Connection);
        let hover = alpha(p.muted, 0.6);
        let status = div()
            .id("call-status")
            .relative()
            .ml(px(-4.0))
            .mr(px(-4.0))
            .px(px(4.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(radius_md())
            .text_sm()
            .line_height(px(20.0))
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_color(tint)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(|s| s.opacity(0.9))
            .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("dms-calls.calls.panel.details")).build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_call_pop(CallPop::Connection, cx)))
            .child(signal(&call.status, &call.quality, &p, "panel", window))
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
            .when(connected, |el| {
                el.child(
                    div()
                        .ml_auto()
                        .flex_none()
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(ping_color(&call.quality, &p))
                        .child(call.quality.ping_text()),
                )
            })
            .when(open, |el| el.child(self.hang(self.connection_card(&call, window, cx), Side::Above)));
        let fg = p.foreground;
        let place = div()
            .id("call-where")
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(4.0))
            .text_xs()
            .line_height(px(16.0))
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .hover(move |s| s.text_color(fg))
            .on_click(cx.listener(|this, _, window, cx| this.open_call_place(window, cx)))
            .when(dm, |el| {
                el.child(
                    div()
                        .id("call-lock")
                        .flex_none()
                        .tooltip(|window, cx| {
                            crate::ui::overlay::Tip::new(t("dms-calls.calls.panel.encrypted")).build(window, cx)
                        })
                        .child(icon("lock-keyhole").size(px(12.0))),
                )
            })
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(where_))
            .when(connected, |el| el.child(div().ml_auto().flex_none().pl(px(8.0)).child(clock(seconds))));
        let blocked = call.trouble.contains(&Trouble::MicrophoneBlocked);
        let no_mic = blocked || call.trouble.contains(&Trouble::NoMicrophone);
        if no_mic && !self.calls.mic_missing && !call.self_mute {
            self.core.mute_for_no_microphone();
        }
        self.calls.mic_missing = no_mic;
        let trouble = match (no_mic, call.trouble.contains(&Trouble::NoSpeakers)) {
            _ if blocked => Some(t("desktop.voice.micBlocked")),
            (true, true) => Some(t("desktop.voice.noDevices")),
            (true, false) => Some(t("workspace.calls.mic.notFound")),
            (false, true) => Some(t("desktop.voice.noSpeakers")),
            (false, false) => None,
        };
        // The system won't let the camera or screen start (macOS's privacy settings).
        let video_trouble = call.video_trouble.as_ref().map(|(screen, _)| *screen);
        let trouble = trouble.or_else(|| {
            video_trouble
                .map(|screen| t(if screen { "desktop.video.screenBlocked" } else { "desktop.video.cameraBlocked" }))
        });
        // Where to let it in: the microphone first, as its line comes first.
        let settings = if blocked { Some(None) } else { video_trouble.map(Some) };
        let allow = settings.filter(|_| access::has_settings()).map(|screen| {
            div()
                .id("call-allow")
                .ml(px(18.0))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.primary)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(t("desktop.voice.openPrivacy"))
                .on_click(move |_, _, _| match screen {
                    None => access::open_settings(access::Device::Microphone),
                    Some(true) => crate::core::voice::capture::open_screen_settings(),
                    Some(false) => access::open_settings(access::Device::Camera),
                })
        });
        let warn: gpui_kit::Rgba = if p.dark { gpui_kit::rgb(0xfbbf24) } else { gpui_kit::rgb(0xd97706) };
        let leave = if dm { t("dms-calls.calls.dm.hangUp") } else { t("dms-calls.calls.controls.disconnect") };
        let buttons = div()
            .flex()
            .gap(px(6.0))
            .child(self.camera_button("panel", Size::Sm, true, cx))
            .child(self.screen_button("panel", Size::Sm, true, window, cx))
            .when_some(self.record_button("panel", Size::Sm, true, window, cx), |el, b| el.child(b));
        let body = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(8.0))
            .pb(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w_0().px(px(6.0)).flex().flex_col().child(status).child(place))
                    .child(hang_up_button("call-leave", Size::Sm, leave, &p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.core.leave_voice();
                            cx.notify();
                        },
                    ))),
            )
            .child(buttons)
            .when_some(trouble, |el, text| {
                el.child(motion::rise(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(6.0))
                        .px(px(6.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(warn)
                        .child(div().mt(px(2.0)).flex_none().child(icon("triangle-alert").size(px(12.0))))
                        .child(div().min_w_0().child(text)),
                    "call-problem",
                    Duration::ZERO,
                    6.0,
                ))
            })
            .children(allow);
        let panel = div().flex_none().border_t_1().border_color(p.border).bg(alpha(p.background, 0.6)).child(body);
        let id = SharedString::from(format!("call-bar|{}|{}{}", call.instance, call.channel_id, call.conversation_id));
        Some(motion::rise(panel, id, Duration::ZERO, 12.0).into_any_element())
    }

    /// Draws the window again every second while a call is connected, for its clock.
    fn keep_ticking(&mut self, call: &CallView, cx: &mut Context<Self>) {
        if call.status != Status::Connected {
            self.calls.ticking = None;
            return;
        }
        if self.calls.ticking.is_some() {
            return;
        }
        self.calls.ticking = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    /// Where the call is, as the panel says it ("Lounge / Waifu Devs", or
    /// the other person), and whether it's a direct message's.
    pub(crate) fn call_place(&self, call: &CallView) -> (String, bool) {
        self.core.shared.read(|s| {
            let i = s.instance(&call.instance);
            if call.is_dm() {
                let me = i.and_then(|i| i.me.as_ref().map(|m| m.id.clone())).unwrap_or_default();
                let name = i
                    .and_then(|i| i.dms.conversations.iter().find(|c| c.id == call.conversation_id))
                    .and_then(|c| c.users.iter().find(|u| u.id != me))
                    .map(crate::core::store::user_name)
                    .unwrap_or_default();
                return (name, true);
            }
            let channel = i
                .and_then(|i| i.channel(&call.server_id, &call.channel_id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| t("dms-calls.calls.panel.voice"));
            let server = i.and_then(|i| i.server(&call.server_id)).map(|s| s.name.clone()).unwrap_or_default();
            let text = if server.is_empty() {
                channel
            } else {
                t_with("dms-calls.calls.panel.where", &[("channel", Arg::Str(&channel)), ("server", Arg::Str(&server))])
            };
            (text, false)
        })
    }

    /// Goes to the call: its voice channel, or its conversation.
    fn open_call_place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(call) = self.core.call() else { return };
        if call.is_dm() {
            self.navigate(Nav::Home { dm: Some((call.instance.clone(), call.conversation_id.clone())) }, window, cx);
        } else {
            self.open_stage(&call.instance, &call.server_id, &call.channel_id, window, cx);
        }
    }

    /// The people in a voice channel, for under its row, and how tall they are.
    pub(crate) fn voice_people(
        &mut self,
        key: &str,
        server: &str,
        channel_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, f32)> {
        let p = pal(cx);
        let (people, me) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let list: Vec<_> = crate::core::calls::in_channel(&i.voice, server, channel_id)
                .into_iter()
                .map(|v| (v.clone(), i.users.get(&v.user_id).cloned(), i.display_name(Some(server), &v.user_id)))
                .collect();
            Some((list, i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default()))
        })?;
        if people.is_empty() {
            return None;
        }
        let call: Option<CallView> = self.core.call().filter(|c| c.in_channel(key, channel_id));
        let height = PERSON * people.len() as f32;
        let mut rows = div().ml(px(24.0)).flex().flex_col();
        for (n, (state, user, name)) in people.into_iter().enumerate() {
            let speaking = call.as_ref().is_some_and(|c| c.speaking.contains(&state.user_id));
            let tag = format!("side|{server}|{}", state.user_id);
            let from = format!("side|{}", state.user_id);
            let pop = CallPop::Person {
                key: key.to_owned(),
                server: Some(server.to_owned()),
                channel: Some(channel_id.to_owned()),
                user: state.user_id.clone(),
                from: from.clone(),
            };
            let open = self.calls.pop.as_ref() == Some(&pop);
            let hover = alpha(p.muted, 0.7);
            let fg = p.foreground;
            let agent = is_agent(user.as_ref());
            let row = div()
                .id(SharedString::from(format!("voice-person|{channel_id}|{}", state.user_id)))
                .h(px(PERSON))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_lg())
                .text_sm()
                .line_height(px(20.0))
                .text_color(if speaking { p.foreground } else { p.muted_foreground })
                .cursor_pointer()
                .when(open, |el| el.bg(hover))
                .hover(move |s| s.bg(hover).text_color(fg))
                .child(voice_avatar(user.as_ref(), &state.user_id, 24.0, 9.6, 2.0, speaking, &tag, window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .when(speaking, |el| el.font_weight(FontWeight::BOLD))
                        .child(name),
                )
                .when(agent, |el| el.child(app_badge(SharedString::from(format!("agent|{tag}")), "APP", &p)))
                .child(voice_flags(&state, &p));
            let row = if state.user_id == me {
                row
            } else {
                row.on_click(cx.listener(move |this, _, _, cx| this.toggle_call_pop(pop.clone(), cx)))
            };
            let mut holder = div().relative().child(row);
            if open {
                let card = self.person_card(key, Some(server), Some(channel_id), &state.user_id, window, cx);
                holder = holder.child(self.hang(card, Side::Right));
            }
            rows = rows.child(motion::slide_in(
                holder,
                SharedString::from(format!("voice-in|{channel_id}|{}|{n}", state.user_id)),
                -12.0,
            ));
        }
        // The row above keeps a 2 px gap under it that, on the web, comes after the people.
        Some((div().mt(px(-2.0)).pb(px(2.0)).child(rows).into_any_element(), height))
    }
}
