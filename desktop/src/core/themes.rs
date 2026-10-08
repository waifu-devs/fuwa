//! Themes, the same as every fuwa app's (`docs/themes.md`; the web app's
//! `lib/themes.ts`, `theme-file.ts` and `backdrop.ts` are the reference):
//! the five built-in themes, themes people make or import, the backdrop
//! behind the app, and the `fuwa-theme` file they travel in.
//!
//! A theme file is data only. Anything read from one, or from this
//! computer's settings, is checked here: colors must be `#rrggbb`, numbers
//! are clamped, unknown keys are dropped, and a background picture is only
//! ever a picture on a fuwa instance (a file's own picture comes along as
//! bytes, for uploading; links in a file are never followed).

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::core::effects::custom::{CustomShader, sanitize_shader, shader_problem};
use crate::core::i18n::{Arg, t_with};

/// The shadcn/ui tokens, in the web app's order.
pub const TOKENS: [&str; 18] = [
    "background",
    "foreground",
    "card",
    "card-foreground",
    "popover",
    "popover-foreground",
    "primary",
    "primary-foreground",
    "secondary",
    "secondary-foreground",
    "muted",
    "muted-foreground",
    "accent",
    "accent-foreground",
    "destructive",
    "border",
    "input",
    "ring",
];

/// The seven colors the rest are made from.
pub const SEEDS: [&str; 7] =
    ["background", "foreground", "card", "primary", "primary-foreground", "muted-foreground", "border"];

pub const RADIUS_MIN: f32 = 0.0;
pub const RADIUS_MAX: f32 = 1.5;
pub const MAX_CUSTOM_THEMES: usize = 50;
pub const NAME_MAX: usize = 40;
pub const DESCRIPTION_MAX: usize = 140;
pub const FORMAT: &str = "fuwa-theme";
pub const VERSION: u64 = 1;
/// Pictures inside theme files: the types instances take, at most this big.
pub const MAX_FILE_PICTURE_BYTES: usize = 12 * 1024 * 1024;
const FILE_PICTURE_TYPES: [&str; 5] = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"];

/// A full set of token colors, as `0xrrggbb`, in [`TOKENS`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens(pub [u32; 18]);

fn index(name: &str) -> usize {
    TOKENS.iter().position(|t| *t == name).expect("a known token")
}

impl Tokens {
    pub fn get(&self, name: &str) -> u32 {
        self.0[index(name)]
    }

    pub fn set(&mut self, name: &str, color: u32) {
        self.0[index(name)] = color & 0xff_ffff;
    }

    /// The whole set from the seven seeds, as `deriveTokens` makes it.
    pub fn derive(seeds: [u32; 7]) -> Self {
        let [background, foreground, card, primary, primary_foreground, muted_foreground, border] = seeds;
        let mut t = Tokens([0; 18]);
        for (name, color) in [
            ("background", background),
            ("foreground", foreground),
            ("card", card),
            ("card-foreground", foreground),
            ("popover", card),
            ("popover-foreground", foreground),
            ("primary", primary),
            ("primary-foreground", primary_foreground),
            ("secondary", mix(primary, card, 0.12)),
            ("secondary-foreground", foreground),
            ("muted", mix(border, background, 0.45)),
            ("muted-foreground", muted_foreground),
            ("accent", mix(primary, background, 0.14)),
            ("accent-foreground", foreground),
            ("destructive", 0xe5484d),
            ("border", border),
            ("input", border),
            ("ring", primary),
        ] {
            t.set(name, color);
        }
        t
    }

