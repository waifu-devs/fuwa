//! The color side of the Accessibility settings (`docs/themes.md`,
//! "Accessibility"), the web's `lib/accessibility.ts` in Rust: correcting
//! colors for color blindness, and the high contrast version of any theme.

use serde::{Deserialize, Serialize};

use crate::core::themes::{Tokens, mix};

/// Which kind of color blindness the app's colors are corrected for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorVision {
    #[default]
    Off,
    Protanopia,
    Deuteranopia,
    Tritanopia,
}

/// A 3×3 matrix over sRGB channels (0 to 1), row by row.
pub type Matrix = [f32; 9];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// How each kind sees sRGB (Machado, Oliveira and Fernandes 2009, severity 1).
pub fn seen_by(kind: ColorVision) -> Matrix {
    match kind {
        ColorVision::Off => IDENTITY,
        ColorVision::Protanopia => {
            [0.152286, 1.052583, -0.204868, 0.114503, 0.786281, 0.099216, -0.003882, -0.048116, 1.051998]
        }
        ColorVision::Deuteranopia => {
            [0.367322, 0.860646, -0.227968, 0.280085, 0.672501, 0.047413, -0.01182, 0.04294, 0.968881]
        }
        ColorVision::Tritanopia => {
            [1.255528, -0.076749, -0.178779, -0.078411, 0.930809, 0.147602, 0.004733, 0.691367, 0.3039]
        }
    }
}

/// Where the difference someone can't see goes: red-green into green and
/// blue, blue-yellow into red and green.
fn shift(kind: ColorVision) -> Matrix {
    match kind {
        ColorVision::Off => [0.0; 9],
        ColorVision::Protanopia | ColorVision::Deuteranopia => [0.0, 0.0, 0.0, 0.7, 1.0, 0.0, 0.7, 0.0, 1.0],
        ColorVision::Tritanopia => [1.0, 0.0, 0.7, 0.0, 1.0, 0.7, 0.0, 0.0, 0.0],
    }
}

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

/// The correction (daltonizing): each color plus the part of it this kind
/// loses, moved where it can be seen, `strength` (0 to 1) of the way.
/// `I + strength · SHIFT · (I − SEEN)`.
pub fn color_vision_matrix(kind: ColorVision, strength: f32) -> Matrix {
    if kind == ColorVision::Off || strength <= 0.0 {
        return IDENTITY;
    }
    let seen = seen_by(kind);
    let lost: Matrix = std::array::from_fn(|i| IDENTITY[i] - seen[i]);
    let moved = multiply(&shift(kind), &lost);
    std::array::from_fn(|i| IDENTITY[i] + strength * moved[i])
}

/// Channels (0 to 1) through a matrix, clamped.
pub fn apply(m: &Matrix, [r, g, b]: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| (m[i * 3] * r + m[i * 3 + 1] * g + m[i * 3 + 2] * b).clamp(0.0, 1.0))
}

fn channels(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0]
}

/// A `0xrrggbb` color through a matrix.
pub fn apply_hex(m: &Matrix, color: u32) -> u32 {
    let ch = apply(m, channels(color)).map(|v| (v * 255.0).round() as u32);
    (ch[0] << 16) | (ch[1] << 8) | ch[2]
}

// ───────────────────────── High contrast ─────────────────────────

const BLACK: u32 = 0x000000;
const WHITE: u32 = 0xffffff;

