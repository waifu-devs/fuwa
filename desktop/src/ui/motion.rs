//! How things move, the same everywhere: the desktop's counterpart of the web
//! app's `components/motion.tsx`. Things enter with a spring (a little rise
//! and fade, sometimes a pop), selections glide, counts swap, and presses
//! and hovers answer. All of it respects reduced motion: GPUI's animations
//! and Base's springs settle at once when it's on.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;

use gpui_kit::base::motion::{Spring, spring};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Div, ElementId, Entity, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, RenderOnce, SharedString, SpringConfig, Stateful, StatefulInteractiveElement as _,
    Styled, Window, px, radians, sampled_easing,
};

/// The spring things enter with: quick, with a touch of overshoot.
const ENTER: SpringConfig = SpringConfig::new(420.0, 26.0, 1.0);

/// The web's dialog spring (`stiffness: 420, damping: 32`).
const DIALOG: SpringConfig = SpringConfig::new(420.0, 32.0, 1.0);

/// Fades something in while it rises into place, after `delay`.
pub fn rise<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    delay: Duration,
    distance: f32,
) -> impl IntoElement {
    let (duration, easing) = sampled_easing(ENTER, 0.002);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32().max(0.001);
    el.with_animation(id, Animation::new(total).with_easing(delayed(start, easing)), move |el, t| {
        el.opacity(t.clamp(0.0, 1.0)).relative().top(px((1.0 - t) * distance))
    })
}

/// Slides something in from the side (toasts, panels).
pub fn slide_in<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>, from: f32) -> impl IntoElement {
    let (duration, easing) = sampled_easing(ENTER, 0.002);
    el.with_animation(id, Animation::new(duration).with_easing(easing), move |el, t| {
        el.opacity(t.clamp(0.0, 1.0)).relative().left(px((1.0 - t) * from))
    })
}

/// Rises from below the window's edge with a spring, like a sheet pulled up.
pub fn sheet_up<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>) -> impl IntoElement {
    let (duration, easing) = sampled_easing(SpringConfig::new(420.0, 38.0, 1.0), 0.002);
    el.with_animation(id, Animation::new(duration).with_easing(easing), move |el, t| {
        el.relative().top(px((1.0 - t) * 640.0))
    })
}

/// The web's dialogs coming in: a fade while they rise 40px and grow from 96%.
pub fn dialog_in<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>) -> impl IntoElement {
    let (duration, easing) = sampled_easing(DIALOG, 0.002);
    el.with_animation(id, Animation::new(duration).with_easing(easing), move |el, t| {
        el.opacity(t.clamp(0.0, 1.0)).translate_y(px((1.0 - t) * 40.0)).scale(0.96 + 0.04 * t)
    })
}

/// Fades something in while it grows from `from` (a picture opened large
/// comes in from 92%), on the dialogs' spring.
pub fn grow_in<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>, from: f32) -> impl IntoElement {
    let (duration, easing) = sampled_easing(DIALOG, 0.002);
    el.with_animation(id, Animation::new(duration).with_easing(easing), move |el, t| {
        el.opacity(t.clamp(0.0, 1.0)).scale(from + (1.0 - from) * t)
    })
}

/// The web's popovers and menus coming in (`zoom-in-95`, or `from` of their
/// size): a fade while they grow out of the corner they hang from (`origin`,
/// as fractions of their size) and slide `rise` pixels into place.
pub fn pop_in<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    origin: (f32, f32),
    from: f32,
    rise: f32,
) -> impl IntoElement {
    let (duration, easing) = sampled_easing(ENTER, 0.002);
    el.with_animation(id, Animation::new(duration).with_easing(easing), move |el, t| {
        el.opacity(t.clamp(0.0, 1.0))
            .transform_origin(origin.0, origin.1)
            .translate_y(px((1.0 - t) * rise))
            .scale(from + (1.0 - from) * t)
    })
}

/// A badge or icon popping in after `delay`: it grows from `from` of its size
/// and turns upright from `turn` degrees on a lively spring that overshoots
/// (the web's `stiffness: 500, damping: 14`).
pub fn pop<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    from: f32,
    turn: f32,
    delay: Duration,
) -> impl IntoElement {
    let (duration, easing) = sampled_easing(SpringConfig::new(500.0, 14.0, 1.0), 0.002);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32().max(0.001);
    el.with_animation(id, Animation::new(total).with_easing(delayed(start, easing)), move |el, t| {
        el.scale(from + (1.0 - from) * t).rotate(radians(((1.0 - t) * turn).to_radians()))
    })
}

