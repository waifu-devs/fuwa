//! App settings on every device (docs/app-settings.md), the pure part: the
//! settings that travel, each one's value in the web app's shape (its
//! `lib/prefs.ts` names) and back, the document the instance keeps, and
//! merging it with this computer's.
//!
//! Each synced setting has a stamp here (`Prefs::sync_stamps`, unix ms):
//! when it last changed on this computer, or the stamp of the remote value
//! it was taken from. None means never changed here, so it isn't sent and a
//! new computer takes everything the instance has. Merging takes, setting
//! by setting, the later stamp (the instance's on a tie) and keeps settings
//! this app doesn't know. A value this app can't use is left in the document
//! and this computer keeps its own. `core::app_settings` does the calls.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::core::accessibility::ColorVision;
use crate::core::config::{
    Clock, Density, FontPick, InputMode, MotionChoice, NotifyFor, PopoutFit, Prefs, RoleColors, SendWith, Spacing,
};
use crate::core::keybinds::{self, CustomKeybind};
use crate::core::themes::{self, Backdrop, Theme, Tokens};
use crate::core::voice::vp8::Share;

/// What the document says it is.
pub const FORMAT: &str = "fuwa.app-settings.v1";
/// The instance feature that keeps the document.
pub const FEATURE: &str = "app-settings-sync";
/// How long after the last change here the settings go out.
pub const PUSH_AFTER: Duration = Duration::from_secs(2);
/// Coming back to the window reads them again at most this often.
pub const REFRESH_EVERY: Duration = Duration::from_secs(60);
/// Writes refused because another device wrote meanwhile are tried again this many times in all.
pub const TRIES: usize = 3;

/// The settings this app sends and takes, by their names in the document.
/// Custom themes come before the theme picks, so a pick of a theme that
/// arrives with them is known when it's applied.
pub const SYNCED: &[&str] = &[
    "customThemes",
    "theme",
    "followSystem",
    "lightTheme",
    "darkTheme",
    "backdrop",
    "density",
    "messageDisplay",
    "chatFontSize",
    "font",
    "codeFont",
    "reduceMotion",
    "othersEffects",
    "highContrast",
    "underlineLinks",
    "saturation",
    "colorVision",
    "colorVisionStrength",
    "roleColors",
    "language",
    "clock",
    "sendWith",
    "notifyFor",
    "unreadBadge",
    "sounds",
    "volume",
    "streamerHidePersonal",
    "streamerMuteSounds",
    "streamerMuteNotifications",
    "keybinds",
    "customKeybinds",
    "developerMode",
    "shareReports",
    "inputMode",
    "pttRelease",
    "userVolumes",
    "mirrorVideo",
    "shareSound",
    "cameraHeight",
    "cameraFps",
    "shareHeight",
    "shareFps",
    "popoutName",
    "popoutGlow",
    "popoutFit",
    "trustedDomains",
];

/// One setting in the document: its value and when it last changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub at: i64,
    pub value: Value,
}

/// The document's settings, by name.
pub type Doc = BTreeMap<String, Entry>;

/// Reads the instance's document. Empty is none yet; an entry without a
/// stamp is left out; None when it's another format (a newer app's), which
/// this one mustn't write over.
pub fn read(text: &str) -> Option<Doc> {
    if text.trim().is_empty() {
        return Some(Doc::new());
    }
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(text) else { return Some(Doc::new()) };
    match root.get("format") {
        Some(Value::String(f)) if f != FORMAT => return None,
        _ => {}
    }
    let mut doc = Doc::new();
    let Some(settings) = root.get("settings").and_then(Value::as_object) else { return Some(doc) };
    for (name, entry) in settings {
        let Some(entry) = entry.as_object() else { continue };
        let at = entry.get("at").and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.0);
        let (Some(at), Some(value)) = (at, entry.get("value")) else { continue };
        doc.insert(name.clone(), Entry { at: at as i64, value: value.clone() });
    }
    Some(doc)
}

/// The document as it's written.
pub fn write(doc: &Doc) -> String {
    let settings: Map<String, Value> =
        doc.iter().map(|(name, e)| (name.clone(), json!({ "at": e.at, "value": e.value }))).collect();
    json!({ "format": FORMAT, "settings": settings }).to_string()
}

