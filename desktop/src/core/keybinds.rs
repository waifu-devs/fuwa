//! Keyboard shortcuts, the web app's `lib/keybinds.ts`: every action has an
//! id, a default combo and a place on the Keyboard page, and the window's key
//! handler, the shortcut sheet and custom binds all read this one list.
//!
//! Combos are written the same on every system and in both apps: "Mod" is Cmd
//! on macOS and Ctrl elsewhere, then Ctrl (macOS only), Meta (elsewhere),
//! Alt, Shift and the key, joined with "+", such as "Mod+K" or
//! "Alt+Shift+ArrowUp". Keys use the web's names (`KeyboardEvent.code`
//! without its prefix), so a combo set in one app means the same in the other.
//! The voice actions come with calls, which the desktop app doesn't have yet.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Navigation,
    Messages,
    Chat,
    App,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Navigation, Group::Messages, Group::Chat, Group::App];

    pub fn name(self) -> &'static str {
        match self {
            Group::Navigation => "Navigation",
            Group::Messages => "Messages",
            Group::Chat => "Chat",
            Group::App => "App",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    /// None when it has no shortcut until someone gives it one.
    pub combo: Option<&'static str>,
    /// Works while typing in a text box too.
    pub while_typing: bool,
    /// Keeps going while the keys are held, like walking down channels.
    pub repeats: bool,
}

const fn action(
    id: &'static str,
    label: &'static str,
    group: Group,
    combo: Option<&'static str>,
    while_typing: bool,
    repeats: bool,
) -> Action {
    Action { id, label, group, combo, while_typing, repeats }
}

pub const ACTIONS: [Action; 13] = [
    action("quickSwitcher", "Find a server or channel", Group::Navigation, Some("Mod+K"), true, false),
    action("previousServer", "Previous server", Group::Navigation, Some("Mod+Alt+ArrowUp"), true, true),
    action("nextServer", "Next server", Group::Navigation, Some("Mod+Alt+ArrowDown"), true, true),
    action("previousChannel", "Previous channel", Group::Navigation, Some("Alt+ArrowUp"), true, true),
    action("nextChannel", "Next channel", Group::Navigation, Some("Alt+ArrowDown"), true, true),
    action("previousUnread", "Previous unread channel", Group::Navigation, Some("Alt+Shift+ArrowUp"), true, true),
    action("nextUnread", "Next unread channel", Group::Navigation, Some("Alt+Shift+ArrowDown"), true, true),
    action("markServerRead", "Mark the server as read", Group::Messages, Some("Shift+Escape"), true, false),
    action("focusComposer", "Start typing a message", Group::Chat, Some("Tab"), false, false),
    action("toggleMembers", "Show or hide members", Group::Chat, Some("Mod+U"), true, false),
    action("openSettings", "Open settings", Group::App, Some("Mod+Comma"), true, false),
    action("shortcuts", "Show keyboard shortcuts", Group::App, Some("Mod+Slash"), true, false),
    action("toggleStreamer", "Turn streamer mode on or off", Group::App, None, true, false),
];

pub fn action_by_id(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// An extra shortcut for an action, on top of its own (`customKeybinds` on the web).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomKeybind {
    pub id: String,
    pub action: String,
    pub combo: String,
}

const MODIFIERS: [&str; 5] = ["Mod", "Ctrl", "Meta", "Alt", "Shift"];

/// Puts a combo's parts in the one order, so equal combos compare equal.
pub fn normalize(combo: &str) -> String {
    let mut parts: Vec<&str> = combo.split('+').collect();
    // "Mod++" can't happen: the key for + is "Equal".
    let key = parts.pop().unwrap_or_default();
    let mut out: Vec<&str> = MODIFIERS.iter().copied().filter(|m| parts.contains(m)).collect();
    out.push(key);
    out.join("+")
}

/// Whether a combo reads as one: modifiers this app knows, then one key.
pub fn valid(combo: &str) -> bool {
    let parts: Vec<&str> = combo.split('+').collect();
    let Some((key, mods)) = parts.split_last() else { return false };
    !key.is_empty() && !MODIFIERS.contains(key) && mods.iter().all(|m| MODIFIERS.contains(m))
}

/// An action's shortcut now: its default, or what someone changed it to (None when taken away).
pub fn binding_of(action: &Action, keybinds: &BTreeMap<String, Option<String>>) -> Option<String> {
    match keybinds.get(action.id) {
        Some(changed) => changed.clone(),
        None => action.combo.map(str::to_owned),
    }
}

