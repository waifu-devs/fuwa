//! A voice channel, open (the web's components/calls/VoiceStage.tsx):
//! shared screens on top, big, then everyone in it as a tile that glows
//! while they talk (their camera, when it's on), who's recording, and the
//! controls for your own place there. Each tile pops out into a window of
//! its own.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, BoxShadow, Context, FontWeight, Image, ImageFormat,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Rgba, SharedString, SpringConfig,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, point, px, relative, sampled_easing,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::voice::Status;
use crate::pb;
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::call_parts::{
    CallPop, Side, Size, green, hang_up_button, in_voice, ping_dot, red, toggle_icon, voice_avatar, voice_flags,
};
use crate::ui::motion;
use crate::ui::popout::Popped;
use crate::ui::settings_controls::shadow_sm;
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_3xl, radius_xl};
use crate::ui::video::{live_badge, pop_out_button};
use crate::ui::widgets::{app_badge, hue_gradient, icon, is_agent, pal};

/// The stage's scroll area's padding (`p-6`) and the gap between tiles (`gap-4`).
const PAD: f32 = 24.0;
const GAP: f32 = 16.0;

/// The web's `gridFor`: how many columns, and the widest the grid gets.
pub(crate) fn grid_for(n: usize) -> (usize, f32) {
    match n {
        1 => (1, 576.0),
        2 => (2, 768.0),
        3..=4 => (2, 1024.0),
        5..=9 => (3, 1024.0),
        _ => (4, 1024.0),
    }
}

thread_local! {
    /// The stage's glow, made once per primary color.
    static GLOW: RefCell<Option<(u32, Arc<Image>)>> = const { RefCell::new(None) };
}

/// The web's `radial-gradient(ellipse at top, primary 10%, transparent 65%)`
/// behind the stage, as a small picture stretched over it (GPUI has no
/// radial gradients). Its ellipse reaches 0.69 of the width and 1.38 of the
/// height, as Chromium draws `ellipse at top` there.
fn glow(primary: Rgba) -> Arc<Image> {
    let key = (u32::from((primary.r * 255.0) as u8) << 16)
        | (u32::from((primary.g * 255.0) as u8) << 8)
        | u32::from((primary.b * 255.0) as u8);
    if let Some(found) = GLOW.with(|g| g.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, i)| i.clone())) {
        return found;
    }
    const N: u32 = 96;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = ((x as f32 + 0.5) / N as f32, (y as f32 + 0.5) / N as f32);
            let (dx, dy) = ((u - 0.5) / 0.69, v / 1.38);
            let t = (dx * dx + dy * dy).sqrt();
            let a = (0.10 * (1.0 - t / 0.65)).max(0.0);
            rgba.extend_from_slice(&[(key >> 16) as u8, (key >> 8) as u8, key as u8, (a * 255.0).round() as u8]);
        }
    }
    let image = Arc::new(Image::from_bytes(ImageFormat::Png, crate::ui::png::png(N, N, &rgba, false)));
    GLOW.with(|g| *g.borrow_mut() = Some((key, image.clone())));
    image
}

impl FuwaApp {
    /// The voice channel open in this server, if one is.
    pub(crate) fn stage_in(&self, key: &str, server: &str) -> Option<String> {
        let (k, s, c) = self.calls.stage.as_ref()?;
        (k == key && s == server).then(|| c.clone())
    }

    /// Opens a voice channel, joining it as it opens when you may connect
    /// there, as the web's (and Discord's) do.
    pub(crate) fn open_stage(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let may = self
            .core
            .shared
            .read(|s| s.instance(key).is_some_and(|i| i.access(server).has_in(channel, pb::Permission::Connect)));
        self.navigate(Nav::Server { key: key.to_owned(), server: server.to_owned() }, window, cx);
        self.calls.stage = Some((key.to_owned(), server.to_owned(), channel.to_owned()));
        self.calls.pop = None;
        if may {
            self.core.join_voice(key, server, channel);
        }
        cx.notify();
    }