/// WCAG relative luminance.
pub fn luminance(color: u32) -> f32 {
    let [r, g, b] = channels(color).map(|c| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The color as near as it can stay to itself with at least `ratio`
/// against every color in `against`: mixed toward black or white
/// (whichever stands out more), as little as it takes; if even that can't,
/// black or white.
pub fn ensure_contrast(color: u32, against: &[u32], ratio: f32) -> u32 {
    let worst = |c: u32| against.iter().map(|a| contrast(c, *a)).fold(f32::INFINITY, f32::min);
    if worst(color) >= ratio {
        return color;
    }
    let target = if worst(BLACK) >= worst(WHITE) { BLACK } else { WHITE };
    if worst(target) < ratio {
        return target;
    }
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if worst(mix(target, color, mid)) >= ratio {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix(target, color, hi)
}

/// A theme's tokens in high contrast: surfaces halfway to black (dark
/// themes) or white (light ones), text at 12:1 against them, quiet text at
/// 7:1, buttons, links and danger at 4.5:1, and lines and focus rings at 3:1.
pub fn high_contrast(t: &Tokens) -> Tokens {
    let dark = luminance(t.get("background")) < luminance(t.get("foreground"));
    let deep = |c: u32| mix(if dark { BLACK } else { WHITE }, c, 0.5);
    let background = deep(t.get("background"));
    let card = deep(t.get("card"));
    let popover = deep(t.get("popover"));
    let surfaces = [background, card];
    let accent = ensure_contrast(t.get("accent"), &[background], 1.5);
    let primary = ensure_contrast(t.get("primary"), &surfaces, 4.5);
    let (secondary, muted) = (t.get("secondary"), t.get("muted"));
    let mut out = *t;
    for (name, color) in [
        ("background", background),
        ("foreground", ensure_contrast(t.get("foreground"), &surfaces, 12.0)),
        ("card", card),
        ("card-foreground", ensure_contrast(t.get("card-foreground"), &[card], 12.0)),
        ("popover", popover),
        ("popover-foreground", ensure_contrast(t.get("popover-foreground"), &[popover], 12.0)),
        ("primary", primary),
        ("primary-foreground", ensure_contrast(t.get("primary-foreground"), &[primary], 7.0)),
        ("secondary-foreground", ensure_contrast(t.get("secondary-foreground"), &[secondary], 7.0)),
        ("muted-foreground", ensure_contrast(t.get("muted-foreground"), &[background, card, muted], 7.0)),
        ("accent", accent),
        ("accent-foreground", ensure_contrast(t.get("accent-foreground"), &[accent], 7.0)),
        ("destructive", ensure_contrast(t.get("destructive"), &surfaces, 4.5)),
        ("border", ensure_contrast(t.get("border"), &surfaces, 3.0)),
        ("input", ensure_contrast(t.get("input"), &surfaces, 3.0)),
        ("ring", ensure_contrast(t.get("ring"), &surfaces, 3.0)),
    ] {
        out.set(name, color);
    }
    out
}

// ───────────────────────── Encryption marks ─────────────────────────

/// What end-to-end encryption is drawn with (the web's `SealColors`): green,
/// and amber when a safety number changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealColors {
    /// A pill or note's own solid surface, and pointed at.
    pub surface: u32,
    pub surface_hover: u32,
    /// Text and icons on that surface.
    pub ink: u32,
    /// Its outline, so it stands apart from whatever is behind it.
    pub edge: u32,
    /// A mark on its own, on the theme's card or background.
    pub icon: u32,
    pub warn_surface: u32,
    pub warn_surface_hover: u32,
    pub warn_ink: u32,
    pub warn_edge: u32,
    pub warn_icon: u32,
}

const SEAL_GREEN: u32 = 0x00bc7d;
const SEAL_AMBER: u32 = 0xfe9a00;

/// The encryption marks in a theme. They sit over see-through panels and
/// backdrop pictures, so each draws on a solid surface of its own (the
/// theme's card, tinted), never on what's behind it, with text at 4.5:1
/// against that surface and lone icons at 3:1 against the card and the
/// background, whatever the theme. The web's `sealColors` does the same math.
pub fn seal_colors(t: &Tokens) -> SealColors {
    let (card, background) = (t.get("card"), t.get("background"));
    let dark = luminance(card) < luminance(t.get("card-foreground"));
    let paint = |hue: u32, ink: u32, icon: u32| {
        let surface = mix(hue, card, 0.14);
        let surface_hover = mix(hue, card, 0.22);
        (
            surface,
            surface_hover,
            ensure_contrast(ink, &[surface, surface_hover], 4.5),
            ensure_contrast(mix(hue, card, 0.45), &[surface], 2.0),
            ensure_contrast(icon, &[card, background], 3.0),
        )
    };
    let (surface, surface_hover, ink, edge, icon) =
        paint(SEAL_GREEN, if dark { 0x5ee9b5 } else { 0x007a55 }, if dark { 0x00d492 } else { 0x009966 });
    let (warn_surface, warn_surface_hover, warn_ink, warn_edge, warn_icon) =
        paint(SEAL_AMBER, if dark { 0xffd230 } else { 0xbb4d00 }, if dark { 0xffb900 } else { 0xe17100 });
    SealColors {
        surface,
        surface_hover,
        ink,
        edge,
        icon,
        warn_surface,
        warn_surface_hover,
        warn_ink,
        warn_edge,
        warn_icon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::themes::builtins;

    fn distance(a: u32, b: u32) -> f32 {
        let (x, y) = (channels(a), channels(b));
        ((0..3).map(|i| ((x[i] - y[i]) * 255.0).powi(2)).sum::<f32>()).sqrt()
    }

    #[test]
    fn off_changes_nothing() {
        assert_eq!(apply_hex(&color_vision_matrix(ColorVision::Off, 1.0), 0xf06292), 0xf06292);
        assert_eq!(apply_hex(&color_vision_matrix(ColorVision::Deuteranopia, 0.0), 0xf06292), 0xf06292);
    }

    #[test]
    fn greys_stay_grey() {
        for kind in [ColorVision::Protanopia, ColorVision::Deuteranopia, ColorVision::Tritanopia] {
            for grey in [0x000000, 0x808080, 0xffffff] {
                assert!(distance(apply_hex(&color_vision_matrix(kind, 1.0), grey), grey) <= 3.0, "{kind:?} {grey:06x}");
            }
        }
    }

    #[test]
    fn the_correction_pulls_apart_colors_each_kind_confuses() {
        for (kind, a, b) in [
            (ColorVision::Protanopia, 0xe5484d, 0x2fb47c),
            (ColorVision::Deuteranopia, 0xe5484d, 0x2fb47c),
            (ColorVision::Tritanopia, 0x3b8beb, 0x5a8a3c),
        ] {
            let (seen, m) = (seen_by(kind), color_vision_matrix(kind, 1.0));
            let before = distance(apply_hex(&seen, a), apply_hex(&seen, b));
            let after = distance(apply_hex(&seen, apply_hex(&m, a)), apply_hex(&seen, apply_hex(&m, b)));
            assert!(after > before * 1.2, "{kind:?}: {before} -> {after}");
        }
    }

    #[test]
    fn ensure_contrast_reaches_the_ratio_or_the_extreme() {
        assert_eq!(ensure_contrast(0x000000, &[0xffffff], 4.5), 0x000000);
        assert!(contrast(ensure_contrast(0xf06292, &[0xfff5f8], 4.5), 0xfff5f8) >= 4.5);
        assert_eq!(ensure_contrast(0x444444, &[0x444444], 30.0), 0xffffff);
        assert_eq!(ensure_contrast(0x999999, &[0x999999], 30.0), 0x000000);
    }

    #[test]
    fn every_builtin_theme_meets_the_high_contrast_ratios() {
        for theme in builtins() {
            let t = high_contrast(&theme.tokens);
            for s in [t.get("background"), t.get("card")] {
                assert!(contrast(t.get("foreground"), s) >= 12.0, "{} text", theme.id);
                assert!(contrast(t.get("muted-foreground"), s) >= 7.0, "{} quiet text", theme.id);
                assert!(contrast(t.get("primary"), s) >= 4.5, "{} primary", theme.id);
                assert!(contrast(t.get("border"), s) >= 3.0, "{} border", theme.id);
            }
            assert!(contrast(t.get("primary-foreground"), t.get("primary")) >= 4.5, "{} on primary", theme.id);
        }
    }

    /// The web app's `lib/accessibility.test.ts` pins the same values, so both apps paint alike.
    #[test]
    fn seal_marks_read_on_every_theme() {
        let mut themes: Vec<Tokens> = builtins().iter().flat_map(|t| [t.tokens, high_contrast(&t.tokens)]).collect();
        // Custom themes can be anything: mid greys, the seal's own green, loud yellow.
        for (card, fg, background) in
            [(0x808080, 0xffffff, 0x707070), (0x00bc7d, 0x000000, 0x00bc7d), (0xffff00, 0x000000, 0xffffff)]
        {
            let mut t = builtins()[0].tokens;
            t.set("card", card);
            t.set("card-foreground", fg);
            t.set("background", background);
            themes.push(t);
        }
        for t in &themes {
            let s = seal_colors(t);
            for (ink, surfaces, icon) in [
                (s.ink, [s.surface, s.surface_hover], s.icon),
                (s.warn_ink, [s.warn_surface, s.warn_surface_hover], s.warn_icon),
            ] {
                for surface in surfaces {
                    assert!(contrast(ink, surface) >= 4.5, "{:06x} text", t.get("card"));
                }
                assert!(contrast(icon, t.get("card")) >= 3.0 && contrast(icon, t.get("background")) >= 3.0);
            }
            assert!(contrast(s.edge, s.surface) >= 2.0);
        }
    }

    #[test]
    fn matches_the_web() {
        let shown = |t: Tokens| t.0.map(crate::core::themes::hex).join(" ");
        let all = builtins();
        let seal = |t: &Tokens| {
            let s = seal_colors(t);
            [
                s.surface,
                s.surface_hover,
                s.ink,
                s.edge,
                s.icon,
                s.warn_surface,
                s.warn_surface_hover,
                s.warn_ink,
                s.warn_edge,
                s.warn_icon,
            ]
            .map(crate::core::themes::hex)
            .join(" ")
        };
        assert_eq!(
            seal(&all[0].tokens),
            "#dbf6ed #c7f0e2 #007753 #73b9a2 #009966 #fff1db #ffe9c7 #b44a00 #cca870 #e07100"
        );
        assert_eq!(
            seal(&all[1].tokens),
            "#1b3139 #183e3f #5ee9b5 #166756 #00d492 #3e2c28 #503624 #ffd230 #835419 #ffb900"
        );
        assert_eq!(
            shown(high_contrast(&all[0].tokens)),
            "#fffafc #3b2330 #ffffff #3b2330 #ffffff #3b2330 #be4d73 #ffffff #fdecf2 #3b2330 #fce6ed #604753 #e3c9d2 #3b2330 #cf4146 #a48c95 #a48c95 #ee6191"
        );
        assert_eq!(
            shown(high_contrast(&all[1].tokens)),
            "#0a0910 #ece6ff #100d17 #ece6ff #100d17 #ece6ff #b388ff #13101e #312747 #ece6ff #221d34 #aea6c6 #332c46 #ece6ff #e5484d #625b76 #625b76 #b388ff"
        );
        for (kind, red, green) in [
            (ColorVision::Protanopia, 0xe593ab, 0x2f9352),
            (ColorVision::Deuteranopia, 0xe56295, 0x2fa658),
            (ColorVision::Tritanopia, 0xc0554d, 0x2ea57c),
        ] {
            assert_eq!(apply_hex(&color_vision_matrix(kind, 1.0), 0xe5484d), red, "{kind:?}");
            assert_eq!(apply_hex(&color_vision_matrix(kind, 0.5), 0x2fb47c), green, "{kind:?}");
        }
    }
}
