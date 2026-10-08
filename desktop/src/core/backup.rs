//! The account's message backup, as this device takes part in it (the web's
//! `e2ee/backup.ts` and `backupkey.ts`, the same format byte for byte): every
//! line it reads in direct messages and secure channels goes, a few seconds
//! later, into an encrypted part on the instance; a new device with the
//! recovery key reads them all back. The key is 32 random bytes that never
//! leave the person's devices; from it come the key check the instance keeps
//! and the AES-256-GCM key each part is sealed with (HKDF-SHA256). See
//! docs/e2ee.md.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use prost::Message as _;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf;
use sha2::{Digest, Sha256};
use tonic::Code;

use crate::core::api::Problem;
use crate::core::dms::DmEngine;
use crate::core::i18n::t;
use crate::core::vault::{DeviceRef, Item, ItemKind, Signed, VoiceFile};
use crate::pb;
use crate::rpc;

const SALT: &[u8] = b"fuwa backup v1";
/// Crockford's base 32: no I, L, O or U.
const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const KEY_BYTES: usize = 32;
const SUM_BYTES: usize = 3;
const NONCE_BYTES: usize = 12;
/// Parts are padded to a multiple of this.
pub const PAD_TO: usize = 4096;
/// How long after a new line the backup takes it, so a burst goes in one part.
const SETTLE: Duration = Duration::from_secs(4);
/// The most plaintext in one part.
const PART_BYTES: usize = 200 * 1024;
/// Lines read at once while making parts.
const BATCH: usize = 400;

// ───────────────────────── The key ─────────────────────────

pub fn new_key() -> Vec<u8> {
    let mut key = vec![0u8; KEY_BYTES];
    let _ = getrandom::fill(&mut key);
    key
}

fn checksum(key: &[u8]) -> Vec<u8> {
    Sha256::digest(key)[..SUM_BYTES].to_vec()
}

/// The key as people write it down: 56 characters in groups of four.
pub fn format_key(key: &[u8]) -> String {
    let mut bytes = key.to_vec();
    bytes.extend(checksum(key));
    let (mut bits, mut value, mut out) = (0u32, 0u32, String::new());
    for byte in bytes {
        value = ((value << 8) | u32::from(byte)) & 0xffff;
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((value >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((value << (5 - bits)) & 31) as usize] as char);
    }
    out.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

/// Reads a key back, forgiving spaces, dashes, case and the letters people confuse with digits.
pub fn parse_key(text: &str) -> Option<Vec<u8>> {
    let clean: String = text
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    let (mut bits, mut value, mut bytes) = (0u32, 0u32, Vec::new());
    for ch in clean.bytes() {
        let n = ALPHABET.iter().position(|a| *a == ch)? as u32;
        value = ((value << 5) | n) & 0xffff;
        bits += 5;
        if bits >= 8 {
            bytes.push(((value >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
    }
    if bytes.len() != KEY_BYTES + SUM_BYTES {
        return None;
    }
    let key = bytes[..KEY_BYTES].to_vec();
    (checksum(&key) == bytes[KEY_BYTES..]).then_some(key)
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// The key check the instance keeps, and the key parts are sealed with.
pub struct Keys {
    pub check: Vec<u8>,
    seal: Vec<u8>,
}

pub fn derive(key: &[u8]) -> Keys {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, SALT).extract(key);
    let expand = |info: &[u8]| {
        let mut out = vec![0u8; 32];
        if let Ok(okm) = prk.expand(&[info], Len(32)) {
            let _ = okm.fill(&mut out);
        }
        out
    };
    Keys { check: expand(b"check"), seal: expand(b"encrypt") }
}

fn associated(account_id: &str, sequence: i64) -> Vec<u8> {
    format!("fuwa backup v1|{account_id}|{sequence}").into_bytes()
}

/// A nonce, then the sealed bytes.
pub fn seal(keys: &Keys, account_id: &str, sequence: i64, plain: &[u8]) -> Option<Vec<u8>> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &keys.seal).ok()?);
    let mut nonce = [0u8; NONCE_BYTES];
    getrandom::fill(&mut nonce).ok()?;
    let mut data = plain.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(associated(account_id, sequence)),
        &mut data,
    )
    .ok()?;
    let mut out = nonce.to_vec();
    out.extend(data);
    Some(out)
}

/// What a part said, or None if it wasn't sealed with these keys for this account and place.
pub fn open(keys: &Keys, account_id: &str, sequence: i64, data: &[u8]) -> Option<Vec<u8>> {
    if data.len() <= NONCE_BYTES {
        return None;
    }
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &keys.seal).ok()?);
    let nonce: [u8; NONCE_BYTES] = data[..NONCE_BYTES].try_into().ok()?;
    let mut sealed = data[NONCE_BYTES..].to_vec();
    let plain = key
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(associated(account_id, sequence)), &mut sealed)
        .ok()?;
    Some(plain.to_vec())
}

