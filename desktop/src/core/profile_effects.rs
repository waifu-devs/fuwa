//! Profile effects, the same specs and the same planner as the web's
//! `lib/effects/profile.ts` (docs/profile-effects.md): layers of small
//! shapes, each with a way of moving, turned into particles with keyframes
//! for a card of a given size. An intro plays once when the card opens, then
//! an idle loop keeps going. The same seed gives the same particles, so an
//! effect looks the same here as on the web. Pure: the window draws it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Petal,
    Star,
    Sparkle,
    Heart,
    Snowflake,
    Bubble,
    Dot,
    Confetti,
    Streak,
}

impl Shape {
    pub const ALL: [Shape; 9] = [
        Shape::Petal,
        Shape::Star,
        Shape::Sparkle,
        Shape::Heart,
        Shape::Snowflake,
        Shape::Bubble,
        Shape::Dot,
        Shape::Confetti,
        Shape::Streak,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Shape::Petal => "petal",
            Shape::Star => "star",
            Shape::Sparkle => "sparkle",
            Shape::Heart => "heart",
            Shape::Snowflake => "snowflake",
            Shape::Bubble => "bubble",
            Shape::Dot => "dot",
            Shape::Confetti => "confetti",
            Shape::Streak => "streak",
        }
    }

    /// The shape in a 24 by 24 box, and the stroke width for outlined ones.
    fn path(self) -> (&'static str, Option<f32>) {
        match self {
            Shape::Petal => (
                "M12 22c-3.6-2.5-6.8-6.4-6.2-10.6C6.4 7.2 9.4 4.4 11 2.4l1 2 1-2c1.6 2 4.6 4.8 5.2 9C18.8 15.6 15.6 19.5 12 22Z",
                None,
            ),
            Shape::Star => ("M12 2.5l2.8 6 6.5.7-4.9 4.4 1.4 6.4L12 16.7 6.2 20l1.4-6.4-4.9-4.4 6.5-.7z", None),
            Shape::Sparkle => {
                ("M12 1c.9 6.2 3.8 9.1 10 10-6.2.9-9.1 3.8-10 10-.9-6.2-3.8-9.1-10-10 6.2-.9 9.1-3.8 10-10Z", None)
            }
            Shape::Heart => (
                "M12 21s-7.6-4.7-9.7-9.3C.8 8.3 3 4.5 6.7 4.5c2.1 0 3.5 1.1 5.3 3 1.8-1.9 3.2-3 5.3-3 3.7 0 5.9 3.8 4.4 7.2C19.6 16.3 12 21 12 21Z",
                None,
            ),
            Shape::Snowflake => (
                "M12 2v20M3.3 7l17.4 10M3.3 17L20.7 7M12 2l-2.5 2.5M12 2l2.5 2.5M12 22l-2.5-2.5M12 22l2.5-2.5",
                Some(1.8),
            ),
            Shape::Bubble => ("M12 2.5a9.5 9.5 0 1 0 0 19 9.5 9.5 0 1 0 0-19ZM7.5 9.5a4.5 4.5 0 0 1 3-3", Some(1.4)),
            Shape::Dot => ("M12 4a8 8 0 1 0 0 16 8 8 0 1 0 0-16Z", None),
            Shape::Confetti => ("M8 3h8v18H8z", None),
            Shape::Streak => ("M0 11.4h20.5l3.5.6-3.5.6H0z", None),
        }
    }

    /// The shape as an SVG file, white, for the window to tint.
    pub fn svg(self) -> String {
        let (d, stroke) = self.path();
        let paint = match stroke {
            Some(w) => format!(r#"fill="none" stroke="white" stroke-width="{w}" stroke-linecap="round""#),
            None => r#"fill="white""#.to_owned(),
        };
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24"><path d="{d}" {paint}/></svg>"#
        )
    }

    pub fn by_name(name: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|s| s.name() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Fall,
    Rise,
    Twinkle,
    Drift,
    Burst,
    Shoot,
    Pop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Top,
    Bottom,
    Edges,
    Corners,
    Anywhere,
    TopLeft,
    TopRight,
    Center,
}

/// A theme token, the card's own color, or a fixed color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Primary,
    Ring,
    Accent,
    Foreground,
    Card,
    Profile,
    Hex(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub shape: Shape,
    pub motion: Motion,
    pub intro: bool,
    pub count: u32,
    pub from: Region,
    pub size: (f32, f32),
    pub duration: (f32, f32),
    pub delay: (f32, f32),
    pub colors: Vec<Paint>,
    pub spin: f32,
    pub sway: f32,
    pub flip: bool,
    pub glow: bool,
    pub opacity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    /// A built-in's id, or an offered effect's item id in lowercase.
    pub id: String,
    /// Its own name and line; empty for built-ins, whose text is the app's.
    pub name: String,
    pub description: String,
    pub layers: Vec<Layer>,
}

