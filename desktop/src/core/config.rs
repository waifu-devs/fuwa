//! What this app keeps on this computer outside the vaults: the instances
//! you added (with a session token for each one you signed in to) and the
//! app's own settings, which apply to every instance.
//!
//! Both are JSON files in the app's config folder (`FUWA_DESKTOP_HOME` moves
//! everything, for tests and portable installs). The tokens themselves live
//! in the system keychain (see `secrets.rs`), never in these files, and
//! never leave this computer except to their own instance.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::keybinds;
use crate::core::secrets::Secrets;
use crate::core::themes::{self, Backdrop, Theme};
use crate::core::vault::write_json;

/// Where the app keeps its files.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Settings and the instance list.
    pub config: PathBuf,
    /// The encrypted-messages vaults.
    pub vaults: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        if let Some(home) = std::env::var_os("FUWA_DESKTOP_HOME") {
            return Self::under(Path::new(&home));
        }
        let config = dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("fuwa");
        let data = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("fuwa");
        Self { config, vaults: data.join("vaults") }
    }

    pub fn under(home: &Path) -> Self {
        Self { config: home.join("config"), vaults: home.join("vaults") }
    }

    fn instances(&self) -> PathBuf {
        self.config.join("instances.json")
    }

    fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// Held while the app runs, so two copies don't share one device.
    pub fn lock_file(&self) -> PathBuf {
        self.config.join("app.lock")
    }
}

/// An instance you added, with its session token when you're signed in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInstance {
    pub url: String,
    /// The account in use's token. Only ever read from older files, which
    /// kept it here: it's moved to the keychain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Every account signed in to here, the one in use too, as cards to
    /// switch between (the web's `fuwa/saved.ts`); their tokens are in the keychain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<SavedAccount>,
}

/// An account kept on an instance: what the switcher shows of it, and its session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedAccount {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    /// Its picture, only ever one on its own instance.
    #[serde(default)]
    pub avatar_url: String,
    /// In the keychain, never in the file.
    #[serde(skip)]
    pub token: Option<String>,
}

fn token_name(url: &str) -> String {
    format!("token:{url}")
}

/// Where a kept account's token is, beside the instance's own (the one in use).
fn account_token_name(url: &str, user_id: &str) -> String {
    format!("token:{url}#{user_id}")
}

pub fn load_instances(paths: &Paths, secrets: &Secrets) -> Vec<SavedInstance> {
    let mut list: Vec<SavedInstance> = read(&paths.instances()).unwrap_or_default();
    let mut moved = false;
    for saved in &mut list {
        match saved.token.take() {
            // From before tokens moved to the keychain.
            Some(token) => {
                secrets.set(&token_name(&saved.url), &token);
                saved.token = Some(token);
                moved = true;
            }
            None => saved.token = secrets.get(&token_name(&saved.url)),
        }
        for account in &mut saved.accounts {
            account.token = secrets.get(&account_token_name(&saved.url, &account.user_id));
        }
        // An account whose token is gone (the keychain was cleared) can't be switched to.
        saved.accounts.retain(|a| a.token.is_some() && !a.user_id.is_empty());
    }
    if moved {
        store_instances(paths, secrets, &list);
    }
    list
}

pub fn store_instances(paths: &Paths, secrets: &Secrets, list: &[SavedInstance]) {
    let before: Vec<SavedInstance> = read(&paths.instances()).unwrap_or_default();
    for old in &before {
        let now = list.iter().find(|s| s.url == old.url);
        if now.is_none() {
            secrets.delete(&token_name(&old.url));
        }
        for gone in
            old.accounts.iter().filter(|a| !now.is_some_and(|s| s.accounts.iter().any(|b| b.user_id == a.user_id)))
        {
            secrets.delete(&account_token_name(&old.url, &gone.user_id));
        }
    }
    for saved in list {
        match &saved.token {
            Some(token) => secrets.set(&token_name(&saved.url), token),
            None => secrets.delete(&token_name(&saved.url)),
        }
        for account in &saved.accounts {
            if let Some(token) = &account.token {
                secrets.set(&account_token_name(&saved.url, &account.user_id), token);
            }
        }
    }
    let plain: Vec<SavedInstance> = list
        .iter()
        .map(|s| SavedInstance {
            url: s.url.clone(),
            token: None,
            accounts: s.accounts.iter().map(|a| SavedAccount { token: None, ..a.clone() }).collect(),
        })
        .collect();
    if let Err(err) = write_json(&paths.instances(), &plain) {
        tracing::warn!("couldn't save the instance list: {err}");
    }
}

