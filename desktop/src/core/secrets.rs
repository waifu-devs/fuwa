//! Secrets this computer keeps: each instance's session token and the key
//! that encrypts the vaults. They go in the system's keychain (Keychain on
//! macOS, Credential Manager on Windows, the Secret Service on Linux), never
//! in the app's plain files. Where there's no keychain (a Linux box without
//! a Secret Service, or `FUWA_DESKTOP_KEYCHAIN=off`), they fall back to one
//! file only you can read, and the app says so in the log.
//!
//! macOS asks for the login password for each keychain entry an app reads or
//! writes until it's always allowed, and again for every new build (the app
//! is signed ad hoc), so there all of them live in one entry, read once at
//! startup and written only when something changes. Entries from before that
//! are moved into it the first time they're asked for.
//!
//! Entries are named after the app's data folder, so two copies with their
//! own `FUWA_DESKTOP_HOME` keep apart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::core::vault::{sha256_hex, write_json};

const SERVICE: &str = "fuwa";

/// The one entry holding every secret, on macOS.
const BUNDLE: &str = "secrets";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Store {
    /// The private file.
    File,
    /// One keychain entry per secret (Windows and Linux, which don't ask per
    /// entry; Credential Manager also caps an entry's size).
    Entries,
    /// One keychain entry holding them all (macOS).
    Bundle,
}

pub struct Secrets {
    /// Prefix for this data folder's entries.
    scope: String,
    store: Store,
    /// The fallback file.
    file: PathBuf,
    known: Mutex<Known>,
}

#[derive(Default)]
struct Known {
    /// Every secret for the file and the bundle; those read or written so far
    /// for separate entries.
    values: BTreeMap<String, String>,
    /// Names whose own keychain entry has been looked for already.
    looked: BTreeSet<String>,
}

/// Runs a keychain call on a thread of its own: some backends drive their
/// own async runtime and refuse to run inside another one.
fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| s.spawn(f).join().expect("keychain call panicked"))
}

fn read_entry(entry: String) -> keyring::Result<String> {
    off_runtime(move || keyring::Entry::new(SERVICE, &entry).and_then(|e| e.get_password()))
}

fn write_entry(entry: String, value: String) {
    off_runtime(move || {
        if let Err(err) = keyring::Entry::new(SERVICE, &entry).and_then(|e| e.set_password(&value)) {
            tracing::warn!("couldn't keep a secret in the keychain: {err}");
        }
    });
}

fn delete_entry(entry: String) {
    off_runtime(move || match keyring::Entry::new(SERVICE, &entry).and_then(|e| e.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(err) => tracing::warn!("couldn't remove a secret from the keychain: {err}"),
    });
}

impl Secrets {
    pub fn open(config: &Path) -> Self {
        let scope = sha256_hex(config.to_string_lossy().as_bytes())[..12].to_owned();
        let file = config.join("secrets.json");
        let wanted = std::env::var("FUWA_DESKTOP_KEYCHAIN").map(|v| v != "off").unwrap_or(true);
        let mut known = Known::default();
        let store = if !wanted {
            Store::File
        } else if cfg!(target_os = "macos") {
            match read_entry(format!("{scope}:{BUNDLE}")) {
                Ok(json) => {
                    known.values = serde_json::from_str(&json).unwrap_or_else(|err| {
                        tracing::warn!("the keychain's secrets didn't read ({err}); starting over");
                        BTreeMap::new()
                    });
                    Store::Bundle
                }
                Err(keyring::Error::NoEntry) => Store::Bundle,
                Err(err) => {
                    tracing::warn!("no system keychain ({err}); keeping secrets in a private file instead");
                    Store::File
                }
            }
        } else {
            match read_entry(format!("{scope}:probe")) {
                Ok(_) | Err(keyring::Error::NoEntry) => Store::Entries,
                Err(err) => {
                    tracing::warn!("no system keychain ({err}); keeping secrets in a private file instead");
                    Store::File
                }
            }
        };
        if store == Store::File {
            known.values =
                std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        }
        Self { scope, store, file, known: Mutex::new(known) }
    }

    /// Whether secrets are in the system keychain (rather than the private file).
    pub fn in_keychain(&self) -> bool {
        self.store != Store::File
    }

    fn entry(&self, name: &str) -> String {
        format!("{}:{name}", self.scope)
    }

    pub fn get(&self, name: &str) -> Option<String> {
        let mut known = self.known.lock();
        if let Some(value) = known.values.get(name) {
            return Some(value.clone());
        }
        // The file holds everything; so does the bundle, but for entries from before it.
        if self.store == Store::File || !known.looked.insert(name.to_owned()) {
            return None;
        }
        let value = match read_entry(self.entry(name)) {
            Ok(value) => value,
            Err(keyring::Error::NoEntry) => return None,
            Err(err) => {
                tracing::warn!("couldn't read a secret from the keychain: {err}");
                return None;
            }
        };
        known.values.insert(name.to_owned(), value.clone());
        if self.store == Store::Bundle {
            self.save(&known);
            delete_entry(self.entry(name));
        }
        Some(value)
    }

    pub fn set(&self, name: &str, value: &str) {
        let mut known = self.known.lock();
        let looked = !known.looked.insert(name.to_owned());
        if known.values.get(name).map(String::as_str) == Some(value) {
            return;
        }
        known.values.insert(name.to_owned(), value.to_owned());
        match self.store {
            Store::Entries => write_entry(self.entry(name), value.to_owned()),
            Store::File | Store::Bundle => self.save(&known),
        }
        if self.store == Store::Bundle && !looked {
            delete_entry(self.entry(name));
        }
    }

    pub fn delete(&self, name: &str) {
        let mut known = self.known.lock();
        let looked = !known.looked.insert(name.to_owned());
        let had = known.values.remove(name).is_some();
        match self.store {
            Store::File | Store::Bundle if had => self.save(&known),
            Store::Entries if had || !looked => delete_entry(self.entry(name)),
            _ => {}
        }
        if self.store == Store::Bundle && !looked {
            delete_entry(self.entry(name));
        }
    }

    fn save(&self, known: &Known) {
        match self.store {
            Store::File => {
                if let Err(err) = write_json(&self.file, &known.values) {
                    tracing::warn!("couldn't save secrets: {err}");
                }
            }
            Store::Bundle => match serde_json::to_string(&known.values) {
                Ok(json) => write_entry(self.entry(BUNDLE), json),
                Err(err) => tracing::warn!("couldn't save secrets: {err}"),
            },
            Store::Entries => {}
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