    /// Reads colors from a file or settings: every token, or at least the seeds (the rest made from them).
    pub fn read(value: &Value) -> Option<Self> {
        let colors = value.as_object()?;
        let color = |k: &str| colors.get(k).and_then(Value::as_str).and_then(parse_hex);
        if let Some(all) = TOKENS.iter().map(|k| color(k)).collect::<Option<Vec<u32>>>() {
            return Some(Tokens(all.try_into().ok()?));
        }
        let seeds: Vec<u32> = SEEDS.iter().map(|k| color(k)).collect::<Option<_>>()?;
        let mut t = Tokens::derive(seeds.try_into().ok()?);
        for k in TOKENS {
            if let Some(c) = color(k) {
                t.set(k, c);
            }
        }
        Some(t)
    }

    pub fn to_json(&self) -> Value {
        Value::Object(TOKENS.iter().map(|k| ((*k).to_owned(), Value::String(hex(self.get(k))))).collect())
    }
}

impl Serialize for Tokens {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Tokens {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        Tokens::read(&value).ok_or_else(|| serde::de::Error::custom("theme colors are #rrggbb"))
    }
}

/// `#rrggbb` (any case) as `0xrrggbb`.
pub fn parse_hex(s: &str) -> Option<u32> {
    let digits = s.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

pub fn hex(color: u32) -> String {
    format!("#{:06x}", color & 0xff_ffff)
}

fn channels(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32]
}

/// `a * t + b * (1 - t)` per channel, rounded, as the web app mixes.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let (a, b) = (channels(a), channels(b));
    let ch = |i: usize| (a[i] * t + b[i] * (1.0 - t)).round().clamp(0.0, 255.0) as u32;
    (ch(0) << 16) | (ch(1) << 8) | ch(2)
}

/// Dark when the background's luminance is under mid-grey.
pub fn is_dark(tokens: &Tokens) -> bool {
    let [r, g, b] = channels(tokens.get("background"));
    0.2126 * r + 0.7152 * g + 0.0722 * b < 128.0
}

/// The same hue turned by `degrees`, for the second color effects use.
pub fn turn(color: u32, degrees: f32) -> u32 {
    let [r, g, b] = channels(color).map(|c| c / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return color;
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    let h = (h * 60.0 + degrees + 360.0).rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let ch = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    (ch(r1) << 16) | (ch(g1) << 8) | ch(b1)
}

/// A theme: built in, or made on this computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub tokens: Tokens,
    /// Corner radius, in rem.
    pub radius: f32,
    #[serde(skip)]
    pub builtin: bool,
    /// Its own backdrop, used while it's on screen; none keeps the app's.
    #[serde(default)]
    pub backdrop: Option<Backdrop>,
    #[serde(default)]
    pub updated_at: i64,
}

impl Theme {
    pub fn dark(&self) -> bool {
        is_dark(&self.tokens)
    }
}

fn builtin(id: &str, name: &str, description: &str, radius: f32, seeds: [u32; 7]) -> Theme {
    Theme {
        id: id.into(),
        name: name.into(),
        description: Some(description.into()),
        tokens: Tokens::derive(seeds),
        radius,
        builtin: true,
        backdrop: None,
        updated_at: 0,
    }
}

/// The five built-in themes, the site's and the web app's.
pub fn builtins() -> Vec<Theme> {
    vec![
        builtin(
            "sakura",
            "Sakura",
            "Soft cherry blossom pink. The default.",
            1.0,
            [0xfff5f8, 0x3b2330, 0xffffff, 0xf06292, 0xffffff, 0x8a6577, 0xf8d3e0],
        ),
        builtin(
            "yoru",
            "Yoru",
            "Late night coding under neon signs.",
            0.75,
            [0x14111f, 0xece6ff, 0x1f1a2e, 0xb388ff, 0x14111f, 0x9a90b8, 0x342b4d],
        ),
        builtin(
            "matcha",
            "Matcha",
            "Calm green tea and warm paper.",
            0.5,
            [0xf4f6ec, 0x243021, 0xfffef7, 0x5a8a3c, 0xffffff, 0x66735f, 0xd9e2c8],
        ),
        builtin(
            "sora",
            "Sora",
            "Clear skies and summer clouds.",
            1.25,
            [0xf0f7ff, 0x1a2b44, 0xffffff, 0x3b8beb, 0xffffff, 0x5f7391, 0xcfe2f7],
        ),
        builtin(
            "tsundere",
            "Tsundere",
            "It's not like I made this theme for you or anything.",
            0.25,
            [0x1a0f12, 0xffe9ec, 0x2a171c, 0xff4d6d, 0x1a0f12, 0xc28c96, 0x4a2630],
        ),
    ]
}