fn varint_bytes(n: usize) -> usize {
    if n < 0x80 {
        1
    } else if n < 0x4000 {
        2
    } else if n < 0x20_0000 {
        3
    } else {
        4
    }
}

/// How many zeros the padding field holds so a `bare`-byte part comes to a multiple of PAD_TO.
pub fn padding_for(bare: usize) -> usize {
    let mut total = (bare + 2).div_ceil(PAD_TO) * PAD_TO;
    loop {
        for len in 1..=4 {
            if let Some(zeros) = total.checked_sub(bare + 1 + len)
                && varint_bytes(zeros) == len
            {
                return zeros;
            }
        }
        total += PAD_TO;
    }
}

/// Whether a line from the backup should replace the one this device has.
fn newer(have: Option<&Item>, incoming: &Item) -> bool {
    let Some(have) = have else { return true };
    if have.kind == ItemKind::Unreadable {
        return true;
    }
    if have.kind != incoming.kind || have.deleted {
        return false;
    }
    incoming.deleted || incoming.edited_at > have.edited_at
}

// ───────────────────────── Lines ─────────────────────────

fn signed_form(s: &Option<Signed>) -> Option<pb::SignedForm> {
    s.as_ref().map(|s| pb::SignedForm {
        payload: s.payload.clone(),
        signature: s.signature.clone(),
        signature_key: s.key.clone(),
    })
}

fn from_signed(s: Option<pb::SignedForm>) -> Option<Signed> {
    s.filter(|s| !s.payload.is_empty()).map(|s| Signed {
        payload: s.payload,
        signature: s.signature,
        key: s.signature_key,
    })
}

fn to_backup(conversation: &str, i: &Item) -> Option<pb::BackupItem> {
    let kind = match (i.kind, i.voice.is_some()) {
        (ItemKind::Text, true) => pb::BackupItemKind::Voice,
        (ItemKind::Text, false) => pb::BackupItemKind::Text,
        (ItemKind::Devices, _) => pb::BackupItemKind::Devices,
        (ItemKind::Reset, _) => pb::BackupItemKind::Reset,
        (ItemKind::Setting, _) => pb::BackupItemKind::Setting,
        (ItemKind::Thread, _) => pb::BackupItemKind::Thread,
        _ => return None,
    };
    let device = |d: &DeviceRef| pb::BackupDevice { user_id: d.user_id.clone(), device_id: d.device_id.clone() };
    Some(pb::BackupItem {
        conversation_id: conversation.to_owned(),
        sequence: i.seq,
        at_ms: i.at,
        sender_id: i.sender_id.clone(),
        device_id: i.device_id.clone(),
        kind: kind as i32,
        content: i.content.clone(),
        reply_to_sequence: i.reply_to,
        edited_at_ms: i.edited_at,
        deleted: i.deleted,
        added: i.added.iter().map(device).collect(),
        removed: i.removed.iter().map(device).collect(),
        // A deleted line's signed copies hold its text, so they never go in the backup.
        signed: if i.deleted { None } else { signed_form(&i.signed) },
        edit_signed: if i.deleted { None } else { signed_form(&i.edit_signed) },
        shared_by: i.shared_by.clone(),
        voice: i.voice.as_ref().filter(|_| !i.deleted).map(|v| pb::DirectMessageVoice {
            file: Some(pb::SealedFile {
                media_id: v.media_id.clone(),
                key: v.key.clone(),
                sha256: v.sha256.clone(),
                size: v.size,
                ..Default::default()
            }),
            duration_ms: v.duration_ms,
            waveform: v.waveform.clone(),
            reply_to_sequence: i.reply_to,
        }),
        files: if i.deleted { Vec::new() } else { crate::core::sealed_files::to_sealed(&i.files) },
        thread_sequence: i.thread,
        in_channel: i.in_channel,
        locked: i.kind == ItemKind::Thread && i.content == "locked",
    })
}

