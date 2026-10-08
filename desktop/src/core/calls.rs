//! Calls, as far as the desktop app has them: who's in each voice channel
//! and which conversations have a call going (both kept in the [`Store`]
//! from the event streams), and the end-to-end encryption calls in direct
//! messages use, byte for byte the same as the web app's
//! (web/src/calls/frames.worker.ts), so a desktop app and a browser can
//! share one.
//!
//! The call itself (voice channels, sound only so far) is in
//! [`voice`](crate::core::voice): str0m, Opus and cpal, talking to
//! `CallService` the way web/src/calls/engine.ts does. Direct-message calls
//! will seal their frames with [`FrameKey`].
//!
//! [`Store`]: crate::core::store::Store

use std::collections::HashMap;

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf::{HKDF_SHA256, KeyType, Salt};

use crate::core::Core;
use crate::core::api::{CALL_TIMEOUT, Problem};
use crate::{pb, rpc};

/// The MLS exporter label a conversation's call secret comes from.
pub const CALL_LABEL: &str = "fuwa call v1";
/// How long that secret is.
pub const CALL_SECRET_LEN: usize = 32;

const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
/// The nonce, the epoch (big endian) and the version, after the ciphertext.
const TRAILER: usize = NONCE_LEN + 4 + 1;
const TAG_LEN: usize = 16;

/// Puts someone's voice state in a server's list: where they were, or at the end.
pub fn put(list: &mut Vec<pb::VoiceState>, state: pb::VoiceState) {
    match list.iter_mut().find(|v| v.user_id == state.user_id) {
        Some(slot) => *slot = state,
        None => list.push(state),
    }
}

/// Who's in a voice channel, in the order they joined.
pub fn in_channel<'a>(
    voice: &'a HashMap<String, Vec<pb::VoiceState>>,
    server_id: &str,
    channel_id: &str,
) -> Vec<&'a pb::VoiceState> {
    voice.get(server_id).map(|l| l.iter().filter(|v| v.channel_id == channel_id).collect()).unwrap_or_default()
}

/// A conversation's call changed: an empty one has ended.
pub fn set_dm_call(calls: &mut HashMap<String, pb::DmCall>, call: pb::DmCall) {
    if call.participants.is_empty() {
        calls.remove(&call.conversation_id);
    } else {
        calls.insert(call.conversation_id.clone(), call);
    }
}

struct Len(usize);

impl KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// One sender's key at one epoch: HKDF-SHA256 of the conversation's call
/// secret, with no salt, and "fuwa call frame <their account id>" as info.
pub struct FrameKey {
    epoch: u32,
    key: LessSafeKey,
}

impl FrameKey {
    pub fn new(epoch: u64, secret: &[u8], sender: &str) -> Option<Self> {
        let info = format!("fuwa call frame {sender}");
        let info = [info.as_bytes()];
        let prk = Salt::new(HKDF_SHA256, &[]).extract(secret);
        let okm = prk.expand(&info, Len(32)).ok()?;
        let mut bytes = [0u8; 32];
        okm.fill(&mut bytes).ok()?;
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &bytes).ok()?);
        Some(Self { epoch: u32::try_from(epoch).ok()?, key })
    }

    /// Seals a frame this sender sends, with a fresh random nonce.
    pub fn seal(&self, frame: &[u8]) -> Option<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).ok()?;
        self.seal_with(frame, nonce)
    }

    fn seal_with(&self, frame: &[u8], nonce: [u8; NONCE_LEN]) -> Option<Vec<u8>> {
        let mut aad = [0u8; 5];
        aad[..4].copy_from_slice(&self.epoch.to_be_bytes());
        aad[4] = VERSION;
        let mut out = frame.to_vec();
        self.key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut out).ok()?;
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&aad);
        Some(out)
    }

    /// Opens a frame this sender sealed at this key's epoch.
    pub fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        if frame_epoch(sealed)? != self.epoch {
            return None;
        }
        let (body, trailer) = sealed.split_at(sealed.len() - TRAILER);
        let nonce = Nonce::try_assume_unique_for_key(&trailer[..NONCE_LEN]).ok()?;
        let mut out = body.to_vec();
        let plain = self.key.open_in_place(nonce, Aad::from(&trailer[NONCE_LEN..]), &mut out).ok()?;
        Some(plain.to_vec())
    }
}