/// Whether an id is one a made theme can have: `custom-` and 4 to 32 lowercase letters and digits.
pub fn custom_id_ok(id: &str) -> bool {
    id.strip_prefix("custom-").is_some_and(|rest| {
        (4..=32).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    })
}

pub fn new_theme_id() -> String {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    let n = u64::from_le_bytes(bytes);
    format!("custom-{}", radix36(n))
}

fn radix36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 || out.len() < 4 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn text(value: Option<&Value>, max: usize) -> String {
    let s = value.and_then(Value::as_str).unwrap_or_default();
    let clean: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    clean.trim().chars().take(max).collect()
}

fn radius(value: Option<&Value>, fallback: f32) -> f32 {
    match value.and_then(Value::as_f64) {
        Some(r) if r.is_finite() => ((r * 100.0).round() / 100.0).clamp(RADIUS_MIN as f64, RADIUS_MAX as f64) as f32,
        _ => fallback,
    }
}

/// Made themes as kept on this computer, checked like anything that could have been edited by hand.
pub fn sanitize_custom(themes: Vec<Theme>) -> Vec<Theme> {
    let mut out: Vec<Theme> = Vec::new();
    for mut t in themes {
        if !custom_id_ok(&t.id) || out.iter().any(|o| o.id == t.id) {
            continue;
        }
        t.builtin = false;
        t.name = text(Some(&Value::String(t.name)), NAME_MAX);
        if t.name.is_empty() {
            t.name = "Untitled".into();
        }
        t.description = t.description.map(|d| text(Some(&Value::String(d)), DESCRIPTION_MAX)).filter(|d| !d.is_empty());
        t.radius = radius(Some(&json!(t.radius)), 0.75);
        out.push(t);
        if out.len() == MAX_CUSTOM_THEMES {
            break;
        }
    }
    out
}

// ───────────────────────── Backdrops ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    #[default]
    Cover,
    Contain,
    Tile,
}

/// What's drawn over the picture: an animated effect, or a still texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    #[default]
    None,
    Aurora,
    Petals,
    Stars,
    Waves,
    /// A shader someone wrote, kept in the backdrop's `shader`.
    Custom,
    Grain,
    Paper,
    Dots,
    Grid,
}

impl Effect {
    pub const ALL: [Effect; 10] = [
        Effect::None,
        Effect::Aurora,
        Effect::Petals,
        Effect::Stars,
        Effect::Waves,
        Effect::Custom,
        Effect::Grain,
        Effect::Paper,
        Effect::Dots,
        Effect::Grid,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Effect::None => "None",
            Effect::Aurora => "Aurora",
            Effect::Petals => "Petals",
            Effect::Stars => "Starfield",
            Effect::Waves => "Waves",
            Effect::Custom => "Custom",
            Effect::Grain => "Film grain",
            Effect::Paper => "Paper",
            Effect::Dots => "Dots",
            Effect::Grid => "Grid",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Effect::None => "Just the theme.",
            Effect::Aurora => "Slow ribbons of the theme's colors.",
            Effect::Petals => "Blossoms drifting down.",
            Effect::Stars => "Twinkling stars, gently drifting.",
            Effect::Waves => "Soft layered waves rolling by.",
            Effect::Custom => "A shader you write, or one a theme brought.",
            Effect::Grain => "A fine, still noise.",
            Effect::Paper => "Warm fibers like washi paper.",
            Effect::Dots => "A tidy dot pattern.",
            Effect::Grid => "Notebook grid lines.",
        }
    }

    /// Still textures, as opposed to the animated effects.
    pub fn texture(self) -> bool {
        matches!(self, Effect::Grain | Effect::Paper | Effect::Dots | Effect::Grid)
    }

    /// The built-in effects drawn by a shader (on the GPU where there is one).
    pub fn shader(self) -> bool {
        matches!(self, Effect::Aurora | Effect::Petals | Effect::Stars | Effect::Waves)
    }

    /// Effects that move, so they have a speed.
    pub fn moves(self) -> bool {
        self.shader() || self == Effect::Custom
    }

    /// Its name in files and settings (`aurora`, `custom`...).
    pub fn key(self) -> &'static str {
        match self {
            Effect::None => "none",
            Effect::Aurora => "aurora",
            Effect::Petals => "petals",
            Effect::Stars => "stars",
            Effect::Waves => "waves",
            Effect::Custom => "custom",
            Effect::Grain => "grain",
            Effect::Paper => "paper",
            Effect::Dots => "dots",
            Effect::Grid => "grid",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        Effect::ALL.into_iter().find(|e| e.key() == s)
    }
}