/// Fades in, nothing else.
pub fn fade_in<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    duration: Duration,
) -> impl IntoElement {
    el.with_animation(id, Animation::new(duration).with_easing(gpui_kit::ease_out_quint()), |el, t| el.opacity(t))
}

/// A gentle loop that's only there to look nice (a mascot bobbing, a glow
/// drifting): it runs while the window is in front, and rests at its
/// starting pose while it isn't, so a window in the background doesn't
/// redraw many times a second for nobody.
pub fn ambient<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    period: Duration,
    window: &Window,
    pose: impl Fn(E, f32) -> E + 'static,
) -> AnyElement {
    if window.is_window_active() {
        el.with_animation(id, Animation::new(period).repeat(), pose).into_any_element()
    } else {
        pose(el, 0.0).into_any_element()
    }
}

/// A flourish that plays once when it first shows (a wiggle, a bob, dots
/// drifting by) and then rests at `pose(1.0)`, so a page that holds still
/// stops drawing. For ones that keep going while the window's in front, see
/// [`ambient`].
pub fn once<E: IntoElement + Styled + 'static>(
    el: E,
    id: impl Into<ElementId>,
    duration: Duration,
    pose: impl Fn(E, f32) -> E + 'static,
) -> AnyElement {
    el.with_animation(id, Animation::new(duration), pose).into_any_element()
}

