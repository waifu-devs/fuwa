//! What calls share across the window, the web's components/calls/parts.tsx,
//! Connection.tsx and the call buttons of Video.tsx: avatars that glow while
//! someone talks, the little flags after a name, mute, deafen, camera, record
//! and hang-up buttons in their two sizes, the signal bars, and the cards
//! that open from them (someone's volume and moderation, the connection's
//! details, where to record).

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::base::{Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, BoxShadow, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Rgba, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Window, div, point, px, relative,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::voice::quality::{Level, Quality, Route};
use crate::core::voice::{CallView, Status};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::motion;
use crate::ui::settings_controls::{shadow_sm, shadow_xl};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_lg, radius_md, radius_xl};
use crate::ui::widgets::{hue_gradient, icon, initials, pal};

/// Discord's call green, the web's #3ba55d.
pub(crate) fn green() -> Rgba {
    gpui_kit::rgb(0x3ba55d)
}

/// The recording red, the web's #ed4245.
pub(crate) fn red() -> Rgba {
    gpui_kit::rgb(0xed4245)
}

/// Tailwind's amber-500, for a connection that's coming back.
pub(crate) fn amber() -> Rgba {
    gpui_kit::rgb(0xf59e0b)
}

/// What calls keep in the window: the open voice channel, the open card,
/// calls turned down, and the recordings dialog.
#[derive(Default)]
pub(crate) struct CallsUi {
    /// The voice channel open in the main area, as (instance, server, channel).
    pub stage: Option<(String, String, String)>,
    /// The card open over the window, if any.
    pub pop: Option<CallPop>,
    /// Calls you turned down, by "instance/conversation/started at", so they stop ringing.
    pub declined: HashSet<String>,
    /// When the ring last played.
    pub rang_at: Option<Instant>,
    /// The voice stage's size, measured as it's drawn: its tiles fit it.
    pub stage_box: Rc<Cell<(f32, f32)>>,
    /// The volume slider of the person card open now.
    pub volume: Option<(String, Entity<SliderState>, bool, Subscription)>,
    pub recordings: Option<crate::ui::recordings::Recordings>,
    /// A clock that ticks once a second while a call runs, for the panel's timer.
    pub ticking: Option<gpui_kit::Task<()>>,
    /// Whether the call's microphone was missing last frame, so finding it
    /// missing mutes you once, as on the web.
    pub mic_missing: bool,
    /// Where the thing the open card hangs off was last drawn: its left and
    /// right edges and the window's width, to pick the card's side.
    pub hang_box: Rc<Cell<(f32, f32, f32)>>,
    /// Push to talk's key while it's held.
    pub ptt_key: Option<String>,
    /// The settings the call last heard (volumes, push to talk).
    pub applied: Option<Applied>,
    /// Cameras and screens popped out into windows of their own, by `Popped::key`.
    pub popouts: std::collections::HashMap<String, gpui_kit::AnyWindowHandle>,
    /// Whether a call was going on at the last change, to notice it ending.
    pub in_call: bool,
    /// The share dialog: what can be shared, their pictures, what's picked.
    pub picker: crate::ui::screen_share::SharePicker,
}

impl FuwaApp {
    /// Pops someone's camera (or screen) out into a window of its own, or
    /// brings the one already open to the front.
    pub(crate) fn pop_out(&mut self, popped: crate::ui::popout::Popped, cx: &mut Context<Self>) {
        let key = popped.key();
        if let Some(handle) = self.calls.popouts.get(&key) {
            if handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
                return;
            }
            self.calls.popouts.remove(&key);
        }
        let (w, h) = if popped.screen { (960.0, 540.0) } else { (640.0, 360.0) };
        let bounds = gpui_kit::Bounds::centered(None, gpui_kit::size(px(w), px(h)), cx);
        let options = gpui_kit::WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(bounds)),
            window_min_size: Some(gpui_kit::size(px(160.0), px(90.0))),
            titlebar: Some(gpui_kit::TitlebarOptions { title: Some("fuwa".into()), ..Default::default() }),
            app_id: Some("fuwa".into()),
            ..Default::default()
        };
        let core = self.core.clone();
        let opened = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| crate::ui::popout::PopOut::new(core, popped, window, cx))
        });
        if let Ok((handle, _)) = opened {
            self.calls.popouts.insert(key, handle);
        }
    }

    /// Hanging up closes every popped-out camera.
    pub(crate) fn close_pop_outs(&mut self, cx: &mut Context<Self>) {
        for (_, handle) in self.calls.popouts.drain() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}

/// What `CallsUi::applied` keeps: push to talk's mode, its delay, the input
/// and output volumes, and each person's volume.
pub(crate) type Applied = (u8, u16, u16, u16, std::collections::BTreeMap<String, u16>);

/// A card that hangs off something in a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CallPop {
    /// Someone's volume, and moderation in a voice channel; `from` names what it opened from.
    Person { key: String, server: Option<String>, channel: Option<String>, user: String, from: String },
    /// The connection's ping, loss, jitter and route.
    Connection,
    /// Recording on this computer or on the server; `from` names the button.
    Record { from: String },
}

/// The two sizes call buttons come in: the call panel's and the stage's.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Size {
    Sm,
    Lg,
}

impl Size {
    fn icon(self) -> f32 {
        if self == Size::Sm { 18.0 } else { 20.0 }
    }
}

