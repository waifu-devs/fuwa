//! Files in encrypted conversations and secure channels, sealed on this
//! device before they go anywhere (the web's `files/sealed.ts`,
//! `e2ee/files.ts` and the file half of `fuwa/dms.ts`).
//!
//! A file gets a fresh random AES-256-GCM key of its own. It's padded (one
//! 0x80 byte, then zeros) up to a size step, so the instance learns only
//! roughly how big it is, then sealed in chunks of `chunk_bytes`: chunk i
//! under the nonce of 4 zero bytes then i as 8 big-endian bytes, with the top
//! bit set on the last chunk. Reordering, cutting short or adding to the
//! chunks fails to open, and the key is never used for anything else, so the
//! fixed nonces are safe. The instance keeps the sealed chunks one after
//! another; the key, the SHA-256 of those bytes, the file's name, type and
//! size go inside the encrypted message.
//!
//! What an opened file is shown as is decided from its own first bytes,
//! never from what the sender said it was.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use sha2::{Digest as _, Sha256};

use crate::core::api::Problem;
use crate::core::dms::{Content, DmError};
use crate::core::vault::FileRef;
use crate::pb;
use crate::rpc;

/// How big this app's chunks are.
pub const CHUNK_BYTES: usize = 1024 * 1024;
/// The chunk sizes a device takes from a message.
pub const MIN_CHUNK: u32 = 64 * 1024;
pub const MAX_CHUNK: u32 = 8 * 1024 * 1024;
/// The biggest file this app seals or opens: it's held in memory while it is.
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Files a message can carry (the instance's limit too).
pub const MAX_FILES: usize = 10;
const TAG: u64 = 16;

/// The size a file is padded to, from its length plus the 0x80 marker: up to
/// 64 KiB the next 4 KiB, above that the next 1/16 of the power of two below,
/// so padding costs at most about 6%.
pub fn padded_size(length: u64) -> u64 {
    let n = length + 1;
    if n <= 64 * 1024 {
        return n.div_ceil(4096) * 4096;
    }
    let step = (1u64 << (63 - n.leading_zeros())) / 16;
    n.div_ceil(step) * step
}

/// How many bytes the instance keeps for a file padded to `padded`.
pub fn sealed_size(padded: u64, chunk: u64) -> u64 {
    padded + padded.div_ceil(chunk) * TAG
}

/// Whether `size` stored bytes can be a file sealed in chunks of `chunk`:
/// checked before fetching anything a message names.
pub fn plausible(size: i64, chunk: u32) -> bool {
    if !(MIN_CHUNK..=MAX_CHUNK).contains(&chunk) || size <= TAG as i64 {
        return false;
    }
    let (size, chunk) = (size as u64, u64::from(chunk));
    if size > sealed_size(padded_size(MAX_FILE_BYTES), chunk) {
        return false;
    }
    let whole = chunk + TAG;
    let last = size - (size - 1) / whole * whole;
    last > TAG
}

fn nonce(index: u64, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    let high = (index >> 32) as u32 | if last { 0x8000_0000 } else { 0 };
    n[4..8].copy_from_slice(&high.to_be_bytes());
    n[8..12].copy_from_slice(&(index as u32).to_be_bytes());
    n
}

/// A file sealed on this device: the bytes to upload and what opens them.
pub struct Sealed {
    pub bytes: Vec<u8>,
    pub key: [u8; 32],
    pub sha256: [u8; 32],
    pub chunk_bytes: u32,
}

/// Seals a file: padded, then sealed chunk by chunk under a key of its own.
pub fn seal(plain: &[u8], chunk: usize) -> Option<Sealed> {
    if plain.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let padded = padded_size(plain.len() as u64) as usize;
    let count = padded.div_ceil(chunk);
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).ok()?;
    let sealing = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).ok()?);
    let mut bytes = Vec::with_capacity(sealed_size(padded as u64, chunk as u64) as usize);
    for i in 0..count {
        let (start, end) = (i * chunk, ((i + 1) * chunk).min(padded));
        let mut part = vec![0u8; end - start];
        if start < plain.len() {
            let to = end.min(plain.len());
            part[..to - start].copy_from_slice(&plain[start..to]);
        }
        if plain.len() >= start && plain.len() < end {
            part[plain.len() - start] = 0x80;
        }
        let n = Nonce::assume_unique_for_key(nonce(i as u64, i + 1 == count));
        sealing.seal_in_place_append_tag(n, Aad::empty(), &mut part).ok()?;
        bytes.extend_from_slice(&part);
    }
    let sha256 = Sha256::digest(&bytes).into();
    Some(Sealed { bytes, key, sha256, chunk_bytes: chunk as u32 })
}

