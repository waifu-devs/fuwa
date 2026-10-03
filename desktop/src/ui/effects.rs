//! The animated backdrop effects (aurora, petals, stars, waves): the web
//! app's shaders (`web/src/lib/effects/shaders.ts`) redrawn with what GPUI
//! paints cheaply. Light is soft shadows, shapes are paths and stars are
//! small quads: a few hundred primitives a frame, the same motion and colors
//! as the web (the theme's primary, the same hue turned 48 degrees, and the
//! page).
//!
//! They draw 30 frames a second while the window is in front, like the
//! web's, and hold still with reduced motion, at speed 0 or while the window
//! is behind others. The effect keeps its own clock, so a speed change
//! carries on from where it is instead of jumping.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Bounds, BoxShadow, Corners, Hsla, IntoElement as _, ParentElement as _, Path, PathBuilder, Pixels,
    Rgba, Styled as _, Window, canvas, div, fill, point, px, size,
};
use parking_lot::Mutex;

use crate::core::themes::Effect;
use crate::ui::theme::Palette;

/// Time between frames: 30 a second, like the web's effects.
const FRAME: Duration = Duration::from_millis(33);

/// The layer for an animated effect, filling its parent; none for the still ones.
pub fn layer(
    effect: Effect,
    intensity: u8,
    speed: u8,
    p: &Palette,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    if !matches!(effect, Effect::Aurora | Effect::Petals | Effect::Stars | Effect::Waves) {
        return None;
    }
    let running = speed > 0 && window.is_window_active() && !cx.reduce_motion() && !HELD.load(Ordering::Relaxed);
    let time = clock(running, speed);
    if running {
        again(window, cx);
    }
    let colors = Colors::of(p);
    Some(
        div()
            .absolute()
            .inset_0()
            .opacity(f32::from(intensity) / 100.0)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| match effect {
                        Effect::Aurora => aurora(bounds, time, &colors, window),
                        Effect::Petals => petals(bounds, time, &colors, window),
                        Effect::Stars => stars(bounds, time, &colors, window),
                        Effect::Waves => waves(bounds, time, &colors, window),
                        _ => {}
                    },
                )
                .size_full(),
            )
            .into_any_element(),
    )
}

/// The effect's clock in seconds, moving at `speed` percent while running.
fn clock(running: bool, speed: u8) -> f32 {
    struct Clock {
        time: f32,
        last: Option<Instant>,
    }
    // Starting a while in, so everything is already spread out.
    static CLOCK: Mutex<Clock> = Mutex::new(Clock { time: 40.0, last: None });
    let mut clock = CLOCK.lock();
    let now = Instant::now();
    if running {
        if let Some(last) = clock.last {
            // A long gap (a busy moment, a sleep) moves it on a little, not by the whole gap.
            clock.time += (now - last).as_secs_f32().min(0.25) * f32::from(speed) / 100.0;
        }
        clock.last = Some(now);
    } else {
        clock.last = None;
    }
    clock.time
}

/// Asks the view being drawn for another frame, one frame time from now.
/// Set while something opaque covers the whole window, so the effect stands still
/// instead of drawing frames nobody sees.
static HELD: AtomicBool = AtomicBool::new(false);

pub fn hold(covered: bool) {
    HELD.store(covered, Ordering::Relaxed);
}

fn again(window: &Window, cx: &mut App) {
    static WAITING: AtomicBool = AtomicBool::new(false);
    if WAITING.swap(true, Ordering::Relaxed) {
        return;
    }
    let view = window.current_view();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FRAME).await;
        WAITING.store(false, Ordering::Relaxed);
        cx.update(|cx| cx.notify(view));
    })
    .detach();
}

#[derive(Clone, Copy)]
struct Colors {
    c1: Rgba,
    c2: Rgba,
    dark: bool,
}

impl Colors {
    fn of(p: &Palette) -> Self {
        Self { c1: p.primary, c2: p.glow, dark: p.dark }
    }
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }
}