/// The epoch a sealed frame says it's from, so the right key (or a
/// catch-up, when it's newer than any) can be found before opening it.
pub fn frame_epoch(sealed: &[u8]) -> Option<u32> {
    if sealed.len() <= TRAILER + TAG_LEN || sealed[sealed.len() - 1] != VERSION {
        return None;
    }
    let at = sealed.len() - TRAILER + NONCE_LEN;
    Some(u32::from_be_bytes(sealed[at..at + 4].try_into().ok()?))
}

/// A direct-message call's sealing: everyone's key at the conversation's
/// current epoch, the web's FrameCrypto (frames.worker.ts).
pub struct Frames {
    me: String,
    epoch: u64,
    secret: Vec<u8>,
    keys: HashMap<String, FrameKey>,
}

/// What opening a frame found.
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    Plain(Vec<u8>),
    /// Sealed at an epoch newer than the secret here: catch up to it.
    Newer(u32),
    /// Not one of ours, or tampered with: dropped.
    Dropped,
}

impl Frames {
    pub fn new(me: &str, epoch: u64, secret: Vec<u8>) -> Self {
        Self { me: me.to_owned(), epoch, secret, keys: HashMap::new() }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The conversation moved on to a new epoch, with a new secret.
    pub fn set_secret(&mut self, epoch: u64, secret: Vec<u8>) {
        if epoch != self.epoch || secret != self.secret {
            self.epoch = epoch;
            self.secret = secret;
            self.keys.clear();
        }
    }

    fn key(&mut self, sender: &str) -> Option<&FrameKey> {
        if !self.keys.contains_key(sender) {
            let key = FrameKey::new(self.epoch, &self.secret, sender)?;
            self.keys.insert(sender.to_owned(), key);
        }
        self.keys.get(sender)
    }

    /// Seals a frame of your own sound.
    pub fn seal(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        let me = self.me.clone();
        self.key(&me)?.seal(frame)
    }

    /// Opens a frame from `sender` (a stream id: their account id).
    pub fn open(&mut self, sender: &str, sealed: &[u8]) -> Opened {
        match frame_epoch(sealed) {
            Some(epoch) if u64::from(epoch) > self.epoch => Opened::Newer(epoch),
            Some(epoch) if u64::from(epoch) == self.epoch => match self.key(sender).and_then(|k| k.open(sealed)) {
                Some(plain) => Opened::Plain(plain),
                None => Opened::Dropped,
            },
            _ => Opened::Dropped,
        }
    }
}

/// How long a call has gone on: 4:07, or 1:02:33 (the web's call-clock.ts).
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

// ───────────────────────── Moderating, recordings ─────────────────────────

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

/// A recording's files, as the web's Recordings.tsx names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Sound,
    Camera,
    Screen,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Sound, Part::Camera, Part::Screen];

    pub fn bytes(self, track: &pb::RecordingTrack) -> i64 {
        match self {
            Part::Sound => track.size_bytes,
            Part::Camera => track.camera_bytes,
            Part::Screen => track.screen_bytes,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Part::Sound => ".opus",
            Part::Camera => " camera.webm",
            Part::Screen => " screen.webm",
        }
    }

    fn wire(self) -> pb::RecordingPart {
        match self {
            Part::Sound => pb::RecordingPart::Unspecified,
            Part::Camera => pb::RecordingPart::Camera,
            Part::Screen => pb::RecordingPart::Screen,
        }
    }

    /// The files a person has in a recording.
    pub fn of(track: &pb::RecordingTrack) -> Vec<Part> {
        Part::ALL.into_iter().filter(|p| p.bytes(track) > 0).collect()
    }
}