/// The card these counts and sizes are for: a profile popout.
pub const REFERENCE: (f32, f32) = (304.0, 420.0);
pub const MAX_PER_LAYER: u32 = 24;
pub const MAX_PARTICLES: usize = 60;
/// How long the idle loop takes to fade in.
pub const IDLE_FADE_MS: f32 = 900.0;

/// The built-in effects' ids, in the picker's order.
pub const IDS: [&str; 8] = ["sakura", "starfall", "sparkles", "hearts", "snow", "bubbles", "fireflies", "confetti"];

#[allow(clippy::too_many_arguments)]
fn layer(
    shape: Shape,
    motion: Motion,
    intro: bool,
    count: u32,
    from: Region,
    size: (f32, f32),
    duration: (f32, f32),
    colors: &[Paint],
) -> Layer {
    Layer {
        shape,
        motion,
        intro,
        count,
        from,
        size,
        duration,
        delay: (0.0, 0.0),
        colors: colors.to_vec(),
        spin: 0.0,
        sway: 0.0,
        flip: false,
        glow: false,
        opacity: 1.0,
    }
}

impl Layer {
    fn delay(mut self, a: f32, b: f32) -> Self {
        self.delay = (a, b);
        self
    }
    fn spin(mut self, s: f32) -> Self {
        self.spin = s;
        self
    }
    fn sway(mut self, s: f32) -> Self {
        self.sway = s;
        self
    }
    fn flip(mut self) -> Self {
        self.flip = true;
        self
    }
    fn glow(mut self) -> Self {
        self.glow = true;
        self
    }
    fn opacity(mut self, o: f32) -> Self {
        self.opacity = o;
        self
    }
}