const WHITE: Rgba = Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

fn with_alpha(c: Rgba, a: f32) -> Hsla {
    Hsla::from(Rgba { a: a.clamp(0.0, 1.0), ..c })
}

/// The shaders' `hash21`: a value in 0..1 for a point, the same everywhere.
fn hash(x: f32, y: f32) -> f32 {
    let (mut qx, mut qy) = ((x * 123.34).fract().abs(), (y * 456.21).fract().abs());
    let d = qx * (qx + 45.32) + qy * (qy + 45.32);
    qx += d;
    qy += d;
    (qx * qy).fract()
}

/// The shaders' smooth value noise.
fn noise(x: f32, y: f32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    lerp(lerp(hash(ix, iy), hash(ix + 1.0, iy), ux), lerp(hash(ix, iy + 1.0), hash(ix + 1.0, iy + 1.0), ux), uy)
}

/// A soft blob of light: a blurred rounded box, centered at `(x, y)`.
fn glow(window: &mut Window, x: f32, y: f32, w: f32, h: f32, blur: f32, color: Hsla) {
    let bounds = Bounds::new(point(px(x - w / 2.0), px(y - h / 2.0)), size(px(w), px(h)));
    let shadow = BoxShadow {
        color,
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(blur),
        spread_radius: px(0.0),
        inset: false,
    };
    window.paint_drop_shadows(bounds, Corners::all(px(w.min(h) / 2.0)), &[shadow]);
}

/// Three curtains of light swaying across the top, in the primary color and its neighbor.
fn aurora(b: Bounds<Pixels>, time: f32, c: &Colors, window: &mut Window) {
    let (left, top) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let aspect = w / h.max(1.0);
    let t = time * 0.12;
    // Blobs overlap along each curtain so it reads as one band.
    let steps = 16;
    let step = w / steps as f32;
    for i in 0..3 {
        let fi = i as f32;
        for s in 0..=steps {
            let u = s as f32 / steps as f32;
            let x = u * aspect;
            let center = 0.22
                + fi * 0.2
                + 0.09 * (x * (1.3 + fi * 0.4) + t * (1.0 + fi * 0.35) + fi * 2.1).sin()
                + 0.06 * (noise(x * 1.8 + t * 0.8, fi * 3.7) - 0.5);
            let width = 0.07 + 0.04 * noise(x * 2.5 - t, fi + 9.0);
            let streak = 0.55 + 0.45 * noise(x * 6.0 + t * 2.0, fi);
            let tint = mix(c.c1, c.c2, 0.5 + 0.5 * (x * 0.9 + t + fi * 1.9).sin());
            let band = width * h * 1.6;
            glow(window, left + u * w, top + center * h, step * 1.6, band, band * 0.3, with_alpha(tint, 0.5 * streak));
        }
    }
}

/// Blossom petals falling and turning, a near layer and a smaller, deeper one behind it.
fn petals(b: Bounds<Pixels>, time: f32, c: &Colors, window: &mut Window) {
    let shade = mix(c.c1, WHITE, 0.25);
    let deeper = Rgba { r: shade.r * 0.85, g: shade.g * 0.85, b: shade.b * 0.85, a: 1.0 };
    petal_layer(b, time, 7.5, 0.2, 17.0, with_alpha(deeper, 0.7), window);
    petal_layer(b, time, 4.0, 0.32, 1.0, with_alpha(shade, 1.0), window);
}

