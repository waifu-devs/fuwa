//! What this app keeps on this computer outside the vaults: the instances
//! you added (with a session token for each one you signed in to) and the
//! app's own settings, which apply to every instance.
//!
//! Both are JSON files in the app's config folder (`FUWA_DESKTOP_HOME` moves
//! everything, for tests and portable installs). Tokens never leave this
//! computer except to their own instance, like the web app's localStorage.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// An instance you added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInstance {
    pub url: String,
    pub token: Option<String>,
}

pub fn load_instances(paths: &Paths) -> Vec<SavedInstance> {
    read(&paths.instances()).unwrap_or_default()
}

pub fn store_instances(paths: &Paths, list: &[SavedInstance]) {
    if let Err(err) = write_json(&paths.instances(), list) {
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
    /// A little card for messages that arrive while you're looking elsewhere.
    pub notifications: bool,
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
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        assert!(load_instances(&paths).is_empty());
        assert_eq!(load_prefs(&paths), Prefs::default());
        let list = vec![SavedInstance { url: "https://fuwa.chat".into(), token: Some("t".into()) }];
        store_instances(&paths, &list);
        assert_eq!(load_instances(&paths), list);
        let prefs = Prefs { theme: ThemeChoice::Dark, streamer_mode: true, ..Prefs::default() };
        store_prefs(&paths, &prefs);
        assert_eq!(load_prefs(&paths), prefs);
        // Settings from a newer app keep what this one knows.
        std::fs::write(home.path().join("config/settings.json"), r#"{"theme":"light","shiny":true}"#).unwrap();
        assert_eq!(load_prefs(&paths).theme, ThemeChoice::Light);
    }
}