/// Someone's avatar in a call, on their color or their picture, with
/// `text` sized initials (the web's `UserAvatar` with a size class).
pub(crate) fn person_avatar(user: Option<&pb::User>, id: &str, size: f32, text: f32) -> Div {
    let name = user.map(crate::core::store::user_name).unwrap_or_else(|| "?".into());
    let url = user.map(|u| u.avatar_url.clone()).unwrap_or_default();
    let letter: String = initials(&name).chars().take(1).collect();
    let id = user.map(|u| u.id.clone()).unwrap_or_else(|| id.to_owned());
    let fallback = move || {
        hue_gradient(&id, size / 2.0, div().relative().size_full().rounded_full().overflow_hidden())
            .flex()
            .items_center()
            .justify_center()
            .text_color(gpui_kit::white())
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(text))
            .child(div().relative().child(letter.clone()))
            .into_any_element()
    };
    let base = div().size(px(size)).flex_none().rounded_full().overflow_hidden();
    if url.is_empty() {
        base.child(fallback())
    } else {
        let again = fallback.clone();
        base.child(
            gpui_kit::img(SharedString::from(url))
                .size_full()
                .rounded_full()
                .object_fit(gpui_kit::ObjectFit::Cover)
                .with_fallback(fallback)
                .with_loading(again),
        )
    }
}

/// The web's `.speaking-ring` glow: a green ring whose glow breathes.
fn ring_glow(spread: f32, blur: f32, a: f32) -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: gpui_kit::hsla(136.0 / 360.0, 0.47, 0.44, 0.18 * a * spread / 4.0 + 0.5 * a * (1.0 - spread / 4.0)),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(spread),
            inset: false,
        },
        BoxShadow {
            color: gpui_kit::hsla(136.0 / 360.0, 0.47, 0.44, (0.45 + 0.1 * spread / 4.0) * a),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(blur),
            spread_radius: px(0.0),
            inset: false,
        },
    ]
}

/// The web's `VoiceAvatar`: the avatar, and a soft ring `ring` thick that
/// springs in while they talk and breathes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn voice_avatar(
    user: Option<&pb::User>,
    id: &str,
    size: f32,
    text: f32,
    ring: f32,
    speaking: bool,
    tag: &str,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> AnyElement {
    let on = motion::follow(SharedString::from(format!("speak|{tag}")), if speaking { 1.0 } else { 0.0 }, window, cx)
        .clamp(0.0, 1.0);
    let mut el = div().relative().flex_none().size(px(size));
    if on > 0.01 {
        let scale = 0.85 + 0.15 * on;
        let grow = (1.0 - scale) * (size + ring * 2.0) / 2.0;
        let around = |el: Div| {
            el.absolute()
                .top(px(-ring + grow))
                .left(px(-ring + grow))
                .size(px(size + ring * 2.0 - grow * 2.0))
                .rounded_full()
        };
        // The glow goes behind the avatar: a CSS box-shadow is only drawn
        // outside its box, while GPUI fills the box too.
        let glow = motion::ambient(
            around(div()),
            SharedString::from(format!("speak-glow|{tag}")),
            Duration::from_millis(1100),
            window,
            move |el, t| {
                let k = (t * std::f32::consts::TAU).sin().abs();
                el.shadow(ring_glow(4.0 * k, 14.0 + 8.0 * k, on))
            },
        );
        el = el
            .child(glow)
            .child(person_avatar(user, id, size, text))
            .child(around(div()).border(px(ring)).border_color(alpha(green(), on)));
    } else {
        el = el.child(person_avatar(user, id, size, text));
    }
    el.into_any_element()
}

/// The web's `VoiceFlags`: camera on, muted, deafened, recording; the ones
/// a moderator set in red. Each pops in with a little turn.
pub(crate) fn voice_flags(state: &pb::VoiceState, p: &Palette) -> Div {
    let mut flags: Vec<(&'static str, &'static str, bool)> = Vec::new();
    if state.server_record {
        flags.push(("server", "dms-calls.calls.flags.serverRecord", true));
    }
    if state.self_record {
        flags.push(("circle-dot", "dms-calls.calls.flags.record", true));
    }
    if state.self_stream {
        flags.push(("monitor-up", "dms-calls.calls.flags.screen", false));
    }
    if state.self_video {
        flags.push(("video", "dms-calls.calls.flags.video", false));
    }
    if state.server_video_off {
        flags.push(("video-off", "dms-calls.calls.flags.serverVideoOff", true));
    }
    if state.server_mute {
        flags.push(("mic-off", "dms-calls.calls.flags.serverMute", true));
    } else if state.suppress {
        flags.push(("mic-off", "dms-calls.calls.flags.suppress", true));
    } else if state.self_mute {
        flags.push(("mic-off", "dms-calls.calls.flags.mute", false));
    }
    if state.server_deaf {
        flags.push(("headphone-off", "dms-calls.calls.flags.serverDeaf", true));
    } else if state.self_deaf {
        flags.push(("headphone-off", "dms-calls.calls.flags.deaf", false));
    }
    let mut row = div().ml_auto().flex_none().flex().items_center().gap(px(2.0));
    for (glyph, label, by_mod) in flags {
        let id = SharedString::from(format!("flag|{}|{glyph}|{label}", state.user_id));
        let cell = div()
            .id(id.clone())
            .size(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .text_color(if by_mod { p.destructive } else { p.muted_foreground })
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(t(label)).build(window, cx))
            .child(icon(glyph).size(px(14.0)));
        row = row.child(motion::once(cell, id, Duration::from_millis(260), |el, t| el.opacity(t)));
    }
    row
}