/// Whether things move: as the system says, or always calm.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionChoice {
    #[default]
    System,
    Reduced,
    Full,
}

/// How tightly messages sit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Cozy,
    Compact,
}

/// The 12 or 24 hour clock for message times; `Auto` is the language's own
/// (the web app's `clock` pref).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clock {
    #[default]
    Auto,
    #[serde(rename = "12h")]
    H12,
    #[serde(rename = "24h")]
    H24,
}

/// How much room messages and lists get (the web app's `density`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spacing {
    Compact,
    #[default]
    Default,
    Spacious,
}

/// Where role colors show: on names, as a dot beside them, or not at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleColors {
    #[default]
    Names,
    Beside,
    Off,
}

/// How the microphone decides when you're talking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    Voice,
    Ptt,
}

/// Popped-out cameras fill their window (cropping the edges) or fit in it whole.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PopoutFit {
    #[default]
    Cover,
    Contain,
}

/// Which sounds play (the web app's `sounds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sounds {
    pub message: bool,
    pub mention: bool,
    pub join: bool,
    pub call: bool,
    pub ring: bool,
}

impl Default for Sounds {
    fn default() -> Self {
        Self { message: true, mention: true, join: false, call: true, ring: true }
    }
}

/// What sends a message: Enter (Shift+Enter for a new line), or Ctrl/Cmd+Enter
/// (Enter for a new line); the web app's `sendWith`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SendWith {
    #[default]
    Enter,
    ModEnter,
}

/// The app's settings: this computer's, for every instance (like the web
/// app's `lib/prefs.ts`). Settings of an instance or a server live on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The theme's id (built in, or one of `custom_themes`) when not following the system.
    pub theme: String,
    /// Light or dark like the system, with a theme for each.
    pub follow_system: bool,
    pub light_theme: String,
    pub dark_theme: String,
    /// Themes made or imported on this computer.
    pub custom_themes: Vec<Theme>,
    /// The picture and effect behind the app, under themes that don't bring their own.
    pub backdrop: Backdrop,
    pub motion: MotionChoice,
    pub density: Density,
    /// Hides instance addresses and your username, for streaming or sharing your screen.
    pub streamer_mode: bool,
    /// Text size, as a multiple of the normal size.
    pub text_scale: f32,
    /// Notifications for messages that arrive while you're looking elsewhere:
    /// a card in the window, or the system's own while it's in the background.
    pub notifications: bool,
    /// Which messages notify you in servers whose settings don't say.
    pub notify_for: NotifyFor,
    /// Servers whose welcome screen you've seen, as `instance/server`.
    pub welcomed: std::collections::BTreeSet<String>,
    /// Shortcuts changed from their defaults, by action; None takes one away.
    pub keybinds: std::collections::BTreeMap<String, Option<String>>,
    /// Extra shortcuts for any action, on top of their own.
    pub custom_keybinds: Vec<keybinds::CustomKeybind>,
    /// Sends anonymous counts of errors, slow paths and feature use to your
    /// own instance, while its telemetry is on (see `reports.rs`).
    pub share_reports: bool,
    /// The announcement closed on each instance, by its id, so it stays closed until a new one.
    pub closed_announcements: std::collections::BTreeMap<String, String>,
    /// Fetches and checks new versions of the app in the background, for
    /// "Restart to update" (see `updates.rs`); off, it only says when one is out.
    pub auto_update: bool,
    /// Shared channels whose note at the start was closed, as `instance/channel`.
    pub shared_notes_closed: std::collections::BTreeSet<String>,
    /// Games and apps that report to Discord show what you're doing here too
    /// (`core::presence`), once you allow each one.
    pub game_activity: bool,
    /// Your answer for each game or app that asked, by `Program::key`.
    pub game_answers: std::collections::BTreeMap<String, bool>,
    /// Emoji picked lately, newest first: `u:` and the character, or `c:` and a server emoji's id.
    pub recent_emoji: Vec<String>,
    /// The skin tone for standard emoji: 0 is the default yellow, 1 to 5 light to dark.
    pub skin_tone: u8,
    /// The app's language as a shipped locale code (`core::i18n`); none follows the computer's.
    pub language: Option<String>,
    /// Searches made lately, newest first, by `instance/account/server` (see `search::place`).
    pub recent_searches: std::collections::BTreeMap<String, Vec<String>>,
    /// GIFs sent lately, newest first, by instance (`core::gifs`).
    pub recent_gifs: std::collections::BTreeMap<String, Vec<crate::core::gifs::KeptGif>>,
    /// Shows "Copy … ID" on servers, channels, people and messages.
    pub developer_mode: bool,
    /// The 12 or 24 hour clock for times in chat.
    pub clock: Clock,
    /// Which keys send a message.
    pub send_with: SendWith,
    /// Live tiles turned off everywhere (`core::live_tiles`; the web's `fuwa:live-tiles:off`).
    pub live_tiles_off: bool,
    /// Live tiles someone hid, by id, newest last.
    pub live_tiles_hidden: Vec<String>,
    /// Servers whose live tiles are off, by id.
    pub live_tiles_quiet: std::collections::BTreeSet<String>,
    /// Rail folders open on this computer, as `instance/folder` (the web's `lib/rail-open.ts`).
    pub rail_open: std::collections::BTreeSet<String>,
    /// The sign-in notice closed on each instance, by the way to sign in it was about.
    pub sign_in_notice_closed: std::collections::BTreeMap<String, String>,
    /// How fast voice messages play: 1, 1.5 or 2 (the web's voice rate).
    pub voice_rate: f32,
    /// How much room messages and lists get.
    pub spacing: Spacing,
    /// Message text size, in pixels (12 to 20).
    pub chat_font_size: u8,
    /// The whole app's size, in percent (80 to 150).
    pub zoom: u16,
    /// Effects on other people's profile cards play (your own always shows to you).
    pub others_effects: bool,
    /// Color saturation, in percent.
    pub saturation: u8,
    pub underline_links: bool,
    pub role_colors: RoleColors,
    /// A count of what's unread where the app shows it (the web's tab title).
    pub unread_badge: bool,
    pub sounds: Sounds,
    /// Sound volume, in percent.
    pub volume: u8,
    /// Streamer mode hides addresses and your username.
    pub streamer_hide_personal: bool,
    /// Streamer mode keeps sounds quiet.
    pub streamer_mute_sounds: bool,
    /// Streamer mode keeps notifications quiet.
    pub streamer_mute_notifications: bool,
    /// Voice and audio: devices by name, "" for the system's default.
    pub input_device: String,
    pub output_device: String,
    /// Microphone and call volume, in percent (up to 200).
    pub input_volume: u16,
    pub output_volume: u16,
    pub input_mode: InputMode,
    /// Voice activity picks its level by itself, or uses `sensitivity` (in dB).
    pub auto_sensitivity: bool,
    pub sensitivity: i8,
    /// How long push to talk keeps going after the key comes up, in milliseconds.
    pub ptt_release: u16,
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    pub auto_gain_control: bool,
    /// How loud each person is for you, in percent, by "instance/user id". Missing means 100.
    pub user_volumes: std::collections::BTreeMap<String, u16>,
    /// The camera by name, "" for the system's default.
    pub video_device: String,
    /// Your own camera shows mirrored to you.
    pub mirror_video: bool,
    pub popout_name: bool,
    pub popout_glow: bool,
    pub popout_fit: PopoutFit,
    /// Sharing a screen brings its sound too.
    pub share_sound: bool,
}

