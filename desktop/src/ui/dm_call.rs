//! Calls in direct messages (the web's components/calls/DmCall.tsx and
//! IncomingCalls.tsx): the call button in a conversation's header, the
//! call across the top of the conversation while one goes on (tiles while a
//! camera or screen is on), and the card that rings wherever you are when
//! someone calls you. Every frame, sound and picture, is sealed end to end
//! (core/voice): the instance only passes them on.

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, relative,
};

use crate::core::calls::clock;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::voice::Status;
use crate::pb;
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::call_parts::{
    CallPop, Side, Size, camera_button, green, hang_up_button, person_avatar, screen_button, voice_avatar,
};
use crate::ui::motion;
use crate::ui::popout::Popped;
use crate::ui::settings_controls::shadow_xl;
use crate::ui::text::ms_of;
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_3xl, radius_lg};
use crate::ui::video::{live_badge, pop_out_button};
use crate::ui::widgets::{hue_gradient, icon, pal};

/// How long a new call rings for; after that it's still there to join from the conversation.
const RING: Duration = Duration::from_secs(45);
const RING_EVERY: Duration = Duration::from_millis(2600);

/// A call ringing for you.
struct Ringing {
    instance: String,
    conversation: String,
    caller: Option<pb::User>,
    started_at: i64,
    key: String,
}

/// Waves going out from someone, `size` across: the web's `.call-wave`, twice.
fn waves(tag: &str, size: f32, window: &mut Window) -> impl Iterator<Item = AnyElement> {
    let mut out = Vec::new();
    for (n, a) in [(0u8, 0.6f32), (1, 0.4)] {
        let ring = div().absolute().size(px(size)).rounded_full().border_2().border_color(alpha(green(), a));
        out.push(motion::ambient(
            ring,
            SharedString::from(format!("wave|{tag}|{n}")),
            Duration::from_secs(2),
            window,
            move |el, t| {
                let t = (t + f32::from(n) * 0.5).rem_euclid(1.0);
                let k = 1.0 - (1.0 - t).powf(2.6);
                let s = size * (1.0 + 0.9 * k);
                el.size(px(s)).top(px((size - s) / 2.0)).left(px((size - s) / 2.0)).opacity(0.55 * (1.0 - k))
            },
        ));
    }
    out.into_iter()
}