/// Opens a sealed file, after checking its bytes are the ones the message
/// named. Nothing if anything's off: a changed, cut short, extended or
/// reordered file doesn't open.
pub fn open(bytes: &[u8], key: &[u8], sha256: &[u8], chunk: u32) -> Option<Vec<u8>> {
    if key.len() != 32 || !plausible(bytes.len() as i64, chunk) || Sha256::digest(bytes).as_slice() != sha256 {
        return None;
    }
    let opening = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let whole = chunk as usize + TAG as usize;
    let count = bytes.len().div_ceil(whole);
    let mut out = Vec::with_capacity(bytes.len());
    for i in 0..count {
        let mut part = bytes[i * whole..((i + 1) * whole).min(bytes.len())].to_vec();
        let n = Nonce::assume_unique_for_key(nonce(i as u64, i + 1 == count));
        let plain = opening.open_in_place(n, Aad::empty(), &mut part).ok()?.len();
        out.extend_from_slice(&part[..plain]);
    }
    let end = out.iter().rposition(|&b| b != 0)?;
    (out[end] == 0x80).then(|| {
        out.truncate(end);
        out
    })
}

/// A file's name with no folders or control characters, at most 255 characters (the web's `cleanName`).
pub fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base.chars().filter(|c| !c.is_control()).take(255).collect();
    let clean = clean.trim().to_owned();
    if clean.is_empty() || clean == "." || clean == ".." { "file".into() } else { clean }
}

/// The files someone's message carries, checked: each one this app can fetch
/// and open, at most ten. Anything off leaves that file out, never the message.
pub fn files_of(list: &[pb::SealedFile]) -> Vec<FileRef> {
    let mut out: Vec<FileRef> = Vec::new();
    for f in list.iter().take(MAX_FILES) {
        let id_ok = f.media_id.len() == 26 && f.media_id.bytes().all(|b| b.is_ascii_alphanumeric());
        if !id_ok || f.key.len() != 32 || f.sha256.len() != 32 || !plausible(f.size, f.chunk_bytes) {
            continue;
        }
        let media_id = f.media_id.to_ascii_lowercase();
        if out.iter().any(|x| x.media_id == media_id) {
            continue;
        }
        let file_size =
            if f.file_size >= 0 && f.file_size <= f.size.min(MAX_FILE_BYTES as i64) { f.file_size } else { 0 };
        out.push(FileRef {
            media_id,
            key: f.key.clone(),
            sha256: f.sha256.clone(),
            size: f.size,
            chunk_bytes: f.chunk_bytes,
            name: clean_name(&f.name),
            content_type: f.content_type.chars().take(100).collect(),
            width: f.width.min(16_384),
            height: f.height.min(16_384),
            file_size,
        });
    }
    out
}

/// Files as they go inside an encrypted message.
pub fn to_sealed(files: &[FileRef]) -> Vec<pb::SealedFile> {
    files
        .iter()
        .map(|f| pb::SealedFile {
            media_id: f.media_id.clone(),
            url: String::new(),
            key: f.key.clone(),
            sha256: f.sha256.clone(),
            size: f.size,
            content_type: f.content_type.clone(),
            name: f.name.clone(),
            width: f.width,
            height: f.height,
            chunk_bytes: f.chunk_bytes,
            file_size: f.file_size,
        })
        .collect()
}