/// Every combo in use and the action it runs, defaults and custom binds together.
pub fn bindings(
    keybinds: &BTreeMap<String, Option<String>>,
    custom: &[CustomKeybind],
) -> HashMap<String, &'static Action> {
    let mut out = HashMap::new();
    for action in &ACTIONS {
        if let Some(combo) = binding_of(action, keybinds) {
            out.insert(normalize(&combo), action);
        }
    }
    for c in custom {
        if let Some(action) = action_by_id(&c.action) {
            out.entry(normalize(&c.combo)).or_insert(action);
        }
    }
    out
}

/// What the binding being changed is, so recording its own combo again is fine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Except<'a> {
    pub action: Option<&'a str>,
    pub custom: Option<&'a str>,
}

/// Why a combo can't be used, or None if it can.
pub fn problem_with(
    combo: &str,
    keybinds: &BTreeMap<String, Option<String>>,
    custom: &[CustomKeybind],
    except: Except,
) -> Option<String> {
    let c = normalize(combo);
    if let Some((_, what)) = FIXED.iter().find(|(k, _)| normalize(k) == c) {
        return Some(format!("{} is kept for “{what}”.", label(&c)));
    }
    for action in &ACTIONS {
        if except.action == Some(action.id) {
            continue;
        }
        if binding_of(action, keybinds).is_some_and(|b| normalize(&b) == c) {
            return Some(format!("{} is already used for “{}”.", label(&c), action.label));
        }
    }
    for k in custom {
        if except.custom == Some(k.id.as_str()) {
            continue;
        }
        if normalize(&k.combo) == c {
            let what = action_by_id(&k.action).map_or(k.action.as_str(), |a| a.label);
            return Some(format!("{} is already used for “{what}”.", label(&c)));
        }
    }
    None
}

/// Keys the desktop app keeps for itself, so they can't be given to an action.
pub const FIXED: [(&str, &str); 2] = [("Mod+Shift+N", "Add an instance"), ("Escape", "Close what's open")];

/// Keys that only work inside the message box, listed on the sheet but not changeable.
pub const COMPOSER_KEYS: [(&str, &str); 4] = [
    ("Send the message", "Enter"),
    ("New line", "Shift+Enter"),
    ("Edit your last message", "ArrowUp"),
    ("Stop editing", "Escape"),
];

/// A combo as keycaps to show: ["Ctrl", "K"], or ["⌘", "K"] on a Mac.
pub fn keycaps(combo: &str) -> Vec<String> {
    keycaps_for(combo, cfg!(target_os = "macos"))
}

fn keycaps_for(combo: &str, mac: bool) -> Vec<String> {
    combo
        .split('+')
        .map(|part| {
            let modifier = match (part, mac) {
                ("Mod" | "Meta", true) => Some("⌘"),
                ("Ctrl", true) => Some("⌃"),
                ("Alt", true) => Some("⌥"),
                ("Shift", true) => Some("⇧"),
                ("Mod" | "Ctrl", false) => Some("Ctrl"),
                ("Alt", false) => Some("Alt"),
                ("Shift", false) => Some("Shift"),
                ("Meta", false) => Some("Win"),
                _ => None,
            };
            let key = match part {
                "ArrowUp" => "↑",
                "ArrowDown" => "↓",
                "ArrowLeft" => "←",
                "ArrowRight" => "→",
                "Escape" => "Esc",
                "Comma" => ",",
                "Period" => ".",
                "Slash" => "/",
                "Backslash" => "\\",
                "Semicolon" => ";",
                "Quote" => "'",
                "BracketLeft" => "[",
                "BracketRight" => "]",
                "Minus" => "-",
                "Equal" => "=",
                "Backquote" => "`",
                "Enter" if mac => "Return",
                "Backspace" if mac => "Delete",
                other => other,
            };
            modifier.unwrap_or(key).to_owned()
        })
        .collect()
}

/// A combo in words, for messages.
pub fn label(combo: &str) -> String {
    let mac = cfg!(target_os = "macos");
    keycaps_for(combo, mac).join(if mac { "" } else { "+" })
}