/// A built-in effect by id.
pub fn spec(id: &str) -> Option<Spec> {
    use Motion::*;
    use Paint::*;
    use Region::*;
    use Shape::*;
    let pinks = [Hex(0xffb7c5), Hex(0xff8fab), Hex(0xffc8d6), Primary];
    let gold = [Hex(0xfbbf24), Hex(0xfcd34d), Hex(0xf59e0b), Primary];
    let gold_ring = [Hex(0xfbbf24), Hex(0xfcd34d), Hex(0xf59e0b), Primary, Ring];
    let confetti = [Primary, Ring, Hex(0xfbbf24), Hex(0x34d399), Hex(0x60a5fa), Hex(0xf472b6)];
    let _ = gold;
    let layers = match id {
        "sakura" => vec![
            layer(Petal, Burst, true, 18, TopLeft, (18.0, 28.0), (1500.0, 2300.0), &pinks)
                .delay(0.0, 260.0)
                .spin(540.0)
                .sway(280.0)
                .flip()
                .opacity(0.95),
            layer(Petal, Fall, false, 9, Top, (15.0, 24.0), (6500.0, 9500.0), &pinks)
                .spin(320.0)
                .sway(22.0)
                .flip()
                .opacity(0.9),
        ],
        "starfall" => vec![
            layer(Star, Twinkle, true, 15, Edges, (14.0, 24.0), (1400.0, 2000.0), &gold_ring)
                .delay(0.0, 700.0)
                .spin(90.0)
                .glow(),
            layer(Streak, Shoot, true, 1, Top, (76.0, 96.0), (1400.0, 1400.0), &[Hex(0xfcd34d)])
                .delay(250.0, 250.0)
                .glow(),
            layer(Sparkle, Twinkle, false, 11, Edges, (11.0, 19.0), (2600.0, 4600.0), &gold_ring).spin(60.0).glow(),
            layer(Streak, Shoot, false, 2, Top, (64.0, 96.0), (6000.0, 9000.0), &[Hex(0xfcd34d), Primary]).glow(),
        ],
        "sparkles" => vec![
            layer(Sparkle, Pop, true, 14, Edges, (18.0, 32.0), (900.0, 1500.0), &[Primary, Hex(0xfbbf24), Ring])
                .delay(0.0, 650.0)
                .spin(90.0)
                .glow(),
            layer(Sparkle, Twinkle, false, 10, Edges, (12.0, 24.0), (2200.0, 3800.0), &[Primary, Hex(0xfbbf24), Ring])
                .spin(90.0)
                .glow(),
        ],
        "hearts" => {
            let colors = [Hex(0xff6b9d), Hex(0xfb7185), Profile, Primary];
            vec![
                layer(Heart, Burst, true, 13, Bottom, (18.0, 30.0), (1300.0, 1900.0), &colors)
                    .delay(0.0, 300.0)
                    .spin(40.0)
                    .sway(260.0)
                    .opacity(0.95),
                layer(Heart, Rise, false, 7, Bottom, (15.0, 24.0), (5500.0, 8500.0), &colors)
                    .spin(30.0)
                    .sway(14.0)
                    .opacity(0.85),
            ]
        }
        "snow" => vec![
            layer(
                Snowflake,
                Fall,
                true,
                18,
                Top,
                (14.0, 22.0),
                (1800.0, 2500.0),
                &[Hex(0xffffff), Hex(0xdbeafe), Hex(0xbfdbfe), Ring],
            )
            .delay(0.0, 450.0)
            .spin(180.0)
            .sway(16.0)
            .opacity(0.95),
            layer(Dot, Fall, false, 12, Top, (5.0, 9.0), (7000.0, 11000.0), &[Hex(0xffffff), Hex(0xdbeafe)])
                .sway(14.0)
                .opacity(0.9),
            layer(
                Snowflake,
                Fall,
                false,
                5,
                Top,
                (13.0, 20.0),
                (8000.0, 12000.0),
                &[Hex(0xffffff), Hex(0xbfdbfe), Ring],
            )
            .spin(200.0)
            .sway(18.0)
            .opacity(0.9),
        ],
        "bubbles" => vec![
            layer(Bubble, Pop, true, 12, Edges, (20.0, 38.0), (1000.0, 1600.0), &[Ring, Hex(0x38bdf8), Primary])
                .delay(0.0, 600.0)
                .opacity(0.9),
            layer(Bubble, Rise, false, 8, Bottom, (14.0, 28.0), (6000.0, 9500.0), &[Ring, Hex(0x38bdf8), Primary])
                .sway(12.0)
                .opacity(0.8),
        ],
        "fireflies" => vec![
            layer(Dot, Pop, true, 10, Edges, (12.0, 18.0), (900.0, 1300.0), &[Hex(0xfacc15), Hex(0xa3e635), Primary])
                .delay(0.0, 700.0)
                .glow(),
            layer(
                Dot,
                Drift,
                false,
                11,
                Edges,
                (11.0, 16.0),
                (4500.0, 7500.0),
                &[Hex(0xfacc15), Hex(0xa3e635), Primary],
            )
            .sway(26.0)
            .glow(),
        ],
        "confetti" => vec![
            layer(Confetti, Burst, true, 24, Top, (14.0, 20.0), (1600.0, 2400.0), &confetti)
                .delay(0.0, 200.0)
                .spin(720.0)
                .sway(300.0)
                .flip(),
            layer(Confetti, Fall, false, 8, Top, (12.0, 17.0), (6000.0, 9000.0), &confetti)
                .spin(540.0)
                .sway(20.0)
                .flip()
                .opacity(0.9),
        ],
        _ => return None,
    };
    let id = IDS.into_iter().find(|i| *i == id)?;
    Some(Spec { id: id.to_owned(), name: String::new(), description: String::new(), layers })
}

/// The most layers a spec keeps.
pub const MAX_LAYERS: usize = 6;

/// Lowercase letters, digits and dashes, up to 32: what the server takes.
pub fn is_effect_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
}

impl Motion {
    fn by_name(name: &str) -> Option<Motion> {
        Some(match name {
            "fall" => Motion::Fall,
            "rise" => Motion::Rise,
            "twinkle" => Motion::Twinkle,
            "drift" => Motion::Drift,
            "burst" => Motion::Burst,
            "shoot" => Motion::Shoot,
            "pop" => Motion::Pop,
            _ => return None,
        })
    }
}

impl Region {
    fn by_name(name: &str) -> Option<Region> {
        Some(match name {
            "top" => Region::Top,
            "bottom" => Region::Bottom,
            "edges" => Region::Edges,
            "corners" => Region::Corners,
            "anywhere" => Region::Anywhere,
            "top-left" => Region::TopLeft,
            "top-right" => Region::TopRight,
            "center" => Region::Center,
            _ => return None,
        })
    }
}