fn petal_layer(b: Bounds<Pixels>, t: f32, scale: f32, speed: f32, seed: f32, color: Hsla, window: &mut Window) {
    let (left, top) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    // One cell is h / scale points; a cell holds a petal about two times in five.
    let cell = h / scale.max(0.1);
    let fall = t * speed;
    let cols = (w / cell).ceil() as i32 + 1;
    let first = (-fall).floor() as i32 - 1;
    for j in first..=first + scale.ceil() as i32 + 2 {
        for i in -1..=cols {
            let (ci, cj) = (i as f32, j as f32);
            let n = hash(ci + seed, cj + seed);
            if n > 0.42 {
                continue;
            }
            let jx = (hash(ci + 3.1, cj + 3.1) - 0.5) * 0.5;
            let jy = (hash(ci + 7.7, cj + 7.7) - 0.5) * 0.5;
            let py = cj + 0.5 + jy;
            let px_ = ci + 0.5 + jx - (t * 0.4 + (py + fall) * 0.6 + seed).sin() * 0.35;
            let (x, y) = (left + px_ * cell, top + (py + fall) * cell);
            if y < top - cell || y > top + h + cell {
                continue;
            }
            let angle = t * (0.6 + n * 1.5) + n * TAU;
            let flutter = 0.55 + 0.45 * (t * (1.0 + n) + n * 9.0).sin().abs();
            petal(window, x, y, cell * 0.25, flutter, angle, color);
        }
    }
}

/// A petal `size` long: a rounded teardrop with a notch at its tip, squeezed
/// across by `flutter` as it turns over, turned by `angle`. Shapes are made
/// once for a few dozen turns and kept, so a frame only moves them.
fn petal(window: &mut Window, x: f32, y: f32, size: f32, flutter: f32, angle: f32, color: Hsla) {
    const TURNS: f32 = 48.0;
    const FLUTTERS: f32 = 12.0;
    /// Made petals by (size, turn, squeeze).
    type Shapes = HashMap<(u32, u8, u8), Path<Pixels>>;
    static SHAPES: Mutex<Option<Shapes>> = Mutex::new(None);
    let turn = (angle.rem_euclid(TAU) / TAU * TURNS).round() as u8 % TURNS as u8;
    let squeeze = (flutter.clamp(0.0, 1.0) * FLUTTERS).round() as u8;
    let key = ((size * 4.0).round() as u32, turn, squeeze);
    let mut shapes = SHAPES.lock();
    let shapes = shapes.get_or_insert_with(HashMap::new);
    if shapes.len() > 4000 {
        shapes.clear();
    }
    let shape = match shapes.get(&key) {
        Some(shape) => shape,
        None => {
            let Some(made) = petal_shape(size, f32::from(squeeze) / FLUTTERS, f32::from(turn) / TURNS * TAU) else {
                return;
            };
            shapes.entry(key).or_insert(made)
        }
    };
    let mut path = shape.clone();
    let by = point(px(x), px(y));
    path.bounds.origin += by;
    for v in &mut path.vertices {
        v.xy_position += by;
    }
    window.paint_path(path, color);
}

fn petal_shape(size: f32, flutter: f32, angle: f32) -> Option<Path<Pixels>> {
    let (sin, cos) = angle.sin_cos();
    let at = |u: f32, v: f32| {
        let (u, v) = (u * size * flutter.max(0.05) * 0.6, v * size);
        point(px(u * cos - v * sin), px(u * sin + v * cos))
    };
    let mut path = PathBuilder::fill();
    path.move_to(at(0.0, -0.5));
    path.cubic_bezier_to(at(0.5, 0.1), at(0.45, -0.5), at(0.55, -0.05));
    path.cubic_bezier_to(at(0.0, 0.32), at(0.45, 0.35), at(0.12, 0.48));
    path.cubic_bezier_to(at(-0.5, 0.1), at(-0.12, 0.48), at(-0.45, 0.35));
    path.cubic_bezier_to(at(0.0, -0.5), at(-0.55, -0.05), at(-0.45, -0.5));
    path.close();
    path.build().ok()
}