fn from_backup(b: pb::BackupItem) -> Option<(String, Item)> {
    let kind = match pb::BackupItemKind::try_from(b.kind).ok()? {
        pb::BackupItemKind::Text | pb::BackupItemKind::Voice => ItemKind::Text,
        pb::BackupItemKind::Devices => ItemKind::Devices,
        pb::BackupItemKind::Reset => ItemKind::Reset,
        pb::BackupItemKind::Setting => ItemKind::Setting,
        pb::BackupItemKind::Thread => ItemKind::Thread,
        _ => return None,
    };
    if b.conversation_id.is_empty() || b.sequence <= 0 {
        return None;
    }
    // Thread replies and locks only exist in secure channels, whose lines are signed (a deleted
    // reply keeps its thread but not its signed copy); a device takes a lock only signed.
    let threaded = b.thread_sequence > 0 && (b.signed.is_some() || b.deleted);
    if kind == ItemKind::Thread && !(b.thread_sequence > 0 && b.signed.is_some()) {
        return None;
    }
    let voice = (b.kind == pb::BackupItemKind::Voice as i32 && !b.deleted)
        .then(|| {
            let v = b.voice.clone()?;
            let f = v.file?;
            Some(VoiceFile {
                media_id: f.media_id,
                key: f.key,
                sha256: f.sha256,
                size: f.size,
                duration_ms: v.duration_ms,
                waveform: v.waveform,
            })
        })
        .flatten();
    if b.kind == pb::BackupItemKind::Voice as i32 && voice.is_none() && !b.deleted {
        return None;
    }
    let device = |d: pb::BackupDevice| DeviceRef { user_id: d.user_id, device_id: d.device_id };
    let mut item = Item::new(b.sequence, kind, b.at_ms, &b.sender_id, &b.device_id);
    item.content = if b.deleted { String::new() } else { b.content };
    item.reply_to = b.reply_to_sequence;
    item.edited_at = b.edited_at_ms;
    item.deleted = b.deleted;
    item.added = b.added.into_iter().map(device).collect();
    item.removed = b.removed.into_iter().map(device).collect();
    if !b.deleted {
        item.files = crate::core::sealed_files::files_of(&b.files);
    }
    if !b.deleted {
        item.signed = from_signed(b.signed);
        item.edit_signed = from_signed(b.edit_signed);
    }
    item.shared_by = b.shared_by;
    if threaded {
        item.thread = b.thread_sequence;
        item.in_channel = b.in_channel;
    }
    if kind == ItemKind::Thread {
        item.content = if b.locked { "locked" } else { "unlocked" }.into();
    }
    item.voice = voice;
    Some((b.conversation_id, item))
}

/// A part's plaintext, padded to an exact multiple of PAD_TO.
fn encode_part(items: Vec<pb::BackupItem>) -> Vec<u8> {
    let bare = pb::BackupPart { items: items.clone(), padding: Vec::new() }.encode_to_vec().len();
    pb::BackupPart { items, padding: vec![0; padding_for(bare)] }.encode_to_vec()
}