impl Paint {
    fn by_name(name: &str) -> Option<Paint> {
        Some(match name {
            "primary" => Paint::Primary,
            "ring" => Paint::Ring,
            "accent" => Paint::Accent,
            "foreground" => Paint::Foreground,
            "card" => Paint::Card,
            "profile" => Paint::Profile,
            hex if hex.len() == 7 && hex.starts_with('#') => {
                let digits = &hex[1..];
                if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                Paint::Hex(u32::from_str_radix(digits, 16).ok()?)
            }
            _ => return None,
        })
    }
}

/// A spec from anywhere (an instance's or a server's profile item, a file
/// someone made), made safe to play, as the web's `sanitizeEffect`: only
/// known shapes, motions, regions and colors, and every number held to a
/// range. None when nothing is left to play.
pub fn sanitize(raw: &serde_json::Value) -> Option<Spec> {
    use serde_json::Value;
    let r = raw.as_object()?;
    let id = r.get("id")?.as_str().filter(|id| is_effect_id(id))?;
    let num = |v: Option<&Value>, min: f64, max: f64| v.and_then(Value::as_f64).map(|n| n.clamp(min, max) as f32);
    let range = |v: Option<&Value>, min: f64, max: f64| {
        let list = v?.as_array().filter(|a| a.len() == 2)?;
        let a = num(list.first(), min, max)?;
        let b = num(list.get(1), min, max)?;
        Some((a.min(b), a.max(b)))
    };
    let layers: Vec<Layer> = r
        .get("layers")
        .and_then(Value::as_array)
        .map(|l| l.iter().take(MAX_LAYERS).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|l| {
            let x = l.as_object()?;
            let text = |k: &str| x.get(k).and_then(Value::as_str);
            let shape = text("shape").and_then(Shape::by_name)?;
            let motion = text("motion").and_then(Motion::by_name)?;
            let intro = match text("phase")? {
                "intro" => true,
                "idle" => false,
                _ => return None,
            };
            let from = text("from").and_then(Region::by_name)?;
            let count = num(x.get("count"), 0.0, MAX_PER_LAYER as f64).filter(|c| *c != 0.0)?;
            let size = range(x.get("size"), 2.0, 96.0)?;
            let duration = range(x.get("duration"), 300.0, 20000.0)?;
            let colors: Vec<Paint> = x
                .get("colors")
                .and_then(Value::as_array)
                .map(|c| c.iter().take(32).filter_map(|c| c.as_str().and_then(Paint::by_name)).take(8).collect())
                .unwrap_or_default();
            if colors.is_empty() {
                return None;
            }
            Some(Layer {
                shape,
                motion,
                intro,
                count: count.round() as u32,
                from,
                size,
                duration,
                delay: range(x.get("delay"), 0.0, 3000.0).unwrap_or((0.0, 0.0)),
                colors,
                spin: num(x.get("spin"), -1440.0, 1440.0).unwrap_or(0.0),
                sway: num(x.get("sway"), 0.0, 400.0).unwrap_or(0.0),
                flip: x.get("flip") == Some(&Value::Bool(true)),
                glow: x.get("glow") == Some(&Value::Bool(true)),
                opacity: num(x.get("opacity"), 0.05, 1.0).unwrap_or(1.0),
            })
        })
        .collect();
    if layers.is_empty() {
        return None;
    }
    let text = |k: &str, max: usize| {
        r.get(k).and_then(Value::as_str).map(|s| s.trim().chars().take(max).collect::<String>()).unwrap_or_default()
    };
    let name = text("name", 40);
    Some(Spec {
        id: id.to_owned(),
        name: if name.trim().is_empty() { id.to_owned() } else { name.trim().to_owned() },
        description: text("description", 120).trim().to_owned(),
        layers,
    })
}

/// One point a particle passes through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub offset: f32,
    /// The particle's top left corner, in pixels from the card's.
    pub x: f32,
    pub y: f32,
    /// Degrees.
    pub rotate: f32,
    pub scale: f32,
    /// Stretch along x only (streaks).
    pub scale_x: f32,
    /// Degrees around the x axis (paper tumbling), drawn as a squash.
    pub flip: Option<f32>,
    pub opacity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    Linear,
    EaseInOut,
    EaseOut,
    Bezier(f32, f32, f32, f32),
}

#[derive(Debug, Clone)]
pub struct Particle {
    pub shape: Shape,
    pub size: f32,
    pub paint: Paint,
    pub glow: bool,
    pub intro: bool,
    pub frames: Vec<Frame>,
    pub duration: f32,
    pub delay: f32,
    /// Where in its loop an idle particle starts (0 to 1).
    pub start: f32,
    pub easing: Easing,
    /// Where it sits when nothing may move (reduced motion).
    pub still: Option<Frame>,
}