/// Three depths of stars drifting slowly and twinkling, over a faint haze.
fn stars(b: Bounds<Pixels>, time: f32, c: &Colors, window: &mut Window) {
    let (left, top) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    // The haze: a few wide, faint clouds wandering about.
    for k in 0..3 {
        let fk = k as f32;
        let u = 0.2 + 0.3 * fk + 0.12 * (time * 0.02 + fk * 2.0).sin();
        let v = 0.3 + 0.2 * (time * 0.015 + fk * 1.3).cos();
        let tint = mix(c.c1, c.c2, u);
        glow(window, left + u * w, top + v * h, w * 0.4, h * 0.3, h * 0.2, with_alpha(tint, 0.14));
    }
    // White stars vanish on a light page, so there they're deeper, like the web's.
    let star = if c.dark {
        mix(c.c2, WHITE, 0.6)
    } else {
        let s = mix(c.c2, c.c1, 0.3);
        Rgba { r: s.r * 0.6, g: s.g * 0.6, b: s.b * 0.6, a: 1.0 }
    };
    // (cells down the window, seed, brightness, glows, how many cells hold a star)
    for (scale, seed, strength, big, share) in
        [(9.0, 0.0, 1.0, true, 0.22), (17.0, 11.0, 0.7, false, 0.22), (31.0, 23.0, 0.45, false, 0.12)]
    {
        let cell = h / scale;
        let drift = time * 0.01 * scale;
        let first = drift.floor() as i32 - 1;
        let cols = (w / cell).ceil() as i32 + 2;
        for j in 0..=scale as i32 {
            for i in first..first + cols {
                let (ci, cj) = (i as f32, j as f32);
                let n = hash(ci + seed, cj + seed);
                if n > share {
                    continue;
                }
                let cx = (hash(ci + 1.3, cj + 1.3) * 0.7 + 0.15 + ci - drift) * cell;
                let cy = (hash(ci + 5.9, cj + 5.9) * 0.7 + 0.15 + cj) * cell;
                if cx < -8.0 || cx > w + 8.0 || cy > h + 8.0 {
                    continue;
                }
                let twinkle = 0.55 + 0.45 * (time * (1.0 + n * 5.0) + n * 40.0).sin();
                let r = if big { 1.0 + n * 6.0 } else { 0.7 + n * 3.0 };
                let a = strength * twinkle;
                let (x, y) = (left + cx, top + cy);
                if big {
                    glow(window, x, y, r * 2.0, r * 2.0, r * 3.0, with_alpha(star, a * 0.6));
                }
                let dot = Bounds::new(point(px(x - r), px(y - r)), size(px(r * 2.0), px(r * 2.0)));
                window.paint_quad(fill(dot, with_alpha(star, a)).corner_radii(px(r)));
            }
        }
    }
}

/// Four layers of waves rolling along the lower part, front over back.
fn waves(b: Bounds<Pixels>, time: f32, c: &Colors, window: &mut Window) {
    let (left, top) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let aspect = w / h.max(1.0);
    let t = time * 0.35;
    let steps = 48;
    for i in 0..4 {
        let fi = i as f32;
        let mut path = PathBuilder::fill();
        path.move_to(point(px(left), px(top + h)));
        for s in 0..=steps {
            let u = s as f32 / steps as f32;
            let x = u * aspect;
            let height = 0.58
                + fi * 0.1
                + 0.035 * (x * (2.2 - fi * 0.3) + t * (1.0 + fi * 0.25) + fi * 1.3).sin()
                + 0.02 * (x * (5.1 + fi) - t * 1.4 + fi).sin();
            path.line_to(point(px(left + u * w), px(top + height * h)));
        }
        path.line_to(point(px(left + w), px(top + h)));
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, with_alpha(mix(c.c1, c.c2, fi / 3.0), 0.3 + fi * 0.1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_stays_in_range_and_is_the_same_every_time() {
        for k in 0..500 {
            let (x, y) = (k as f32 * 0.37 - 40.0, k as f32 * 1.13);
            let n = noise(x, y);
            assert!((0.0..=1.0).contains(&n), "{n}");
            assert!((0.0..1.0).contains(&hash(x, y)));
            assert_eq!(n, noise(x, y));
        }
    }

    #[test]
    fn the_clock_holds_still_unless_running() {
        let a = clock(false, 100);
        assert_eq!(clock(false, 100), a);
    }
}