// ───────────────────────── Taking part ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Not known yet.
    #[default]
    Loading,
    Unsupported,
    Off,
    /// The account has one, and this device needs its key.
    Locked,
    On,
    Full,
    Restoring,
}

/// Where this device stands with the backup, for the Devices page.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub status: Status,
    pub problem: Option<String>,
    pub size: i64,
    pub max_size: i64,
    pub updated_at: i64,
    pub restored: i64,
    pub total: i64,
    next: i64,
}

static STATES: parking_lot::Mutex<Option<HashMap<String, State>>> = parking_lot::Mutex::new(None);

pub fn state(key: &str) -> State {
    STATES.lock().as_ref().and_then(|m| m.get(key).cloned()).unwrap_or_default()
}

fn update(key: &str, f: impl FnOnce(&mut State)) {
    let mut all = STATES.lock();
    f(all.get_or_insert_with(HashMap::new).entry(key.to_owned()).or_default());
}

fn show_backup(key: &str, b: Option<&pb::Backup>, status: Status, problem: Option<String>) {
    update(key, |s| {
        if let Some(b) = b {
            s.next = b.next_sequence;
        }
        s.status = status;
        s.problem = problem;
        s.size = b.map_or(0, |b| b.size);
        s.max_size = b.map_or(0, |b| b.max_size);
        s.updated_at = b.and_then(|b| b.updated_at.as_ref()).map_or(0, |t| t.seconds * 1000);
    });
}

fn missing() -> Problem {
    Problem::new(Code::NotFound, "Direct messages aren't running on this instance.")
}

impl DmEngine {
    async fn get_backup(&self) -> Result<Option<pb::Backup>, Problem> {
        Ok(rpc!(self.api.dms(), get_backup(pb::GetBackupRequest {})).await?.backup)
    }

