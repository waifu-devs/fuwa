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

use crate::core::secrets::Secrets;
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInstance {
    pub url: String,
    /// Only ever read from older files, which kept it here: it's moved to the keychain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

fn token_name(url: &str) -> String {
    format!("token:{url}")
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
    }
    if moved {
        store_instances(paths, secrets, &list);
    }
    list
}

pub fn store_instances(paths: &Paths, secrets: &Secrets, list: &[SavedInstance]) {
    let before: Vec<SavedInstance> = read(&paths.instances()).unwrap_or_default();
    for gone in before.iter().filter(|b| !list.iter().any(|s| s.url == b.url)) {
        secrets.delete(&token_name(&gone.url));
    }
    for saved in list {
        match &saved.token {
            Some(token) => secrets.set(&token_name(&saved.url), token),
            None => secrets.delete(&token_name(&saved.url)),
        }
    }
    let plain: Vec<SavedInstance> = list.iter().map(|s| SavedInstance { url: s.url.clone(), token: None }).collect();
    if let Err(err) = write_json(&paths.instances(), &plain) {
        tracing::warn!("couldn't save the instance list: {err}");
    }
}

/// Light, dark, or whatever the system uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
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

/// The app's settings: this computer's, for every instance (like the web
/// app's `lib/prefs.ts`). Settings of an instance or a server live on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemeChoice,
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
            theme: ThemeChoice::System,
            motion: MotionChoice::System,
            density: Density::Cozy,
            streamer_mode: false,
            text_scale: 1.0,
            notifications: true,
            notify_for: NotifyFor::Mentions,
            welcomed: Default::default(),
        }
    }
}

pub fn load_prefs(paths: &Paths) -> Prefs {
    read(&paths.settings()).unwrap_or_default()
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
        let list = vec![SavedInstance { url: "https://fuwa.chat".into(), token: Some("t".into()) }];
        store_instances(&paths, &secrets, &list);
        assert_eq!(load_instances(&paths, &secrets), list);
        // The token isn't in the instance list's file.
        let file = std::fs::read_to_string(home.path().join("config/instances.json")).unwrap();
        assert!(!file.contains("\"t\""), "{file}");
        // A file from before the keychain hands its tokens over.
        std::fs::write(home.path().join("config/instances.json"), r#"[{"url":"https://old.chat","token":"o"}]"#)
            .unwrap();
        let moved = load_instances(&paths, &secrets);
        assert_eq!(moved[0].token.as_deref(), Some("o"));
        assert!(!std::fs::read_to_string(home.path().join("config/instances.json")).unwrap().contains("\"o\""));
        let prefs = Prefs { theme: ThemeChoice::Dark, streamer_mode: true, ..Prefs::default() };
        store_prefs(&paths, &prefs);
        assert_eq!(load_prefs(&paths), prefs);
        // Settings from a newer app keep what this one knows.
        std::fs::write(home.path().join("config/settings.json"), r#"{"theme":"light","shiny":true}"#).unwrap();
        assert_eq!(load_prefs(&paths).theme, ThemeChoice::Light);
    }
}