/// The web's `ToggleIcon`: mute or deafen, red while on.
pub(crate) fn toggle_icon(
    id: impl Into<SharedString>,
    glyph: &str,
    on: bool,
    size: Size,
    label: String,
    p: &Palette,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let (bg, fg, hover_bg, hover_fg) = match (on, size) {
        (true, _) => {
            (Some(alpha(p.destructive, 0.12)), p.destructive.into(), alpha(p.destructive, 0.2), p.destructive.into())
        }
        (false, Size::Sm) => (None, p.muted_foreground.into(), p.muted.into(), p.foreground.into()),
        (false, Size::Lg) => (Some(p.muted.into()), p.foreground.into(), alpha(p.muted, 0.7), p.foreground.into()),
    };
    call_button_frame(id, size, bg, fg, hover_bg, hover_fg, label).child(icon(glyph).size(px(size.icon())))
}

/// A call button's frame: `size-8 rounded-lg` or `size-12 rounded-2xl`, pressing in.
fn call_button_frame(
    id: SharedString,
    size: Size,
    bg: Option<gpui_kit::Hsla>,
    fg: gpui_kit::Hsla,
    hover_bg: gpui_kit::Hsla,
    hover_fg: gpui_kit::Hsla,
    label: String,
) -> Stateful<Div> {
    let side = if size == Size::Sm { 32.0 } else { 48.0 };
    div()
        .id(id)
        .relative()
        .h(px(side))
        .min_w(px(side))
        .flex_none()
        .rounded(if size == Size::Sm { radius_lg() } else { radius_2xl() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(fg)
        .when_some(bg, |el, bg| el.bg(bg))
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .active(|s| s.opacity(0.85))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
}

/// The web's `HangUpButton`: big and red, its phone turning on hover.
pub(crate) fn hang_up_button(id: impl Into<SharedString>, size: Size, label: String, p: &Palette) -> Stateful<Div> {
    let (w, h) = if size == Size::Sm { (36.0, 36.0) } else { (64.0, 48.0) };
    let _ = p;
    div()
        .id(id.into())
        .w(px(w))
        .h(px(h))
        .flex_none()
        .rounded(if size == Size::Sm { radius_xl() } else { radius_2xl() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(p.destructive)
        .text_color(gpui_kit::white())
        .shadow(shadow_sm())
        .hover(|s| s.opacity(0.92))
        .active(|s| s.opacity(0.8))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
        .child(icon("phone-off").size(px(size.icon())))
}

/// Whether you may turn your camera on or share your screen in the call
/// you're in: VIDEO in a voice channel, always in a conversation.
pub(crate) fn may_film(app: &FuwaApp, call: &CallView) -> bool {
    if call.is_dm() {
        return true;
    }
    app.core.shared.read(|s| {
        s.instance(&call.instance)
            .is_some_and(|i| i.access(&call.server_id).has_in(&call.channel_id, pb::Permission::Video))
    })
}

/// A camera or screen button's colors: `on` in its own color.
fn film_colors(on: bool, on_bg: Rgba, on_fg: gpui_kit::Hsla, size: Size, p: &Palette) -> FrameColors {
    match (on, size) {
        (true, _) => (Some(on_bg.into()), on_fg, alpha(on_bg, 0.9), on_fg),
        (false, Size::Sm) => (None, p.muted_foreground.into(), p.muted.into(), p.foreground.into()),
        (false, Size::Lg) => (Some(p.muted.into()), p.foreground.into(), alpha(p.muted, 0.7), p.foreground.into()),
    }
}

type FrameColors = (Option<gpui_kit::Hsla>, gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla);

impl FuwaApp {
    /// The web's `CameraButton`: your camera on (green) or off, greyed where
    /// you may not turn it on.
    pub(crate) fn camera_button(&self, tag: &str, size: Size, grow: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some(call) = self.core.call() else { return div().into_any_element() };
        let on = call.self_video;
        let may = may_film(self, &call);
        let label = t(if !may {
            "dms-calls.calls.video.cameraNotAllowed"
        } else if on {
            "dms-calls.calls.video.cameraOff"
        } else {
            "dms-calls.calls.video.cameraOn"
        });
        let (bg, fg, hover_bg, hover_fg) = film_colors(on, green(), gpui_kit::white(), size, &p);
        let glyph = icon(if on { "video" } else { "video-off" }).size(px(size.icon()));
        let glyph = motion::once(
            div().child(glyph),
            SharedString::from(format!("camera-glyph|{tag}|{on}")),
            Duration::from_millis(260),
            |el, t| el.opacity(t),
        );
        call_button_frame(SharedString::from(format!("camera|{tag}")), size, bg, fg, hover_bg, hover_fg, label)
            .when(grow, |el| el.flex_1())
            .when(!may, |el| el.opacity(0.4).cursor_default())
            .when(may, |el| {
                el.on_click(cx.listener(move |this, _, _, cx| {
                    this.core.set_camera(!on);
                    cx.notify();
                }))
            })
            .child(glyph)
            .into_any_element()
    }

    /// The web's `ScreenButton`: starts sharing through the share dialog
    /// (`ui/screen_share.rs`), or stops.
    pub(crate) fn screen_button(
        &mut self,
        tag: &str,
        size: Size,
        grow: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let Some(call) = self.core.call() else { return div().into_any_element() };
        let on = call.self_stream;
        let may = may_film(self, &call);
        let label = t(if !may {
            "dms-calls.calls.video.screenNotAllowed"
        } else if on {
            "dms-calls.calls.video.screenStop"
        } else {
            "dms-calls.calls.video.screenShare"
        });
        let (bg, fg, hover_bg, hover_fg) = film_colors(on, p.primary, p.primary_foreground.into(), size, &p);
        let button =
            call_button_frame(SharedString::from(format!("screen|{tag}")), size, bg, fg, hover_bg, hover_fg, label)
                .when(grow, |el| el.flex_1())
                .when(!may, |el| el.opacity(0.4).cursor_default())
                .when(may, |el| {
                    el.on_click(cx.listener(move |this, _, window, cx| {
                        if on {
                            this.core.set_screen(false, None);
                        } else {
                            this.open_share_picker(window, cx);
                        }
                        cx.notify();
                    }))
                })
                .child(icon(if on { "monitor-x" } else { "monitor-up" }).size(px(size.icon())));
        let holder = div().relative().flex().when(grow, |el| el.flex_1()).child(button);
        holder.into_any_element()
    }
}

/// Whether you may record in the call you're in: RECORD in a voice channel, always in a conversation.
pub(crate) fn may_record(app: &FuwaApp, call: &CallView) -> bool {
    if call.is_dm() {
        return true;
    }
    app.core.shared.read(|s| {
        s.instance(&call.instance)
            .is_some_and(|i| i.access(&call.server_id).has_in(&call.channel_id, pb::Permission::Record))
    })
}

/// The web's `SignalBars`: three bars, pulsing while it connects, then
/// how good it is (three green, two amber, one red).
pub(crate) fn signal(status: &Status, quality: &Quality, p: &Palette, tag: &str, window: &mut Window) -> AnyElement {
    let ok = *status == Status::Connected;
    let level = quality.level;
    let lit = match (ok, level) {
        (true, Some(Level::Okay)) => 2,
        (true, Some(Level::Poor)) => 1,
        _ => 3,
    };
    let color: Rgba = match (status, level) {
        (Status::Connected, Some(Level::Okay)) => gpui_kit::rgb(0xf0b232),
        (Status::Connected, Some(Level::Poor)) => red(),
        (Status::Connected, _) => green(),
        (Status::Reconnecting, _) => amber(),
        (Status::Connecting, _) => p.muted_foreground,
    };
    let mut bars = div().flex_none().h(px(14.0)).flex().items_end().gap(px(2.0));
    for (n, h) in [5.0f32, 9.0, 13.0].into_iter().enumerate() {
        let bar = div().w(px(3.0)).h(px(h)).rounded_full().bg(color);
        let bar = if ok {
            bar.opacity(if n < lit { 1.0 } else { 0.25 }).into_any_element()
        } else {
            // Fill one by one while it's working on it.
            let delay = n as f32 * 0.125;
            motion::ambient(
                bar,
                SharedString::from(format!("bars|{tag}|{n}")),
                Duration::from_millis(1200),
                window,
                move |el, t| {
                    let x = (t - delay).rem_euclid(1.0);
                    let k = if x < 0.3 {
                        x / 0.3
                    } else if x < 0.6 {
                        1.0
                    } else {
                        1.0 - (x - 0.6) / 0.4
                    };
                    el.opacity(0.25 + 0.75 * k)
                },
            )
        };
        bars = bars.child(bar);
    }
    bars.into_any_element()
}

/// The ping's color beside "Voice connected": muted while it's good.
pub(crate) fn ping_color(quality: &Quality, p: &Palette) -> Rgba {
    match quality.level {
        Some(Level::Okay) => gpui_kit::rgb(0xf0b232),
        Some(Level::Poor) => red(),
        _ => p.muted_foreground,
    }
}

/// A card's frame: `rounded-2xl border bg-popover p-3 shadow-xl`, `w` wide.
pub(crate) fn pop_card(w: f32, p: &Palette) -> Div {
    div()
        .w(px(w))
        .rounded(radius_2xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .text_color(p.foreground)
        // Nothing of what it hangs off (a bold, colored button) carries into it.
        .font_weight(FontWeight::NORMAL)
        .text_size(px(16.0))
        .line_height(px(24.0))
        .cursor_default()
        .p(px(12.0))
        .shadow(shadow_xl())
        .occlude()
}

impl FuwaApp {
    /// Push to talk's key went down: talking until it comes up.
    pub(crate) fn push_to_talk(&mut self, key: &gpui_kit::Keystroke, cx: &mut Context<Self>) {
        if self.prefs.input_mode != crate::core::config::InputMode::Ptt {
            return;
        }
        self.calls.ptt_key = Some(key.key.clone());
        self.core.set_pushing(true);
        cx.notify();
    }

    /// Any key coming up: push to talk's key (or a modifier of it) lets go.
    pub(crate) fn on_key_up(&mut self, ev: &gpui_kit::KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(held) = self.calls.ptt_key.as_ref() else { return };
        let k = ev.keystroke.key.as_str();
        if k == held || matches!(k, "shift" | "control" | "alt" | "platform" | "function") {
            self.calls.ptt_key = None;
            self.core.set_pushing(false);
            cx.notify();
        }
    }

    /// Hands the call the settings it plays by, when they changed.
    pub(crate) fn apply_call_settings(&mut self) {
        let now = (
            self.prefs.input_mode as u8,
            self.prefs.ptt_release,
            self.prefs.input_volume,
            self.prefs.output_volume,
            self.prefs.user_volumes.clone(),
        );
        if self.calls.applied.as_ref() != Some(&now) {
            self.calls.applied = Some(now);
            self.core.apply_volumes();
        }
    }

    /// Opens or closes a call's card.
    pub(crate) fn toggle_call_pop(&mut self, pop: CallPop, cx: &mut Context<Self>) {
        self.calls.pop = if self.calls.pop.as_ref() == Some(&pop) { None } else { Some(pop) };
        cx.notify();
    }

    /// The clear layer behind an open card that closes it on any click elsewhere.
    pub(crate) fn render_call_pop_layer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.calls.pop.as_ref()?;
        Some(
            div()
                .id("call-pop-away")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.calls.pop = None;
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.calls.pop = None;
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    /// Plays a call's cues and says what the call wants said, each once.
    pub(crate) fn call_notices(&mut self, cx: &mut Context<Self>) {
        for text in self.core.take_call_notices() {
            self.toast("phone", text, String::new(), None, None, cx);
        }
    }

    /// A card hanging off whatever drew it, `side` of it: drawn over
    /// everything, kept inside the window.
    pub(crate) fn hang(&self, card: AnyElement, side: Side) -> AnyElement {
        // A card to the right goes to the left instead when the window has no
        // room for it there, as Radix flips the web's popovers; the side is
        // decided from where what it hangs off was last drawn.
        let (left_x, right_x, width) = self.calls.hang_box.get();
        let flip = side == Side::Right && right_x + 8.0 + 256.0 + 12.0 > width && left_x - 8.0 - 256.0 >= 12.0;
        let holder = match side {
            Side::Right if flip => div().absolute().top_0().right(relative(1.0)).pr(px(8.0)),
            Side::Right => div().absolute().top_0().left(relative(1.0)).pl(px(8.0)),
            Side::Above => div().absolute().top_0().left_0(),
            Side::AboveCenter => div().absolute().top_0().left(relative(0.5)),
        };
        let anchored = gpui_kit::anchored().snap_to_window_with_margin(gpui_kit::Edges::all(px(12.0)));
        let anchored = match side {
            Side::Right if flip => anchored.anchor(gpui_kit::Anchor::TopRight),
            Side::Right => anchored,
            Side::Above | Side::AboveCenter => anchored.anchor(gpui_kit::Anchor::BottomLeft),
        };
        let card = match side {
            Side::Right => div().child(card),
            Side::Above => div().pb(px(10.0)).child(card),
            Side::AboveCenter => div().pb(px(8.0)).relative().left(px(-144.0)).child(card),
        };
        let holder = holder.child(gpui_kit::deferred(anchored.child(card)).with_priority(2));
        if side != Side::Right {
            return holder.into_any_element();
        }
        let cell = self.calls.hang_box.clone();
        let measure = gpui_kit::canvas(
            move |bounds, window, _| {
                let spot = (
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.x + bounds.size.width),
                    f32::from(window.viewport_size().width),
                );
                if cell.get() != spot {
                    cell.set(spot);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div().absolute().inset_0().child(measure).child(holder).into_any_element()
    }

    /// The web's `ParticipantMenu` card: how loud someone is for you, and
    /// with Mute or Move members there, what you can do about them for everyone.
    pub(crate) fn person_card(
        &mut self,
        key: &str,
        server: Option<&str>,
        channel: Option<&str>,
        user_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let (user, name, state, can_mute, can_move) = self.core.shared.read(|s| {
            let i = s.instance(key);
            let user = i.and_then(|i| i.users.get(user_id).cloned());
            let name = i.map(|i| i.display_name(server, user_id)).unwrap_or_default();
            let state = server
                .and_then(|server| i.and_then(|i| i.voice.get(server)))
                .and_then(|l| l.iter().find(|v| v.user_id == user_id).cloned());
            let (mut can_mute, mut can_move) = (false, false);
            if let (Some(i), Some(server), Some(channel)) = (i, server, channel) {
                let mine = i.access(server);
                let ranked = crate::core::moderation::outranks(&mine, &i.standing(server, user_id));
                can_mute = ranked && mine.has_in(channel, pb::Permission::MuteMembers);
                can_move = ranked && mine.has_in(channel, pb::Permission::MoveMembers);
            }
            (user, name, state, can_mute, can_move)
        });
        let volume_key = format!("{key}/{user_id}");
        let volume = f32::from(self.prefs.user_volumes.get(&volume_key).copied().unwrap_or(100));
        let head = div()
            .mb(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(person_avatar(user.as_ref(), user_id, 32.0, 14.0))
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
            );
        let silent = volume < 1.0;
        let mute_label =
            t(if silent { "dms-calls.calls.participant.unmuteForMe" } else { "dms-calls.calls.participant.muteForMe" });
        let speaker = {
            let volume_key = volume_key.clone();
            let fg = p.foreground;
            div()
                .id("person-silence")
                .size(px(32.0))
                .flex_none()
                .rounded(radius_lg())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(p.muted_foreground)
                .hover(move |s| s.bg(p.muted).text_color(fg))
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(mute_label.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let to = if silent { 100.0 } else { 0.0 };
                    this.set_person_volume(&volume_key, to, cx);
                }))
                .child(if silent {
                    icon("volume-x").size(px(16.0)).text_color(p.destructive)
                } else {
                    icon("volume-2").size(px(16.0))
                })
        };
        let slider = self.volume_slider(&volume_key, volume, &p, window, cx);
        let row = div().flex().items_center().gap(px(8.0)).child(speaker).child(div().flex_1().child(slider));
        let mut card = pop_card(256.0, &p).child(head).child(row);
        if let (true, Some(state), Some(server)) = (can_mute || can_move, state, server) {
            let item = |id: &'static str, glyph: &'static str, text: String, red_text: bool| {
                let hover = p.muted;
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_lg())
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .when(red_text, |el| el.text_color(p.destructive))
                    .hover(move |s| s.bg(hover))
                    .active(|s| s.opacity(0.85))
                    .child(icon(glyph).size(px(16.0)))
                    .child(text)
            };
            let moderate = |change: pb::ModerateVoiceRequest| {
                let (key, server, user) = (key.to_owned(), server.to_owned(), user_id.to_owned());
                cx.listener(move |this, _, _, cx| {
                    let request =
                        pb::ModerateVoiceRequest { server_id: server.clone(), user_id: user.clone(), ..change.clone() };
                    let disconnect = request.disconnect;
                    if disconnect {
                        this.calls.pop = None;
                    }
                    let core = this.core.clone();
                    let key = key.clone();
                    let task = this.core.spawn(async move { core.moderate_voice(&key, request).await });
                    cx.spawn(async move |this, cx| {
                        if let Ok(Err(problem)) = task.await {
                            let _ = this.update(cx, |this, cx| {
                                this.toast("triangle-alert", problem.message, String::new(), None, None, cx)
                            });
                        }
                    })
                    .detach();
                    cx.notify();
                })
            };
            let mut list =
                div().mt(px(12.0)).pt(px(8.0)).border_t_1().border_color(p.border).flex().flex_col().gap(px(4.0));
            if can_mute {
                let text = t(if state.server_mute {
                    "dms-calls.calls.participant.unmuteAll"
                } else {
                    "dms-calls.calls.participant.muteAll"
                });
                list = list.child(item("mod-mute", "mic-off", text, state.server_mute).on_click(moderate(
                    pb::ModerateVoiceRequest { server_mute: Some(!state.server_mute), ..Default::default() },
                )));
                let text = t(if state.server_deaf {
                    "dms-calls.calls.participant.undeafenAll"
                } else {
                    "dms-calls.calls.participant.deafenAll"
                });
                list = list.child(item("mod-deaf", "headphone-off", text, state.server_deaf).on_click(moderate(
                    pb::ModerateVoiceRequest { server_deaf: Some(!state.server_deaf), ..Default::default() },
                )));
                let text = t(if state.server_video_off {
                    "dms-calls.calls.participant.allowVideo"
                } else if state.self_stream && !state.self_video {
                    "dms-calls.calls.participant.stopScreen"
                } else if state.self_video && !state.self_stream {
                    "dms-calls.calls.participant.stopCamera"
                } else {
                    "dms-calls.calls.participant.stopVideo"
                });
                list = list.child(item("mod-video", "video-off", text, state.server_video_off).on_click(moderate(
                    pb::ModerateVoiceRequest { server_video_off: Some(!state.server_video_off), ..Default::default() },
                )));
            }
            if can_move {
                list = list.child(
                    item("mod-out", "shield-off", t("dms-calls.calls.participant.disconnect"), true)
                        .on_click(moderate(pb::ModerateVoiceRequest { disconnect: true, ..Default::default() })),
                );
            }
            card = card.child(list);
        }
        motion::rise(card, SharedString::from(format!("person-card|{user_id}")), Duration::ZERO, 6.0).into_any_element()
    }

    /// Sets how loud someone is for you (100 is as they sound), and tells the call.
    pub(crate) fn set_person_volume(&mut self, key: &str, volume: f32, cx: &mut Context<Self>) {
        let v = volume.round().clamp(0.0, 200.0) as u16;
        let key = key.to_owned();
        self.core.set_prefs(|pr| {
            if v == 100 {
                pr.user_volumes.remove(&key);
            } else {
                pr.user_volumes.insert(key.clone(), v);
            }
        });
        self.prefs = self.core.prefs();
        self.core.apply_volumes();
        cx.notify();
    }

    /// The web's `Slider` for someone's volume: 0 to 200%, in steps of 5,
    /// with a mark at 100%.
    fn volume_slider(
        &mut self,
        key: &str,
        value: f32,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.calls.volume.as_ref().is_none_or(|(k, ..)| k != key) {
            let state = cx.new(|_| SliderState::new().min(0.0).max(200.0).step(5.0).default_value(value));
            let k = key.to_owned();
            let sub = cx.subscribe_in(&state, window, move |this, _, event: &SliderEvent, _, cx| match event {
                SliderEvent::Change(v) => {
                    if let Some(s) = this.calls.volume.as_mut() {
                        s.2 = true;
                    }
                    this.set_person_volume(&k, v.end(), cx);
                }
                SliderEvent::Release(v) => {
                    if let Some(s) = this.calls.volume.as_mut() {
                        s.2 = false;
                    }
                    this.set_person_volume(&k, v.end(), cx);
                }
            });
            self.calls.volume = Some((key.to_owned(), state, false, sub));
        }
        let Some((_, state, held, _)) = self.calls.volume.as_ref() else { return div().into_any_element() };
        let (state, held) = (state.clone(), *held);
        let shown = state.read(cx).value().end();
        if !held && (shown - value).abs() > 2.5 {
            state.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        let now = if held { shown } else { value };
        let frac = (now / 200.0).clamp(0.0, 1.0);
        let thumb = SliderThumb::new(&state)
            .absolute()
            .top(px(-6.0))
            .left(relative(frac))
            .ml(px(-10.0))
            .size(px(20.0))
            .rounded_full()
            .border(px(3.0))
            .border_color(p.primary)
            .bg(p.background)
            .shadow(shadow_sm())
            .cursor_pointer();
        let bubble = held.then(|| {
            div().absolute().bottom(px(20.0)).left(relative(frac)).child(
                div().relative().left(px(-40.0)).w(px(80.0)).flex().justify_center().child(
                    div()
                        .rounded(radius_lg())
                        .bg(p.primary)
                        .px(px(8.0))
                        .py(px(2.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(p.primary_foreground)
                        .child(format!("{}%", now.round() as i32)),
                ),
            )
        });
        let track =
            SliderTrack::new(&state).relative().w_full().h(px(20.0)).flex().items_center().cursor_pointer().child(
                SliderIndicator::new(&state)
                    .relative()
                    .w_full()
                    .h(px(8.0))
                    .rounded_full()
                    .bg(p.muted)
                    .child(div().absolute().top_0().bottom_0().left_0().w(relative(frac)).rounded_full().bg(p.primary))
                    .child(thumb),
            );
        let on = (now - 100.0).abs() < 2.5;
        let key = key.to_owned();
        let fg = p.foreground;
        let mark = div()
            .relative()
            .mt(px(6.0))
            .h(px(16.0))
            .text_size(px(11.2))
            .line_height(px(16.8))
            .text_color(p.muted_foreground)
            .child(
                div().absolute().top_0().left(relative(0.5)).child(
                    div()
                        .id("volume-mark")
                        .relative()
                        .left(px(-30.0))
                        .w(px(60.0))
                        .flex()
                        .justify_center()
                        .cursor_pointer()
                        .when(on, |el| el.font_weight(FontWeight::BOLD).text_color(p.primary))
                        .when(!on, |el| el.hover(move |s| s.text_color(fg)))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_person_volume(&key, 100.0, cx)))
                        .child("100%"),
                ),
            );
        div()
            .pt(px(28.0))
            .pb(px(4.0))
            .child(BaseSlider::new(&state).relative().w_full().child(track).children(bubble))
            .child(mark)
            .into_any_element()
    }

    /// The web's `ConnectionDetails` card: your ping over the last minute,
    /// packet loss, jitter and how you're connected.
    pub(crate) fn connection_card(&self, call: &CallView, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let q = &call.quality;
        let heading = match call.status {
            Status::Reconnecting => t("dms-calls.calls.status.reconnecting"),
            Status::Connecting => t("dms-calls.calls.status.connecting"),
            Status::Connected => match q.level {
                Some(Level::Good) => t("dms-calls.calls.connection.good"),
                Some(Level::Okay) => t("dms-calls.calls.connection.okay"),
                Some(Level::Poor) => t("dms-calls.calls.connection.poor"),
                None => t("dms-calls.calls.connection.measuring"),
            },
        };
        let dash = q.ping.is_none();
        let loss = if dash {
            "–".to_owned()
        } else if q.loss < 0.1 {
            format!("{:.1}%", q.loss * 100.0)
        } else {
            format!("{:.0}%", q.loss * 100.0)
        };
        let jitter = if dash { "–".to_owned() } else { format!("{} ms", q.jitter) };
        let route = match q.route {
            Some(Route::Udp) => t("dms-calls.calls.connection.udp"),
            Some(Route::Tcp) => t("dms-calls.calls.connection.tcp"),
            None => "–".into(),
        };
        let row = |label: String, value: String| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .text_xs()
                .line_height(px(16.0))
                .child(div().text_color(p.muted_foreground).child(label))
                .child(div().font_weight(FontWeight::BOLD).child(value))
        };
        let color = match q.level {
            Some(Level::Okay) => gpui_kit::rgb(0xf0b232),
            Some(Level::Poor) => red(),
            _ => green(),
        };
        let card =
            pop_card(256.0, &p)
                .child(
                    div().flex().items_center().gap(px(8.0)).child(signal(&call.status, q, &p, "card", window)).child(
                        div().text_sm().line_height(px(20.0)).font_weight(FontWeight::EXTRA_BOLD).child(heading),
                    ),
                )
                .child(
                    div()
                        .mt(px(12.0))
                        .flex()
                        .items_end()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(24.0))
                                .line_height(px(32.0))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(q.ping_text()),
                        )
                        .child(
                            div()
                                // On the big number's baseline, as the web's `items-baseline`.
                                .mb(px(4.0))
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(t("dms-calls.calls.connection.ping")),
                        ),
                )
                .child(div().mt(px(4.0)).mb(px(12.0)).child(ping_graph(&q.history, color)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(row(t("dms-calls.calls.connection.loss"), loss))
                        .child(row(t("dms-calls.calls.connection.jitter"), jitter))
                        .child(row(t("dms-calls.calls.connection.route"), route)),
                )
                .child(
                    div()
                        .mt(px(12.0))
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(p.muted_foreground)
                        .child(t("dms-calls.calls.connection.about")),
                );
        motion::rise(card, "connection-card", Duration::ZERO, 8.0).into_any_element()
    }

    /// The web's `RecordMenu`: on this computer, or everyone's sound on the server.
    pub(crate) fn record_card(&self, call: &CallView, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let video = self.core.shared.read(|s| {
            s.instance(&call.instance).and_then(|i| i.server(&call.server_id)).is_some_and(|s| s.record_video)
        });
        let item = |id: &'static str, glyph: &'static str, title: String, text: String, on: bool| {
            let hover = alpha(p.primary, 0.1);
            div()
                .id(id)
                .flex()
                .items_start()
                .gap(px(10.0))
                .px(px(8.0))
                .py(px(8.0))
                .rounded(radius_md())
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(div().mt(px(2.0)).child(icon(glyph).size(px(16.0)).text_color(p.muted_foreground)))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(title))
                        .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(text)),
                )
                .when(on, |el| el.child(div().mt(px(6.0)).size(px(8.0)).flex_none().rounded_full().bg(red())))
        };
        let device = call.self_record;
        let server = call.server_record;
        let card = div()
            .w(px(288.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(4.0))
            .shadow(shadow_xl())
            .occlude()
            .child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.muted_foreground)
                    .child(t("dms-calls.calls.video.recordMenu")),
            )
            .child(
                item(
                    "record-device",
                    "laptop",
                    t(if device { "dms-calls.calls.video.deviceStop" } else { "dms-calls.calls.video.device" }),
                    t(if device { "dms-calls.calls.video.deviceStopText" } else { "dms-calls.calls.video.deviceText" }),
                    device,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.calls.pop = None;
                    this.core.set_recording(!device);
                    cx.notify();
                })),
            )
            .child(
                item(
                    "record-server",
                    "server",
                    t(if server { "dms-calls.calls.video.serverStop" } else { "dms-calls.calls.video.server" }),
                    t(if server {
                        "dms-calls.calls.video.serverStopText"
                    } else if video {
                        "dms-calls.calls.video.serverVideoText"
                    } else {
                        "dms-calls.calls.video.serverText"
                    }),
                    server,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.calls.pop = None;
                    this.core.set_server_recording(!server);
                    cx.notify();
                })),
            );
        motion::rise(card, "record-card", Duration::ZERO, 8.0).into_any_element()
    }

    /// The web's `RecordButton`: on this computer, or (in a voice channel
    /// that may record on the server) a menu of both. Hidden where you can't record.
    pub(crate) fn record_button(
        &self,
        from: &str,
        size: Size,
        grow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let call = self.core.call()?;
        let on = call.self_record || call.server_record;
        if !on && !may_record(self, &call) {
            return None;
        }
        let p = pal(cx);
        let both = !call.is_dm() && (call.server_recordings || call.server_record);
        let label = t(match (both, on) {
            (true, true) => "dms-calls.calls.video.recordingStop",
            (true, false) => "dms-calls.calls.video.recordChannel",
            (false, true) => "dms-calls.calls.video.recordStop",
            (false, false) => "dms-calls.calls.video.recordStart",
        });
        let (bg, fg, hover_bg, hover_fg) = match (on, size) {
            (true, _) => (Some(red().into()), gpui_kit::white(), alpha(red(), 0.9), gpui_kit::white()),
            (false, Size::Sm) => (None, p.muted_foreground.into(), p.muted.into(), p.foreground.into()),
            (false, Size::Lg) => (Some(p.muted.into()), p.foreground.into(), alpha(p.muted, 0.7), p.foreground.into()),
        };
        let pop = CallPop::Record { from: from.to_owned() };
        let open = self.calls.pop.as_ref() == Some(&pop);
        let mut button =
            call_button_frame(SharedString::from(format!("record|{from}")), size, bg, fg, hover_bg, hover_fg, label)
                .when(grow, |el| el.flex_1());
        if on {
            // A soft ping goes out from it while it records.
            let radius = if size == Size::Sm { radius_lg() } else { radius_2xl() };
            let wave = div().absolute().inset_0().rounded(radius).bg(alpha(red(), 0.4));
            button = button.child(motion::ambient(
                wave,
                SharedString::from(format!("record-ping|{from}")),
                Duration::from_secs(2),
                window,
                |el, t| {
                    let k = (t * 2.0).min(1.0);
                    el.opacity(1.0 - k)
                },
            ));
        }
        let button = button.child(div().relative().child(icon("circle-dot").size(px(size.icon()))));
        let button = if both {
            button.on_click(cx.listener(move |this, _, _, cx| this.toggle_call_pop(pop.clone(), cx)))
        } else {
            let on = call.self_record;
            button.on_click(cx.listener(move |this, _, _, cx| {
                this.core.set_recording(!on);
                cx.notify();
            }))
        };
        let mut holder = div().relative().flex().when(grow, |el| el.flex_1()).child(button);
        if open {
            holder = holder.child(self.hang(self.record_card(&call, cx), Side::AboveCenter));
        }
        Some(holder.into_any_element())
    }
}