    /// Works out where this device stands: in the backup, or not.
    pub async fn backup_start(&self) -> Result<(), Problem> {
        let backup = match self.get_backup().await {
            Ok(b) => b,
            Err(e) if e.code == Code::Unimplemented => {
                update(&self.key, |s| s.status = Status::Unsupported);
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let mut inner = self.inner.lock().await;
        let stored = inner.vault.backup_key();
        let Some(backup) = backup else {
            if stored.is_some() {
                let _ = inner.vault.drop_backup();
            }
            show_backup(&self.key, None, Status::Off, None);
            return Ok(());
        };
        match stored {
            Some((_, check)) if check == backup.key_check => show_backup(&self.key, Some(&backup), Status::On, None),
            Some(_) => {
                let _ = inner.vault.drop_backup();
                show_backup(&self.key, Some(&backup), Status::Locked, None);
            }
            None => show_backup(&self.key, Some(&backup), Status::Locked, None),
        }
        Ok(())
    }

    /// Puts every line waiting for the backup into parts.
    pub async fn backup_flush(&self) {
        loop {
            let mut inner = self.inner.lock().await;
            let Some((key, check)) = inner.vault.backup_key() else { return };
            let Ok(waiting) = inner.vault.unbacked(BATCH) else { return };
            if waiting.is_empty() {
                return;
            }
            let keys = derive(&key);
            if keys.check != check {
                return;
            }
            let mut items = Vec::new();
            let mut taken = Vec::new();
            let mut size = 0;
            for ((conversation, seq), item) in waiting {
                let b = item.as_ref().and_then(|i| to_backup(&conversation, i));
                let bytes = b.as_ref().map_or(0, |b| b.encoded_len() + 4);
                if !items.is_empty() && size + bytes > PART_BYTES {
                    break;
                }
                taken.push((conversation, seq));
                if let Some(b) = b {
                    items.push(b);
                    size += bytes;
                }
            }
            drop(inner);
            if !items.is_empty() {
                let plain = encode_part(items);
                let mut sent = false;
                for _ in 0..4 {
                    let sequence = state(&self.key).next.max(1);
                    let Some(data) = seal(&keys, &self.me.id, sequence, &plain) else { return };
                    let req = pb::AddBackupPartRequest { key_check: keys.check.clone(), data, sequence };
                    match rpc!(self.api.dms(), add_backup_part(req)).await {
                        Ok(res) => {
                            show_backup(&self.key, res.backup.as_ref(), Status::On, None);
                            sent = true;
                            break;
                        }
                        Err(e) if e.code == Code::AlreadyExists => {
                            // Another device took that place: seal it again for the next one.
                            match self.get_backup().await {
                                Ok(Some(b)) => update(&self.key, |s| s.next = b.next_sequence),
                                _ => return,
                            }
                        }
                        Err(e) if e.code == Code::FailedPrecondition => {
                            // Started over (or turned off) on another device: this key is no good now.
                            let _ = self.inner.lock().await.vault.drop_backup();
                            let _ = self.backup_start().await;
                            return;
                        }
                        Err(e) if e.code == Code::ResourceExhausted => {
                            crate::core::reports::error("e2ee.backup_full", "backup");
                            update(&self.key, |s| {
                                s.status = Status::Full;
                                s.problem = Some(e.message.clone());
                            });
                            return;
                        }
                        Err(e) => {
                            crate::core::reports::error("e2ee.backup_failed", "backup");
                            update(&self.key, |s| s.problem = Some(e.message.clone()));
                            return;
                        }
                    }
                }
                if !sent {
                    return;
                }
            }
            if self.inner.lock().await.vault.mark_backed(&taken).is_err() {
                return;
            }
        }
    }

    /// Starts a backup with a new recovery key, holding everything this device kept.
    /// `replace` deletes one the account already has. The key, as it's written down.
    pub async fn backup_create(&self, replace: bool) -> Result<String, Problem> {
        let key = new_key();
        let keys = derive(&key);
        let res = rpc!(self.api.dms(), start_backup(pb::StartBackupRequest { key_check: keys.check.clone(), replace }))
            .await?;
        self.inner
            .lock()
            .await
            .vault
            .keep_backup(&key, &keys.check, true)
            .map_err(|e| Problem::new(Code::Internal, e.to_string()))?;
        show_backup(&self.key, res.backup.as_ref(), Status::On, None);
        Ok(format_key(&key))
    }

    /// Reads the backup back with its recovery key, keeping every line this device has no
    /// newer copy of, and from then on adds to it.
    pub async fn backup_restore(&self, text: &str) -> Result<(), Problem> {
        let key = parse_key(text).ok_or_else(|| Problem::new(Code::InvalidArgument, t("system.backup.notAKey")))?;
        let keys = derive(&key);
        let backup = self.get_backup().await?.ok_or_else(|| Problem::new(Code::NotFound, t("system.backup.gone")))?;
        if backup.key_check != keys.check {
            return Err(Problem::new(Code::InvalidArgument, t("system.backup.wrongKey")));
        }
        update(&self.key, |s| {
            s.status = Status::Restoring;
            s.restored = 0;
            s.total = backup.parts;
            s.problem = None;
        });
        let mut touched: HashSet<String> = HashSet::new();
        let (mut after, mut done, mut unreadable, mut lost) = (0i64, 0i64, 0i64, 0i64);
        let result: Result<(), Problem> = async {
            self.inner
                .lock()
                .await
                .vault
                .keep_backup(&key, &keys.check, true)
                .map_err(|e| Problem::new(Code::Internal, e.to_string()))?;
            loop {
                let res = rpc!(
                    self.api.dms(),
                    list_backup_parts(pb::ListBackupPartsRequest { after_sequence: after, limit: 50 })
                )
                .await?;
                for part in &res.parts {
                    if part.sequence > after + 1 {
                        lost += part.sequence - after - 1;
                    }
                    after = part.sequence;
                    done += 1;
                    let Some(plain) = open(&keys, &self.me.id, part.sequence, &part.data) else {
                        unreadable += 1;
                        continue;
                    };
                    let Ok(decoded) = pb::BackupPart::decode(plain.as_slice()) else {
                        unreadable += 1;
                        continue;
                    };
                    let items: Vec<(String, Item)> = decoded.items.into_iter().filter_map(from_backup).collect();
                    let mut inner = self.inner.lock().await;
                    let mut take = Vec::new();
                    for (conversation, item) in items {
                        let have = inner.vault.item_at(&conversation, item.seq).ok().flatten();
                        if newer(have.as_ref(), &item) {
                            touched.insert(conversation.clone());
                            take.push((conversation, item));
                        }
                    }
                    if !take.is_empty() {
                        inner.vault.write_restored(take).map_err(|e| Problem::new(Code::Internal, e.to_string()))?;
                    }
                }
                update(&self.key, |s| s.restored = done);
                if !res.has_more || res.parts.is_empty() {
                    break;
                }
            }
            if backup.next_sequence > after + 1 {
                lost += backup.next_sequence - after - 1;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = self.inner.lock().await.vault.drop_backup();
            show_backup(&self.key, Some(&backup), Status::Locked, None);
            return Err(e);
        }
        if unreadable > 0 {
            crate::core::reports::error("e2ee.backup_unreadable", "backup");
        }
        let gone = unreadable + lost;
        let problem = (gone > 0).then(|| {
            crate::core::i18n::t_with(
                if lost > 0 { "system.backup.lostMissing" } else { "system.backup.lostUnreadable" },
                &[("count", crate::core::i18n::Arg::Num(gone))],
            )
        });
        show_backup(&self.key, Some(&backup), Status::On, problem);
        for id in touched {
            self.refresh(&id).await;
        }
        Ok(())
    }

    /// Deletes the account's backup, for every device.
    pub async fn backup_remove(&self) -> Result<(), Problem> {
        rpc!(self.api.dms(), delete_backup(pb::DeleteBackupRequest {})).await?;
        let _ = self.inner.lock().await.vault.drop_backup();
        show_backup(&self.key, None, Status::Off, None);
        Ok(())
    }
}

/// Keeps a device in the backup: learns where it stands, then takes new lines a few seconds
/// after they're written, until the engine stops.
pub async fn run(engine: Arc<DmEngine>) {
    if engine.backup_start().await.is_err() {
        update(&engine.key, |s| s.status = Status::Off);
    }
    loop {
        tokio::time::sleep(SETTLE).await;
        if engine.stopped() {
            return;
        }
        if matches!(state(&engine.key).status, Status::On) {
            engine.backup_flush().await;
        }
    }
}

/// The engine for an instance, or why there's none.
pub fn engine(core: &crate::core::Core, key: &str) -> Result<Arc<DmEngine>, Problem> {
    core.dm_engine(key).ok_or_else(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_back_and_catch_typos() {
        let key: Vec<u8> = (0..32).collect();
        let text = format_key(&key);
        assert_eq!(text.replace('-', "").len(), 56);
        assert_eq!(parse_key(&text), Some(key.clone()));
        assert_eq!(parse_key(&text.to_lowercase().replace('-', " ")), Some(key));
        let mut wrong = text.into_bytes();
        wrong[0] = if wrong[0] == b'A' { b'B' } else { b'A' };
        assert_eq!(parse_key(&String::from_utf8(wrong).unwrap()), None);
    }

    #[test]
    fn parts_open_only_where_they_were_sealed() {
        let keys = derive(&new_key());
        let sealed = seal(&keys, "alice", 3, b"hello").unwrap();
        assert_eq!(open(&keys, "alice", 3, &sealed).as_deref(), Some(&b"hello"[..]));
        assert!(open(&keys, "alice", 4, &sealed).is_none());
        assert!(open(&keys, "bob", 3, &sealed).is_none());
    }

    #[test]
    fn padding_lands_on_a_multiple() {
        for bare in [0, 1, 100, 4000, 4094, 4095, 9000, 200_000] {
            let zeros = padding_for(bare);
            assert_eq!((bare + 1 + varint_bytes(zeros) + zeros) % PAD_TO, 0, "{bare}");
        }
    }
}