/// An easing that waits for the first `start` of the time, then runs `easing` over the rest.
fn delayed(start: f32, easing: impl Fn(f32) -> f32 + 'static) -> impl Fn(f32) -> f32 + 'static {
    move |t| {
        if t <= start { 0.0 } else { easing(((t - start) / (1.0 - start)).clamp(0.0, 1.0)) }
    }
}

/// A value that springs toward `target` whenever it changes (a pill's height,
/// a corner's radius), keyed by `id` within the current element.
pub fn follow(id: impl Into<ElementId>, target: f32, window: &mut Window, cx: &mut App) -> f32 {
    spring(id.into(), target, Spring::new(Duration::from_millis(320)), window, cx)
}

/// Like [`follow`], with overshoot, for things that may bounce.
pub fn follow_bouncy(id: impl Into<ElementId>, target: f32, window: &mut Window, cx: &mut App) -> f32 {
    spring(id.into(), target, Spring::new(Duration::from_millis(380)).with_damping(0.62), window, cx)
}

/// A number that rolls up from zero when it first shows, after `delay`,
/// written by `format` (the web's `CountUp`).
pub fn count_up(id: impl Into<ElementId>, value: f64, delay: Duration, format: fn(f64) -> String) -> AnyElement {
    let duration = Duration::from_millis(900);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32().max(0.001);
    gpui_kit::div()
        .with_animation(
            id,
            Animation::new(total).with_easing(delayed(start, gpui_kit::ease_out_quint())),
            move |el, t| gpui_kit::ParentElement::child(el, format(value * f64::from(t.clamp(0.0, 1.0)))),
        )
        .into_any_element()
}

/// The web's `SPRING` (`stiffness: 520, damping: 34`), for text that swaps and counts that roll.
const SWAP: SpringConfig = SpringConfig::new(520.0, 34.0, 1.0);

/// What a swapping text showed, and how many times it has changed.
struct Swapped {
    now: SharedString,
    before: Option<SharedString>,
    up: bool,
    changes: u64,
}

/// Remembers `text` under `id` and says what it was before, if it just
/// changed: the web's `AnimatePresence` with `initial={false}`, so nothing
/// moves when it first shows.
fn swapped(
    id: &SharedString,
    text: SharedString,
    up: bool,
    window: &mut Window,
    cx: &mut App,
) -> (Option<SharedString>, bool, u64) {
    let state = window.use_keyed_state(SharedString::from(format!("{id}|swap")), cx, |_, _| Swapped {
        now: text.clone(),
        before: None,
        up,
        changes: 0,
    });
    state.update(cx, |s, _| {
        if s.now != text {
            s.before = Some(std::mem::replace(&mut s.now, text));
            s.up = up;
            s.changes += 1;
        }
        (s.before.clone(), s.up, s.changes)
    })
}

/// Text that slides and fades to its new value when it changes, like a
/// renamed server (the web's `SwapText`): the new one rises `0.6em` into
/// place while the old one rises out. `size` is the text's size in pixels.
pub fn swap_text(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    size: f32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = id.into();
    let text = text.into();
    let (before, _, changes) = swapped(&id, text.clone(), true, window, cx);
    roll(id, text, before, true, changes, size * 0.6, false)
}

/// A small count, like an unread badge, that rolls to its new value: up when
/// it grows, down when it shrinks (the web's `Count`). Past `max` it reads
/// "max+", and it's written as the app's language writes numbers ("12,345"). `size` is the text's size in pixels.
pub fn count(
    id: impl Into<SharedString>,
    value: u64,
    max: Option<u64>,
    size: f32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = id.into();
    let text: SharedString = match max {
        Some(max) if value > max => format!("{max}+").into(),
        _ => crate::core::i18n::number(value as i64).into(),
    };
    let last = window.use_keyed_state(SharedString::from(format!("{id}|count")), cx, |_, _| value);
    let up = value >= *last.read(cx);
    last.update(cx, |v, _| *v = value);
    let (before, up_then, changes) = swapped(&id, text.clone(), up, window, cx);
    roll(id, text, before, up_then, changes, size * 1.25, true)
}

/// Draws `text` coming in and `before` going out, `distance` pixels apart,
/// rising when `up`. A count clips to its line, as the web's `overflow-hidden`.
fn roll(
    id: SharedString,
    text: SharedString,
    before: Option<SharedString>,
    up: bool,
    changes: u64,
    distance: f32,
    clip: bool,
) -> AnyElement {
    let Some(before) = before else {
        return gpui_kit::div().child(text).into_any_element();
    };
    let (duration, easing) = sampled_easing(SWAP, 0.002);
    let easing = std::rc::Rc::new(easing);
    let sign = if up { 1.0 } else { -1.0 };
    let incoming = {
        let easing = easing.clone();
        gpui_kit::div().child(text).with_animation(
            ElementId::Name(format!("{id}|in{changes}").into()),
            Animation::new(duration).with_easing(move |t| easing(t)),
            move |el, t| el.opacity(t.clamp(0.0, 1.0)).translate_y(px((1.0 - t) * distance * sign)),
        )
    };
    let outgoing = gpui_kit::div().absolute().top_0().left_0().child(before).with_animation(
        ElementId::Name(format!("{id}|out{changes}").into()),
        Animation::new(duration).with_easing(move |t| easing(t)),
        move |el, t| el.opacity((1.0 - t).clamp(0.0, 1.0)).translate_y(px(-t * distance * sign)),
    );
    gpui_kit::div().relative().when(clip, |el| el.overflow_hidden()).child(incoming).child(outgoing).into_any_element()
}

/// A count that rolls ([`count`]), for places drawn without the window at hand.
#[derive(IntoElement)]
pub struct Rolling {
    id: SharedString,
    value: u64,
    max: Option<u64>,
    size: f32,
}

/// `value`, rolling to each new one; past `max` it reads "max+". `size` is the text's size in pixels.
pub fn rolling(id: impl Into<SharedString>, value: u64, max: Option<u64>, size: f32) -> Rolling {
    Rolling { id: id.into(), value, max, size }
}

impl RenderOnce for Rolling {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        count(self.id, self.value, self.max, self.size, window, cx)
    }
}

/// `key`'s translated words for `n` ("3 replies") with the number rolling in
/// them, as the web puts `<Count>` inside `<T>`. `size` is the text's size in pixels.
pub fn counted(id: impl Into<SharedString>, key: &str, n: u64, size: f32) -> Div {
    let text = crate::core::i18n::t_with(key, &[("count", crate::core::i18n::Arg::Num(n as i64))]);
    let number = crate::core::i18n::number(n as i64);
    let Some(at) = text.find(&number) else { return gpui_kit::div().child(text) };
    let (before, after) = (text[..at].to_owned(), text[at + number.len()..].to_owned());
    gpui_kit::div()
        .flex()
        .whitespace_nowrap()
        .when(!before.is_empty(), |el| el.child(before))
        .child(rolling(id, n, None, size))
        .when(!after.is_empty(), |el| el.child(after))
}

/// Text that swaps ([`swap_text`]), for places drawn without the window at hand.
#[derive(IntoElement)]
pub struct Swapping {
    id: SharedString,
    text: SharedString,
    size: f32,
}