/// What an opened file's own first bytes say it is, when it's a picture this app draws.
pub fn picture_kind(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// A picture's size in pixels from its header (PNG, GIF, JPEG), for keeping its place while it opens.
pub fn picture_size(bytes: &[u8]) -> (u32, u32) {
    match picture_kind(bytes) {
        Some("png") if bytes.len() >= 24 => (
            u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        ),
        Some("gif") if bytes.len() >= 10 => {
            (u32::from(u16::from_le_bytes([bytes[6], bytes[7]])), u32::from(u16::from_le_bytes([bytes[8], bytes[9]])))
        }
        Some("jpeg") => {
            let mut i = 2;
            while i + 9 < bytes.len() {
                if bytes[i] != 0xff {
                    break;
                }
                let marker = bytes[i + 1];
                let length = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
                if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                    let h = u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]));
                    let w = u32::from(u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]));
                    return (w, h);
                }
                i += 2 + length;
            }
            (0, 0)
        }
        _ => (0, 0),
    }
}

/// Files this device opened (or sent) this run, by media id: never fetched twice.
static OPENED: LazyLock<Mutex<HashMap<String, Arc<Vec<u8>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn keep_opened(media_id: &str, bytes: Arc<Vec<u8>>) {
    let mut opened = OPENED.lock();
    // Kept only while there's room for a few: they're whole files in memory.
    let total: usize = opened.values().map(|b| b.len()).sum();
    if total + bytes.len() > 256 * 1024 * 1024 {
        opened.clear();
    }
    opened.insert(media_id.to_owned(), bytes);
}

/// A file this device has already opened this run, if it has.
pub fn opened(media_id: &str) -> Option<Arc<Vec<u8>>> {
    OPENED.lock().get(media_id).cloned()
}

/// A file picked to send: its name, what the system says it is, and its bytes.
#[derive(Clone)]
pub struct Outgoing {
    pub name: String,
    pub content_type: String,
    pub bytes: Arc<Vec<u8>>,
}

/// Why these files can't go, if they can't (the web's `cantSendFiles`).
pub fn cant_send(files: &[Outgoing]) -> Option<String> {
    use crate::core::i18n::{Arg, t_with};
    if files.len() > MAX_FILES {
        return Some(t_with("system.files.tooMany", &[("count", Arg::Num(MAX_FILES as i64))]));
    }
    let big = files.iter().find(|f| f.bytes.len() as u64 > MAX_FILE_BYTES)?;
    Some(t_with("system.files.oneTooBig", &[("name", Arg::Str(&clean_name(&big.name))), ("size", Arg::Str("256 MB"))]))
}