/// Which messages notify you, where a server's settings leave it to this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NotifyFor {
    #[default]
    Mentions,
    All,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "sakura".into(),
            follow_system: false,
            light_theme: "sakura".into(),
            dark_theme: "yoru".into(),
            custom_themes: Vec::new(),
            backdrop: Backdrop::default(),
            motion: MotionChoice::System,
            density: Density::Cozy,
            streamer_mode: false,
            text_scale: 1.0,
            notifications: true,
            notify_for: NotifyFor::Mentions,
            welcomed: Default::default(),
            keybinds: Default::default(),
            custom_keybinds: Vec::new(),
            share_reports: true,
            closed_announcements: Default::default(),
            auto_update: true,
            shared_notes_closed: Default::default(),
            game_activity: true,
            game_answers: Default::default(),
            recent_emoji: Vec::new(),
            skin_tone: 0,
            language: None,
            recent_searches: Default::default(),
            recent_gifs: Default::default(),
            developer_mode: false,
            clock: Clock::Auto,
            send_with: SendWith::Enter,
            live_tiles_off: false,
            live_tiles_hidden: Vec::new(),
            live_tiles_quiet: Default::default(),
            rail_open: Default::default(),
            sign_in_notice_closed: Default::default(),
            voice_rate: 1.0,
            spacing: Spacing::Default,
            chat_font_size: 15,
            zoom: 100,
            others_effects: true,
            saturation: 100,
            underline_links: false,
            role_colors: RoleColors::Names,
            unread_badge: true,
            sounds: Sounds::default(),
            volume: 60,
            streamer_hide_personal: true,
            streamer_mute_sounds: true,
            streamer_mute_notifications: true,
            input_device: String::new(),
            output_device: String::new(),
            input_volume: 100,
            output_volume: 100,
            input_mode: InputMode::Voice,
            auto_sensitivity: true,
            sensitivity: -50,
            ptt_release: 200,
            echo_cancellation: true,
            noise_suppression: true,
            auto_gain_control: true,
            user_volumes: Default::default(),
            video_device: String::new(),
            mirror_video: true,
            popout_name: true,
            popout_glow: true,
            popout_fit: PopoutFit::Cover,
            share_sound: true,
        }
    }
}