/// Setting by setting, the later stamp; the instance's on a tie. Settings
/// only one side has are kept.
pub fn merge(local: &Doc, remote: &Doc) -> Doc {
    let mut out = remote.clone();
    for (name, entry) in local {
        if remote.get(name).is_none_or(|r| entry.at > r.at) {
            out.insert(name.clone(), entry.clone());
        }
    }
    out
}

// ───────────────────────── Prefs to values ─────────────────────────

fn str_value(s: &str) -> Value {
    Value::String(s.to_owned())
}

/// A radius as the web keeps it, to the hundredth (an f32 written plainly isn't).
fn round2(r: f32) -> Value {
    json!((f64::from(r) * 100.0).round() / 100.0)
}

fn backdrop_value(b: &Backdrop) -> Value {
    serde_json::to_value(b).unwrap_or(Value::Null)
}

/// A made theme in the web's `CustomTheme` shape (`lib/theme-file.ts`).
fn theme_value(t: &Theme) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "description": t.description,
        "builtin": false,
        "variant": { "tokens": t.tokens.to_json(), "radius": round2(t.radius) },
        "backdrop": t.backdrop.as_ref().map(backdrop_value),
        "updatedAt": t.updated_at,
    })
}

/// A font pick in the web's shape (`lib/fonts.ts`); a file of yours isn't sent.
fn font_value(f: &FontPick) -> Option<Value> {
    Some(match f {
        FontPick::Default => json!({ "kind": "default" }),
        FontPick::System => json!({ "kind": "system" }),
        FontPick::Installed { family } => json!({ "kind": "installed", "family": family }),
        FontPick::File { .. } => return None,
    })
}

/// A picked theme's id; Omarchy's is this computer's own, so it isn't sent.
fn theme_id_value(id: &str) -> Option<Value> {
    (id != crate::core::omarchy::ID).then(|| str_value(id))
}

/// One setting's value as the document holds it; None when it can't be sent.
pub fn value(p: &Prefs, name: &str) -> Option<Value> {
    Some(match name {
        "customThemes" => Value::Array(p.custom_themes.iter().map(theme_value).collect()),
        "theme" => theme_id_value(&p.theme)?,
        "followSystem" => json!(p.follow_system),
        "lightTheme" => theme_id_value(&p.light_theme)?,
        "darkTheme" => theme_id_value(&p.dark_theme)?,
        "backdrop" => backdrop_value(&p.backdrop),
        "density" => str_value(match p.spacing {
            Spacing::Compact => "compact",
            Spacing::Default => "default",
            Spacing::Spacious => "spacious",
        }),
        "messageDisplay" => str_value(match p.density {
            Density::Cozy => "cozy",
            Density::Compact => "compact",
        }),
        "chatFontSize" => json!(p.chat_font_size),
        "font" => font_value(&p.font)?,
        "codeFont" => font_value(&p.code_font)?,
        "reduceMotion" => str_value(match p.motion {
            MotionChoice::System => "system",
            MotionChoice::Reduced => "always",
            MotionChoice::Full => "never",
        }),
        "othersEffects" => json!(p.others_effects),
        "highContrast" => json!(p.high_contrast),
        "underlineLinks" => json!(p.underline_links),
        "saturation" => json!(p.saturation),
        "colorVision" => serde_json::to_value(p.color_vision).ok()?,
        "colorVisionStrength" => json!(p.color_vision_strength),
        "roleColors" => serde_json::to_value(p.role_colors).ok()?,
        "language" => str_value(p.language.as_deref().unwrap_or("auto")),
        "clock" => serde_json::to_value(p.clock).ok()?,
        "sendWith" => serde_json::to_value(p.send_with).ok()?,
        "notifyFor" => serde_json::to_value(p.notify_for).ok()?,
        "unreadBadge" => json!(p.unread_badge),
        "sounds" => {
            let s = &p.sounds;
            json!({ "message": s.message, "mention": s.mention, "join": s.join, "call": s.call, "ring": s.ring })
        }
        "volume" => json!(p.volume),
        "streamerHidePersonal" => json!(p.streamer_hide_personal),
        "streamerMuteSounds" => json!(p.streamer_mute_sounds),
        "streamerMuteNotifications" => json!(p.streamer_mute_notifications),
        "keybinds" => serde_json::to_value(&p.keybinds).ok()?,
        "customKeybinds" => serde_json::to_value(&p.custom_keybinds).ok()?,
        "developerMode" => json!(p.developer_mode),
        "shareReports" => json!(p.share_reports),
        "inputMode" => serde_json::to_value(p.input_mode).ok()?,
        "pttRelease" => json!(p.ptt_release),
        "userVolumes" => serde_json::to_value(&p.user_volumes).ok()?,
        "mirrorVideo" => json!(p.mirror_video),
        "shareSound" => json!(p.share_sound),
        "cameraHeight" => json!(p.camera_height),
        "cameraFps" => json!(p.camera_fps),
        "shareHeight" => json!(p.share_height),
        "shareFps" => json!(p.share_fps),
        "popoutName" => json!(p.popout_name),
        "popoutGlow" => json!(p.popout_glow),
        "popoutFit" => serde_json::to_value(p.popout_fit).ok()?,
        "trustedDomains" => json!(p.trusted_domains),
        _ => return None,
    })
}

