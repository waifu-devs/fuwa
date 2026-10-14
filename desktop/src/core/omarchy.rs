//! Omarchy's theme as one of fuwa's, on Linux computers that run Omarchy
//! (omarchy.org). Omarchy keeps the theme in use under
//! `~/.local/state/omarchy/current/theme` (older versions:
//! `~/.config/omarchy/current/theme`, a link to the theme's folder), with its
//! colors in `colors.toml`, or only in `alacritty.toml` for themes from before
//! that file. Those colors become a theme here, `omarchy`, offered next to the
//! built-in ones while Omarchy is there and nowhere else.
//!
//! Switching themes in Omarchy replaces that folder, so the app looks at it
//! every couple of seconds (`changed`): a few `stat`s, and the files are only
//! read again when one of them changed. Nothing read here leaves this
//! computer: the theme isn't synced to other devices (`core::settings_sync`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::Mutex;

use crate::core::i18n::{Arg, t_with};
use crate::core::themes::{Theme, Tokens, mix, parse_hex};

/// The theme's id, beside the built-in ones.
pub const ID: &str = "omarchy";
/// How often Omarchy's folder is looked at for a new theme.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// The folders Omarchy keeps the theme in use in, newest layout first.
fn currents(home: &Path) -> [PathBuf; 2] {
    [home.join(".local/state/omarchy/current"), home.join(".config/omarchy/current")]
}

#[cfg(target_os = "linux")]
fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

#[cfg(not(target_os = "linux"))]
fn home() -> Option<PathBuf> {
    None
}

/// Whether this computer runs Omarchy: always no off Linux.
pub fn installed() -> bool {
    home().is_some_and(|home| installed_at(&home))
}

fn installed_at(home: &Path) -> bool {
    currents(home).iter().any(|c| c.join("theme").is_dir())
}

/// What the theme's files were when last read: each one's inode and change time.
type Stamp = Vec<Option<(u64, SystemTime)>>;

fn stamp(home: &Path) -> Stamp {
    let mut out = Vec::new();
    for current in currents(home) {
        for file in ["theme", "theme/colors.toml", "theme/alacritty.toml", "theme/light.mode", "theme.name"] {
            out.push(std::fs::metadata(current.join(file)).ok().and_then(|m| Some((inode(&m), m.modified().ok()?))));
        }
    }
    out
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(m)
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0
}

struct Seen {
    stamp: Stamp,
    theme: Option<Theme>,
}

/// None until first asked.
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// Omarchy's theme now, when there's one to offer.
pub fn theme() -> Option<Theme> {
    let mut seen = SEEN.lock();
    if seen.is_none() {
        let home = home()?;
        *seen = Some(Seen { stamp: stamp(&home), theme: read_at(&home) });
    }
    let mut theme = seen.as_ref()?.theme.clone()?;
    theme.description =
        Some(t_with("desktop.look.omarchy", &[("theme", Arg::Str(theme.description.as_deref().unwrap_or_default()))]));
    Some(theme)
}

/// Looks for a new Omarchy theme; true when it changed. Halfway through a
/// switch the folder can be missing for a moment: the last theme stays then,
/// and the next look reads it again.
pub fn changed() -> bool {
    let Some(home) = home() else { return false };
    let now = stamp(&home);
    let mut seen = SEEN.lock();
    if seen.as_ref().is_some_and(|s| s.stamp == now) {
        return false;
    }
    let theme = read_at(&home);
    let before = seen.as_ref().and_then(|s| s.theme.clone());
    match theme {
        Some(theme) => {
            *seen = Some(Seen { stamp: now, theme: Some(theme.clone()) });
            before.as_ref() != Some(&theme)
        }
        None if !installed_at(&home) => {
            *seen = Some(Seen { stamp: now, theme: None });
            before.is_some()
        }
        None => false,
    }
}