/// How many recently picked emoji are kept.
pub const MAX_RECENT_EMOJI: usize = 24;

pub fn load_prefs(paths: &Paths) -> Prefs {
    let mut prefs: Prefs = read(&paths.settings()).unwrap_or_default();
    prefs.tidy();
    prefs
}

impl Prefs {
    /// Remembers a search made in one server (or, with `forget`, takes it off the list).
    pub fn remember_search(&mut self, place: &str, query: &str, forget: bool) {
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        let list = self.recent_searches.entry(place.to_owned()).or_default();
        list.retain(|x| x != q);
        if !forget {
            list.insert(0, q.to_owned());
            list.truncate(crate::core::search::MAX_RECENT);
        }
        if list.is_empty() {
            self.recent_searches.remove(place);
        }
    }

    /// Forgets every search made on an instance, whoever made them (signing out).
    pub fn forget_searches(&mut self, key: &str) {
        let prefix = format!("{key}/");
        self.recent_searches.retain(|place, _| !place.starts_with(&prefix));
    }

    /// Settings from an older app, or edited by hand, made into ones this app can show.
    pub fn tidy(&mut self) {
        // Before themes, the choice was light, dark, or the system's.
        match self.theme.as_str() {
            "system" => (self.theme, self.follow_system) = ("sakura".into(), true),
            "light" => (self.theme, self.follow_system) = ("sakura".into(), false),
            "dark" => (self.theme, self.follow_system) = ("yoru".into(), false),
            _ => {}
        }
        self.custom_themes = themes::sanitize_custom(std::mem::take(&mut self.custom_themes));
        self.keybinds.retain(|_, combo| combo.as_deref().is_none_or(keybinds::valid));
        self.custom_keybinds = keybinds::tidy_custom(std::mem::take(&mut self.custom_keybinds));
        self.recent_emoji.truncate(MAX_RECENT_EMOJI);
        for list in self.recent_searches.values_mut() {
            list.truncate(crate::core::search::MAX_RECENT);
        }
        self.recent_searches.retain(|_, list| !list.is_empty());
        for list in self.recent_gifs.values_mut() {
            list.retain(|g| !g.url.is_empty() && !g.seal.is_empty());
            list.truncate(crate::core::gifs::RECENT);
        }
        self.recent_gifs.retain(|_, list| !list.is_empty());
        // Before zoom, the app's size was a text scale.
        if self.zoom == 100 && (self.text_scale - 1.0).abs() > 0.01 {
            self.zoom = ((self.text_scale * 10.0).round() * 10.0) as u16;
        }
        self.zoom = self.zoom.clamp(80, 150);
        self.text_scale = f32::from(self.zoom) / 100.0;
        self.chat_font_size = self.chat_font_size.clamp(12, 20);
        self.saturation = self.saturation.min(100);
        self.volume = self.volume.min(100);
        self.input_volume = self.input_volume.min(200);
        self.output_volume = self.output_volume.min(200);
        self.sensitivity = self.sensitivity.clamp(-100, 0);
        self.ptt_release = self.ptt_release.min(2000);
        self.user_volumes.retain(|_, v| *v <= 200);
        if self.skin_tone > 5 {
            self.skin_tone = 0;
        }
        let known =
            |id: &str| themes::builtins().iter().any(|t| t.id == id) || self.custom_themes.iter().any(|t| t.id == id);
        if !known(&self.theme) {
            self.theme = "sakura".into();
        }
        if !known(&self.light_theme) {
            self.light_theme = "sakura".into();
        }
        if !known(&self.dark_theme) {
            self.dark_theme = "yoru".into();
        }
    }

    /// Whether notifications show: on, and not quieted by streamer mode.
    pub fn notifies(&self) -> bool {
        self.notifications && !(self.streamer_mode && self.streamer_mute_notifications)
    }

    /// Whether sounds play: not quieted by streamer mode.
    pub fn sounds_on(&self) -> bool {
        !(self.streamer_mode && self.streamer_mute_sounds)
    }

    /// Whether streamer mode hides addresses and your username now.
    pub fn hides_personal(&self) -> bool {
        self.streamer_mode && self.streamer_hide_personal
    }