/// Every synced setting's value now, for telling what a change touched.
pub fn values(p: &Prefs) -> BTreeMap<&'static str, Value> {
    SYNCED.iter().filter_map(|name| Some((*name, value(p, name)?))).collect()
}

// ───────────────────────── Values to prefs ─────────────────────────

fn number(v: &Value, min: f64, max: f64) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite()).map(|n| n.round().clamp(min, max))
}

fn one_of<'a>(v: &'a Value, options: &[&str]) -> Option<&'a str> {
    v.as_str().filter(|s| options.contains(s))
}

/// What a value from the document is to this app, as the web's `sanitize` would take it.
fn font_from(v: &Value, code: bool) -> Option<FontPick> {
    let o = v.as_object()?;
    Some(match o.get("kind")?.as_str()? {
        "default" => FontPick::Default,
        "system" if code => FontPick::Default,
        "system" => FontPick::System,
        "installed" => {
            let family: String = o.get("family")?.as_str()?.chars().filter(|c| !c.is_control()).collect();
            let family: String = family.trim().chars().take(100).collect();
            if family.is_empty() {
                return None;
            }
            FontPick::Installed { family }
        }
        // A file is another device's: nothing here to draw it with.
        _ => return None,
    })
}

fn theme_from(v: &Value) -> Option<Theme> {
    let o = v.as_object()?;
    let variant = o.get("variant")?.as_object()?;
    let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(Theme {
        id: text("id")?,
        name: text("name").unwrap_or_default(),
        description: text("description"),
        tokens: Tokens::read(variant.get("tokens")?)?,
        radius: variant.get("radius").and_then(Value::as_f64).filter(|r| r.is_finite()).unwrap_or(0.75) as f32,
        builtin: false,
        backdrop: o.get("backdrop").filter(|b| b.is_object()).map(Backdrop::sanitize),
        updated_at: o.get("updatedAt").and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0) as i64,
    })
}

fn known_theme(p: &Prefs, id: &str) -> bool {
    themes::builtins().iter().any(|t| t.id == id) || p.custom_themes.iter().any(|t| t.id == id)
}