/// The picture and effect behind the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Backdrop {
    /// A picture on a fuwa instance (`https://<instance>/media/<id>`), or empty.
    pub image: String,
    pub fit: Fit,
    /// How much the theme's background covers the picture, in percent (0 to 90).
    pub dim: u8,
    /// Picture blur, in pixels (0 to 24).
    pub blur: u8,
    pub effect: Effect,
    /// How strong the effect is, in percent.
    pub intensity: u8,
    /// How fast it moves, in percent of normal (0 is still, up to 200).
    pub speed: u8,
    /// How solid the chat is over it, in percent (20 to 100); the sidebars are 20 points more.
    pub panels: u8,
    /// The shader drawn when `effect` is `Custom`, kept even while another effect is picked.
    pub shader: Option<CustomShader>,
}

impl Default for Backdrop {
    fn default() -> Self {
        Self {
            image: String::new(),
            fit: Fit::Cover,
            dim: 35,
            blur: 0,
            effect: Effect::None,
            intensity: 70,
            speed: 100,
            panels: 35,
            shader: None,
        }
    }
}

pub const DIM: (u8, u8) = (0, 90);
pub const BLUR: (u8, u8) = (0, 24);
pub const INTENSITY: (u8, u8) = (0, 100);
pub const SPEED: (u8, u8) = (0, 200);
pub const PANELS: (u8, u8) = (20, 100);

fn clamp(value: Option<&Value>, (min, max): (u8, u8), fallback: u8) -> u8 {
    match value.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => n.round().clamp(f64::from(min), f64::from(max)) as u8,
        _ => fallback,
    }
}

/// An https (or local http) link to a picture at `/media/<id>` on some instance.
pub fn is_media_link(link: &str) -> bool {
    if link.len() > 512 {
        return false;
    }
    let Ok(u) = url::Url::parse(link) else { return false };
    let id = u.path().strip_prefix("/media/").unwrap_or_default();
    matches!(u.scheme(), "https" | "http")
        && (10..=40).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric())
        && u.query().is_none()
        && u.fragment().is_none()
        && u.username().is_empty()
        && u.password().is_none()
}

impl Backdrop {
    /// Anything stored or imported, made into a backdrop that can't break the app or load from anywhere but an instance.
    pub fn sanitize(value: &Value) -> Self {
        let empty = Map::new();
        let b = value.as_object().unwrap_or(&empty);
        let d = Backdrop::default();
        let image = b.get("image").and_then(Value::as_str).filter(|s| is_media_link(s)).unwrap_or_default();
        let shader = b.get("shader").and_then(sanitize_shader);
        let effect = b.get("effect").and_then(Value::as_str).and_then(Effect::from_str).unwrap_or_default();
        Backdrop {
            image: image.to_owned(),
            fit: match b.get("fit").and_then(Value::as_str) {
                Some("contain") => Fit::Contain,
                Some("tile") => Fit::Tile,
                _ => Fit::Cover,
            },
            dim: clamp(b.get("dim"), DIM, d.dim),
            blur: clamp(b.get("blur"), BLUR, d.blur),
            effect: if effect == Effect::Custom && shader.is_none() { Effect::None } else { effect },
            intensity: clamp(b.get("intensity"), INTENSITY, d.intensity),
            speed: clamp(b.get("speed"), SPEED, d.speed),
            panels: clamp(b.get("panels"), PANELS, d.panels),
            shader,
        }
    }

