//! Two-step sign-in: codes from an authenticator app (TOTP, RFC 6238) and
//! single-use backup codes.

use hmac::{Hmac, Mac};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::id::now_ms;
use crate::node::NodeDb;

/// Seconds each code lasts.
const PERIOD: i64 = 30;
const DIGITS: u32 = 6;
/// Codes from this many steps either side of now still count, for clocks that drift.
const SKEW: i64 = 1;
/// Bytes of secret: 160 bits, as RFC 4226 recommends.
const SECRET_BYTES: usize = 20;

pub const BACKUP_CODES: usize = 10;
/// Letters and digits that can't be mistaken for each other.
const BACKUP_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const BACKUP_LENGTH: usize = 8;

const BASE32: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A new secret, base32 encoded.
pub fn new_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    base32_encode(&bytes)
}

/// The otpauth:// address authenticator apps read from a QR code.
pub fn uri(secret: &str, issuer: &str, username: &str) -> String {
    let issuer = utf8_percent_encode(issuer, NON_ALPHANUMERIC).to_string();
    let username = utf8_percent_encode(username, NON_ALPHANUMERIC).to_string();
    format!(
        "otpauth://totp/{issuer}:{username}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={PERIOD}"
    )
}

/// The time step a moment falls in.
pub fn step_at(ms: i64) -> i64 {
    ms.div_euclid(1000) / PERIOD
}

/// The code for one time step.
pub fn code_at(secret: &[u8], step: i64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes keys of any length");
    mac.update(&step.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[hash.len() - 1] & 0x0f) as usize;
    let value = u32::from_be_bytes([hash[offset] & 0x7f, hash[offset + 1], hash[offset + 2], hash[offset + 3]]);
    value % 10u32.pow(DIGITS)
}

/// The time step a code from the app belongs to, if it's right for now.
pub fn matching_step(secret: &str, code: &str, now_ms: i64) -> Option<i64> {
    let code: u32 = code.parse().ok()?;
    let secret = base32_decode(secret)?;
    let now = step_at(now_ms);
    (now - SKEW..=now + SKEW).find(|&step| constant_time_eq(code_at(&secret, step), code))
}

/// The app's code for a moment, as it would show it. For tests and tools.
pub fn code_for(secret: &str, now_ms: i64) -> Option<String> {
    Some(format!("{:0width$}", code_at(&base32_decode(secret)?, step_at(now_ms)), width = DIGITS as usize))
}

fn constant_time_eq(a: u32, b: u32) -> bool {
    (a ^ b) == 0
}

/// A code as typed, without spaces or dashes, lowercased.
pub fn normalize(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(char::to_lowercase).collect()
}

/// Whether a code is six digits from an app, rather than a backup code.
fn is_app_code(code: &str) -> bool {
    code.len() == DIGITS as usize && code.chars().all(|c| c.is_ascii_digit())
}

/// New backup codes, as shown (`abcd-efgh`), and their hashes, as stored.
pub fn new_backup_codes() -> (Vec<String>, Vec<String>) {
    let mut shown = Vec::with_capacity(BACKUP_CODES);
    let mut hashes = Vec::with_capacity(BACKUP_CODES);
    for _ in 0..BACKUP_CODES {
        let mut bytes = [0u8; BACKUP_LENGTH];
        getrandom::fill(&mut bytes).expect("the OS random number generator failed");
        // 256 isn't a multiple of the alphabet's length, so a little bias; it
        // costs well under a bit of each code's 39.
        let code: String = bytes.iter().map(|b| BACKUP_ALPHABET[*b as usize % BACKUP_ALPHABET.len()] as char).collect();
        hashes.push(hash_backup_code(&code));
        shown.push(format!("{}-{}", &code[..4], &code[4..]));
    }
    (shown, hashes)
}

fn hash_backup_code(normalized: &str) -> String {
    Sha256::digest(normalized.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks a code for an account with two-step sign-in on: a code from the app
/// (each usable once) or an unused backup code, which it uses up.
pub async fn check(node: &NodeDb, account_id: &str, code: &str) -> Result<bool> {
    let code = normalize(code);
    if code.is_empty() {
        return Ok(false);
    }
    if is_app_code(&code) {
        let Some(secret) = node.totp_state(account_id).await?.secret else { return Ok(false) };
        let Some(step) = matching_step(&secret, &code, now_ms()) else { return Ok(false) };
        return node.use_totp_step(account_id, step).await;
    }
    node.use_backup_code(account_id, &hash_backup_code(&code)).await
}

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
        let value = BASE32.iter().position(|&b| b == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238's SHA-1 test vectors, cut to six digits.
    #[test]
    fn totp_matches_the_rfc() {
        let secret = b"12345678901234567890";
        for (seconds, expected) in [
            (59, 94287082u32),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
        ] {
            assert_eq!(code_at(secret, seconds / PERIOD), expected % 1_000_000, "at {seconds}");
        }
    }

    #[test]
    fn base32_round_trips() {
        assert_eq!(base32_encode(b"12345678901234567890"), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32_decode("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap(), b"12345678901234567890");
        assert_eq!(base32_decode("gezd gnbv").unwrap(), base32_decode("GEZDGNBV").unwrap());
        assert!(base32_decode("not base32!").is_none());
        let secret = new_secret();
        assert_eq!(secret.len(), 32);
        assert_eq!(base32_decode(&secret).unwrap().len(), SECRET_BYTES);
    }

    #[test]
    fn codes_from_the_steps_around_now_count() {
        let secret = new_secret();
        let raw = base32_decode(&secret).unwrap();
        let now = 1_700_000_000_000;
        let step = step_at(now);
        for s in [step - 1, step, step + 1] {
            let code = format!("{:06}", code_at(&raw, s));
            assert_eq!(matching_step(&secret, &code, now), Some(s));
        }
        let stale = format!("{:06}", code_at(&raw, step - 3));
        // A stale code can collide with a live one by chance; that's the only way it would match.
        if !(step - 1..=step + 1).any(|s| code_at(&raw, s) == code_at(&raw, step - 3)) {
            assert_eq!(matching_step(&secret, &stale, now), None);
        }
        assert_eq!(matching_step(&secret, "abcdef", now), None);
    }

    #[test]
    fn backup_codes_look_right() {
        let (shown, hashes) = new_backup_codes();
        assert_eq!(shown.len(), BACKUP_CODES);
        for (code, hash) in shown.iter().zip(&hashes) {
            assert_eq!(code.len(), BACKUP_LENGTH + 1);
            assert_eq!(&hash_backup_code(&normalize(code)), hash);
            assert_eq!(&hash_backup_code(&normalize(&code.to_uppercase().replace('-', " "))), hash);
            assert!(!is_app_code(&normalize(code)));
        }
    }

    #[test]
    fn uri_escapes_names() {
        let uri = uri("ABC", "Waifu Devs", "juan.a");
        assert_eq!(
            uri,
            "otpauth://totp/Waifu%20Devs:juan%2Ea?secret=ABC&issuer=Waifu%20Devs&algorithm=SHA1&digits=6&period=30"
        );
    }
}