/// Characters no file system takes in a name, as the web's `safe`.
pub fn safe_name(name: &str) -> String {
    let mut out = String::new();
    let mut bad = false;
    for c in name.chars() {
        if "\\/:*?\"<>|".contains(c) {
            if !bad {
                out.push('-');
            }
            bad = true;
        } else {
            out.push(c);
            bad = false;
        }
    }
    let out = out.trim().to_owned();
    if out.is_empty() { "someone".into() } else { out }
}

/// CRC-32 as zip uses it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { 0xedb8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    crc ^ 0xffff_ffff
}

/// Files as they are (stored, not compressed: Opus doesn't shrink) as one
/// zip, byte for byte the web's lib/zip.ts. `when` is (year, month, day,
/// hour, minute, second).
pub fn zip(files: &[(String, Vec<u8>)], when: (u32, u32, u32, u32, u32, u32)) -> Vec<u8> {
    let (y, mo, d, h, mi, s) = when;
    let time = ((h << 11) | (mi << 5) | (s >> 1)) as u16;
    let date = (((y.max(1980) - 1980) << 9) | (mo << 5) | d) as u16;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let name = name.as_bytes();
        let crc = crc32(data);
        let len = data.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x0800u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&time.to_le_bytes());
        out.extend_from_slice(&date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&time.to_le_bytes());
        central.extend_from_slice(&date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let start = out.len() as u32;
    let size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

impl Core {
    /// Mutes, deafens, stops the video of or disconnects someone in a voice
    /// channel, for everyone (Mute members, Move members).
    pub async fn moderate_voice(&self, key: &str, request: pb::ModerateVoiceRequest) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.calls(), moderate_voice(request)).await?;
        Ok(())
    }

    /// A voice channel's recordings on the server, and how much they take.
    pub async fn recordings(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
    ) -> Result<pb::ListRecordingsResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let request = pb::ListRecordingsRequest { server_id: server_id.into(), channel_id: channel_id.into() };
        rpc!(api.calls(), list_recordings(request)).await
    }

    pub async fn delete_recording(&self, key: &str, server_id: &str, recording_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let request = pb::DeleteRecordingRequest { server_id: server_id.into(), recording_id: recording_id.into() };
        rpc!(api.calls(), delete_recording(request)).await?;
        Ok(())
    }

    /// One person's file in a recording, as it is, telling how far along it is (0 to 1).
    pub async fn download_recording(
        &self,
        key: &str,
        server_id: &str,
        recording_id: &str,
        track: &pb::RecordingTrack,
        part: Part,
        progress: impl Fn(f32) + Send,
    ) -> Result<Vec<u8>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let request = pb::DownloadRecordingRequest {
            server_id: server_id.into(),
            recording_id: recording_id.into(),
            user_id: track.user_id.clone(),
            part: part.wire() as i32,
        };
        let late = || Problem::new(tonic::Code::DeadlineExceeded, "The instance took too long to answer.");
        let mut stream = tokio::time::timeout(CALL_TIMEOUT, api.calls().download_recording(request))
            .await
            .map_err(|_| late())?
            .map_err(Problem::from)?
            .into_inner();
        let total = part.bytes(track).max(1) as f32;
        let mut data = Vec::new();
        while let Some(chunk) =
            tokio::time::timeout(CALL_TIMEOUT, stream.message()).await.map_err(|_| late())?.map_err(Problem::from)?
        {
            data.extend_from_slice(&chunk.data);
            progress((data.len() as f32 / total).min(0.98));
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> Vec<u8> {
        (0..32).collect()
    }

    /// Sealed by the web app's frame worker (WebCrypto): secret 0..32,
    /// sender "u1", epoch 7, nonce a0..ab.
    const FROM_WEB: &str =
        "433c53d835cf0f9a903c45f94361885b695201db449892cfc268c4f49290ad79a0a1a2a3a4a5a6a7a8a9aaab0000000701";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn opens_what_the_web_app_seals() {
        let sealed = unhex(FROM_WEB);
        assert_eq!(frame_epoch(&sealed), Some(7));
        let key = FrameKey::new(7, &secret(), "u1").unwrap();
        assert_eq!(key.open(&sealed).as_deref(), Some(&b"hello opus frame"[..]));
        let nonce: [u8; 12] = std::array::from_fn(|i| 0xa0 + i as u8);
        assert_eq!(key.seal_with(b"hello opus frame", nonce).unwrap(), sealed, "and seals the same way");
    }

    #[test]
    fn only_the_sender_and_epoch_open_it() {
        let key = FrameKey::new(3, &secret(), "u1").unwrap();
        let sealed = key.seal(b"frame").unwrap();
        assert_eq!(key.open(&sealed).as_deref(), Some(&b"frame"[..]));
        assert!(FrameKey::new(3, &secret(), "u2").unwrap().open(&sealed).is_none(), "someone else's key");
        assert!(FrameKey::new(4, &secret(), "u1").unwrap().open(&sealed).is_none(), "another epoch");
        let mut tampered = sealed.clone();
        tampered[0] ^= 1;
        assert!(key.open(&tampered).is_none());
        assert!(key.open(b"short").is_none());
    }

    #[test]
    fn frames_open_between_two_people() {
        let mut alice = Frames::new("a", 2, secret());
        let mut bob = Frames::new("b", 2, secret());
        let sealed = alice.seal(b"hi").unwrap();
        assert_eq!(bob.open("a", &sealed), Opened::Plain(b"hi".to_vec()));
        assert_eq!(bob.open("b", &sealed), Opened::Dropped, "only under the sender's own key");
        alice.set_secret(3, secret());
        let newer = alice.seal(b"later").unwrap();
        assert_eq!(bob.open("a", &newer), Opened::Newer(3));
        bob.set_secret(3, secret());
        assert_eq!(bob.open("a", &newer), Opened::Plain(b"later".to_vec()));
        assert_eq!(bob.open("a", &sealed), Opened::Dropped, "an old epoch's frame");
    }

    #[test]
    fn zips_like_the_web() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let z = zip(&[("a.opus".into(), b"hello".to_vec())], (2026, 10, 3, 9, 41, 0));
        assert_eq!(&z[..4], &[0x50, 0x4b, 0x03, 0x04]);
        assert_eq!(z.len(), 30 + 6 + 5 + 46 + 6 + 22);
        assert_eq!(safe_name("a/b: c"), "a-b- c");
        assert_eq!(safe_name("  "), "someone");
    }

    #[test]
    fn clocks_read_like_the_web() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(247), "4:07");
        assert_eq!(clock(3753), "1:02:33");
    }

    #[test]
    fn voice_states_keep_their_place() {
        let state = |user: &str, channel: &str| pb::VoiceState {
            user_id: user.into(),
            channel_id: channel.into(),
            ..Default::default()
        };
        let mut list = vec![state("a", "c1"), state("b", "c1")];
        put(&mut list, state("a", "c2"));
        put(&mut list, state("c", "c1"));
        let voice = HashMap::from([("s".to_owned(), list)]);
        let here: Vec<_> = in_channel(&voice, "s", "c1").iter().map(|v| v.user_id.as_str()).collect();
        assert_eq!(here, ["b", "c"]);

        let mut calls = HashMap::new();
        let call = pb::DmCall { conversation_id: "d".into(), participants: vec![state("a", "")], ..Default::default() };
        set_dm_call(&mut calls, call);
        assert!(calls.contains_key("d"));
        set_dm_call(&mut calls, pb::DmCall { conversation_id: "d".into(), ..Default::default() });
        assert!(calls.is_empty(), "an empty call has ended");
    }
}