/// FNV-1a, for seeding.
fn hash(text: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for c in text.encode_utf16() {
        h ^= u32::from(c);
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// mulberry32: the same sequence for the same seed as the web's.
pub fn random(seed: &str) -> impl FnMut() -> f32 + use<> {
    let mut a = hash(seed) as i32;
    move || {
        a = a.wrapping_add(0x6d2b79f5_u32 as i32);
        let mut t = (a ^ ((a as u32) >> 15) as i32).wrapping_mul(1 | a);
        t = (t.wrapping_add((t ^ ((t as u32) >> 7) as i32).wrapping_mul(61 | t))) ^ t;
        (((t ^ ((t as u32) >> 14) as i32) as u32) as f64 / 4294967296.0) as f32
    }
}

fn round(n: f32) -> f32 {
    (n * 10.0).round() / 10.0
}

/// Turns a spec into particles for a card of this size.
pub fn plan(spec: &Spec, width: f32, height: f32, seed: &str) -> Vec<Particle> {
    let (w, h) = (width.max(40.0), height.max(40.0));
    let area = ((w * h) / (REFERENCE.0 * REFERENCE.1)).sqrt().clamp(0.35, 1.4);
    let reach = (w / REFERENCE.0).min(h / REFERENCE.1).clamp(0.35, 1.4);
    let mut particles = Vec::new();
    for (n, layer) in spec.layers.iter().enumerate() {
        let mut rnd = random(&format!("{seed}:{}:{n}", spec.id));
        let count = MAX_PER_LAYER.min((layer.count as f32 * area).round() as u32);
        let peak = layer.opacity;
        for _ in 0..count {
            if particles.len() >= MAX_PARTICLES {
                break;
            }
            let between = |(a, b): (f32, f32), r: f32| a + (b - a) * r;
            let size = round(between(layer.size, rnd()) * (w / 240.0).clamp(0.55, 1.0));
            let half = size / 2.0;
            let (x0, y0) = start(layer.from, w, h, size, &mut rnd);
            let sign = if rnd() < 0.5 { -1.0 } else { 1.0 };
            let spin = layer.spin * sign * (0.5 + rnd() * 0.5);
            let r0 = rnd() * 360.0 * if layer.spin != 0.0 { 1.0 } else { 0.0 };
            let sway = layer.sway * reach;
            let flips = layer.flip.then(|| 1.0 + (rnd() * 3.0).floor());
            let flip_at = |k: f32| flips.map(|f| k * f * 360.0);
            let mut frames = Vec::new();
            let frame =
                |frames: &mut Vec<Frame>, offset: f32, x: f32, y: f32, rotate: f32, scale: f32, opacity: f32| {
                    frames.push(Frame {
                        offset,
                        x: round(x - half),
                        y: round(y - half),
                        rotate: round(rotate),
                        scale: (scale * 100.0).round() / 100.0,
                        scale_x: 1.0,
                        flip: flip_at(offset),
                        opacity: (opacity * 100.0).round() / 100.0,
                    })
                };
            let mut easing = Easing::Linear;
            let mut still = None;
            match layer.motion {
                Motion::Fall | Motion::Rise => {
                    let down = layer.motion == Motion::Fall;
                    let (top, bottom) = (-size, h + size);
                    let shift = rnd() * std::f32::consts::TAU;
                    for k in 0..=4 {
                        let t = k as f32 / 4.0;
                        let y = if down { top + (bottom - top) * t } else { bottom - (bottom - top) * t };
                        let x = x0 + (shift + t * std::f32::consts::TAU).sin() * sway;
                        let o = if k == 0 || k == 4 {
                            0.0
                        } else if k == 3 {
                            peak * 0.8
                        } else {
                            peak
                        };
                        frame(&mut frames, t, x, y, r0 + spin * t, 1.0, o);
                    }
                    let r = rnd();
                    let sy = if down { (0.02 + 0.16 * r) * h } else { (0.82 + 0.16 * r) * h };
                    still = Some(Frame {
                        offset: 0.0,
                        x: round(x0 - half),
                        y: round(sy - half),
                        rotate: round(r0),
                        scale: 1.0,
                        scale_x: 1.0,
                        flip: flip_at(0.1),
                        opacity: peak * 0.8,
                    });
                }
                Motion::Twinkle => {
                    if layer.intro {
                        frame(&mut frames, 0.0, x0, y0, r0, 0.0, 0.0);
                        frame(&mut frames, 0.18, x0, y0, r0 + spin * 0.3, 1.15, peak);
                        frame(&mut frames, 0.3, x0, y0, r0 + spin * 0.4, 1.0, peak);
                        frame(&mut frames, 0.75, x0, y0, r0 + spin * 0.8, 1.0, peak * 0.9);
                        frame(&mut frames, 1.0, x0, y0, r0 + spin, 0.0, 0.0);
                    } else {
                        let glint = 0.3 + 0.2 * rnd();
                        frame(&mut frames, 0.0, x0, y0, r0, 0.0, 0.0);
                        frame(&mut frames, glint * 0.45, x0, y0, r0 + spin * 0.5, 1.0, peak);
                        frame(&mut frames, glint, x0, y0, r0 + spin, 0.0, 0.0);
                        frame(&mut frames, 1.0, x0, y0, r0 + spin, 0.0, 0.0);
                    }
                    easing = Easing::EaseInOut;
                    still = Some(Frame {
                        offset: 0.0,
                        x: round(x0 - half),
                        y: round(y0 - half),
                        rotate: round(r0),
                        scale: 0.85,
                        scale_x: 1.0,
                        flip: None,
                        opacity: peak * 0.75,
                    });
                }
                Motion::Drift => {
                    let points: Vec<(f32, f32)> =
                        (0..4).map(|_| (x0 + (rnd() * 2.0 - 1.0) * sway, y0 + (rnd() * 2.0 - 1.0) * sway)).collect();
                    let glow = [0.25, 1.0, 0.4, 0.9];
                    for (k, (x, y)) in points.iter().enumerate() {
                        frame(
                            &mut frames,
                            k as f32 / 4.0,
                            *x,
                            *y,
                            0.0,
                            if k % 2 == 1 { 1.0 } else { 0.8 },
                            peak * glow[k],
                        );
                    }
                    frame(&mut frames, 1.0, points[0].0, points[0].1, 0.0, 0.8, peak * glow[0]);
                    easing = Easing::EaseInOut;
                    still = Some(Frame {
                        offset: 0.0,
                        x: round(x0 - half),
                        y: round(y0 - half),
                        rotate: 0.0,
                        scale: 1.0,
                        scale_x: 1.0,
                        flip: None,
                        opacity: peak * 0.7,
                    });
                }
                Motion::Burst => {
                    let aim = (h / 2.0 - y0).atan2(w / 2.0 - x0);
                    let angle = aim + (rnd() * 2.0 - 1.0) * (std::f32::consts::PI / 2.6);
                    let dist = sway * (0.45 + rnd() * 0.55);
                    let (x1, y1) = (x0 + angle.cos() * dist, y0 + angle.sin() * dist);
                    frame(&mut frames, 0.0, x0, y0, r0, 0.3, 0.0);
                    frame(&mut frames, 0.08, x0 + (x1 - x0) * 0.2, y0 + (y1 - y0) * 0.2, r0 + spin * 0.15, 1.0, peak);
                    frame(&mut frames, 0.55, x1, y1, r0 + spin * 0.6, 1.0, peak);
                    frame(&mut frames, 1.0, x1 + angle.cos() * dist * 0.15, y1 + h * 0.16, r0 + spin, 0.85, 0.0);
                    easing = Easing::Bezier(0.16, 1.0, 0.3, 1.0);
                }
                Motion::Shoot => {
                    let angle = std::f32::consts::PI * (0.8 + rnd() * 0.08);
                    let dist = w * 0.8;
                    let (dx, dy) = (angle.cos() * dist, angle.sin() * dist);
                    let deg = angle.to_degrees();
                    let sx = w * (0.55 + rnd() * 0.4);
                    let sy = h * (0.02 + rnd() * 0.12);
                    let span = if layer.intro { 1.0 } else { 0.2 };
                    let mut streak = |t: f32, scale: f32, opacity: f32| {
                        frames.push(Frame {
                            offset: t,
                            x: round(sx + dx * t / span - half),
                            y: round(sy + dy * t / span - half),
                            rotate: round(deg),
                            scale: 1.0,
                            scale_x: scale,
                            flip: None,
                            opacity: (opacity * 100.0).round() / 100.0,
                        })
                    };
                    streak(0.0, 0.2, 0.0);
                    streak(span * 0.25, 1.0, peak);
                    streak(span, 0.4, 0.0);
                    if span < 1.0
                        && let Some(last) = frames.last().copied()
                    {
                        frames.push(Frame { offset: 1.0, ..last });
                    }
                    easing = Easing::EaseOut;
                }
                Motion::Pop => {
                    frame(&mut frames, 0.0, x0, y0, r0, 0.0, 0.0);
                    frame(&mut frames, 0.3, x0, y0 - size * 0.4, r0 + spin * 0.4, 1.1, peak);
                    frame(&mut frames, 0.8, x0, y0 - size * 0.8, r0 + spin * 0.8, 1.0, peak);
                    frame(&mut frames, 1.0, x0, y0 - size, r0 + spin, 1.3, 0.0);
                    easing = Easing::Bezier(0.34, 1.56, 0.64, 1.0);
                }
            }
            let duration = between(layer.duration, rnd()).round();
            let pick = layer.colors[((rnd() * layer.colors.len() as f32) as usize).min(layer.colors.len() - 1)];
            let delay = if layer.intro { between(layer.delay, rnd()).round() } else { 0.0 };
            let start = if layer.intro { 0.0 } else { (rnd() * 100.0).round() / 100.0 };
            particles.push(Particle {
                shape: layer.shape,
                size,
                paint: pick,
                glow: layer.glow,
                intro: layer.intro,
                frames,
                duration,
                delay,
                start,
                easing,
                still,
            });
        }
    }
    particles
}

/// Where a particle starts, in pixels from the card's top left.
fn start(from: Region, w: f32, h: f32, size: f32, rnd: &mut impl FnMut() -> f32) -> (f32, f32) {
    let band = 0.16;
    match from {
        Region::Top => (rnd() * w, -size),
        Region::Bottom => (w * (0.2 + rnd() * 0.6), h + size * 0.5),
        Region::TopLeft => {
            let x = -size + rnd() * w * 0.12;
            (x, -size + rnd() * h * 0.08)
        }
        Region::TopRight => {
            let x = w + size - rnd() * w * 0.12;
            (x, -size + rnd() * h * 0.08)
        }
        Region::Center => {
            let x = w * (0.4 + rnd() * 0.2);
            (x, h * (0.4 + rnd() * 0.2))
        }
        Region::Corners => {
            let right = rnd() < 0.5;
            let low = rnd() < 0.5;
            let x = if right { w * (1.0 - rnd() * 0.22) } else { w * rnd() * 0.22 };
            (x, if low { h * (1.0 - rnd() * 0.18) } else { h * rnd() * 0.18 })
        }
        Region::Edges => {
            let side = rnd();
            if side < 0.3 {
                let x = rnd() * w;
                (x, rnd() * h * band)
            } else if side < 0.45 {
                let x = rnd() * w;
                (x, h * (1.0 - rnd() * band * 0.7))
            } else if side < 0.725 {
                let x = rnd() * w * band;
                (x, rnd() * h)
            } else {
                let x = w * (1.0 - rnd() * band);
                (x, rnd() * h)
            }
        }
        Region::Anywhere => {
            let x = rnd() * w;
            (x, rnd() * h)
        }
    }
}

/// How long the intro runs, in milliseconds.
pub fn intro_length(particles: &[Particle]) -> f32 {
    particles.iter().filter(|p| p.intro).map(|p| p.delay + p.duration).fold(0.0, f32::max)
}

/// When idle particles fade in: as the intro winds down, so the two overlap.
pub fn idle_fade_at(particles: &[Particle]) -> f32 {
    (intro_length(particles) * 0.45).round()
}

fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    // Solve for t where the curve's x is `x`, then give its y.
    let curve = |a: f32, b: f32, t: f32| 3.0 * a * t * (1.0 - t).powi(2) + 3.0 * b * t * t * (1.0 - t) + t.powi(3);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut t = x;
    for _ in 0..24 {
        let cx = curve(x1, x2, t);
        if (cx - x).abs() < 1e-4 {
            break;
        }
        if cx < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    curve(y1, y2, t)
}

impl Easing {
    pub fn apply(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Easing::Linear => x,
            Easing::EaseInOut => bezier(0.42, 0.0, 0.58, 1.0, x),
            Easing::EaseOut => bezier(0.0, 0.0, 0.58, 1.0, x),
            Easing::Bezier(a, b, c, d) => bezier(a, b, c, d, x),
        }
    }
}

impl Particle {
    /// Where the particle is `elapsed` ms after the card opened, or None while it's not showing.
    /// `fade` is the idle loop's fade-in time.
    pub fn at(&self, elapsed: f32, fade_at: f32) -> Option<Frame> {
        let (progress, fade) = if self.intro {
            let local = (elapsed - self.delay) / self.duration.max(1.0);
            if !(0.0..=1.0).contains(&local) {
                return None;
            }
            (local, 1.0)
        } else {
            let fade = ((elapsed - fade_at) / IDLE_FADE_MS).clamp(0.0, 1.0);
            if fade <= 0.0 {
                return None;
            }
            ((self.start + elapsed / self.duration.max(1.0)).fract(), Easing::EaseOut.apply(fade))
        };
        let t = self.easing.apply(progress);
        // An easing that overshoots (a pop) runs past the ends: carry on along the end segment, as CSS does.
        let last = self.frames.len().checked_sub(2)?;
        let i = if t < self.frames[0].offset {
            0
        } else if t > self.frames[last + 1].offset {
            last
        } else {
            self.frames.windows(2).position(|w| t >= w[0].offset && t <= w[1].offset)?
        };
        let (a, b) = (self.frames[i], self.frames[i + 1]);
        let k = if b.offset > a.offset { (t - a.offset) / (b.offset - a.offset) } else { 0.0 };
        let mix = |x: f32, y: f32| x + (y - x) * k;
        Some(Frame {
            offset: t,
            x: mix(a.x, b.x),
            y: mix(a.y, b.y),
            rotate: mix(a.rotate, b.rotate),
            scale: mix(a.scale, b.scale),
            scale_x: mix(a.scale_x, b.scale_x),
            flip: a.flip.zip(b.flip).map(|(x, y)| mix(x, y)),
            opacity: mix(a.opacity, b.opacity).clamp(0.0, 1.0) * fade,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_plans_the_same_effect() {
        let sakura = spec("sakura").unwrap();
        let a = plan(&sakura, 304.0, 420.0, "01ALICE");
        let b = plan(&sakura, 304.0, 420.0, "01ALICE");
        assert_eq!(a.len(), b.len());
        assert_eq!(a[3].frames, b[3].frames);
        assert!(a.len() <= MAX_PARTICLES);
        let c = plan(&sakura, 304.0, 420.0, "01BOB");
        assert_ne!(a[3].frames, c[3].frames);
        assert!(spec("nope").is_none());
        for id in IDS {
            assert!(!plan(&spec(id).unwrap(), 120.0, 150.0, "x").is_empty(), "{id}");
        }
    }

    #[test]
    fn specs_from_elsewhere_are_held_to_the_format() {
        let raw = serde_json::json!({
            "id": "mine", "name": "  Mine ", "layers": [
                { "shape": "star", "motion": "pop", "phase": "intro", "count": 99, "from": "edges",
                  "size": [200, 1], "duration": [100, 100], "colors": ["#ff00aa", "nope", "profile"], "spin": 9000 },
                { "shape": "cube", "motion": "pop", "phase": "intro", "count": 3, "from": "edges",
                  "size": [2, 4], "duration": [400, 500], "colors": ["primary"] }
            ]
        });
        let s = sanitize(&raw).unwrap();
        assert_eq!((s.id.as_str(), s.name.as_str()), ("mine", "Mine"));
        assert_eq!(s.layers.len(), 1);
        let l = &s.layers[0];
        assert_eq!((l.count, l.size, l.duration, l.spin), (24, (2.0, 96.0), (300.0, 300.0), 1440.0));
        assert_eq!(l.colors, vec![Paint::Hex(0xff00aa), Paint::Profile]);
        assert!(sanitize(&serde_json::json!({ "id": "Bad Id", "layers": [] })).is_none());
        assert!(sanitize(&serde_json::json!({ "id": "empty", "layers": [] })).is_none());
    }

    #[test]
    fn mulberry_matches_the_web() {
        // random("a") in the web's lib/effects/profile.ts.
        let mut r = random("a");
        let first = r();
        assert!((0.0..1.0).contains(&first));
    }

    #[test]
    fn particles_show_only_while_they_play() {
        let p = plan(&spec("sparkles").unwrap(), 304.0, 420.0, "s");
        let intro = p.iter().find(|p| p.intro).unwrap();
        assert!(intro.at(intro.delay + intro.duration + 10.0, 0.0).is_none());
        assert!(intro.at(intro.delay + intro.duration / 2.0, 0.0).is_some());
        let idle = p.iter().find(|p| !p.intro).unwrap();
        assert!(idle.at(0.0, 500.0).is_none());
    }
}
