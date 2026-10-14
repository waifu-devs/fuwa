//! The picture viewer's zoom and pan (the web's `lib/zoom.ts`, the same
//! rules). A picture is first laid out fitted to the room it has (never
//! stretched past its own size); `scale` is how much bigger than that it's
//! drawn, 1 being fitted, and `x`/`y` how far its middle is moved from the
//! room's middle, in window pixels.

/// Where a picture is: how far in, and how far moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

/// As it opens: fitted and in the middle.
pub const FITTED: View = View { scale: 1.0, x: 0.0, y: 0.0 };

/// How far in a picture can go past its own size, and never less than this past fitted.
const MOST_PAST_ACTUAL: f32 = 4.0;
const MOST_PAST_FITTED: f32 = 8.0;

/// The + and - buttons and keys: a fixed step around the middle.
pub const STEP: f32 = 1.5;

/// The size a picture is laid out at in `room`: its own, or smaller to fit.
pub fn fitted(natural: (f32, f32), room: (f32, f32)) -> (f32, f32) {
    if !(natural.0 > 0.0 && natural.1 > 0.0) {
        return room;
    }
    let scale = 1f32.min(room.0 / natural.0).min(room.1 / natural.1);
    (natural.0 * scale, natural.1 * scale)
}

/// The scale that draws the picture one pixel per pixel of its own (1:1).
pub fn actual_scale(natural: (f32, f32), room: (f32, f32)) -> f32 {
    let fit = fitted(natural, room);
    if fit.0 > 0.0 { (natural.0 / fit.0).max(1.0) } else { 1.0 }
}

/// The closest it can zoom in.
pub fn most_scale(natural: (f32, f32), room: (f32, f32)) -> f32 {
    MOST_PAST_FITTED.max(actual_scale(natural, room) * MOST_PAST_ACTUAL)
}

/// Keeps the picture where it can be seen: no smaller than fitted, no bigger
/// than [`most_scale`], and moved no further than takes its edge to the
/// room's (centered on an axis where it's smaller than the room).
pub fn clamp(view: View, natural: (f32, f32), room: (f32, f32)) -> View {
    let scale = view.scale.max(1.0).min(most_scale(natural, room));
    let fit = fitted(natural, room);
    let room_x = ((fit.0 * scale - room.0) / 2.0).max(0.0);
    let room_y = ((fit.1 * scale - room.1) / 2.0).max(0.0);
    // `+ 0.0` so a centered picture is at 0, never -0.
    View { scale, x: view.x.clamp(-room_x, room_x) + 0.0, y: view.y.clamp(-room_y, room_y) + 0.0 }
}

/// Zooms to `scale` keeping the point at `at` (from the room's middle)
/// where it is, as the wheel or a pinch does.
pub fn zoom_at(view: View, scale: f32, at: (f32, f32), natural: (f32, f32), room: (f32, f32)) -> View {
    let next = scale.max(1.0).min(most_scale(natural, room));
    let k = next / view.scale;
    clamp(View { scale: next, x: at.0 - (at.0 - view.x) * k, y: at.1 - (at.1 - view.y) * k }, natural, room)
}

/// Moves a zoomed picture by a drag.
pub fn pan_by(view: View, dx: f32, dy: f32, natural: (f32, f32), room: (f32, f32)) -> View {
    clamp(View { x: view.x + dx, y: view.y + dy, ..view }, natural, room)
}

/// A double click: back to fitted when zoomed, otherwise to 1:1 (or twice
/// fitted, for a picture already shown at its own size) around the point.
pub fn toggle_at(view: View, at: (f32, f32), natural: (f32, f32), room: (f32, f32)) -> View {
    if view.scale > 1.01 {
        return FITTED;
    }
    zoom_at(view, actual_scale(natural, room).max(2.0), at, natural, room)
}

/// How much a wheel's movement zooms (`dy` in pixels, up is negative).
pub fn wheel_factor(dy: f32) -> f32 {
    (-dy.clamp(-100.0, 100.0) * 0.002).exp()
}

/// The one after or before `index` among `count`, going round.
pub fn step_index(index: usize, count: usize, forward: bool) -> usize {
    match count {
        0 => 0,
        _ if forward => (index + 1) % count,
        _ => (index + count - 1) % count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: (f32, f32) = (1000.0, 800.0);
    const BIG: (f32, f32) = (4000.0, 2000.0);
    const SMALL: (f32, f32) = (200.0, 100.0);

    #[test]
    fn laid_out_fitted_never_stretched() {
        assert_eq!(fitted(BIG, ROOM), (1000.0, 500.0));
        assert_eq!(fitted(SMALL, ROOM), SMALL);
        assert_eq!(actual_scale(BIG, ROOM), 4.0);
        assert_eq!(actual_scale(SMALL, ROOM), 1.0);
        assert_eq!(most_scale(BIG, ROOM), 16.0);
        assert_eq!(most_scale(SMALL, ROOM), 8.0);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let view = zoom_at(FITTED, 2.0, (200.0, 100.0), BIG, ROOM);
        assert_eq!(view, View { scale: 2.0, x: -200.0, y: -100.0 });
        assert_eq!(zoom_at(view, 1.0, (200.0, 100.0), BIG, ROOM), FITTED);
    }

    #[test]
    fn never_past_the_limits() {
        assert_eq!(zoom_at(FITTED, 1000.0, (0.0, 0.0), BIG, ROOM).scale, 16.0);
        assert_eq!(zoom_at(FITTED, 0.2, (0.0, 0.0), BIG, ROOM), FITTED);
    }

    #[test]
    fn pans_only_as_far_as_its_edges() {
        let view = zoom_at(FITTED, 2.0, (0.0, 0.0), BIG, ROOM);
        assert_eq!(pan_by(view, 9999.0, 9999.0, BIG, ROOM), View { scale: 2.0, x: 500.0, y: 100.0 });
        assert_eq!(pan_by(view, -9999.0, -9999.0, BIG, ROOM), View { scale: 2.0, x: -500.0, y: -100.0 });
        assert_eq!(pan_by(FITTED, 50.0, 50.0, BIG, ROOM), FITTED);
    }

    #[test]
    fn double_click_goes_to_one_to_one_and_back() {
        let view = toggle_at(FITTED, (0.0, 0.0), BIG, ROOM);
        assert_eq!(view.scale, 4.0);
        assert_eq!(toggle_at(view, (0.0, 0.0), BIG, ROOM), FITTED);
        assert_eq!(toggle_at(FITTED, (0.0, 0.0), SMALL, ROOM).scale, 2.0);
    }

    #[test]
    fn wheel_and_steps() {
        assert!(wheel_factor(-100.0) > 1.0);
        assert!(wheel_factor(100.0) < 1.0);
        assert_eq!(wheel_factor(-5000.0), wheel_factor(-100.0));
        assert_eq!(step_index(2, 3, true), 0);
        assert_eq!(step_index(0, 3, false), 2);
        assert_eq!(step_index(0, 0, true), 0);
    }
}
