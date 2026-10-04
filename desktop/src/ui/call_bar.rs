//! The voice channel you're in, above who you are at the bottom of the
//! sidebar: where, how the connection is, and mute, deafen and leave. And
//! the people in each voice channel under its row, lit up while they speak.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::voice::devices::Trouble;
use crate::core::voice::{CallView, Status};
use crate::ui::app::FuwaApp;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, icon, icon_button_in, pal};

/// How tall one person under a voice channel is.
pub const PERSON: f32 = 28.0;

impl FuwaApp {
    /// The call bar, while you're in a call; and the notice when one ended on its own.
    pub(crate) fn call_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(ended) = self.core.take_call_ended() {
            self.toast("phone-off", "Left the voice channel".into(), ended.message(), None, None, cx);
        }
        let call = self.core.call()?;
        let p = pal(cx);
        let (channel, server) = self.core.shared.read(|s| {
            let i = s.instance(&call.instance);
            (
                i.and_then(|i| i.channel(&call.server_id, &call.channel_id))
                    .map(|c| c.name.clone())
                    .unwrap_or_default(),
                i.and_then(|i| i.server(&call.server_id)).map(|s| s.name.clone()).unwrap_or_default(),
            )
        });
        let (label, tint) = match call.status {
            Status::Connected => ("Voice connected", p.success),
            Status::Connecting => ("Connecting…", p.primary),
            Status::Reconnecting => ("Reconnecting…", p.destructive),
        };
        let muted = call.self_mute || call.self_deaf;
        let dot = div().size(px(8.0)).rounded_full().bg(tint);
        let dot = if call.status == Status::Connected {
            dot.into_any_element()
        } else {
            // Breathes while it's working on it.
            motion::ambient(dot, "call-dot", Duration::from_millis(1200), window, |el, t| {
                el.opacity(0.35 + 0.65 * (0.5 - 0.5 * (t * std::f32::consts::TAU).cos()))
            })
        };
        let trouble = match (call.trouble.contains(&Trouble::NoMicrophone), call.trouble.contains(&Trouble::NoSpeakers))
        {
            (true, true) => Some("No microphone or speakers found"),
            (true, false) => Some("No microphone found: others can't hear you"),
            (false, true) => Some("No speakers found: you can't hear others"),
            (false, false) => None,
        };
        let bar = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(10.0))
            .bg(alpha(p.rail, 0.6))
            .border_t_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(tint)
                                    .child(dot)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(format!("{channel} · {server}")),
                            ),
                    )
                    .child(icon_button_in("call-leave", "phone-off", &p, p.destructive).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.core.leave_voice();
                            cx.notify();
                        },
                    ))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        // Deafened is muted too.
                        toggle("call-mute", if muted { "mic-off" } else { "mic" }, muted, &p).on_click(cx.listener(
                            |this, _, _, cx| {
                                let muted = this.core.call().is_some_and(|c| c.self_mute || c.self_deaf);
                                this.core.set_self_mute(!muted);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        toggle(
                            "call-deaf",
                            if call.self_deaf { "headphone-off" } else { "headphones" },
                            call.self_deaf,
                            &p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let deaf = this.core.call().is_some_and(|c| c.self_deaf);
                            this.core.set_self_deaf(!deaf);
                            cx.notify();
                        })),
                    ),
            )
            .when_some(trouble, |el, text| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .text_color(p.destructive)
                        .child(icon("triangle-alert").size(px(13.0)))
                        .child(text),
                )
            });
        let id = SharedString::from(format!("call-bar|{}|{}", call.server_id, call.channel_id));
        Some(motion::rise(bar, id, Duration::ZERO, 12.0).into_any_element())
    }

    /// The people in a voice channel, for under its row, and how tall they are.
    pub(crate) fn voice_people(
        &self,
        key: &str,
        server: &str,
        channel_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, f32)> {
        let p = pal(cx);
        let people = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let list: Vec<_> = crate::core::calls::in_channel(&i.voice, server, channel_id)
                .into_iter()
                .map(|v| (v.clone(), i.users.get(&v.user_id).cloned(), i.display_name(Some(server), &v.user_id)))
                .collect();
            Some(list)
        })?;
        if people.is_empty() {
            return None;
        }
        let call: Option<CallView> =
            self.core.call().filter(|c| c.instance == key && c.server_id == server && c.channel_id == channel_id);
        let height = PERSON * people.len() as f32;
        let rows = people.into_iter().map(|(state, user, name)| {
            let speaking = call.as_ref().is_some_and(|c| c.speaking.contains(&state.user_id));
            // The ring springs on and off rather than blinking with each word.
            let ring = motion::follow(
                SharedString::from(format!("ring|{server}|{}", state.user_id)),
                if speaking { 1.0 } else { 0.0 },
                window,
                cx,
            );
            let muted = state.self_mute || state.server_mute;
            let deaf = state.self_deaf || state.server_deaf;
            div()
                .h(px(PERSON))
                .pl(px(34.0))
                .pr(px(10.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_sm()
                .text_color(if speaking { p.foreground } else { p.muted_foreground })
                .child(
                    div().rounded_full().border_2().border_color(alpha(p.success, ring.clamp(0.0, 1.0))).child(avatar(
                        user.as_ref(),
                        18.0,
                        &p,
                    )),
                )
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(name))
                .when(muted, |el| el.child(person_icon("mic-off", state.server_mute, &p)))
                .when(deaf, |el| el.child(person_icon("headphone-off", state.server_deaf, &p)))
        });
        Some((div().flex().flex_col().children(rows).into_any_element(), height))
    }
}

/// Mute or deafen: red while on.
fn toggle(id: &'static str, name: &str, on: bool, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (bg, fg) =
        if on { (alpha(p.destructive, 0.16), p.destructive) } else { (alpha(p.primary, 0.08), p.foreground) };
    let hover = if on { alpha(p.destructive, 0.24) } else { alpha(p.primary, 0.16) };
    div()
        .id(id)
        .flex_1()
        .h(px(30.0))
        .rounded(corner(10.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(bg)
        .text_color(fg)
        .hover(move |s| s.bg(hover))
        .active(|s| s.top(px(1.0)))
        .child(icon(name).size(px(17.0)))
}

/// Muted or deafened, red when a moderator did it.
fn person_icon(name: &str, by_moderator: bool, p: &Palette) -> impl IntoElement {
    icon(name).size(px(13.0)).text_color(if by_moderator { p.destructive } else { p.muted_foreground })
}
