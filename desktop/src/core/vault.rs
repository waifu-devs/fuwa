//! Where this app keeps an account's encrypted-messages device and what its
//! conversations said, on this computer: the desktop's version of the web
//! app's IndexedDB vault (`web/src/e2ee/vault.ts`).
//!
//! What a conversation said can't be read from the instance again (it only
//! keeps ciphertext, and a message can be opened once), so this is the only
//! copy of it. Each vault is a folder under the app's data folder, readable
//! only by you, written by swapping in whole files so a crash never leaves
//! half of one. Every file in it is encrypted (XChaCha20-Poly1305) with a key
//! kept in the system keychain, so a copy of the folder alone reads as noise.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A device in a conversation, as a record mentions it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRef {
    pub user_id: String,
    pub device_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A message.
    Text,
    /// Devices came or went.
    Devices,
    /// This device joined the conversation here; nothing before it is readable on it.
    Joined,
    /// A record this device couldn't open.
    Unreadable,
    /// Someone started a secure channel's encryption over.
    Reset,
    /// Someone turned a secure channel's history sharing on ("on") or off ("off"), in `content`.
    Setting,
}

/// A secure channel message as its sender's device signed it (a
/// `SignedContent`'s parts and the device's signature key), kept so it can be
/// passed on to devices added later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signed {
    #[serde(with = "bytes")]
    pub payload: Vec<u8>,
    #[serde(with = "bytes")]
    pub signature: Vec<u8>,
    #[serde(with = "bytes")]
    pub key: Vec<u8>,
}

/// Bytes as base64 in the vault's JSON.
mod bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        B64.decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// One thing a conversation said, as this device read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub seq: i64,
    pub kind: ItemKind,
    /// Unix milliseconds.
    pub at: i64,
    pub sender_id: String,
    pub device_id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub reply_to: i64,
    #[serde(default)]
    pub edited_at: i64,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub added: Vec<DeviceRef>,
    #[serde(default)]
    pub removed: Vec<DeviceRef>,
    /// In a secure channel: the message as its sender signed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed: Option<Signed>,
    /// Its latest edit, signed the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_signed: Option<Signed>,
    /// Who passed it on to this device, when it came as shared history rather than as it was sent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shared_by: String,
    /// A voice message: what's needed to fetch, open and draw it (the text is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceFile>,
}

/// A voice message's sealed file and what was said about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceFile {
    pub media_id: String,
    #[serde(with = "bytes")]
    pub key: Vec<u8>,
    #[serde(with = "bytes")]
    pub sha256: Vec<u8>,
    pub size: i64,
    pub duration_ms: u32,
    #[serde(with = "bytes")]
    pub waveform: Vec<u8>,
}

impl Item {
    pub fn new(seq: i64, kind: ItemKind, at: i64, sender_id: &str, device_id: &str) -> Self {
        Self {
            seq,
            kind,
            at,
            sender_id: sender_id.to_owned(),
            device_id: device_id.to_owned(),
            content: String::new(),
            reply_to: 0,
            edited_at: 0,
            deleted: false,
            added: Vec::new(),
            removed: Vec::new(),
            signed: None,
            edit_signed: None,
            shared_by: String::new(),
            voice: None,
        }
    }
}

/// Where this device is in a conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// The last record read.
    pub cursor: i64,
    /// The last record you've seen.
    pub read: i64,
    /// The safety number you checked with the other person, if you did.
    #[serde(default)]
    pub verified: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Head {
    /// The SHA-256 of the session token the device belongs to: a new sign-in is a new device.
    session: String,
    /// The saved device (`Device::save`), base64.
    device: String,
    notes: HashMap<String, Note>,
    /// What this device sent, by the SHA-256 of the ciphertext, until it reads its own record back.
    sent: HashMap<String, String>,
}

/// One account's vault on one instance.
pub struct Vault {
    dir: PathBuf,
    key: [u8; 32],
    head: Head,
    items: HashMap<String, Vec<Item>>,
}

/// What an encrypted vault file starts with, then a 24-byte nonce, then the sealed JSON.
const SEALED: &[u8] = b"fuwa-vault-v1\n";

fn seal(key: &[u8; 32], plain: &[u8]) -> std::io::Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead as _, KeyInit as _};
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).map_err(std::io::Error::other)?;
    let sealed = cipher.encrypt(&nonce.into(), plain).map_err(|_| std::io::Error::other("couldn't encrypt"))?;
    Ok([SEALED, &nonce, &sealed].concat())
}

