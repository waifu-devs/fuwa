//! fuwa's look: the theme on screen (`core/themes.rs`, the web app's), its font,
//! and the app's appearance settings, applied to GPUI Kit's theme so its
//! components match the rest of the app.

use std::borrow::Cow;

use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::component::{Theme as KitTheme, ThemeMode};
use gpui_kit::{App, Hsla, Pixels, Rgba, WindowAppearance, px, rgb};

use crate::core::config::{MotionChoice, Prefs};
use crate::core::themes::{self, Backdrop, Theme};

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

/// The colors views draw with: a theme's tokens and the app's surfaces made
/// from them (`docs/themes.md`).
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
    /// The server rail and the channel sidebar.
    pub rail: Rgba,
    pub sidebar: Rgba,
    /// The primary turned 48° in hue, for gradients and effects.
    pub glow: Rgba,
    pub success: Rgba,
    /// Corner radius, in rem (1 is the size the app's corners are drawn for).
    pub radius: f32,
    /// What the chat, the sidebars and the rail are painted with: solid, or
    /// see-through over a backdrop, as solid as its `panels` says.
    pub chat_surface: Hsla,
    pub side_surface: Hsla,
    pub rail_surface: Hsla,
}

impl Palette {
    pub fn of(theme: &Theme) -> Self {
        let t = &theme.tokens;
        let dark = theme.dark();
        let (primary, muted, background) = (t.get("primary"), t.get("muted"), t.get("background"));
        Palette {
            dark,
            background: rgba(background),
            foreground: rgba(t.get("foreground")),
            card: rgba(t.get("card")),
            primary: rgba(primary),
            primary_foreground: rgba(t.get("primary-foreground")),
            secondary: rgba(t.get("secondary")),
            muted: rgba(muted),
            muted_foreground: rgba(t.get("muted-foreground")),
            accent: rgba(t.get("accent")),
            destructive: rgba(t.get("destructive")),
            border: rgba(t.get("border")),
            rail: rgba(if dark { themes::mix(background, 0x000000, 0.78) } else { themes::mix(primary, muted, 0.09) }),
            sidebar: rgba(themes::mix(t.get("card"), background, 0.55)),
            glow: rgba(themes::turn(primary, 48.0)),
            success: rgba(if dark { 0x4ade80 } else { 0x2fb47c }),
            radius: theme.radius,
            chat_surface: rgba(background).into(),
            side_surface: rgba(themes::mix(t.get("card"), background, 0.55)).into(),
            rail_surface: rgba(if dark {
                themes::mix(background, 0x000000, 0.78)
            } else {
                themes::mix(primary, muted, 0.09)
            })
            .into(),
        }
    }

    /// The surfaces turned see-through over a backdrop (the sidebars 20 points more solid than the chat).
    pub fn over(mut self, backdrop: &Backdrop) -> Self {
        if backdrop.any() {
            let chat = f32::from(backdrop.panels) / 100.0;
            let side = (chat + 0.2).min(1.0);
            self.chat_surface = alpha(self.background, chat);
            self.side_surface = alpha(self.sidebar, side);
            self.rail_surface = alpha(self.rail, side);
        }
        self
    }
}