/// Puts one setting from the document into the prefs, checked and clamped
/// like the settings file. False when this app can't use it: then nothing
/// changes.
pub fn apply(p: &mut Prefs, name: &str, v: &Value) -> bool {
    macro_rules! set {
        ($field:expr, $value:expr) => {{
            match $value {
                Some(x) => {
                    $field = x;
                    true
                }
                None => false,
            }
        }};
    }
    let flag = v.as_bool();
    match name {
        "customThemes" => {
            let Some(list) = v.as_array() else { return false };
            p.custom_themes = themes::sanitize_custom(list.iter().filter_map(theme_from).collect());
            true
        }
        "theme" | "lightTheme" | "darkTheme" => {
            let Some(id) = v.as_str().filter(|id| known_theme(p, id)).map(str::to_owned) else { return false };
            match name {
                "theme" => p.theme = id,
                "lightTheme" => p.light_theme = id,
                _ => p.dark_theme = id,
            }
            true
        }
        "followSystem" => set!(p.follow_system, flag),
        "backdrop" => set!(p.backdrop, v.is_object().then(|| Backdrop::sanitize(v))),
        "density" => set!(
            p.spacing,
            one_of(v, &["compact", "default", "spacious"]).map(|s| match s {
                "compact" => Spacing::Compact,
                "spacious" => Spacing::Spacious,
                _ => Spacing::Default,
            })
        ),
        "messageDisplay" => set!(
            p.density,
            one_of(v, &["cozy", "compact"]).map(|s| if s == "compact" { Density::Compact } else { Density::Cozy })
        ),
        "chatFontSize" => set!(p.chat_font_size, number(v, 12.0, 20.0).map(|n| n as u8)),
        "font" => set!(p.font, font_from(v, false)),
        "codeFont" => set!(p.code_font, font_from(v, true)),
        "reduceMotion" => set!(
            p.motion,
            one_of(v, &["system", "always", "never"]).map(|s| match s {
                "always" => MotionChoice::Reduced,
                "never" => MotionChoice::Full,
                _ => MotionChoice::System,
            })
        ),
        "othersEffects" => set!(p.others_effects, flag),
        "highContrast" => set!(p.high_contrast, flag),
        "underlineLinks" => set!(p.underline_links, flag),
        "saturation" => set!(p.saturation, number(v, 0.0, 100.0).map(|n| n as u8)),
        "colorVision" => set!(p.color_vision, serde_json::from_value::<ColorVision>(v.clone()).ok()),
        "colorVisionStrength" => set!(p.color_vision_strength, number(v, 0.0, 100.0).map(|n| n as u8)),
        "roleColors" => set!(p.role_colors, serde_json::from_value::<RoleColors>(v.clone()).ok()),
        "language" => {
            let Some(tag) = v.as_str() else { return false };
            if tag == "auto" {
                p.language = None;
                return true;
            }
            // Only a language this app ships; another stays the other app's.
            if !crate::core::i18n::LANGUAGES.iter().any(|l| l.code == tag) {
                return false;
            }
            p.language = Some(tag.to_owned());
            true
        }
        "clock" => set!(p.clock, serde_json::from_value::<Clock>(v.clone()).ok()),
        "sendWith" => set!(p.send_with, serde_json::from_value::<SendWith>(v.clone()).ok()),
        "notifyFor" => set!(p.notify_for, serde_json::from_value::<NotifyFor>(v.clone()).ok()),
        "unreadBadge" => set!(p.unread_badge, flag),
        "sounds" => {
            let Some(o) = v.as_object() else { return false };
            let s = &mut p.sounds;
            for (key, slot) in [
                ("message", &mut s.message),
                ("mention", &mut s.mention),
                ("join", &mut s.join),
                ("call", &mut s.call),
                ("ring", &mut s.ring),
            ] {
                if let Some(on) = o.get(key).and_then(Value::as_bool) {
                    *slot = on;
                }
            }
            true
        }
        "volume" => set!(p.volume, number(v, 0.0, 100.0).map(|n| n as u8)),
        "streamerHidePersonal" => set!(p.streamer_hide_personal, flag),
        "streamerMuteSounds" => set!(p.streamer_mute_sounds, flag),
        "streamerMuteNotifications" => set!(p.streamer_mute_notifications, flag),
        "keybinds" => {
            let Some(o) = v.as_object() else { return false };
            p.keybinds = o
                .iter()
                .filter_map(|(action, combo)| match combo {
                    Value::Null => Some((action.clone(), None)),
                    Value::String(c) if keybinds::valid(c) => Some((action.clone(), Some(c.clone()))),
                    _ => None,
                })
                .collect();
            true
        }
        "customKeybinds" => {
            let Some(list) = v.as_array() else { return false };
            let binds = list.iter().filter_map(|b| serde_json::from_value::<CustomKeybind>(b.clone()).ok()).collect();
            p.custom_keybinds = keybinds::tidy_custom(binds);
            true
        }
        "developerMode" => set!(p.developer_mode, flag),
        "shareReports" => set!(p.share_reports, flag),
        "inputMode" => set!(p.input_mode, serde_json::from_value::<InputMode>(v.clone()).ok()),
        "pttRelease" => set!(p.ptt_release, number(v, 0.0, 2000.0).map(|n| n as u16)),
        "userVolumes" => {
            let Some(o) = v.as_object() else { return false };
            p.user_volumes = o
                .iter()
                .filter_map(|(who, n)| {
                    let n = n.as_f64().filter(|n| n.is_finite() && (0.0..=200.0).contains(n))?;
                    Some((who.clone(), n.round() as u16))
                })
                .collect();
            true
        }
        "mirrorVideo" => set!(p.mirror_video, flag),
        "shareSound" => set!(p.share_sound, flag),
        "cameraHeight" => set!(
            p.camera_height,
            v.as_u64().map(|n| n as u32).filter(|n| crate::core::voice::ceiling::HEIGHTS.contains(n))
        ),
        "cameraFps" => set!(
            p.camera_fps,
            v.as_u64().map(|n| n as u32).filter(|n| crate::core::voice::ceiling::FRAME_RATES.contains(n))
        ),
        "shareHeight" => set!(p.share_height, v.as_u64().map(|n| n as u32).filter(|n| Share::HEIGHTS.contains(n))),
        "shareFps" => set!(p.share_fps, v.as_u64().map(|n| n as u32).filter(|n| Share::FPS.contains(n))),
        "popoutName" => set!(p.popout_name, flag),
        "popoutGlow" => set!(p.popout_glow, flag),
        "popoutFit" => set!(p.popout_fit, serde_json::from_value::<PopoutFit>(v.clone()).ok()),
        "trustedDomains" => {
            let Some(list) = v.as_array() else { return false };
            let hosts = list.iter().filter_map(Value::as_str).map(str::to_owned).collect();
            p.trusted_domains = crate::core::links::tidy_domains(hosts);
            true
        }
        _ => false,
    }
}