/// Opens a sealed file. One from before vaults were encrypted reads as it is,
/// and is sealed the next time it's written.
fn unseal(key: &[u8; 32], bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead as _, KeyInit as _};
    let Some(rest) = bytes.strip_prefix(SEALED) else { return Ok(bytes.to_vec()) };
    if rest.len() < 24 {
        return Err(std::io::Error::other("a vault file is cut short"));
    }
    let (nonce, sealed) = rest.split_at(24);
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(key.into());
    let nonce: [u8; 24] = nonce.try_into().expect("24 bytes");
    cipher
        .decrypt(&nonce.into(), sealed)
        .map_err(|_| std::io::Error::other("a vault file doesn't open with this computer's key"))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

impl Vault {
    /// The vault folder for an account on an instance.
    pub fn dir_for(root: &Path, instance_key: &str, user_id: &str) -> PathBuf {
        root.join(&sha256_hex(format!("{instance_key}|{user_id}").as_bytes())[..32])
    }

    /// Opens the vault, empty if there's none yet. A vault kept for another
    /// session (an earlier sign-in) is wiped: its device is gone from the instance.
    /// One that doesn't open with `key` (the keychain lost it) is wiped too:
    /// nothing in it can be read again.
    pub fn open(dir: PathBuf, session: &str, key: [u8; 32]) -> std::io::Result<Self> {
        let head: Option<Head> = match read_sealed(&key, &dir.join("vault.json")) {
            Ok(head) => head,
            Err(err) => {
                tracing::warn!("starting a new vault: {err}");
                None
            }
        };
        match head {
            Some(head) if head.session == session => Ok(Self { dir, key, head, items: HashMap::new() }),
            _ => {
                wipe(&dir)?;
                Ok(Self {
                    dir,
                    key,
                    head: Head { session: session.to_owned(), ..Head::default() },
                    items: HashMap::new(),
                })
            }
        }
    }

    /// The saved device, if the vault has one.
    pub fn device(&self) -> Option<Vec<u8>> {
        B64.decode(&self.head.device).ok().filter(|d| !d.is_empty())
    }

    pub fn note(&self, conversation: &str) -> Note {
        self.head.notes.get(conversation).cloned().unwrap_or_default()
    }

    pub fn sent(&self, hash: &str) -> Option<Vec<u8>> {
        self.head.sent.get(hash).and_then(|p| B64.decode(p).ok())
    }

    /// A conversation's items, oldest first.
    pub fn items(&mut self, conversation: &str) -> std::io::Result<&mut Vec<Item>> {
        if !self.items.contains_key(conversation) {
            let list: Vec<Item> = read_sealed(&self.key, &self.items_path(conversation))?.unwrap_or_default();
            self.items.insert(conversation.to_owned(), list);
        }
        Ok(self.items.get_mut(conversation).expect("just loaded"))
    }

    /// Forgets everything kept for one conversation or channel: its note and what it said.
    pub fn forget(&mut self, conversation: &str) -> std::io::Result<()> {
        self.items.remove(conversation);
        self.head.notes.remove(conversation);
        match std::fs::remove_file(self.items_path(conversation)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        if self.dir.exists() {
            self.write_sealed(&self.dir.join("vault.json"), &self.head)?;
        }
        Ok(())
    }

    /// The conversations and channels this device has a note for.
    pub fn noted(&self) -> impl Iterator<Item = &String> {
        self.head.notes.keys()
    }

    fn items_path(&self, conversation: &str) -> PathBuf {
        self.dir.join(format!("items-{}.json", &sha256_hex(conversation.as_bytes())[..32]))
    }

    /// Saves the device, notes and sent messages, and the given conversations' items.
    pub fn write(&mut self, change: Change) -> std::io::Result<()> {
        if let Some(device) = change.device {
            self.head.device = B64.encode(device);
        }
        for (conversation, note) in change.notes {
            self.head.notes.insert(conversation, note);
        }
        for (hash, plaintext) in change.sent {
            self.head.sent.insert(hash, B64.encode(plaintext));
        }
        for hash in change.forget_sent {
            self.head.sent.remove(&hash);
        }
        let mut conversations: Vec<String> = Vec::new();
        for (conversation, item) in change.items {
            let list = self.items(&conversation)?;
            match list.binary_search_by_key(&item.seq, |i| i.seq) {
                Ok(at) => list[at] = item,
                Err(at) => list.insert(at, item),
            }
            if !conversations.contains(&conversation) {
                conversations.push(conversation);
            }
        }
        std::fs::create_dir_all(&self.dir)?;
        restrict(&self.dir, true)?;
        for conversation in conversations {
            self.write_sealed(&self.items_path(&conversation), &self.items[&conversation])?;
        }
        self.write_sealed(&self.dir.join("vault.json"), &self.head)
    }

    fn write_sealed<T: Serialize + ?Sized>(&self, path: &Path, value: &T) -> std::io::Result<()> {
        let plain = serde_json::to_vec(value).map_err(std::io::Error::other)?;
        write_file(path, &seal(&self.key, &plain)?)
    }
}

/// What to change in a vault, in one write.
#[derive(Default)]
pub struct Change {
    pub device: Option<Vec<u8>>,
    pub notes: Vec<(String, Note)>,
    pub items: Vec<(String, Item)>,
    pub sent: Vec<(String, Vec<u8>)>,
    pub forget_sent: Vec<String>,
}

/// Forgets a vault: on signing out.
pub fn wipe(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

fn read_sealed<T: for<'de> Deserialize<'de>>(key: &[u8; 32], path: &Path) -> std::io::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&unseal(key, &bytes)?).map(Some).map_err(std::io::Error::other),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Writes a file whole: to a temporary file next to it, then swapped in.
pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    write_file(path, &bytes)
}

