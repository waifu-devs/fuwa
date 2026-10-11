//! Small looping decorations (a ping around a dot, a ring swelling out of a
//! tile) drawn over the window instead of inside the part they belong to.
//!
//! A view asking for an animation frame is drawn again with every view
//! around it (`panel.rs`), so a ping in the sidebar would build the whole
//! sidebar 60 times a second. Instead the part draws a still placeholder of
//! the decoration's size, which notes where it landed (its bounds, clip,
//! transform and opacity) as the part is drawn; the floats layer, a small
//! view of its own over the parts and under the dialogs, draws each
//! decoration there on every frame. A part that isn't drawn again keeps its
//! placeholders where they were, so its floats stay; one that is drawn again
//! keeps only the placeholders it drew this time.
//!
//! Only parts that are cached panels hold floats: anywhere else (and with
//! the window in the background, or motion turned down) a float is drawn in
//! place, as an ordinary animation.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, EntityId, IntoElement, ParentElement as _, Pixels,
    RoundedMask, SharedString, Styled as _, TransformationMatrix, Window, canvas, div, px,
};

type Pose = Rc<dyn Fn(f32) -> AnyElement>;
/// A float to draw this frame: its pose, how far into its loop, where, its clip, transform and opacity.
type Shown = (Pose, f32, Bounds<Pixels>, RoundedMask<Pixels>, TransformationMatrix, f32);

struct Spot {
    /// The part it's in, and the frame it was placed in.
    owner: EntityId,
    frame: u64,
    bounds: Bounds<Pixels>,
    mask: RoundedMask<Pixels>,
    transform: TransformationMatrix,
    /// The opacity around it, noted as its part paints (after the floats are laid out), so
    /// a float waits a frame for it.
    opacity: Option<f32>,
    period: Duration,
    pose: Pose,
}

#[derive(Default)]
struct Floats {
    /// Counts the window's frames (`begin_frame`).
    frame: u64,
    /// Each cached panel, and the last frame it was drawn in.
    drawn: HashMap<EntityId, u64>,
    /// The cached panels in the window this frame.
    present: HashSet<EntityId>,
    spots: HashMap<SharedString, Spot>,
    /// When each float started, so its loop runs on from where it was.
    started: HashMap<SharedString, Instant>,
}

thread_local! {
    static FLOATS: RefCell<Floats> = RefCell::new(Floats::default());
}

/// A new frame of the window, with the cached panels it holds.
pub fn begin_frame(present: impl IntoIterator<Item = EntityId>) {
    FLOATS.with_borrow_mut(|f| {
        f.frame += 1;
        f.present = present.into_iter().collect();
    });
}

/// The window's frame now (counted by `begin_frame`).
pub fn frame() -> u64 {
    FLOATS.with_borrow(|f| f.frame)
}

/// A cached panel is being drawn (again) this frame.
pub fn panel_drawn(panel: EntityId) {
    FLOATS.with_borrow_mut(|f| {
        let frame = f.frame;
        f.drawn.insert(panel, frame);
    });
}

/// Whether `view` is a cached panel on screen that wasn't drawn again this
/// frame: what it showed last time is still on screen as it was.
pub fn kept(view: EntityId) -> bool {
    FLOATS.with_borrow(|f| f.present.contains(&view) && f.drawn.get(&view).is_some_and(|at| *at != f.frame))
}

/// A decoration `w` by `h` that loops every `period`, `pose(t)` drawing it
/// at `t` (0 to 1) into the loop. Still at `pose(0.0)` while the window is in
/// the background or motion is turned down.
pub fn float(
    id: impl Into<SharedString>,
    w: f32,
    h: f32,
    period: Duration,
    window: &Window,
    cx: &App,
    pose: impl Fn(f32) -> AnyElement + 'static,
) -> AnyElement {
    let id = id.into();
    if !window.is_window_active() || cx.reduce_motion() {
        return pose(0.0);
    }
    let owner = window.current_view();
    let held = FLOATS.with_borrow(|f| f.drawn.contains_key(&owner));
    if !held {
        // Not in a cached panel: an ordinary animation, in place.
        return div()
            .flex_none()
            .w(px(w))
            .h(px(h))
            .with_animation(id, Animation::new(period).repeat(), move |el, t| el.child(pose(t)))
            .into_any_element();
    }
    let pose: Pose = Rc::new(pose);
    let painted = id.clone();
    canvas(
        move |bounds, window, _| {
            let (mask, transform) = (window.rounded_content_mask(), window.element_transform());
            FLOATS.with_borrow_mut(|f| {
                let frame = f.frame;
                let opacity = f.spots.get(&id).and_then(|s| s.opacity);
                f.started.entry(id.clone()).or_insert_with(Instant::now);
                f.spots.insert(id, Spot { owner, frame, bounds, mask, transform, opacity, period, pose });
            });
        },
        move |_, _, window, _| {
            let opacity = window.element_opacity();
            FLOATS.with_borrow_mut(|f| {
                if let Some(spot) = f.spots.get_mut(&painted) {
                    spot.opacity = Some(opacity);
                }
            });
        },
    )
    .flex_none()
    .w(px(w))
    .h(px(h))
    .into_any_element()
}

/// The floats layer: every float placed in a part on screen, drawn where it
/// was placed. Goes over the parts, under anything over the window.
pub fn layer(window: &Window, cx: &App) -> AnyElement {
    let running = window.is_window_active() && !cx.reduce_motion();
    canvas(
        move |_, window, cx| {
            let now = Instant::now();
            let shown: Vec<Shown> = FLOATS.with_borrow_mut(|f| {
                let Floats { drawn, present, spots, started, .. } = f;
                // Placed in its part's latest drawing, in a part still on screen.
                spots.retain(|_, s| present.contains(&s.owner) && drawn.get(&s.owner) == Some(&s.frame));
                started.retain(|id, _| spots.contains_key(id));
                if !running {
                    return Vec::new();
                }
                spots
                    .iter()
                    .filter_map(|(id, s)| {
                        let opacity = s.opacity?;
                        let period = s.period.as_secs_f32().max(0.001);
                        let at = started.get(id).map_or(0.0, |at| now.duration_since(*at).as_secs_f32());
                        Some((s.pose.clone(), (at % period) / period, s.bounds, s.mask, s.transform, opacity))
                    })
                    .collect()
            });
            let waiting = FLOATS.with_borrow(|f| !f.spots.is_empty());
            if running && waiting {
                window.request_animation_frame();
            }
            shown
                .into_iter()
                .map(|(pose, t, bounds, mask, transform, opacity)| {
                    let mut el = div()
                        .w(bounds.size.width)
                        .h(bounds.size.height)
                        .opacity(opacity)
                        .child(pose(t))
                        .into_any_element();
                    window.with_transform(transform, |window| {
                        window.with_rounded_content_mask(Some(mask), |window| {
                            el.layout_as_root(bounds.size.into(), window, cx);
                            el.prepaint_at(bounds.origin, window, cx);
                        })
                    });
                    (el, mask, transform)
                })
                .collect::<Vec<_>>()
        },
        |_, shown, window, cx| {
            for (mut el, mask, transform) in shown {
                window.with_transform(transform, |window| {
                    window.with_rounded_content_mask(Some(mask), |window| el.paint(window, cx))
                });
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}