// ───────────────────────── Stamps and merging ─────────────────────────

/// Stamps every synced setting a change touched: `before` is [`values`]
/// from before it. A change is stamped now, or one past the stamp it
/// replaces when this clock is behind. Whether anything synced changed.
pub fn stamp(before: &BTreeMap<&'static str, Value>, p: &mut Prefs, now: i64) -> bool {
    let after = values(p);
    let mut changed = false;
    for name in SYNCED {
        if before.get(name) != after.get(name) {
            let previous = p.sync_stamps.get(*name).copied().unwrap_or(0);
            p.sync_stamps.insert((*name).to_owned(), now.max(previous + 1));
            changed = true;
        }
    }
    changed
}

/// This computer's settings as a document: the ones changed here (or taken
/// from another device) that can be sent.
pub fn local(p: &Prefs) -> Doc {
    SYNCED
        .iter()
        .filter_map(|name| {
            let at = p.sync_stamps.get(*name).copied().filter(|at| *at > 0)?;
            Some(((*name).to_owned(), Entry { at, value: value(p, name)? }))
        })
        .collect()
}

/// What to do with the instance's document.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The document to keep: this computer's and the instance's, merged.
    pub merged: Doc,
    /// The instance's settings newer than this computer's, in [`SYNCED`] order.
    pub take: Vec<(String, Entry)>,
    /// Whether this computer has anything newer, or anything the instance lacks.
    pub push: bool,
}

pub fn plan(p: &Prefs, remote: &Doc) -> Plan {
    let local = local(p);
    let merged = merge(&local, remote);
    let take = SYNCED
        .iter()
        .filter_map(|name| {
            let entry = remote.get(*name)?;
            (entry.at > p.sync_stamps.get(*name).copied().unwrap_or(0)).then(|| ((*name).to_owned(), entry.clone()))
        })
        .collect();
    let push = local.iter().any(|(name, e)| remote.get(name).is_none_or(|r| e.at > r.at));
    Plan { merged, take, push }
}

