//! How things move, the same everywhere: the desktop's counterpart of the web
//! app's `components/motion.tsx`. Things enter with a spring (a little rise
//! and fade, sometimes a pop), selections glide, counts swap, and presses
//! and hovers answer. All of it respects reduced motion: GPUI's animations
//! and Base's springs settle at once when it's on.

use std::time::Duration;

use gpui_kit::base::motion::{Spring, spring};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, IntoElement, SpringConfig, Styled, Window, px,
    sampled_easing,
};

/// The spring things enter with: quick, with a touch of overshoot.
const ENTER: SpringConfig = SpringConfig::new(420.0, 26.0, 1.0);

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