impl FuwaApp {
    /// The web's `CallButton`: starts a call from a conversation's header, or
    /// joins the one going on (green, the phone shaking).
    pub(crate) fn dm_call_button(
        &self,
        key: &str,
        conversation: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.core.call().is_some_and(|c| c.in_conversation(key, conversation)) {
            return None;
        }
        let p = pal(cx);
        let going = self.core.shared.read(|s| {
            s.instance(key).and_then(|i| i.dms.calls.get(conversation)).is_some_and(|c| !c.participants.is_empty())
        });
        let label = t(if going { "dms-calls.calls.dm.join" } else { "dms-calls.calls.dm.startTitle" });
        let (k, c) = (key.to_owned(), conversation.to_owned());
        let hover = p.muted;
        let fg = p.foreground;
        let glyph = if going {
            motion::ambient(
                icon("phone-call").size(px(18.0)),
                "call-shake",
                Duration::from_millis(1600),
                window,
                |el, t| {
                    let a = if t < 0.4 { (t * 50.0 * std::f32::consts::PI).sin() * 0.28 } else { 0.0 };
                    el.rotate(gpui_kit::radians(a))
                },
            )
        } else {
            icon("phone").size(px(18.0)).into_any_element()
        };
        Some(
            div()
                .id("dm-call")
                .size(px(36.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(going, |el| el.bg(green()).text_color(gpui_kit::white()).hover(|s| s.opacity(0.92)))
                .when(!going, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover).text_color(fg)))
                .active(|s| s.opacity(0.85))
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.core.join_dm_call(&k, &c);
                    cx.notify();
                }))
                .child(glyph)
                .into_any_element(),
        )
    }

    /// The web's `DmCallStrip`: the call across the top of the conversation,
    /// both of you glowing while you talk, with the controls.
    pub(crate) fn dm_call_strip(
        &mut self,
        key: &str,
        conversation: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = pal(cx);
        let mine = self.core.call().filter(|c| c.in_conversation(key, conversation));
        let in_call = mine.is_some();
        let (users, me, call) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let conv = i.dms.conversations.iter().find(|c| c.id == conversation)?;
            Some((conv.users.clone(), i.me.as_ref()?.id.clone(), i.dms.calls.get(conversation).cloned()))
        })?;
        let participants = call.as_ref().map(|c| c.participants.clone()).unwrap_or_default();
        if !in_call && participants.is_empty() {
            return None;
        }
        let mut here: Vec<String> = participants.iter().map(|v| v.user_id.clone()).collect();
        if in_call {
            here.push(me.clone());
        }
        let partner = users.iter().find(|u| u.id != me);
        let name_of = |u: Option<&pb::User>| u.map(crate::core::store::user_name).unwrap_or_default();
        // The line under the call: who started it, connecting, calling them, or how long it's run.
        let mut line = match &mine {
            None => {
                let starter = call.as_ref().and_then(|c| users.iter().find(|u| u.id == c.started_by)).or(partner);
                t_with("dms-calls.calls.dm.started", &[("name", Arg::Str(&name_of(starter)))])
            }
            Some(c) if c.status != Status::Connected => t(if c.status == Status::Reconnecting {
                "dms-calls.calls.status.reconnecting"
            } else {
                "dms-calls.calls.status.connecting"
            }),
            Some(c) => {
                if partner.is_none_or(|u| !here.contains(&u.id)) {
                    t_with("dms-calls.calls.dm.calling", &[("name", Arg::Str(&name_of(partner)))])
                } else {
                    match c.since {
                        Some(since) => clock(since.elapsed().as_secs()),
                        None => t("dms-calls.calls.dm.inCall"),
                    }
                }
            }
        };
        let recorders: Vec<String> = users
            .iter()
            .filter(|u| {
                if u.id == me {
                    mine.as_ref().is_some_and(|c| c.self_record)
                } else {
                    participants.iter().any(|v| v.user_id == u.id && v.self_record)
                }
            })
            .map(|u| if u.id == me { t("dms-calls.calls.dm.you") } else { name_of(Some(u)) })
            .collect();
        if !recorders.is_empty() {
            line = t_with(
                "dms-calls.calls.dm.recording",
                &[("line", Arg::Str(&line)), ("names", Arg::Str(&recorders.join(", ")))],
            );
        }
        let speaking = mine.as_ref().map(|c| c.speaking.clone()).unwrap_or_default();
        // Whose camera and screen are on: the others' only while you're in the call.
        let (my_video, my_stream) = mine.as_ref().map(|c| (c.self_video, c.self_stream)).unwrap_or_default();
        let on = |user: &str, screen: bool| {
            if !in_call {
                return false;
            }
            if user == me {
                return if screen { my_stream } else { my_video };
            }
            participants.iter().any(|v| v.user_id == user && if screen { v.self_stream } else { v.self_video })
        };
        let filming = users.iter().any(|u| on(&u.id, false) || on(&u.id, true));
        let mut people = div().flex().items_center().gap(px(40.0));
        for user in &users {
            let is_here = here.contains(&user.id);
            let ringing = in_call && !is_here;
            let talking = speaking.contains(&user.id);
            let tag = format!("dm|{conversation}|{}", user.id);
            let grow =
                motion::follow(SharedString::from(format!("{tag}|grow")), if talking { 1.06 } else { 1.0 }, window, cx);
            let dim =
                motion::follow(SharedString::from(format!("{tag}|dim")), if is_here { 1.0 } else { 0.45 }, window, cx);
            let pop = CallPop::Person {
                key: key.to_owned(),
                server: None,
                channel: None,
                user: user.id.clone(),
                from: tag.clone(),
            };
            let open = self.calls.pop.as_ref() == Some(&pop);
            let size = 80.0 * grow;
            let face = div()
                .id(SharedString::from(tag.clone()))
                .relative()
                .size(px(80.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .when(ringing, |el| el.children(waves(&tag, 80.0, window)))
                .child(div().opacity(dim.clamp(0.0, 1.0)).child(voice_avatar(
                    Some(user),
                    &user.id,
                    size,
                    24.0 * grow,
                    4.0,
                    talking,
                    &tag,
                    window,
                    cx,
                )))
                .when(user.id != me, |el| {
                    el.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_call_pop(pop.clone(), cx)))
                });
            let mut holder = div().relative().child(face);
            if open {
                let card = self.person_card(key, None, None, &user.id, window, cx);
                holder = holder.child(self.hang(card, Side::Right));
            }
            people = people.child(holder);
        }
        let people = if filming {
            let mut tiles = div().w_full().max_w(px(768.0)).flex().flex_wrap().gap(px(12.0));
            for user in users.iter().filter(|u| on(&u.id, true)) {
                tiles = tiles.child(self.dm_screen_tile(key, user, user.id == me, &p, window, cx));
            }
            for user in &users {
                let here = here.contains(&user.id);
                let talking = speaking.contains(&user.id);
                tiles = tiles.child(self.dm_camera_tile(key, user, here, on(&user.id, false), talking, &p, window, cx));
            }
            motion::rise(tiles, SharedString::from(format!("dm-cameras|{conversation}")), Duration::ZERO, 8.0)
                .into_any_element()
        } else {
            people.into_any_element()
        };
        let controls: AnyElement = if in_call {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(self.mute_buttons("dm", Size::Lg, cx))
                .child(camera_button("dm-camera", Size::Lg, false, &p))
                .child(screen_button("dm-screen", Size::Lg, false, &p))
                .when_some(self.record_button("dm", Size::Lg, false, window, cx), |el, b| el.child(b))
                .child(hang_up_button("dm-hang-up", Size::Lg, t("dms-calls.calls.dm.hangUp"), &p).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.core.leave_voice();
                        cx.notify();
                    }),
                ))
                .into_any_element()
        } else {
            let (k, c) = (key.to_owned(), conversation.to_owned());
            div()
                .id("dm-join-call")
                .h(px(44.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_2xl())
                .bg(green())
                .text_color(gpui_kit::white())
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .cursor_pointer()
                .hover(|s| s.opacity(0.92))
                .active(|s| s.opacity(0.85))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.core.join_dm_call(&k, &c);
                    cx.notify();
                }))
                .child(icon("phone").size(px(16.0)))
                .child(t("dms-calls.calls.dm.joinCall"))
                .into_any_element()
        };
        let top = mix(p.background, green(), 0.14);
        let strip = div()
            .flex_none()
            .overflow_hidden()
            .border_b_1()
            .border_color(p.border)
            .bg(gpui_kit::linear_gradient(
                180.0,
                gpui_kit::linear_color_stop(top, 0.0),
                gpui_kit::linear_color_stop(p.background, 1.0),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(16.0))
                    .py(px(20.0))
                    .child(people)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(icon("lock-keyhole").size(px(14.0)))
                            .child(line),
                    )
                    .child(controls),
            );
        Some(
            motion::rise(strip, SharedString::from(format!("dm-strip|{conversation}")), Duration::ZERO, -8.0)
                .into_any_element(),
        )
    }

    /// One of you while a camera is on in the call (the web's `Camera`):
    /// a tile with the camera, or the avatar.
    #[allow(clippy::too_many_arguments)]
    fn dm_camera_tile(
        &mut self,
        key: &str,
        user: &pb::User,
        here: bool,
        video: bool,
        talking: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = crate::core::store::user_name(user);
        let group = format!("dm-tile|{}", user.id);
        let feed = crate::core::voice::video::feed_of(&user.id, false);
        let camera = video.then(|| crate::ui::video::feed_view(&self.core, &feed, ObjectFit::Cover, window, cx));
        let tile = div()
            .relative()
            .w_full()
            .aspect_ratio(16.0 / 9.0)
            .overflow_hidden()
            .rounded(radius_2xl())
            .border_1()
            .border_color(if talking { gpui_kit::Hsla::from(green()) } else { p.border.into() })
            .bg(p.card)
            .child(hue_gradient(&user.id, 16.0, div().absolute().inset_0()).opacity(0.25))
            .child(div().absolute().inset_0().flex().items_center().justify_center().child(voice_avatar(
                Some(user),
                &user.id,
                64.0,
                20.0,
                4.0,
                talking,
                &format!("dm-tile|{}", user.id),
                window,
                cx,
            )))
            .children(camera)
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .bottom(px(8.0))
                    .max_w(relative(0.8))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .rounded(radius_lg())
                    .bg(alpha(p.background, 0.75))
                    .px(px(8.0))
                    .py(px(2.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .child(name.clone()),
            );
        let popped = Popped { instance: key.to_owned(), user: user.id.clone(), server: None, screen: false };
        div()
            .relative()
            .w(relative(0.48))
            .flex_grow(1.0)
            .group(SharedString::from(group.clone()))
            .when(!here, |el| el.opacity(0.5))
            .child(tile)
            .when(here, |el| {
                el.child(
                    pop_out_button(
                        SharedString::from(format!("dm-pop|{}", user.id)),
                        t_with("dms-calls.calls.video.popOutTitle", &[("name", Arg::Str(&name))]),
                        &group,
                        p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.pop_out(popped.clone(), cx))),
                )
            })
            .into_any_element()
    }

    /// A shared screen in the call: the whole width, shown whole.
    fn dm_screen_tile(
        &mut self,
        key: &str,
        user: &pb::User,
        mine: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = crate::core::store::user_name(user);
        let group = format!("dm-screen|{}", user.id);
        let feed = crate::core::voice::video::feed_of(&user.id, true);
        let label = if mine {
            t("dms-calls.calls.screen.yours")
        } else {
            t_with("dms-calls.calls.screen.theirs", &[("name", Arg::Str(&name))])
        };
        let popped = Popped { instance: key.to_owned(), user: user.id.clone(), server: None, screen: true };
        let tile = div()
            .relative()
            .w_full()
            .aspect_ratio(16.0 / 9.0)
            .overflow_hidden()
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(gpui_kit::black())
            .child(crate::ui::video::feed_view(&self.core, &feed, ObjectFit::Contain, window, cx))
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .bottom(px(8.0))
                    .max_w(relative(0.8))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(radius_lg())
                    .bg(alpha(p.background, 0.8))
                    .px(px(8.0))
                    .py(px(2.0))
                    .child(live_badge())
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .line_height(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .child(label),
                    ),
            );
        div()
            .relative()
            .w_full()
            .group(SharedString::from(group.clone()))
            .child(tile)
            .child(
                pop_out_button(
                    SharedString::from(format!("dm-pop-screen|{}", user.id)),
                    t_with("dms-calls.calls.video.popOutScreenTitle", &[("name", Arg::Str(&name))]),
                    &group,
                    p,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pop_out(popped.clone(), cx))),
            )
            .into_any_element()
    }

    /// Calls that started a moment ago in conversations you're in, that you're not in yet.
    fn ringing(&self) -> Vec<Ringing> {
        let now = crate::core::dms::now_ms();
        let current = self.core.call();
        let mut out = Vec::new();
        self.core.shared.read(|s| {
            for key in &s.order {
                let Some(i) = s.instance(key) else { continue };
                let Some(me) = i.me.as_ref().map(|m| m.id.clone()) else { continue };
                for call in i.dms.calls.values() {
                    if call.participants.is_empty() || call.participants.iter().any(|v| v.user_id == me) {
                        continue;
                    }
                    let started_at = ms_of(call.started_at.as_ref());
                    let ring_key = format!("{key}/{}/{started_at}", call.conversation_id);
                    if self.calls.declined.contains(&ring_key)
                        || now - started_at >= RING.as_millis() as i64
                        || current.as_ref().is_some_and(|c| c.in_conversation(key, &call.conversation_id))
                    {
                        continue;
                    }
                    let caller = i
                        .dms
                        .conversations
                        .iter()
                        .find(|c| c.id == call.conversation_id)
                        .and_then(|c| c.users.iter().find(|u| u.id == call.started_by).cloned());
                    out.push(Ringing {
                        instance: key.clone(),
                        conversation: call.conversation_id.clone(),
                        caller,
                        started_at,
                        key: ring_key,
                    });
                }
            }
        });
        out.sort_by_key(|r| r.started_at);
        out
    }

    /// The web's `IncomingCalls`: a card for each call ringing for you, in the
    /// corner wherever you are, with their face, a ring that keeps going, and
    /// answer or decline.
    pub(crate) fn render_incoming_calls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ringing = self.ringing();
        if ringing.is_empty() {
            self.calls.rang_at = None;
            return None;
        }
        // The ring, every couple of seconds while anything rings.
        if self.calls.rang_at.is_none_or(|at| at.elapsed() >= RING_EVERY) {
            self.calls.rang_at = Some(Instant::now());
            let streamer_quiet = self.prefs.streamer_mode && self.prefs.streamer_mute_sounds;
            if self.prefs.sounds.ring && !streamer_quiet {
                crate::core::sounds::play(
                    crate::core::sounds::Sound::Ring,
                    self.prefs.volume,
                    &self.prefs.output_device,
                );
            }
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RING_EVERY).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        let p = pal(cx);
        let many = self.core.shared.read(|s| s.order.len() > 1);
        let mut stack = div().flex().flex_col().items_end().gap(px(8.0));
        for r in ringing {
            stack = stack.child(self.call_card(r, many, &p, window, cx));
        }
        Some(div().absolute().right(px(16.0)).bottom(px(16.0)).child(stack).into_any_element())
    }

    fn call_card(
        &self,
        r: Ringing,
        many: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = r.caller.as_ref().map(crate::core::store::user_name).unwrap_or_default();
        let place = many
            .then(|| self.core.shared.read(|s| s.instance(&r.instance).map(|i| i.name())))
            .flatten()
            .filter(|_| !self.prefs.streamer_mode);
        let sub = match place {
            Some(place) => t_with("dms-calls.calls.incoming.encryptedOn", &[("place", Arg::Str(&place))]),
            None => t("dms-calls.calls.incoming.encrypted"),
        };
        let decline_key = r.key.clone();
        let (k, c) = (r.instance.clone(), r.conversation.clone());
        let round = |id: String, bg: gpui_kit::Rgba| {
            div()
                .id(SharedString::from(id))
                .size(px(44.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(bg)
                .text_color(gpui_kit::white())
                .cursor_pointer()
                .hover(|s| s.opacity(0.92))
                .active(|s| s.opacity(0.8))
        };
        let shake = motion::ambient(
            icon("phone").size(px(20.0)),
            SharedString::from(format!("answer-shake|{}", r.key)),
            Duration::from_millis(1600),
            window,
            |el, t| {
                let a = if t < 0.4 { (t * 50.0 * std::f32::consts::PI).sin() * 0.28 } else { 0.0 };
                el.rotate(gpui_kit::radians(a))
            },
        );
        let card = div()
            .w(px(384.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.card, 0.95))
            .p(px(12.0))
            .pr(px(14.0))
            .shadow(shadow_xl())
            .occlude()
            .child(
                div()
                    .relative()
                    .size(px(48.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(waves(&r.key, 48.0, window))
                    .child(person_avatar(r.caller.as_ref(), "?", 48.0, 16.0)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(icon("lock-keyhole").size(px(12.0)))
                            .child(sub),
                    ),
            )
            .child(
                round(format!("decline|{}", r.key), p.destructive)
                    .tooltip(|window, cx| {
                        crate::ui::overlay::Tip::new(t("dms-calls.calls.incoming.decline")).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.calls.declined.insert(decline_key.clone());
                        cx.notify();
                    }))
                    .child(icon("phone-off").size(px(20.0))),
            )
            .child(
                round(format!("answer|{}", r.key), green())
                    .tooltip(|window, cx| {
                        crate::ui::overlay::Tip::new(t("dms-calls.calls.incoming.answer")).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.core.join_dm_call(&k, &c);
                        this.navigate(Nav::Home { dm: Some((k.clone(), c.clone())) }, window, cx);
                        cx.notify();
                    }))
                    .child(shake),
            );
        motion::rise(card, SharedString::from(format!("ring|{}", r.key)), Duration::ZERO, -24.0).into_any_element()
    }
}