/// Where a card hangs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// To the right, its top lined up (the web's `side="right" align="start"`).
    Right,
    /// Above, its left lined up.
    Above,
    /// Above, centered on a 288 px card.
    AboveCenter,
}

/// The last minute of pings as a line over a soft fill, scaled to the worst
/// of it (the web's `PingGraph`), 44 tall.
fn ping_graph(history: &[u32], color: Rgba) -> AnyElement {
    const H: f32 = 44.0;
    let history = history.to_vec();
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if history.len() < 2 {
                return;
            }
            let top = history.iter().copied().max().unwrap_or(0).max(100) as f32 * 1.15;
            let w = f32::from(bounds.size.width).max(1.0);
            let step = w / (history.len() - 1) as f32;
            let at = |i: usize, v: u32| {
                point(bounds.origin.x + px(i as f32 * step), bounds.origin.y + px(H - v as f32 / top * H))
            };
            // The fill: the area under the line, fading down from it.
            let mut area = gpui_kit::PathBuilder::fill();
            area.move_to(point(bounds.origin.x, bounds.origin.y + px(H)));
            for (i, v) in history.iter().enumerate() {
                area.line_to(at(i, *v));
            }
            area.line_to(point(bounds.origin.x + px(w), bounds.origin.y + px(H)));
            area.close();
            if let Ok(area) = area.build() {
                window.paint_path(
                    area,
                    gpui_kit::linear_gradient(
                        180.0,
                        gpui_kit::linear_color_stop(alpha(color, 0.35), 0.0),
                        gpui_kit::linear_color_stop(alpha(color, 0.0), 1.0),
                    ),
                );
            }
            let mut path = gpui_kit::PathBuilder::stroke(px(2.0));
            path.move_to(at(0, history[0]));
            for (i, v) in history.iter().enumerate().skip(1) {
                path.line_to(at(i, *v));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w_full()
    .h(px(H))
    .into_any_element()
}

/// "4 in voice", and the like, through the shared translations.
pub(crate) fn in_voice(count: usize) -> String {
    t_with("dms-calls.calls.stage.inVoice", &[("count", Arg::Num(count as i64))])
}