/// `text`, sliding to each new value. `size` is the text's size in pixels.
pub fn swapping(id: impl Into<SharedString>, text: impl Into<SharedString>, size: f32) -> Swapping {
    Swapping { id: id.into(), text: text.into(), size }
}

impl RenderOnce for Swapping {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        swap_text(self.id, self.text, self.size, window, cx)
    }
}

/// A value that springs toward a target ([`follow`]) for something drawn
/// without the window at hand: `build` draws it at the value it's at.
#[derive(IntoElement)]
pub struct Springing {
    id: SharedString,
    target: f32,
    build: Box<dyn FnOnce(f32) -> AnyElement>,
}

/// Draws `build` at a value springing toward `target`.
pub fn springing(
    id: impl Into<SharedString>,
    target: f32,
    build: impl FnOnce(f32) -> AnyElement + 'static,
) -> Springing {
    Springing { id: id.into(), target, build: Box::new(build) }
}

impl RenderOnce for Springing {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        (self.build)(follow(self.id, self.target, window, cx))
    }
}

/// How something looks while it's pointed at or held: scaled, turned
/// clockwise by `turn` degrees and lifted `lift` pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub scale: f32,
    pub turn: f32,
    pub lift: f32,
}

impl Pose {
    /// As laid out.
    pub const REST: Pose = Pose { scale: 1.0, turn: 0.0, lift: 0.0 };

    /// Turned clockwise by `degrees` (the web's `hover:rotate-90`).
    pub const fn turn(degrees: f32) -> Pose {
        Pose { scale: 1.0, turn: degrees, lift: 0.0 }
    }
}

/// Whether the pointer is over something and holding it down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    pub hovered: bool,
    pub pressed: bool,
}

/// Follows the pointer over `el`, keyed by `id`: what it's doing as this frame
/// is drawn, and the element, listening for changes. `el` mustn't have an
/// `on_hover` of its own.
pub fn pointer(el: Stateful<Div>, id: &SharedString, window: &mut Window, cx: &mut App) -> (Stateful<Div>, Pointer) {
    let state = window.use_keyed_state(SharedString::from(format!("{id}|pointer")), cx, |_, _| Pointer::default());
    let now = *state.read(cx);
    let (hover, down, up) = (state.clone(), state.clone(), state);
    let el = el
        .on_hover(move |hovered, _, cx| {
            let hovered = *hovered;
            // Let go outside, it isn't held any more.
            update_pointer(&hover, cx, |p| Pointer { hovered, pressed: p.pressed && hovered });
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| update_pointer(&down, cx, |p| Pointer { pressed: true, ..p }))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| update_pointer(&up, cx, |p| Pointer { pressed: false, ..p }));
    (el, now)
}

fn update_pointer(state: &Entity<Pointer>, cx: &mut App, change: impl Fn(Pointer) -> Pointer) {
    state.update(cx, |pointer, cx| {
        let next = change(*pointer);
        if next != *pointer {
            *pointer = next;
            cx.notify();
        }
    });
}

/// Eases toward `target` on a spring, as the web's `transition` eases its
/// `hover:` and `active:` transforms.
pub fn follow_pose(id: &SharedString, target: Pose, window: &mut Window, cx: &mut App) -> Pose {
    Pose {
        scale: follow(SharedString::from(format!("{id}|scale")), target.scale, window, cx),
        turn: follow(SharedString::from(format!("{id}|turn")), target.turn, window, cx),
        lift: follow(SharedString::from(format!("{id}|lift")), target.lift, window, cx),
    }
}

/// Puts a pose on an element, around its center.
pub fn posed<E: Styled>(el: E, pose: Pose) -> E {
    if pose == Pose::REST {
        return el;
    }
    el.scale(pose.scale).rotate(radians(pose.turn.to_radians())).translate_y(px(-pose.lift))
}

/// Makes `el` answer the pointer: it springs to `hover` while pointed at and
/// to `press` while held, and back when left. `el` mustn't have an `on_hover`
/// of its own; to pose something inside it, use [`pointer`] and [`posed`].
pub fn answer(
    el: Stateful<Div>,
    id: impl Into<SharedString>,
    hover: Pose,
    press: Pose,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let (el, now) = pointer(el, &id, window, cx);
    let target = if now.pressed {
        press
    } else if now.hovered {
        hover
    } else {
        Pose::REST
    };
    posed(el, follow_pose(&id, target, window, cx))
}