const fn rgba(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// The theme's corner radius, for corners drawn at 1 rem.
static CORNERS: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

/// A corner of `n` pixels at the usual radius, scaled to the theme's.
pub fn corner(n: f32) -> Pixels {
    px(n * f32::from_bits(CORNERS.load(Ordering::Relaxed)))
}

/// The web's Tailwind radii (app.css `@theme`): `rounded-lg` is the theme's
/// radius (1rem = 16px at radius 1), the others step from it.
pub fn radius_lg() -> Pixels {
    corner(16.0)
}
/// `rounded-sm`: the radius less 4px.
pub fn radius_sm() -> Pixels {
    px((f32::from(corner(16.0)) - 4.0).max(0.0))
}
/// `rounded-md`: the radius less 2px.
pub fn radius_md() -> Pixels {
    px((f32::from(corner(16.0)) - 2.0).max(0.0))
}
/// `rounded-xl`: the radius and 4px.
pub fn radius_xl() -> Pixels {
    px(f32::from(corner(16.0)) + 4.0)
}
/// `rounded-2xl`: the radius and 8px.
pub fn radius_2xl() -> Pixels {
    px(f32::from(corner(16.0)) + 8.0)
}
/// `rounded-3xl`: the radius and 16px.
pub fn radius_3xl() -> Pixels {
    px(f32::from(corner(16.0)) + 16.0)
}

/// Message text size, in pixels (`Prefs::chat_font_size`).
static CHAT_FONT: AtomicU32 = AtomicU32::new(15);

/// Whether names take their role's color (the Role colors setting).
static ROLE_NAMES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn role_names() -> bool {
    ROLE_NAMES.load(Ordering::Relaxed)
}

pub fn chat_font() -> f32 {
    CHAT_FONT.load(Ordering::Relaxed) as f32
}

/// A color with its saturation scaled by `s` (0 grey, 1 as it is), as CSS's `saturate()`.
fn saturate(c: Rgba, s: f32) -> Rgba {
    let (r, g, b) = (c.r, c.g, c.b);
    let m = |a: f32, b2: f32, c2: f32| -> [f32; 3] { [a, b2, c2] };
    let rows = [
        m(0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s),
        m(0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s),
        m(0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s),
    ];
    let at = |row: [f32; 3]| (row[0] * r + row[1] * g + row[2] * b).clamp(0.0, 1.0);
    Rgba { r: at(rows[0]), g: at(rows[1]), b: at(rows[2]), a: c.a }
}

impl Palette {
    /// The palette with every color's saturation scaled (the Saturation setting).
    pub fn saturated(mut self, s: f32) -> Self {
        if s >= 0.999 {
            return self;
        }
        for c in [
            &mut self.background,
            &mut self.foreground,
            &mut self.card,
            &mut self.primary,
            &mut self.primary_foreground,
            &mut self.secondary,
            &mut self.muted,
            &mut self.muted_foreground,
            &mut self.accent,
            &mut self.destructive,
            &mut self.border,
            &mut self.rail,
            &mut self.sidebar,
            &mut self.glow,
            &mut self.success,
        ] {
            *c = saturate(*c, s);
        }
        let hsla = |h: Hsla| -> Hsla { saturate(h.to_rgb(), s).into() };
        self.chat_surface = hsla(self.chat_surface);
        self.side_surface = hsla(self.side_surface);
        self.rail_surface = hsla(self.rail_surface);
        self
    }
}

/// The palette in use, kept as a global so views can read it.
pub struct Current(pub Palette);

impl gpui_kit::Global for Current {}

pub fn palette(cx: &App) -> Palette {
    cx.try_global::<Current>().map(|c| c.0).unwrap_or_else(|| Palette::of(&themes::builtins()[1]))
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

/// Whether the system is in dark mode.
pub fn system_dark(appearance: WindowAppearance) -> bool {
    matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// The backdrop on screen, kept as a global for the window to draw.
pub struct CurrentBackdrop(pub Backdrop);

impl gpui_kit::Global for CurrentBackdrop {}

pub fn backdrop(cx: &App) -> Backdrop {
    cx.try_global::<CurrentBackdrop>().map(|c| c.0.clone()).unwrap_or_default()
}

/// Applies the app's settings: the theme (or the system's light or dark
/// pick), its backdrop, the font, the text size, and whether things move.
pub fn apply(prefs: &Prefs, appearance: WindowAppearance, cx: &mut App) {
    let theme = prefs.active_theme(system_dark(appearance));
    let backdrop = prefs.active_backdrop(&theme);
    let p = Palette::of(&theme).over(&backdrop).saturated(f32::from(prefs.saturation) / 100.0);
    let dark = p.dark;
    CHAT_FONT.store(u32::from(prefs.chat_font_size.clamp(12, 20)), Ordering::Relaxed);
    ROLE_NAMES.store(prefs.role_colors == crate::core::config::RoleColors::Names, Ordering::Relaxed);
    CORNERS.store(p.radius.to_bits(), Ordering::Relaxed);
    crate::ui::text::set_clock(prefs.clock);
    cx.set_global(CurrentBackdrop(backdrop));
    cx.set_global(Current(p));
    KitTheme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    KitTheme::update(cx, |t| {
        t.font_family = FONT.into();
        t.font_size = px(15.0 * prefs.text_scale.clamp(0.8, 1.5));
        t.radius = corner(10.0);
        t.radius_lg = corner(16.0);
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
        c.slider_bar = p.primary.into();
        c.slider_thumb = p.card.into();
        t.tokens.slider_bar = Hsla::from(p.primary).into();
        t.tokens.slider_thumb = Hsla::from(p.card).into();
    });
    match prefs.motion {
        MotionChoice::System => gpui_kit::base::apply_system_reduce_motion(cx),
        MotionChoice::Reduced => cx.set_reduce_motion(true),
        MotionChoice::Full => cx.set_reduce_motion(false),
    }
}