/// The key part of a combo, in the web's names, from the name the window
/// gives a key (GPUI's: "k", "up", ",", "f5"). None for keys a shortcut
/// can't use.
pub fn key_name(key: &str) -> Option<String> {
    let named = match key {
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "escape" => "Escape",
        "tab" => "Tab",
        "enter" => "Enter",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "insert" => "Insert",
        "space" | " " => "Space",
        "," | "<" => "Comma",
        "." | ">" => "Period",
        "/" | "?" => "Slash",
        "\\" | "|" => "Backslash",
        ";" | ":" => "Semicolon",
        "'" | "\"" => "Quote",
        "[" | "{" => "BracketLeft",
        "]" | "}" => "BracketRight",
        "-" | "_" => "Minus",
        "=" | "+" => "Equal",
        "`" | "~" => "Backquote",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_owned());
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase().to_string()),
        (Some('f'), Some(_)) if key[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => {
            Some(key.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// The modifiers held, in combo order. `platform` is Cmd on a Mac and the
/// Windows key elsewhere.
pub fn modifiers(control: bool, alt: bool, shift: bool, platform: bool) -> Vec<&'static str> {
    modifiers_for(control, alt, shift, platform, cfg!(target_os = "macos"))
}

fn modifiers_for(control: bool, alt: bool, shift: bool, platform: bool, mac: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if (mac && platform) || (!mac && control) {
        out.push("Mod");
    }
    if mac && control {
        out.push("Ctrl");
    }
    if !mac && platform {
        out.push("Meta");
    }
    if alt {
        out.push("Alt");
    }
    if shift {
        out.push("Shift");
    }
    out
}

/// Keeps custom binds that read as combos for actions this app knows of (or the web does).
pub fn tidy_custom(custom: Vec<CustomKeybind>) -> Vec<CustomKeybind> {
    custom.into_iter().filter(|c| !c.id.is_empty() && valid(&c.combo)).take(100).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_compare_whatever_order_they_were_written_in() {
        assert_eq!(normalize("Shift+Alt+ArrowUp"), "Alt+Shift+ArrowUp");
        assert_eq!(normalize("Mod+K"), "Mod+K");
        assert!(valid("Mod+Shift+K") && valid("Tab"));
        assert!(!valid("Mod+") && !valid("Shift") && !valid("Hyper+K"));
    }

    #[test]
    fn changed_and_taken_away_shortcuts_win_over_the_defaults() {
        let mut keybinds = BTreeMap::new();
        let switcher = action_by_id("quickSwitcher").unwrap();
        assert_eq!(binding_of(switcher, &keybinds).as_deref(), Some("Mod+K"));
        keybinds.insert("quickSwitcher".to_owned(), Some("Mod+P".to_owned()));
        keybinds.insert("toggleMembers".to_owned(), None);
        let custom = vec![CustomKeybind { id: "custom-1".into(), action: "shortcuts".into(), combo: "F1".into() }];
        let all = bindings(&keybinds, &custom);
        assert_eq!(all.get("Mod+P").map(|a| a.id), Some("quickSwitcher"));
        assert!(!all.contains_key("Mod+K") && !all.contains_key("Mod+U"));
        assert_eq!(all.get("F1").map(|a| a.id), Some("shortcuts"));

        assert!(problem_with("Mod+P", &keybinds, &custom, Except::default()).is_some());
        assert!(
            problem_with("Mod+P", &keybinds, &custom, Except { action: Some("quickSwitcher"), custom: None }).is_none()
        );
        assert!(problem_with("F1", &keybinds, &custom, Except::default()).unwrap().contains("Show keyboard shortcuts"));
        assert!(problem_with("Mod+K", &keybinds, &custom, Except::default()).is_none());
    }

    #[test]
    fn the_apps_own_keys_cant_be_taken() {
        let none = BTreeMap::new();
        let why = problem_with("Shift+Mod+N", &none, &[], Except::default()).unwrap();
        assert!(why.contains("Add an instance"), "{why}");
        assert!(problem_with("Mod+J", &none, &[], Except::default()).is_none());
    }

    #[test]
    fn keys_read_the_way_the_web_writes_them() {
        assert_eq!(key_name("k").as_deref(), Some("K"));
        assert_eq!(key_name("up").as_deref(), Some("ArrowUp"));
        assert_eq!(key_name(",").as_deref(), Some("Comma"));
        assert_eq!(key_name("?").as_deref(), Some("Slash"));
        assert_eq!(key_name("f5").as_deref(), Some("F5"));
        assert_eq!(key_name("7").as_deref(), Some("7"));
        assert_eq!(key_name("shift"), None);
        assert_eq!(key_name("fn"), None);
        assert_eq!(modifiers_for(true, true, false, false, false), ["Mod", "Alt"]);
        assert_eq!(modifiers_for(true, false, true, true, true), ["Mod", "Ctrl", "Shift"]);
        assert_eq!(keycaps_for("Mod+Shift+Slash", true), ["⌘", "⇧", "/"]);
        assert_eq!(keycaps_for("Mod+Comma", false), ["Ctrl", ","]);
    }
}