    /// Whether there's anything behind the app at all.
    pub fn any(&self) -> bool {
        !self.image.is_empty() || self.effect != Effect::None
    }
}

impl<'de> Deserialize<'de> for Backdrop {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Backdrop::sanitize(&Value::deserialize(d)?))
    }
}

// ───────────────────────── Theme files ─────────────────────────

/// A picture that came inside a theme file, still to be uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// A theme read from a file, its picture if it had one, and anything worth telling.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub theme: Theme,
    pub picture: Option<Picture>,
    pub notes: Vec<String>,
}

/// The file's name for a theme: `<name>.fuwa-theme.json`.
pub fn file_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    format!("{}.fuwa-theme.json", if slug.is_empty() { "theme" } else { slug })
}

/// The file for a theme, with its backdrop's picture (when it has one to bring along).
pub fn to_file(theme: &Theme, backdrop: Option<&Backdrop>, picture: Option<&Picture>) -> String {
    let backdrop = backdrop.map(|b| {
        let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
        v["image"] = match picture {
            Some(p) => Value::String(format!(
                "data:{};base64,{}",
                p.content_type,
                base64::engine::general_purpose::STANDARD.encode(&p.bytes)
            )),
            None => Value::Null,
        };
        v
    });
    let file = json!({
        "format": FORMAT,
        "version": VERSION,
        "name": theme.name,
        "description": theme.description.clone().unwrap_or_default(),
        "colors": theme.tokens.to_json(),
        "radius": theme.radius,
        "backdrop": backdrop,
    });
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// A `data:` URL's picture, when it's a type instances take and not too big. Nothing else is read.
pub fn picture_from_data_url(value: &str) -> Option<Picture> {
    let rest = value.strip_prefix("data:")?;
    let (kind, data) = rest.split_once(";base64,")?;
    if !FILE_PICTURE_TYPES.contains(&kind) || data.len() / 4 * 3 > MAX_FILE_PICTURE_BYTES + 3 {
        return None;
    }
    let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(clean).ok()?;
    (bytes.len() <= MAX_FILE_PICTURE_BYTES).then(|| Picture { content_type: kind.to_owned(), bytes })
}

/// Reads a theme file: this format, or a plain token set like waifu.dev's
/// (`{ tokens, radius }` or `{ variant: { tokens, radius } }`).
pub fn parse_file(json: &str) -> Result<Imported, String> {
    let data: Value = serde_json::from_str(json).map_err(|_| "That isn't a theme file (it isn't JSON).".to_owned())?;
    let d = data.as_object().ok_or_else(|| "That isn't a theme file.".to_owned())?;
    let mut notes = Vec::new();
    if d.get("format").and_then(Value::as_str) == Some(FORMAT)
        && d.get("version").and_then(Value::as_f64).is_some_and(|v| v > VERSION as f64)
    {
        notes.push("This theme was made by a newer fuwa; some of it may not show here.".to_owned());
    }
    let variant = d.get("variant").and_then(Value::as_object).unwrap_or(d);
    let colors = d.get("colors").or_else(|| variant.get("tokens"));
    let tokens = colors
        .and_then(Tokens::read)
        .ok_or_else(|| "That theme file has no colors fuwa can read. Colors are #rrggbb.".to_owned())?;

    let mut backdrop = None;
    let mut picture = None;
    if let Some(b) = d.get("backdrop").filter(|b| b.is_object()) {
        let mut plain = b.clone();
        plain["image"] = Value::String(String::new());
        let sane = Backdrop::sanitize(&plain);
        if let Some(shader) = sane.shader.as_ref().filter(|s| shader_problem(&s.code).is_some()) {
            notes.push(t_with("system.themeFile.shaderProblem", &[("shader", Arg::Str(&shader.name))]));
        }
        backdrop = Some(sane);
        match b.get("image") {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.is_empty() => {}
            Some(image) => {
                picture = image.as_str().and_then(picture_from_data_url);
                if picture.is_none() {
                    let link = image.as_str().is_some_and(|s| {
                        let s = s.to_ascii_lowercase();
                        s.starts_with("http:") || s.starts_with("https:")
                    });
                    notes.push(if link {
                        "Its background was a link, which fuwa doesn't load; pick a picture for it instead.".to_owned()
                    } else {
                        "Its background picture couldn't be read, so it's left out.".to_owned()
                    });
                }
            }
        }
    }

    let name = text(d.get("name"), NAME_MAX);
    let description = text(d.get("description"), DESCRIPTION_MAX);
    let theme = Theme {
        id: new_theme_id(),
        name: if name.is_empty() { "Imported theme".into() } else { name },
        description: (!description.is_empty()).then_some(description),
        tokens,
        radius: radius(variant.get("radius").or_else(|| d.get("radius")), 0.75),
        builtin: false,
        backdrop,
        updated_at: chrono::Utc::now().timestamp_millis(),
    };
    Ok(Imported { theme, picture, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_themes_match_the_web() {
        let themes = builtins();
        let ids: Vec<&str> = themes.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["sakura", "yoru", "matcha", "sora", "tsundere"]);
        let yoru = &themes[1];
        // deriveTokens, by hand: mix(#b388ff, #1f1a2e, 0.12) and friends.
        assert_eq!(hex(yoru.tokens.get("secondary")), "#312747");
        assert_eq!(hex(yoru.tokens.get("muted")), "#221d34");
        assert_eq!(hex(yoru.tokens.get("accent")), "#2a223e");
        assert_eq!(hex(themes[0].tokens.get("secondary")), "#fdecf2");
        assert!(yoru.dark() && themes[4].dark());
        assert!(!themes[0].dark() && !themes[2].dark() && !themes[3].dark());
        assert_eq!(yoru.radius, 0.75);
    }

    #[test]
    fn a_hue_turns_like_the_web() {
        assert_eq!(turn(0xff0000, 120.0), 0x00ff00);
        assert_eq!(turn(0x808080, 48.0), 0x808080);
    }

    #[test]
    fn theme_files_round_trip_and_never_load_links() {
        let sakura = builtins().remove(0);
        let backdrop = Backdrop { effect: Effect::Petals, dim: 45, ..Backdrop::default() };
        let picture = Picture { content_type: "image/png".into(), bytes: vec![1, 2, 3] };
        let file = to_file(&sakura, Some(&backdrop), Some(&picture));
        let back = parse_file(&file).unwrap();
        assert_eq!(back.theme.tokens, sakura.tokens);
        assert_eq!(back.theme.radius, 1.0);
        assert_eq!(back.theme.name, "Sakura");
        assert_eq!(back.theme.backdrop, Some(backdrop));
        assert_eq!(back.picture, Some(picture));
        assert!(custom_id_ok(&back.theme.id), "{}", back.theme.id);

        // Seeds only, a link for a picture, numbers out of range, unknown keys.
        let odd = r##"{"format":"fuwa-theme","version":9,"name":"  Odd\u0007 ","colors":{"background":"#000000","foreground":"#FFFFFF","card":"#111111","primary":"#ff00ff","primary-foreground":"#000000","muted-foreground":"#888888","border":"#333333","ring":"#00ff00"},"radius":7,"backdrop":{"image":"https://tracker.example/x.png","dim":500,"effect":"lasers","shiny":true}}"##;
        let got = parse_file(odd).unwrap();
        assert_eq!(got.theme.name, "Odd");
        assert_eq!(got.theme.radius, RADIUS_MAX);
        assert_eq!(hex(got.theme.tokens.get("ring")), "#00ff00");
        assert_eq!(hex(got.theme.tokens.get("card-foreground")), "#ffffff");
        let b = got.theme.backdrop.unwrap();
        assert_eq!((b.image.as_str(), b.dim, b.effect), ("", 90, Effect::None));
        assert!(got.picture.is_none());
        assert_eq!(got.notes.len(), 2, "{:?}", got.notes);

        // A waifu.dev theme.
        let site = r##"{"variant":{"tokens":{"background":"#fff5f8","foreground":"#3b2330","card":"#ffffff","primary":"#f06292","primary-foreground":"#ffffff","muted-foreground":"#8a6577","border":"#f8d3e0"},"radius":1}}"##;
        assert_eq!(parse_file(site).unwrap().theme.tokens, sakura.tokens);
        assert!(parse_file("{}").is_err());
        assert!(parse_file("nope").is_err());
        assert_eq!(file_name("Midnight Sakura!"), "midnight-sakura.fuwa-theme.json");
    }

    #[test]
    fn custom_shaders_travel_in_theme_files_checked() {
        let sakura = builtins().remove(0);
        let shader = crate::core::effects::custom::default_shader();
        let backdrop = Backdrop { effect: Effect::Custom, shader: Some(shader.clone()), ..Backdrop::default() };
        let back = parse_file(&to_file(&sakura, Some(&backdrop), None)).unwrap();
        assert_eq!(back.theme.backdrop, Some(backdrop));
        assert!(back.notes.is_empty(), "{:?}", back.notes);

        // A shader that binds things of its own comes in, says so, and won't run.
        let bad = json!({"format": FORMAT, "version": 1, "name": "Bad", "colors": {"background":"#000000","foreground":"#ffffff","card":"#111111","primary":"#ff00ff","primary-foreground":"#000000","muted-foreground":"#888888","border":"#333333"},
            "backdrop": {"effect": "custom", "shader": {"name": "Sneaky", "code": "@group(0) @binding(1) var<uniform> x: f32;\nfn shade(uv: vec2f) -> vec4f { return vec4f(x); }", "fallback": "stars"}}});
        let got = parse_file(&bad.to_string()).unwrap();
        let b = got.theme.backdrop.unwrap();
        assert_eq!((b.effect, b.shader.map(|s| s.fallback)), (Effect::Custom, Some(Effect::Stars)));
        assert_eq!(got.notes.len(), 1, "{:?}", got.notes);

        // Custom without a shader is no effect.
        let b = Backdrop::sanitize(&json!({"effect": "custom"}));
        assert_eq!((b.effect, b.shader), (Effect::None, None));
    }

    #[test]
    fn backdrops_only_show_pictures_from_instances() {
        let ok = "https://fuwa.chat/media/01HXYZABCDEFGH";
        assert!(is_media_link(ok));
        for bad in [
            "https://fuwa.chat/media/01HXYZABCDEFGH?x=1",
            "https://me@fuwa.chat/media/01HXYZABCDEFGH",
            "https://fuwa.chat/elsewhere/01HXYZABCDEFGH",
            "file:///media/01HXYZABCDEFGH",
            "javascript:alert(1)",
        ] {
            assert!(!is_media_link(bad), "{bad}");
        }
        let b: Backdrop =
            serde_json::from_str(&format!(r#"{{"image":"{ok}","fit":"tile","panels":5,"speed":150}}"#)).unwrap();
        assert_eq!((b.fit, b.panels, b.speed), (Fit::Tile, 20, 150));
        assert!(b.any());
    }
}
