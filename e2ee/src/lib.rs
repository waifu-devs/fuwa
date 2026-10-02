//! End-to-end encryption for fuwa's direct messages, shared by the server
//! and every client.
//!
//! Conversations are MLS groups (RFC 9420, through OpenMLS) whose members
//! are devices: every device signed in to an account has its own signing
//! key, and a conversation between two people holds every device of both.
//! The instance only delivers: it orders each conversation's messages, keeps
//! the group's public state so a device can join by itself, and hands out
//! the key packages devices publish, but it never has a key that opens a
//! message.
//!
//! Without the `client` feature this is only what the server needs: reading
//! message headers ([`wire`]) and naming devices ([`device_id`]). Clients
//! turn `client` on for [`Device`], and browsers `js` too.

pub mod wire;

#[cfg(feature = "client")]
mod device;
#[cfg(feature = "client")]
pub use device::*;

use sha2::{Digest, Sha256, Sha512};

/// The MLS cipher suite every conversation uses:
/// MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519. ChaCha20 is fast in
/// software, which is what a browser's WebAssembly is.
pub const CIPHER_SUITE: u16 = 0x0003;

/// A device's id: the first half of the SHA-256 of its signing key, as hex.
/// Anyone with the key works out the same id, so the server can't give one
/// device's id to another device's key.
pub fn device_id(signature_key: &[u8]) -> String {
    hex(&Sha256::digest(signature_key)[..16])
}

/// A safety number for a conversation between two people: 60 digits, from
/// each person's id and the signing keys of all their devices in it. Both
/// sides work out the same number. If it matches what the other person sees
/// (read out, or compared side by side), nobody, the server included, has
/// slipped a device into the conversation. It changes whenever either
/// person adds or removes a device.
pub fn safety_number(a: (&str, &[Vec<u8>]), b: (&str, &[Vec<u8>])) -> String {
    let (first, second) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let mut number = fingerprint(first.0, first.1);
    number.push_str(&fingerprint(second.0, second.1));
    number
}

/// Rounds of hashing per fingerprint, as Signal's safety numbers use, so
/// finding keys that give a chosen number costs that much more.
const FINGERPRINT_ROUNDS: usize = 5200;
const FINGERPRINT_VERSION: [u8; 2] = [0, 1];

/// One person's half of a safety number: 30 digits.
fn fingerprint(user_id: &str, keys: &[Vec<u8>]) -> String {
    let mut keys: Vec<&Vec<u8>> = keys.iter().collect();
    keys.sort();
    keys.dedup();
    let mut material = Vec::new();
    for key in keys {
        material.extend((key.len() as u16).to_be_bytes());
        material.extend(key);
    }
    let mut hash =
        Sha512::new().chain_update(FINGERPRINT_VERSION).chain_update(&material).chain_update(user_id).finalize();
    for _ in 1..FINGERPRINT_ROUNDS {
        hash = Sha512::new().chain_update(hash).chain_update(&material).finalize();
    }
    hash[..30]
        .chunks(5)
        .map(|chunk| {
            let value = chunk.iter().fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte));
            format!("{:05}", value % 100_000)
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_come_from_the_key() {
        let id = device_id(&[1; 32]);
        assert_eq!(id.len(), 32);
        assert_eq!(id, device_id(&[1; 32]));
        assert_ne!(id, device_id(&[2; 32]));
    }

    #[test]
    fn both_sides_get_the_same_safety_number() {
        let alice = vec![vec![1u8; 32], vec![2u8; 32]];
        let bob = vec![vec![3u8; 32]];
        let number = safety_number(("alice", &alice), ("bob", &bob));
        assert_eq!(number.len(), 60);
        assert!(number.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(number, safety_number(("bob", &bob), ("alice", &alice)));
        // Device order doesn't matter, the devices do.
        let reordered = vec![vec![2u8; 32], vec![1u8; 32]];
        assert_eq!(number, safety_number(("alice", &reordered), ("bob", &bob)));
        let more = vec![vec![1u8; 32], vec![2u8; 32], vec![4u8; 32]];
        assert_ne!(number, safety_number(("alice", &more), ("bob", &bob)));
        assert_ne!(number, safety_number(("alice", &alice), ("carol", &bob)));
    }
}
