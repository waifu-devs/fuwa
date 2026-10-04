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

use crate::pb;

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
