//! fuwa's look: the web app's palettes (`web/src/styles/app.css`), its font,
//! and the app's appearance settings, applied to GPUI Kit's theme so its
//! components match the rest of the app.

use std::borrow::Cow;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, WindowAppearance, px, rgb};

use crate::core::config::{MotionChoice, Prefs, ThemeChoice};

/// The bundled font, with Japanese: what the site and the web app use.
/// M PLUS Rounded 1c, as its files name their family.
pub const FONT: &str = "Rounded Mplus 1c";

pub fn load_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../../assets/fonts/MPLUSRounded1c-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../../assets/fonts/MPLUSRounded1c-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../../assets/fonts/MPLUSRounded1c-ExtraBold.ttf")),
    ];
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("couldn't load the bundled font: {err}");
    }
}

/// One of fuwa's palettes.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub background: Rgba,
    pub foreground: Rgba,
    pub card: Rgba,
    pub primary: Rgba,
    pub primary_foreground: Rgba,
    pub secondary: Rgba,
    pub muted: Rgba,
    pub muted_foreground: Rgba,
    pub accent: Rgba,
    pub destructive: Rgba,
    pub border: Rgba,
    /// The far-left rail and the sidebar sit a little darker than the page.
    pub rail: Rgba,
    pub sidebar: Rgba,
    /// A second color the primary blends into, for gradients.
    pub glow: Rgba,
    pub success: Rgba,
}

pub const LIGHT: Palette = Palette {
    dark: false,
    background: rgba(0xfff5f8),
    foreground: rgba(0x3b2330),
    card: rgba(0xffffff),
    primary: rgba(0xf06292),
    primary_foreground: rgba(0xffffff),
    secondary: rgba(0xfdedf2),
    muted: rgba(0xfbe5ec),
    muted_foreground: rgba(0x8a6577),
    accent: rgba(0xfde5ed),
    destructive: rgba(0xe5484d),
    border: rgba(0xf8d3e0),
    rail: rgba(0xf6dbe5),
    sidebar: rgba(0xfdebf1),
    glow: rgba(0x7dd3fc),
    success: rgba(0x2fb47c),
};

pub const DARK: Palette = Palette {
    dark: true,
    background: rgba(0x14111f),
    foreground: rgba(0xece6ff),
    card: rgba(0x1f1a2e),
    primary: rgba(0xb388ff),
    primary_foreground: rgba(0x14111f),
    secondary: rgba(0x2a2240),
    muted: rgba(0x241e36),
    muted_foreground: rgba(0x9a90b8),
    accent: rgba(0x2a2240),
    destructive: rgba(0xe5484d),
    border: rgba(0x342b4d),
    rail: rgba(0x0e0c16),
    sidebar: rgba(0x19152a),
    glow: rgba(0x7dd3fc),
    success: rgba(0x4ade80),
};

const fn rgba(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// The palette in use, kept as a global so views can read it.
pub struct Current(pub Palette);

impl gpui_kit::Global for Current {}

pub fn palette(cx: &App) -> Palette {
    cx.try_global::<Current>().map(|c| c.0).unwrap_or(DARK)
}

/// Mixes two colors, `t` of the way from `a` to `b`.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Hsla {
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
        .into()
}

pub fn alpha(c: Rgba, a: f32) -> Hsla {
    Rgba { a, ..c }.into()
}

/// A color for something without a picture (a server, a person), from its id.
pub fn tint(seed: &str) -> Rgba {
    const TINTS: [u32; 8] = [0xf06292, 0xb388ff, 0x7dd3fc, 0x4ade80, 0xfbbf24, 0xfb923c, 0x60a5fa, 0xf472b6];
    let hash = seed.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    rgb(TINTS[(hash as usize) % TINTS.len()])
}

/// Applies the app's settings: light or dark (or the system's), the font,
/// the text size, and whether things move.
pub fn apply(prefs: &Prefs, appearance: WindowAppearance, cx: &mut App) {
    let dark = match prefs.theme {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
    };
    let p = if dark { DARK } else { LIGHT };
    cx.set_global(Current(p));
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    Theme::update(cx, |t| {
        t.font_family = FONT.into();
        t.font_size = px(15.0 * prefs.text_scale.clamp(0.8, 1.4));
        t.radius = px(10.0);
        t.radius_lg = px(16.0);
        let c = &mut t.colors;
        c.background = p.background.into();
        c.foreground = p.foreground.into();
        c.border = p.border.into();
        c.input = p.border.into();
        c.ring = p.primary.into();
        c.caret = p.primary.into();
        c.selection = alpha(p.primary, 0.3);
        c.primary = p.primary.into();
        c.primary_hover = mix(p.primary, p.foreground, 0.12);
        c.primary_active = mix(p.primary, p.foreground, 0.2);
        c.primary_foreground = p.primary_foreground.into();
        c.secondary = p.secondary.into();
        c.secondary_hover = mix(p.secondary, p.primary, 0.12);
        c.secondary_active = mix(p.secondary, p.primary, 0.2);
        c.secondary_foreground = p.foreground.into();
        c.muted = p.muted.into();
        c.muted_foreground = p.muted_foreground.into();
        c.accent = p.accent.into();
        c.accent_foreground = p.foreground.into();
        c.popover = p.card.into();
        c.popover_foreground = p.foreground.into();
        c.danger = p.destructive.into();
        c.danger_hover = mix(p.destructive, p.foreground, 0.12);
        c.danger_active = mix(p.destructive, p.foreground, 0.2);
        c.danger_foreground = rgb(0xffffff).into();
        c.button_primary = p.primary.into();
        c.button_primary_hover = mix(p.primary, p.foreground, 0.12);
        c.button_primary_active = mix(p.primary, p.foreground, 0.2);
        c.button_primary_foreground = p.primary_foreground.into();
        c.switch = p.muted.into();
        c.link = p.primary.into();
        c.link_hover = mix(p.primary, p.foreground, 0.2);
        c.scrollbar = alpha(p.background, 0.0);
        c.scrollbar_thumb = alpha(p.muted_foreground, 0.35);
        c.scrollbar_thumb_hover = alpha(p.muted_foreground, 0.6);
        c.sidebar = p.sidebar.into();
        c.list_hover = alpha(p.primary, 0.08);
        c.list_active = alpha(p.primary, 0.14);
        c.title_bar = p.rail.into();
        c.title_bar_border = p.border.into();
    });
    match prefs.motion {
        MotionChoice::System => gpui_kit::base::apply_system_reduce_motion(cx),
        MotionChoice::Reduced => cx.set_reduce_motion(true),
        MotionChoice::Full => cx.set_reduce_motion(false),
    }
}