/// Puts the instance's newer settings in, each stamped with its own time;
/// one this app can't use is left as it was here. The names taken.
pub fn take(p: &mut Prefs, entries: &[(String, Entry)]) -> Vec<String> {
    let mut taken = Vec::new();
    for (name, entry) in entries {
        if apply(p, name, &entry.value) {
            p.sync_stamps.insert(name.clone(), entry.at);
            taken.push(name.clone());
        }
    }
    if !taken.is_empty() {
        let share = Share::new(p.share_height, p.share_fps);
        (p.share_height, p.share_fps) = (share.height, share.fps);
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Sounds;
    use crate::core::themes::Effect;

    fn entry(at: i64, value: Value) -> Entry {
        Entry { at, value }
    }

    fn changed_prefs() -> Prefs {
        let mut custom = themes::builtins().remove(1);
        custom.id = "custom-abcd1234".into();
        custom.name = "Mine".into();
        custom.builtin = false;
        custom.radius = 0.3;
        custom.updated_at = 1_760_000_000_000;
        custom.backdrop = Some(Backdrop { effect: Effect::Stars, dim: 50, ..Backdrop::default() });
        Prefs {
            custom_themes: vec![custom],
            theme: "custom-abcd1234".into(),
            follow_system: true,
            light_theme: "matcha".into(),
            dark_theme: "custom-abcd1234".into(),
            backdrop: Backdrop { effect: Effect::Aurora, intensity: 40, panels: 60, ..Backdrop::default() },
            spacing: Spacing::Spacious,
            density: Density::Compact,
            chat_font_size: 18,
            font: FontPick::Installed { family: "Inter".into() },
            code_font: FontPick::Default,
            motion: MotionChoice::Reduced,
            others_effects: false,
            high_contrast: true,
            underline_links: true,
            saturation: 40,
            color_vision: ColorVision::Tritanopia,
            color_vision_strength: 70,
            role_colors: RoleColors::Beside,
            language: Some("es".into()),
            clock: Clock::H24,
            send_with: SendWith::ModEnter,
            notify_for: NotifyFor::All,
            unread_badge: false,
            sounds: Sounds { message: false, join: true, ..Sounds::default() },
            volume: 33,
            streamer_hide_personal: false,
            streamer_mute_sounds: false,
            streamer_mute_notifications: false,
            keybinds: [("search".to_owned(), Some("Mod+K".to_owned())), ("quickSwitcher".to_owned(), None)].into(),
            custom_keybinds: vec![CustomKeybind { id: "a1".into(), action: "search".into(), combo: "Alt+S".into() }],
            developer_mode: true,
            share_reports: false,
            input_mode: InputMode::Ptt,
            ptt_release: 500,
            user_volumes: [("fuwa.chat/01ABC".to_owned(), 150)].into(),
            mirror_video: false,
            share_sound: false,
            camera_height: 720,
            camera_fps: 30,
            share_height: 1440,
            share_fps: 60,
            popout_name: false,
            popout_glow: false,
            popout_fit: PopoutFit::Contain,
            trusted_domains: vec!["example.com".into()],
            ..Prefs::default()
        }
    }

    #[test]
    fn every_setting_round_trips_in_the_webs_shape() {
        let p = changed_prefs();
        let sent = values(&p);
        assert_eq!(sent.len(), SYNCED.len());
        let mut fresh = Prefs::default();
        for name in SYNCED {
            assert!(apply(&mut fresh, name, &sent[name]), "{name}");
        }
        assert_eq!(values(&fresh), sent);
        // The web's names and shapes.
        assert_eq!(sent["density"], json!("spacious"));
        assert_eq!(sent["messageDisplay"], json!("compact"));
        assert_eq!(sent["reduceMotion"], json!("always"));
        assert_eq!(sent["language"], json!("es"));
        assert_eq!(sent["clock"], json!("24h"));
        assert_eq!(sent["sendWith"], json!("mod-enter"));
        assert_eq!(sent["font"], json!({ "kind": "installed", "family": "Inter" }));
        assert_eq!(
            sent["sounds"],
            json!({ "message": false, "mention": true, "join": true, "call": true, "ring": true })
        );
        assert_eq!(sent["keybinds"], json!({ "quickSwitcher": null, "search": "Mod+K" }));
        let theme = &sent["customThemes"][0];
        assert_eq!(theme["variant"]["radius"], json!(0.3));
        assert_eq!(theme["variant"]["tokens"]["background"], json!("#14111f"));
        assert_eq!(theme["backdrop"]["effect"], json!("stars"));
        assert_eq!(theme["updatedAt"], json!(1_760_000_000_000i64));
        assert_eq!(sent["backdrop"]["effect"], json!("aurora"));
        assert_eq!(sent["backdrop"]["shader"], Value::Null);
        // Defaults read the same way back.
        let mut back = changed_prefs();
        for (name, v) in values(&Prefs::default()) {
            assert!(apply(&mut back, name, &v), "{name}");
        }
        assert_eq!(values(&back), values(&Prefs::default()));
        assert_eq!(back.language, None);
    }

    #[test]
    fn a_web_theme_reads_as_one() {
        let web = json!([{
            "id": "custom-web1",
            "name": "From the web",
            "description": null,
            "builtin": false,
            "variant": { "tokens": { "background": "#000000", "foreground": "#ffffff", "card": "#111111", "primary": "#ff00ff",
                "primary-foreground": "#000000", "muted-foreground": "#888888", "border": "#333333" }, "radius": 9 },
            "backdrop": null,
            "updatedAt": 5
        }, { "id": "not-custom", "variant": { "tokens": {} } }]);
        let mut p = Prefs::default();
        assert!(apply(&mut p, "customThemes", &web));
        assert_eq!(p.custom_themes.len(), 1);
        let t = &p.custom_themes[0];
        assert_eq!((t.id.as_str(), t.radius, t.backdrop.is_none(), t.updated_at), ("custom-web1", 1.5, true, 5));
        assert!(apply(&mut p, "theme", &json!("custom-web1")));
        assert!(!apply(&mut p, "theme", &json!("custom-gone1")));
        assert_eq!(p.theme, "custom-web1");
    }

    #[test]
    fn values_this_app_cant_use_change_nothing() {
        let mut p = changed_prefs();
        let before = p.clone();
        for (name, v) in [
            ("theme", json!(7)),
            ("density", json!("roomy")),
            ("chatFontSize", json!("big")),
            ("font", json!({ "kind": "file", "id": "abc", "name": "Mine.ttf" })),
            ("font", json!({ "kind": "installed", "family": "  " })),
            ("reduceMotion", json!(true)),
            ("colorVision", json!("achromatopsia")),
            ("language", json!("tlh")),
            ("cameraHeight", json!(999)),
            ("shareFps", json!(24)),
            ("backdrop", json!("sunset")),
            ("customThemes", json!({})),
            ("somethingNew", json!(1)),
        ] {
            assert!(!apply(&mut p, name, &v), "{name}");
        }
        assert_eq!(p, before);
        // Numbers are clamped like the settings file.
        assert!(apply(&mut p, "chatFontSize", &json!(40)));
        assert!(apply(&mut p, "saturation", &json!(-5)));
        assert_eq!((p.chat_font_size, p.saturation), (20, 0));
        // A system font means nothing for code.
        assert!(apply(&mut p, "codeFont", &json!({ "kind": "system" })));
        assert_eq!(p.code_font, FontPick::Default);
    }

    #[test]
    fn a_font_file_is_never_sent() {
        let file = FontPick::File {
            file: "0123456789abcdef01234567.ttf".into(),
            name: "Mine.ttf".into(),
            family: "Mine".into(),
        };
        let mut p = Prefs::default();
        let before = values(&p);
        p.font = file;
        assert!(stamp(&before, &mut p, 1000));
        assert_eq!(p.sync_stamps.get("font"), Some(&1000));
        assert!(!local(&p).contains_key("font"));
        // An older pick from elsewhere doesn't replace it; a newer one does.
        let remote: Doc = [("font".to_owned(), entry(900, json!({ "kind": "system" })))].into();
        assert!(plan(&p, &remote).take.is_empty());
        let remote: Doc = [("font".to_owned(), entry(1100, json!({ "kind": "system" })))].into();
        let plan = plan(&p, &remote);
        take(&mut p, &plan.take);
        assert_eq!(p.font, FontPick::System);
    }

    #[test]
    fn merging_takes_the_later_stamp_and_keeps_the_rest() {
        let local: Doc =
            [("theme".to_owned(), entry(10, json!("yoru"))), ("volume".to_owned(), entry(5, json!(10)))].into();
        let remote: Doc = [
            ("theme".to_owned(), entry(9, json!("sora"))),
            ("volume".to_owned(), entry(5, json!(90))),
            ("overlayCorner".to_owned(), entry(1, json!("top-right"))),
        ]
        .into();
        let merged = merge(&local, &remote);
        assert_eq!(merged["theme"].value, json!("yoru"));
        // A tie is the instance's.
        assert_eq!(merged["volume"].value, json!(90));
        assert_eq!(merged["overlayCorner"], remote["overlayCorner"]);
    }

    #[test]
    fn changes_are_stamped_past_what_they_replace() {
        let mut p = Prefs::default();
        let before = values(&p);
        p.zoom = 120;
        assert!(!stamp(&before, &mut p, 1000), "zoom stays on this device");
        assert!(p.sync_stamps.is_empty());
        let before = values(&p);
        p.volume = 20;
        assert!(stamp(&before, &mut p, 1000));
        assert_eq!(p.sync_stamps["volume"], 1000);
        // Taken from a device whose clock runs ahead, then changed here.
        p.sync_stamps.insert("volume".into(), 5000);
        let before = values(&p);
        p.volume = 30;
        stamp(&before, &mut p, 1200);
        assert_eq!(p.sync_stamps["volume"], 5001);
        assert_eq!(local(&p).keys().collect::<Vec<_>>(), ["volume"]);
    }

    #[test]
    fn a_plan_takes_whats_newer_and_sends_whats_missing() {
        let mut p = Prefs { volume: 20, ..Prefs::default() };
        p.sync_stamps.insert("volume".into(), 100);
        p.sync_stamps.insert("clock".into(), 300);
        p.clock = Clock::H12;
        let remote: Doc = [
            ("volume".to_owned(), entry(200, json!(80))),
            ("clock".to_owned(), entry(200, json!("24h"))),
            ("theme".to_owned(), entry(50, json!("yoru"))),
            ("chatFontSize".to_owned(), entry(60, json!("huge"))),
            ("fromTheFuture".to_owned(), entry(70, json!(true))),
        ]
        .into();
        let plan = plan(&p, &remote);
        let names: Vec<&str> = plan.take.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["theme", "chatFontSize", "volume"]);
        assert!(plan.push, "this computer's clock is newer");
        assert_eq!(plan.merged["clock"].value, json!("12h"));
        assert_eq!(plan.merged["fromTheFuture"].value, json!(true));
        let taken = take(&mut p, &plan.take);
        assert_eq!(taken, ["theme", "volume"]);
        assert_eq!((p.theme.as_str(), p.volume, p.sync_stamps["volume"]), ("yoru", 80, 200));
        // A value it couldn't use stays the other app's, and this one keeps its own, unstamped.
        assert_eq!(p.chat_font_size, 15);
        assert!(!p.sync_stamps.contains_key("chatFontSize"));
        // Once written, nothing's left to send (the value it can't use is only tried again).
        let again = super::plan(&p, &plan.merged);
        assert!(!again.push);
        assert_eq!(again.take.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["chatFontSize"]);
        // A new device sends nothing it never changed.
        assert!(!super::plan(&Prefs::default(), &Doc::new()).push);
    }

    #[test]
    fn the_document_reads_tolerantly() {
        assert_eq!(read(""), Some(Doc::new()));
        assert_eq!(read("not json"), Some(Doc::new()));
        assert_eq!(read(r#"{"format":"fuwa.app-settings.v2","settings":{}}"#), None);
        let doc = read(
            r#"{"format":"fuwa.app-settings.v1","settings":{"theme":{"at":5,"value":"yoru"},"bad":{"value":1},"worse":3,"keybinds":{"at":7,"value":{"search":null}}}}"#,
        )
        .unwrap();
        assert_eq!(doc.keys().collect::<Vec<_>>(), ["keybinds", "theme"]);
        assert_eq!(read(&write(&doc)), Some(doc.clone()));
        let text = write(&doc);
        assert!(text.contains(r#""format":"fuwa.app-settings.v1""#), "{text}");
    }
}