    /// The voice channel's page.
    pub(crate) fn voice_stage(
        &mut self,
        key: &str,
        server: &str,
        channel: &pb::Channel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let (states, can_connect, can_speak, can_record) = self.core.shared.read(|s| {
            let i = s.instance(key);
            let states: Vec<pb::VoiceState> = i
                .map(|i| crate::core::calls::in_channel(&i.voice, server, &channel.id).into_iter().cloned().collect())
                .unwrap_or_default();
            let access = i.map(|i| i.access(server)).unwrap_or_default();
            (
                states,
                access.has_in(&channel.id, pb::Permission::Connect),
                access.has_in(&channel.id, pb::Permission::Speak),
                access.has_in(&channel.id, pb::Permission::Record),
            )
        });
        let call = self.core.call().filter(|c| c.in_channel(key, &channel.id));
        let joined = call.is_some();
        let me = self.core.shared.read(|s| s.instance(key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let me = me.unwrap_or_default();
        let mine = call.as_ref().map(|c| (c.self_video, c.self_stream)).unwrap_or_default();
        // Screens come through only while you're in the channel; yours from this app.
        let sharing: Vec<&pb::VoiceState> =
            states.iter().filter(|s| joined && if s.user_id == me { mine.1 } else { s.self_stream }).collect();

        // ── The header ──
        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("volume-2").size(px(20.0)).text_color(p.muted_foreground))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_size(px(16.0))
                    .line_height(px(24.0))
                    .child(motion::swap_text(
                        SharedString::from(format!("stage-name|{}", channel.id)),
                        channel.name.clone(),
                        16.0,
                        window,
                        cx,
                    )),
            )
            .child(div().flex_1())
            .when_some(self.recording_pill(key, server, &states, &p, window), |el, pill| el.child(pill))
            .when(can_record, |el| el.child(self.recordings_button(key, server, &channel.id, &states, &p, window, cx)))
            .when(!states.is_empty(), |el| {
                el.child(motion::pop_in(
                    div()
                        .flex_none()
                        .rounded_full()
                        .bg(p.muted)
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(in_voice(states.len())),
                    "in-voice-pill",
                    (0.5, 0.5),
                    0.8,
                    0.0,
                ))
            });

        // ── The people ──
        let (w, h) = self.calls.stage_box.get();
        let body: AnyElement = if states.is_empty() {
            empty(&channel.name, &p, window).into_any_element()
        } else {
            let (cols, widest) = grid_for(states.len());
            let width = (w - PAD * 2.0).min(widest).max(200.0);
            let tile_w = (width - GAP * (cols as f32 - 1.0)) / cols as f32;
            let tile_h = tile_w * 9.0 / 16.0;
            let rows = states.len().div_ceil(cols);
            let mut content_h = rows as f32 * tile_h + (rows as f32 - 1.0) * GAP;
            // Shared screens above, as wide as the stage lets them (two a row when it's wide).
            let screens_w = (w - PAD * 2.0).clamp(200.0, 1024.0);
            let screen_cols = if sharing.len() > 1 && screens_w >= 1000.0 { 2 } else { 1 };
            let screen_w = (screens_w - GAP * (screen_cols as f32 - 1.0)) / screen_cols as f32;
            let screen_h = screen_w * 9.0 / 16.0;
            let screens = (!sharing.is_empty()).then(|| {
                let mut row = div().w(px(screens_w)).mx_auto().mb(px(GAP)).flex().flex_wrap().gap(px(GAP));
                for state in &sharing {
                    row = row.child(self.screen_tile(
                        key,
                        server,
                        state,
                        state.user_id == me,
                        screen_w,
                        screen_h,
                        &p,
                        window,
                        cx,
                    ));
                }
                motion::rise(row, "stage-screens", Duration::ZERO, -12.0)
            });
            if !sharing.is_empty() {
                let screen_rows = sharing.len().div_ceil(screen_cols);
                content_h += screen_rows as f32 * (screen_h + GAP);
            }
            let top = ((h - PAD * 2.0 - content_h) / 2.0).max(0.0);
            let mut grid = div().w(px(width)).mx_auto().flex().flex_wrap().gap(px(GAP));
            for (n, state) in states.iter().enumerate() {
                let video = joined && if state.user_id == me { mine.0 } else { state.self_video };
                grid = grid.child(self.tile(
                    key,
                    server,
                    &channel.id,
                    state,
                    n,
                    tile_w,
                    tile_h,
                    joined,
                    video,
                    &p,
                    window,
                    cx,
                ));
            }
            div().mt(px(top)).children(screens).child(grid).into_any_element()
        };
        let measure = {
            let cell = self.calls.stage_box.clone();
            gpui_kit::canvas(
                move |bounds, window, _| {
                    let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    if cell.get() != size {
                        cell.set(size);
                        window.refresh();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        let area = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(measure)
            .child(div().id("stage-scroll").size_full().overflow_y_scroll().p(px(PAD)).child(body));

        // ── The controls ──
        let status = call.as_ref().filter(|c| c.status != Status::Connected).map(|c| {
            t(if c.status == Status::Reconnecting {
                "dms-calls.calls.status.reconnecting"
            } else {
                "dms-calls.calls.status.connecting"
            })
        });
        let controls: AnyElement = if let Some(call) = &call {
            let (mute, deaf) = (call.self_mute || call.self_deaf, call.self_deaf);
            let row = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(self.mute_buttons("stage", Size::Lg, cx))
                .child(self.camera_button("stage", Size::Lg, false, cx))
                .child(self.screen_button("stage", Size::Lg, false, window, cx))
                .when_some(self.record_button("stage", Size::Lg, false, window, cx), |el, b| el.child(b))
                .child(hang_up_button("stage-leave", Size::Lg, t("dms-calls.calls.controls.disconnect"), &p).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.core.leave_voice();
                        cx.notify();
                    }),
                ));
            let _ = (mute, deaf);
            motion::rise(row, "stage-in", Duration::ZERO, 12.0).into_any_element()
        } else {
            let hover_shadow = alpha(p.primary, 1.0);
            let (k, s, c) = (key.to_owned(), server.to_owned(), channel.id.clone());
            let join = div()
                .id("stage-join")
                .group("stage-join")
                .h(px(48.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_2xl())
                .bg(p.primary)
                .text_color(p.primary_foreground)
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .when(!can_connect, |el| el.opacity(0.5))
                .when(can_connect, |el| {
                    el.cursor_pointer()
                        // The web's `.btn`: it lifts with a glow, and presses in.
                        .hover(move |st| {
                            st.translate_y(px(-2.0)).shadow(vec![BoxShadow {
                                color: hover_shadow,
                                offset: point(px(0.0), px(8.0)),
                                blur_radius: px(22.0),
                                spread_radius: px(-8.0),
                                inset: false,
                            }])
                        })
                        .active(|st| st.translate_y(px(0.0)).scale(0.94))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.core.join_voice(&k, &s, &c);
                            cx.notify();
                        }))
                })
                .child(
                    div()
                        .id("stage-join-icon")
                        .when(can_connect, |el| {
                            el.group_hover("stage-join", |st| {
                                st.rotate(gpui_kit::radians(-12f32.to_radians())).scale(1.1)
                            })
                        })
                        .child(icon("headphones").size(px(16.0))),
                )
                .child(t("dms-calls.calls.stage.join"));
            let note = if !can_connect {
                Some(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t("dms-calls.calls.stage.cantJoin")),
                )
            } else if !can_speak {
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(icon("mic-off").size(px(12.0)))
                        .child(t("dms-calls.calls.stage.listenOnly")),
                )
            } else {
                None
            };
            motion::rise(
                div().flex().flex_col().items_center().gap(px(6.0)).child(div().relative().child(join)).children(note),
                "stage-out",
                Duration::ZERO,
                12.0,
            )
            .into_any_element()
        };
        let footer = div()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .border_t_1()
            .border_color(p.border)
            // The web's `bg-card/60 backdrop-blur`.
            .bg(alpha(p.card, 0.6))
            .backdrop_blur(px(8.0))
            .px(px(16.0))
            .py(px(12.0))
            .child(controls)
            .when_some(status, |el, text| {
                el.child(motion::rise(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(text),
                    "stage-status",
                    Duration::ZERO,
                    -6.0,
                ))
            });

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(
                gpui_kit::img(glow(p.primary))
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .object_fit(gpui_kit::ObjectFit::Fill),
            )
            .child(header)
            .child(area)
            .child(footer)
            .into_any_element()
    }

    /// The web's `MuteButtons`: mute and deafen, in or out of a call.
    pub(crate) fn mute_buttons(&self, tag: &str, size: Size, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (mute, deaf) = self.core.selves();
        let muted = mute || deaf;
        let named = |action: &str, id: &str| {
            let combo = crate::core::keybinds::action_by_id(id)
                .and_then(|a| crate::core::keybinds::binding_of(a, &self.prefs.keybinds));
            match combo {
                Some(combo) => t_with(
                    "dms-calls.calls.controls.withShortcut",
                    &[("action", Arg::Str(&t(action))), ("keys", Arg::Str(&crate::core::keybinds::label(&combo)))],
                ),
                None => t(action),
            }
        };
        let mute_label = named(
            if muted { "dms-calls.calls.controls.unmute" } else { "dms-calls.calls.controls.mute" },
            "toggleMute",
        );
        let deaf_label = named(
            if deaf { "dms-calls.calls.controls.undeafen" } else { "dms-calls.calls.controls.deafen" },
            "toggleDeafen",
        );
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                toggle_icon(format!("{tag}-mute"), if muted { "mic-off" } else { "mic" }, muted, size, mute_label, &p)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.core.set_self_mute(!muted);
                        cx.notify();
                    })),
            )
            .child(
                toggle_icon(
                    format!("{tag}-deaf"),
                    if deaf { "headphone-off" } else { "headphones" },
                    deaf,
                    size,
                    deaf_label,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.core.set_self_deaf(!deaf);
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    /// One person's tile: their avatar on their color, glowing green while they talk.
    #[allow(clippy::too_many_arguments)]
    fn tile(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        state: &pb::VoiceState,
        n: usize,
        w: f32,
        h: f32,
        joined: bool,
        video: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (user, name, me) = self.core.shared.read(|s| {
            let i = s.instance(key);
            (
                i.and_then(|i| i.users.get(&state.user_id).cloned()),
                i.map(|i| i.display_name(Some(server), &state.user_id)).unwrap_or_default(),
                i.and_then(|i| i.me.as_ref().map(|m| m.id.clone())).unwrap_or_default(),
            )
        });
        let speaking =
            self.core.call().is_some_and(|c| c.in_channel(key, channel) && c.speaking.contains(&state.user_id));
        let tag = format!("tile|{server}|{}", state.user_id);
        let lit =
            motion::follow(SharedString::from(format!("{tag}|lit")), if speaking { 1.0 } else { 0.0 }, window, cx)
                .clamp(0.0, 1.0);
        let grow =
            motion::follow(SharedString::from(format!("{tag}|grow")), if speaking { 1.06 } else { 1.0 }, window, cx);
        let pop = CallPop::Person {
            key: key.to_owned(),
            server: Some(server.to_owned()),
            channel: Some(channel.to_owned()),
            user: state.user_id.clone(),
            from: format!("tile|{}", state.user_id),
        };
        let open = self.calls.pop.as_ref() == Some(&pop);
        let radius = radius_3xl();
        let hover_border = alpha(p.primary, 0.4);
        // The border turns green as they start talking (`transition-[border-color]`).
        let border = mix(p.border, green(), lit);
        let mut shadow = shadow_sm();
        if lit > 0.01 {
            shadow = vec![
                BoxShadow {
                    color: alpha(green(), lit),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(2.0),
                    inset: false,
                },
                BoxShadow {
                    color: gpui_kit::hsla(136.0 / 360.0, 0.47, 0.44, 0.6 * lit),
                    offset: point(px(0.0), px(10.0)),
                    blur_radius: px(40.0),
                    spread_radius: px(-10.0),
                    inset: false,
                },
            ];
        }
        let size = 80.0 * grow;
        // Their color brightens a little while the tile is hovered.
        let hue = hue_gradient(
            &state.user_id,
            f32::from(radius),
            div().absolute().inset_0().rounded(radius).overflow_hidden(),
        )
        .id("hue")
        .opacity(0.25)
        .group_hover(SharedString::from(tag.clone()), |s| s.opacity(0.35));
        let agent = is_agent(user.as_ref());
        let chip = div()
            .absolute()
            .left(px(8.0))
            .right(px(8.0))
            .bottom(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(radius_xl())
            // The web's `bg-background/75 backdrop-blur`.
            .bg(alpha(p.background, 0.75))
            .backdrop_blur(px(8.0))
            .px(px(10.0))
            .py(px(4.0))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .child(name),
            )
            .when(agent, |el| {
                el.child(app_badge(SharedString::from(format!("tile-agent|{}", state.user_id)), "AGENT", p))
            })
            .child(voice_flags(state, p));
        let feed = crate::core::voice::video::feed_of(&state.user_id, false);
        let camera =
            video.then(|| crate::ui::video::feed_view(&self.core, &feed, ObjectFit::Cover, radius, window, cx));
        let tile = div()
            .id(SharedString::from(tag.clone()))
            .group(SharedString::from(tag.clone()))
            .relative()
            .overflow_hidden()
            .w(px(w))
            .h(px(h))
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(p.card)
            .shadow(shadow)
            .when(lit < 0.5, |el| el.hover(move |s| s.border_color(hover_border)))
            .when(state.user_id != me, |el| {
                el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.toggle_call_pop(pop.clone(), cx)))
            })
            .child(hue)
            .child(div().absolute().inset_0().flex().items_center().justify_center().child(voice_avatar(
                user.as_ref(),
                &state.user_id,
                size,
                24.0 * grow,
                4.0,
                speaking,
                &tag,
                window,
                cx,
            )))
            .children(camera)
            .child(chip);
        let group = format!("tile-group|{}", state.user_id);
        let mut holder = div().relative().group(SharedString::from(group.clone())).child(tile);
        if joined {
            let name = self
                .core
                .shared
                .read(|s| s.instance(key).map(|i| i.display_name(Some(server), &state.user_id)).unwrap_or_default());
            let popped = Popped {
                instance: key.to_owned(),
                user: state.user_id.clone(),
                server: Some(server.to_owned()),
                screen: false,
            };
            holder = holder.child(
                pop_out_button(
                    SharedString::from(format!("pop|{}", state.user_id)),
                    t_with("dms-calls.calls.video.popOutTitle", &[("name", Arg::Str(&name))]),
                    &group,
                    p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    // Not the tile under it too.
                    cx.stop_propagation();
                    this.pop_out(popped.clone(), cx)
                })),
            );
        }
        if open {
            let card = self.person_card(key, Some(server), Some(channel), &state.user_id, window, cx);
            holder = holder.child(self.hang(card, Side::Right));
        }
        tile_in(holder, SharedString::from(format!("{tag}|in")), Duration::from_millis(40 * n.min(8) as u64))
            .into_any_element()
    }

    /// Someone's shared screen (the web's `ScreenTile`): whole, on black,
    /// with whose it is and that it's live.
    #[allow(clippy::too_many_arguments)]
    fn screen_tile(
        &mut self,
        key: &str,
        server: &str,
        state: &pb::VoiceState,
        mine: bool,
        w: f32,
        h: f32,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = self
            .core
            .shared
            .read(|s| s.instance(key).map(|i| i.display_name(Some(server), &state.user_id)).unwrap_or_default());
        let feed = crate::core::voice::video::feed_of(&state.user_id, true);
        let label = if mine {
            t("dms-calls.calls.screen.yours")
        } else {
            t_with("dms-calls.calls.screen.theirs", &[("name", Arg::Str(&name))])
        };
        let group = format!("screen-group|{}", state.user_id);
        let popped = Popped {
            instance: key.to_owned(),
            user: state.user_id.clone(),
            server: Some(server.to_owned()),
            screen: true,
        };
        let tile = div()
            .relative()
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(gpui_kit::black())
            .child(crate::ui::video::feed_view(&self.core, &feed, ObjectFit::Contain, radius_3xl(), window, cx))
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .bottom(px(8.0))
                    .max_w(relative(0.8))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.background, 0.8))
                    .px(px(10.0))
                    .py(px(4.0))
                    .child(live_badge(&state.user_id, window))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .child(label),
                    ),
            );
        let button = pop_out_button(
            SharedString::from(format!("pop-screen|{}", state.user_id)),
            t_with("dms-calls.calls.video.popOutScreenTitle", &[("name", Arg::Str(&name))]),
            &group,
            p,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            // Not the tile under it too.
            cx.stop_propagation();
            this.pop_out(popped.clone(), cx)
        }));
        let sound = crate::ui::video::screen_sound_button(&self.core, &state.user_id, mine, "stage", p);
        // The web's `scale: 0.94, y: -12`.
        motion::pop_in(
            div().relative().group(SharedString::from(group.clone())).child(tile).child(button).children(sound),
            SharedString::from(format!("screen-in|{}", state.user_id)),
            (0.5, 0.5),
            0.94,
            -12.0,
        )
        .into_any_element()
    }

    /// Who's recording the channel, for everyone to see, while anyone is.
    fn recording_pill(
        &self,
        key: &str,
        server: &str,
        states: &[pb::VoiceState],
        p: &Palette,
        window: &mut Window,
    ) -> Option<AnyElement> {
        let recording: Vec<&pb::VoiceState> = states.iter().filter(|v| v.self_record || v.server_record).collect();
        if recording.is_empty() {
            return None;
        }
        let on_server = states.iter().any(|v| v.server_record);
        let (names, video) = self.core.shared.read(|s| {
            let i = s.instance(key);
            let names: Vec<String> = recording
                .iter()
                .map(|v| i.map(|i| i.display_name(Some(server), &v.user_id)).unwrap_or_default())
                .collect();
            (names.join(", "), i.and_then(|i| i.server(server)).is_some_and(|s| s.record_video) && on_server)
        });
        let text = t_with(
            if video {
                "dms-calls.calls.stage.pillVideo"
            } else if on_server {
                "dms-calls.calls.stage.pillServer"
            } else {
                "dms-calls.calls.stage.pill"
            },
            &[("names", Arg::Str(&names))],
        );
        let _ = p;
        Some(
            // In from the right as it grows (`scale: 0.8, x: 8`).
            motion::pop_in(
                div()
                    .flex_none()
                    .max_w(px(420.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded_full()
                    .bg(alpha(red(), 0.12))
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(red())
                    .child(ping_dot("rec-pill-ping", 8.0, red(), alpha(red(), 0.6), window))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text)),
                "rec-pill",
                (1.0, 0.5),
                0.8,
                0.0,
            )
            .into_any_element(),
        )
    }
}