impl crate::core::Core {
    /// Fetches a file a message carries from this instance (by its id, never a link the message
    /// names) and opens it on this device.
    pub async fn open_dm_file(&self, key: &str, file: &FileRef) -> Result<Arc<Vec<u8>>, Problem> {
        if let Some(bytes) = opened(&file.media_id) {
            return Ok(bytes);
        }
        let cant = || Problem::new(tonic::Code::DataLoss, crate::core::i18n::t("system.files.cantOpen"));
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::Unavailable, "That instance isn't here."))?;
        let url = format!("{}/media/{}", api.url.trim_end_matches('/'), file.media_id);
        let bytes = crate::core::account::fetch(&url, file.size.max(0) as usize).await?;
        if bytes.len() as i64 != file.size {
            return Err(cant());
        }
        let (k, s, c) = (file.key.clone(), file.sha256.clone(), file.chunk_bytes);
        let plain =
            tokio::task::spawn_blocking(move || open(&bytes, &k, &s, c)).await.ok().flatten().ok_or_else(cant)?;
        let plain = Arc::new(plain);
        keep_opened(&file.media_id, plain.clone());
        Ok(plain)
    }

    /// Sends files in a conversation or secure channel, with `text`: each sealed on this device
    /// under a key of its own and uploaded, then the keys sent inside an encrypted message.
    /// `thread` is a secure channel thread's message, and whether the reply goes to the channel too.
    pub async fn send_dm_files(
        &self,
        key: &str,
        id: &str,
        files: Vec<Outgoing>,
        text: String,
        thread: Option<(i64, bool)>,
    ) -> Result<(), DmError> {
        let api = self.api(key).ok_or_else(|| DmError("That instance isn't here.".into()))?;
        if let Some(why) = cant_send(&files) {
            return Err(DmError(why));
        }
        let mut refs = Vec::new();
        for file in &files {
            let bytes = file.bytes.clone();
            let sealed = tokio::task::spawn_blocking(move || seal(&bytes, CHUNK_BYTES))
                .await
                .ok()
                .flatten()
                .ok_or_else(|| DmError(crate::core::i18n::t("system.files.cantOpen")))?;
            let req = pb::CreateSealedUploadRequest {
                conversation_id: id.into(),
                size: sealed.bytes.len() as i64,
                kind: pb::SealedKind::File as i32,
            };
            let reserved = rpc!(api.dms(), create_sealed_upload(req)).await?;
            // The bytes go to the instance's own address, whatever name it gave the link.
            let token = reserved.upload_url.rsplit('/').next().unwrap_or_default();
            let target = format!("{}/media/upload/{token}", api.url.trim_end_matches('/'));
            crate::core::account::send(http::Method::PUT, &target, "application/octet-stream", sealed.bytes.clone())
                .await?;
            // This device already has it: no need to fetch it back to show it.
            keep_opened(&reserved.media_id, file.bytes.clone());
            let (width, height) = picture_size(&file.bytes);
            refs.push(FileRef {
                media_id: reserved.media_id,
                key: sealed.key.to_vec(),
                sha256: sealed.sha256.to_vec(),
                size: sealed.bytes.len() as i64,
                chunk_bytes: sealed.chunk_bytes,
                name: clean_name(&file.name),
                content_type: file.content_type.chars().take(100).collect(),
                width,
                height,
                file_size: file.bytes.len() as i64,
            });
        }
        let files = to_sealed(&refs);
        let content = match thread {
            Some((thread, in_channel)) => Content::Reply { text, thread, in_channel, files },
            None => Content::Files { text, files },
        };
        self.send_dm(key, id, content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_steps_like_the_web() {
        assert_eq!(padded_size(0), 4096);
        assert_eq!(padded_size(4095), 4096);
        assert_eq!(padded_size(4096), 8192);
        assert_eq!(padded_size(70_000), 73_728);
    }

    #[test]
    fn sealed_files_open_to_what_went_in() {
        for length in [0usize, 1, 5000, 70_000, 200_000] {
            let plain: Vec<u8> = (0..length).map(|i| (i * 7 % 251) as u8).collect();
            let sealed = seal(&plain, 64 * 1024).unwrap();
            assert!(plausible(sealed.bytes.len() as i64, sealed.chunk_bytes));
            assert_eq!(open(&sealed.bytes, &sealed.key, &sealed.sha256, sealed.chunk_bytes).unwrap(), plain);
        }
    }

    #[test]
    fn a_changed_or_cut_file_does_not_open() {
        let plain = vec![9u8; 150_000];
        let sealed = seal(&plain, 64 * 1024).unwrap();
        let mut changed = sealed.bytes.clone();
        changed[10] ^= 1;
        let digest: [u8; 32] = Sha256::digest(&changed).into();
        assert!(open(&changed, &sealed.key, &digest, sealed.chunk_bytes).is_none());
        let cut = &sealed.bytes[..64 * 1024 + 16];
        let digest: [u8; 32] = Sha256::digest(cut).into();
        assert!(open(cut, &sealed.key, &digest, sealed.chunk_bytes).is_none());
    }

    #[test]
    fn names_lose_their_folders() {
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("C:\\pics\\cat.png"), "cat.png");
        assert_eq!(clean_name(""), "file");
    }

    #[test]
    fn pictures_are_known_by_their_bytes() {
        let mut png = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(picture_kind(&png), Some("png"));
        assert_eq!(picture_size(&png), (640, 480));
        assert_eq!(picture_kind(b"%PDF-1.4"), None);
    }
}