/// The theme in use under a home folder, as one of fuwa's. Its name rides in
/// `description` until `theme` words it.
pub fn read_at(home: &Path) -> Option<Theme> {
    currents(home).iter().find_map(|current| {
        let dir = current.join("theme");
        let colors = std::fs::read_to_string(dir.join("colors.toml"))
            .ok()
            .map(|s| parse(&s))
            .filter(|c| c.contains_key("background") && c.contains_key("foreground"))
            .or_else(|| std::fs::read_to_string(dir.join("alacritty.toml")).ok().map(|s| from_alacritty(&parse(&s))))?;
        let light = dir.join("light.mode").exists();
        let mut theme = from_colors(&colors, light)?;
        theme.description = Some(name(current));
        Some(theme)
    })
}

/// The theme's name: `theme.name` beside the folder, or where the older link points.
fn name(current: &Path) -> String {
    let raw = std::fs::read_to_string(current.join("theme.name"))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let link = std::fs::read_link(current.join("theme")).ok()?;
            Some(link.file_name()?.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let words: Vec<String> = raw
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        })
        .collect();
    let out: String = words.join(" ").chars().filter(|c| !c.is_control()).take(40).collect();
    if out.is_empty() { "Omarchy".into() } else { out }
}

/// The `key = "value"` lines of a TOML file, keyed `section.key` under a
/// section. Only what these files hold: no arrays, tables inline or strings
/// over more than one line.
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            section = rest.split(']').next().unwrap_or_default().trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim().trim_matches(['"', '\'']);
        let value = value.trim();
        let value = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or_default(),
            _ => value.split('#').next().unwrap_or_default().trim(),
        };
        let key = if section.is_empty() { key.to_owned() } else { format!("{section}.{key}") };
        out.entry(key).or_insert_with(|| value.to_owned());
    }
    out
}

/// An `alacritty.toml`'s colors under the names `colors.toml` uses.
fn from_alacritty(a: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut put = |to: &str, from: &[&str]| {
        if let Some(v) = from.iter().find_map(|k| a.get(*k)) {
            out.insert(to.to_owned(), v.clone());
        }
    };
    put("background", &["colors.primary.background", "colors.normal.black"]);
    put("foreground", &["colors.primary.foreground", "colors.normal.white"]);
    put("accent", &["colors.normal.blue"]);
    put("red", &["colors.normal.red"]);
    put("selection", &["colors.selection.background"]);
    out
}

/// `#rrggbb` or `0xrrggbb`.
fn color(c: &BTreeMap<String, String>, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|k| {
        let v = c.get(*k)?;
        let digits = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")).unwrap_or(v);
        parse_hex(&format!("#{}", digits.trim_start_matches('#')))
    })
}

