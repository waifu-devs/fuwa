//! Secrets this computer keeps: each instance's session token and the key
//! that encrypts the vaults. They go in the system's keychain (Keychain on
//! macOS, Credential Manager on Windows, the Secret Service on Linux), never
//! in the app's plain files. Where there's no keychain (a Linux box without
//! a Secret Service, or `FUWA_DESKTOP_KEYCHAIN=off`), they fall back to one
//! file only you can read, and the app says so in the log.
//!
//! Entries are named after the app's data folder, so two copies with their
//! own `FUWA_DESKTOP_HOME` keep apart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::core::vault::{sha256_hex, write_json};

const SERVICE: &str = "fuwa";

pub struct Secrets {
    /// Prefix for this data folder's entries.
    scope: String,
    keychain: bool,
    /// The fallback file, and what's in it.
    file: PathBuf,
    fallback: Mutex<BTreeMap<String, String>>,
}

/// Runs a keychain call on a thread of its own: some backends drive their
/// own async runtime and refuse to run inside another one.
fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| s.spawn(f).join().expect("keychain call panicked"))
}

impl Secrets {
    pub fn open(config: &Path) -> Self {
        let scope = sha256_hex(config.to_string_lossy().as_bytes())[..12].to_owned();
        let file = config.join("secrets.json");
        let wanted = std::env::var("FUWA_DESKTOP_KEYCHAIN").map(|v| v != "off").unwrap_or(true);
        let keychain = wanted && {
            let probe = format!("{scope}:probe");
            off_runtime(move || match keyring::Entry::new(SERVICE, &probe).and_then(|e| e.get_password()) {
                Ok(_) | Err(keyring::Error::NoEntry) => true,
                Err(err) => {
                    tracing::warn!("no system keychain ({err}); keeping secrets in a private file instead");
                    false
                }
            })
        };
        let fallback =
            std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { scope, keychain, file, fallback: Mutex::new(fallback) }
    }

    /// Whether secrets are in the system keychain (rather than the private file).
    pub fn in_keychain(&self) -> bool {
        self.keychain
    }

    fn entry(&self, name: &str) -> String {
        format!("{}:{name}", self.scope)
    }

    pub fn get(&self, name: &str) -> Option<String> {
        if !self.keychain {
            return self.fallback.lock().get(name).cloned();
        }
        let entry = self.entry(name);
        off_runtime(move || match keyring::Entry::new(SERVICE, &entry).and_then(|e| e.get_password()) {
            Ok(value) => Some(value),
            Err(keyring::Error::NoEntry) => None,
            Err(err) => {
                tracing::warn!("couldn't read a secret from the keychain: {err}");
                None
            }
        })
    }

    pub fn set(&self, name: &str, value: &str) {
        if !self.keychain {
            let mut map = self.fallback.lock();
            map.insert(name.to_owned(), value.to_owned());
            self.save(&map);
            return;
        }
        let (entry, value) = (self.entry(name), value.to_owned());
        off_runtime(move || {
            if let Err(err) = keyring::Entry::new(SERVICE, &entry).and_then(|e| e.set_password(&value)) {
                tracing::warn!("couldn't keep a secret in the keychain: {err}");
            }
        });
    }

    pub fn delete(&self, name: &str) {
        if !self.keychain {
            let mut map = self.fallback.lock();
            if map.remove(name).is_some() {
                self.save(&map);
            }
            return;
        }
        let entry = self.entry(name);
        off_runtime(move || match keyring::Entry::new(SERVICE, &entry).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => tracing::warn!("couldn't remove a secret from the keychain: {err}"),
        });
    }

    fn save(&self, map: &BTreeMap<String, String>) {
        if let Err(err) = write_json(&self.file, map) {
            tracing::warn!("couldn't save secrets: {err}");
        }
    }

    /// The key the vaults are encrypted with, made the first time.
    pub fn vault_key(&self) -> [u8; 32] {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        if let Some(key) = self.get("vault-key").and_then(|k| b64.decode(k).ok()).and_then(|k| k.try_into().ok()) {
            return key;
        }
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("the system's random numbers");
        self.set("vault-key", &b64.encode(key));
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_keychain_secrets_stay_in_a_private_file() {
        // SAFETY: tests in this binary don't read this variable concurrently with a write.
        unsafe { std::env::set_var("FUWA_DESKTOP_KEYCHAIN", "off") };
        let dir = tempfile::tempdir().unwrap();
        let secrets = Secrets::open(dir.path());
        assert!(!secrets.in_keychain());
        secrets.set("token:https://fuwa.chat", "t1");
        let key = secrets.vault_key();
        let again = Secrets::open(dir.path());
        assert_eq!(again.get("token:https://fuwa.chat").as_deref(), Some("t1"));
        assert_eq!(again.vault_key(), key);
        again.delete("token:https://fuwa.chat");
        assert_eq!(Secrets::open(dir.path()).get("token:https://fuwa.chat"), None);
    }
}