/// The web's `SPRING` (`stiffness: 520, damping: 34`), for tiles coming in.
const SPRING: SpringConfig = SpringConfig::new(520.0, 34.0, 1.0);

/// A person's tile coming in (the web's `Tile`): it fades in, growing from
/// 80% and rising 16 px, `delay` after the one before it.
fn tile_in(el: gpui_kit::Div, id: SharedString, delay: Duration) -> impl IntoElement {
    let (duration, easing) = sampled_easing(SPRING, 0.002);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32().max(0.001);
    let ease = move |t: f32| if t <= start { 0.0 } else { easing(((t - start) / (1.0 - start)).clamp(0.0, 1.0)) };
    el.with_animation(id, Animation::new(total).with_easing(ease), |el, t| {
        el.opacity(t.clamp(0.0, 1.0)).translate_y(px((1.0 - t) * 16.0)).scale(0.8 + 0.2 * t)
    })
}

/// Nobody's here yet: the speaker in a soft circle, waves going out from it.
fn empty(name: &str, p: &Palette, window: &mut Window) -> impl IntoElement {
    let wave = |n: u64, a: f32| {
        let ring = div().absolute().size(px(80.0)).rounded_full().border_2().border_color(alpha(p.primary, a));
        motion::ambient(
            ring,
            SharedString::from(format!("stage-wave|{n}")),
            Duration::from_secs(2),
            window,
            move |el, t| {
                let t = (t + n as f32 * 0.5).rem_euclid(1.0);
                // The web's cubic-bezier(0.2, 0.7, 0.3, 1): fast, then easing out.
                let k = 1.0 - (1.0 - t).powf(2.6);
                let s = 80.0 * (1.0 + 0.9 * k);
                el.size(px(s)).top(px((80.0 - s) / 2.0)).left(px((80.0 - s) / 2.0)).opacity(0.55 * (1.0 - k))
            },
        )
    };
    div().size_full().flex().items_center().justify_center().child(
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .relative()
                    .size(px(80.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(alpha(p.primary, 0.12))
                    .text_color(p.primary)
                    .child(wave(0, 0.4))
                    .child(wave(1, 0.3))
                    .child(icon("volume-2").size(px(36.0))),
            )
            .child(
                div()
                    .text_size(px(18.0))
                    .line_height(px(28.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(t_with("dms-calls.calls.stage.emptyTitle", &[("name", Arg::Str(name))])),
            )
            .child(
                div()
                    .max_w(px(320.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .text_center()
                    .text_color(p.muted_foreground)
                    .child(t("dms-calls.calls.stage.emptyText")),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::grid_for;

    #[test]
    fn grids_follow_the_web() {
        assert_eq!(grid_for(1), (1, 576.0));
        assert_eq!(grid_for(2), (2, 768.0));
        assert_eq!(grid_for(4), (2, 1024.0));
        assert_eq!(grid_for(7), (3, 1024.0));
        assert_eq!(grid_for(12), (4, 1024.0));
    }
}