/// How far apart two colors look, as WCAG's contrast ratio.
fn contrast(a: u32, b: u32) -> f32 {
    let lum = |c: u32| {
        let ch = |s: u32| {
            let v = ((c >> s) & 0xff) as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Omarchy's colors made into a theme's seeds: its background and text, its
/// accent for what fuwa colors with `primary`, and the rest mixed from those.
fn from_colors(c: &BTreeMap<String, String>, light_file: bool) -> Option<Theme> {
    let background = color(c, &["background"])?;
    let foreground = color(c, &["foreground"])?;
    let accent = color(c, &["accent", "blue", "color4"]).unwrap_or(foreground);
    let dark = match c.get("mode").map(String::as_str) {
        Some("light") => false,
        Some("dark") => true,
        _ if light_file => false,
        _ => contrast(background, 0x000000) < contrast(background, 0xffffff),
    };
    let card = color(c, &["lighter_background"]).unwrap_or(if dark {
        mix(foreground, background, 0.06)
    } else {
        mix(0xffffff, background, 0.6)
    });
    let on_accent = if contrast(accent, background) >= contrast(accent, foreground) { background } else { foreground };
    let muted_foreground = mix(foreground, background, 0.62);
    let border = color(c, &["muted"]).unwrap_or_else(|| mix(foreground, background, 0.18));
    let mut tokens = Tokens::derive([background, foreground, card, accent, on_accent, muted_foreground, border]);
    if let Some(red) = color(c, &["red", "color1"]) {
        tokens.set("destructive", red);
    }
    Some(Theme {
        id: ID.into(),
        name: "Omarchy".into(),
        description: None,
        tokens,
        radius: 0.25,
        builtin: true,
        backdrop: None,
        updated_at: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::themes::hex;

    const TOKYO_NIGHT: &str = r##"mode = "dark"

accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"

background = "#1a1b26"
lighter_background = "#24283b"
foreground = "#a9b1d6"
red = "#f7768e"
"##;

    const OLD_ALACRITTY: &str = r##"[colors.primary]
background = '#eff1f5'
foreground = '#4c4f69' # text

[colors.normal]
black = "0xbcc0cc"
red = "0xd20f39"
blue = "0x1e66f5"
white = "0x5c5f77"
"##;

    fn home_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = home.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        home
    }

    #[test]
    fn colors_toml_makes_the_theme() {
        let home = home_with(&[
            (".local/state/omarchy/current/theme/colors.toml", TOKYO_NIGHT),
            (".local/state/omarchy/current/theme.name", "tokyo-night\n"),
        ]);
        assert!(installed_at(home.path()));
        let t = read_at(home.path()).unwrap();
        assert_eq!(t.id, ID);
        assert!(t.builtin && t.dark());
        assert_eq!(t.description.as_deref(), Some("Tokyo Night"));
        assert_eq!(hex(t.tokens.get("background")), "#1a1b26");
        assert_eq!(hex(t.tokens.get("foreground")), "#a9b1d6");
        assert_eq!(hex(t.tokens.get("card")), "#24283b");
        assert_eq!(hex(t.tokens.get("primary")), "#7aa2f7");
        assert_eq!(hex(t.tokens.get("primary-foreground")), "#1a1b26");
        assert_eq!(hex(t.tokens.get("border")), "#414868");
        assert_eq!(hex(t.tokens.get("destructive")), "#f7768e");
    }

    #[cfg(unix)]
    #[test]
    fn an_older_theme_is_read_from_alacritty() {
        let home = tempfile::tempdir().unwrap();
        let themes = home.path().join(".config/omarchy/themes/catppuccin-latte");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("alacritty.toml"), OLD_ALACRITTY).unwrap();
        std::fs::write(themes.join("light.mode"), "").unwrap();
        let current = home.path().join(".config/omarchy/current");
        std::fs::create_dir_all(&current).unwrap();
        std::os::unix::fs::symlink(&themes, current.join("theme")).unwrap();
        let t = read_at(home.path()).unwrap();
        assert!(!t.dark());
        assert_eq!(t.description.as_deref(), Some("Catppuccin Latte"));
        assert_eq!(hex(t.tokens.get("background")), "#eff1f5");
        assert_eq!(hex(t.tokens.get("foreground")), "#4c4f69");
        assert_eq!(hex(t.tokens.get("primary")), "#1e66f5");
        assert_eq!(hex(t.tokens.get("destructive")), "#d20f39");
    }

    #[test]
    fn without_omarchy_there_is_none() {
        let home = home_with(&[(".config/something/else.toml", "a = 1")]);
        assert!(!installed_at(home.path()));
        assert!(read_at(home.path()).is_none());
        // A theme without its two main colors isn't one.
        let home = home_with(&[(".local/state/omarchy/current/theme/colors.toml", "accent = \"#7aa2f7\"")]);
        assert!(installed_at(home.path()));
        assert!(read_at(home.path()).is_none());
    }

    #[test]
    fn a_new_theme_changes_the_stamp() {
        let home = home_with(&[(".local/state/omarchy/current/theme/colors.toml", TOKYO_NIGHT)]);
        let before = stamp(home.path());
        assert_eq!(before, stamp(home.path()));
        let dir = home.path().join(".local/state/omarchy/current/theme");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("colors.toml"), TOKYO_NIGHT.replace("#1a1b26", "#000000")).unwrap();
        assert_ne!(before, stamp(home.path()));
        assert_eq!(hex(read_at(home.path()).unwrap().tokens.get("background")), "#000000");
    }

    #[test]
    fn names_read_like_names() {
        let home = home_with(&[(".local/state/omarchy/current/theme.name", "rose_pine-dawn")]);
        assert_eq!(name(&home.path().join(".local/state/omarchy/current")), "Rose Pine Dawn");
        assert_eq!(name(&home.path().join("nowhere")), "Omarchy");
    }
}