    /// Every theme there is to pick: the built-in ones, then the ones made here.
    pub fn all_themes(&self) -> Vec<Theme> {
        let mut all = themes::builtins();
        all.extend(self.custom_themes.iter().cloned());
        all
    }

    pub fn theme_by_id(&self, id: &str) -> Theme {
        let all = self.all_themes();
        all.iter().find(|t| t.id == id).cloned().unwrap_or_else(|| all[0].clone())
    }

    /// The theme on screen, given whether the system is dark.
    pub fn active_theme(&self, system_dark: bool) -> Theme {
        if !self.follow_system {
            return self.theme_by_id(&self.theme);
        }
        self.theme_by_id(if system_dark { &self.dark_theme } else { &self.light_theme })
    }

    /// What's behind the app: the theme's own backdrop, or the app's.
    pub fn active_backdrop(&self, theme: &Theme) -> Backdrop {
        theme.backdrop.clone().unwrap_or_else(|| self.backdrop.clone())
    }
}

pub fn store_prefs(paths: &Paths, prefs: &Prefs) {
    if let Err(err) = write_json(&paths.settings(), prefs) {
        tracing::warn!("couldn't save the app's settings: {err}");
    }
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!("ignoring {}: {err}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_and_prefs_round_trip() {
        // SAFETY: tests in this binary don't read this variable concurrently with a write.
        unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        let secrets = Secrets::open(&paths.config);
        assert!(load_instances(&paths, &secrets).is_empty());
        assert_eq!(load_prefs(&paths), Prefs::default());
        let alice = SavedAccount {
            user_id: "a".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            avatar_url: String::new(),
            token: Some("t".into()),
        };
        let bob =
            SavedAccount { user_id: "b".into(), username: "bob".into(), token: Some("u".into()), ..alice.clone() };
        let list = vec![SavedInstance {
            url: "https://fuwa.chat".into(),
            token: Some("t".into()),
            accounts: vec![alice.clone(), bob.clone()],
        }];
        store_instances(&paths, &secrets, &list);
        assert_eq!(load_instances(&paths, &secrets), list);
        // Forgetting an account forgets its token.
        let fewer = vec![SavedInstance { accounts: vec![alice], ..list[0].clone() }];
        store_instances(&paths, &secrets, &fewer);
        assert_eq!(secrets.get("token:https://fuwa.chat#b"), None);
        assert_eq!(load_instances(&paths, &secrets), fewer);
        let file = std::fs::read_to_string(home.path().join("config/instances.json")).unwrap();
        assert!(!file.contains("\"u\"") && file.contains("alice"), "{file}");
        store_instances(&paths, &secrets, &list);
        // The token isn't in the instance list's file.
        let file = std::fs::read_to_string(home.path().join("config/instances.json")).unwrap();
        assert!(!file.contains("\"t\""), "{file}");
        // A file from before the keychain hands its tokens over.
        std::fs::write(home.path().join("config/instances.json"), r#"[{"url":"https://old.chat","token":"o"}]"#)
            .unwrap();
        let moved = load_instances(&paths, &secrets);
        assert_eq!(moved[0].token.as_deref(), Some("o"));
        assert!(!std::fs::read_to_string(home.path().join("config/instances.json")).unwrap().contains("\"o\""));
        let prefs = Prefs { theme: "matcha".into(), follow_system: false, streamer_mode: true, ..Prefs::default() };
        store_prefs(&paths, &prefs);
        assert_eq!(load_prefs(&paths), prefs);
        // Settings from a newer app keep what this one knows; an older one's light or dark becomes a theme.
        std::fs::write(home.path().join("config/settings.json"), r#"{"theme":"dark","shiny":true}"#).unwrap();
        let old = load_prefs(&paths);
        assert_eq!((old.theme.as_str(), old.follow_system), ("yoru", false));
        // A file from before reports shares them, as a new one does; turning them off sticks.
        assert!(old.share_reports);
        store_prefs(&paths, &Prefs { share_reports: false, ..Prefs::default() });
        assert!(!load_prefs(&paths).share_reports);
        // Updates install by themselves unless turned off, which sticks.
        assert!(old.auto_update);
        store_prefs(&paths, &Prefs { auto_update: false, ..Prefs::default() });
        assert!(!load_prefs(&paths).auto_update);
        // A made theme that's gone falls back to a built-in one.
        std::fs::write(home.path().join("config/settings.json"), r#"{"theme":"custom-gone1","follow_system":false}"#)
            .unwrap();
        assert_eq!(load_prefs(&paths).active_theme(true).id, "sakura");
    }
}