pub fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        restrict(&tmp, false)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Makes a folder (and its parents) that only you can open.
pub fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    restrict(path, true)
}

/// Only you can read it (on Unix; Windows keeps a user's app data to them already).
fn restrict(path: &Path, dir: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(if dir { 0o700 } else { 0o600 }))?;
    }
    let _ = (path, dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7; 32];

    #[test]
    fn a_vault_keeps_what_it_was_given_and_forgets_other_sessions() {
        let root = tempfile::tempdir().unwrap();
        let dir = Vault::dir_for(root.path(), "fuwa.chat", "u1");
        let mut vault = Vault::open(dir.clone(), "session-a", KEY).unwrap();
        assert!(vault.device().is_none());
        let mut item = Item::new(2, ItemKind::Text, 5, "u1", "d1");
        item.content = "hello".into();
        vault
            .write(Change {
                device: Some(vec![1, 2, 3]),
                notes: vec![("c".into(), Note { cursor: 2, read: 0, verified: String::new() })],
                items: vec![("c".into(), item.clone()), ("c".into(), Item::new(1, ItemKind::Joined, 1, "u1", "d1"))],
                sent: vec![("h".into(), b"hi".to_vec())],
                forget_sent: vec![],
            })
            .unwrap();

        let mut again = Vault::open(dir.clone(), "session-a", KEY).unwrap();
        assert_eq!(again.device().unwrap(), vec![1, 2, 3]);
        assert_eq!(again.note("c").cursor, 2);
        assert_eq!(again.sent("h").unwrap(), b"hi");
        let seqs: Vec<i64> = again.items("c").unwrap().iter().map(|i| i.seq).collect();
        assert_eq!(seqs, [1, 2]);
        assert_eq!(again.items("c").unwrap()[1], item);

        let mut other = Vault::open(dir.clone(), "session-b", KEY).unwrap();
        assert!(other.device().is_none());
        assert!(other.items("c").unwrap().is_empty());
    }

    #[test]
    fn vault_files_are_sealed_and_need_their_key() {
        let root = tempfile::tempdir().unwrap();
        let dir = Vault::dir_for(root.path(), "fuwa.chat", "u1");
        let mut vault = Vault::open(dir.clone(), "s", KEY).unwrap();
        let mut item = Item::new(1, ItemKind::Text, 5, "u1", "d1");
        item.content = "a secret 🍵".into();
        vault.write(Change { items: vec![("c".into(), item)], ..Change::default() }).unwrap();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            assert!(bytes.starts_with(SEALED));
            assert!(!bytes.windows(8).any(|w| w == b"a secret"));
        }
        // Another key can't read it, and starts over.
        let mut stranger = Vault::open(dir.clone(), "s", [9; 32]).unwrap();
        assert!(stranger.items("c").unwrap().is_empty());
    }

    #[test]
    fn plain_vaults_from_before_still_open() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("v");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vault.json"), r#"{"session":"s","device":"AQID","notes":{},"sent":{}}"#).unwrap();
        let vault = Vault::open(dir, "s", KEY).unwrap();
        assert_eq!(vault.device().unwrap(), vec![1, 2, 3]);
    }
}
