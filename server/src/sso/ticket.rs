//! The instance's sign-ins on their way, carried in the state itself instead
//! of a row. Anyone may start one without signing in, and nothing says who
//! they are (no client address is ever looked at), so anything kept per start
//! could be filled by one person for everyone. A ticket costs the instance
//! nothing until the provider answers; only then is a row kept, under the
//! state, which also makes every state work once.
//!
//! The state is short enough for SAML's RelayState (80 bytes at most): 76
//! characters of URL-safe base64, which are 16 random bytes, when it runs out, the
//! test flag, which app it goes back to (as a small code, below), the first
//! half of the app's secret hash, and a MAC over all of that and the
//! provider's [`Provider::trust_key`], so a ticket stops working when the
//! provider changes. The OIDC verifier and nonce and the SAML request ID are
//! MACs of the state, so they're never stored either.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use super::{Provider, SignIn, TTL_MS};
use crate::error::{Error, Result};
use crate::outside::Key;

const RANDOM: usize = 16;
const SECRET: usize = 16;
const MAC: usize = 16;
/// Random, expiry (seconds, 5 bytes), test, origin kind, origin port or
/// index (2 bytes), secret hash.
const PAYLOAD: usize = RANDOM + 5 + 1 + 1 + 2 + SECRET;

/// Where a sign-in goes back to, compactly: the instance's own app, an app on
/// this device, or one of the allowed origins by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Own,
    Loopback { https: bool, host: u8, port: u16 },
    Listed(u16),
}

const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

fn own_origin(public_url: &str) -> Option<String> {
    let id = crate::linked::client_id(public_url)?;
    Some(reqwest::Url::parse(&id).ok()?.origin().ascii_serialization())
}

impl Origin {
    /// The code for a checked return origin (`linked::return_origin`).
    fn of(origin: &str, public_url: &str, allowed: &[String]) -> Result<Origin> {
        if own_origin(public_url).as_deref() == Some(origin) {
            return Ok(Origin::Own);
        }
        let url = reqwest::Url::parse(origin).map_err(|_| Error::invalid("that return origin can't be read"))?;
        if let Some(host) = url.host_str().and_then(|h| LOOPBACK_HOSTS.iter().position(|l| *l == h)) {
            return Ok(Origin::Loopback {
                https: url.scheme() == "https",
                host: host as u8,
                port: url.port().unwrap_or(0),
            });
        }
        allowed
            .iter()
            .position(|listed| listed == origin)
            .and_then(|index| u16::try_from(index).ok())
            .map(Origin::Listed)
            .ok_or_else(|| Error::denied("this instance doesn't send sign-ins back there"))
    }

    fn origin(self, public_url: &str, allowed: &[String]) -> Option<String> {
        match self {
            Origin::Own => own_origin(public_url),
            Origin::Loopback { https, host, port } => {
                let scheme = if https { "https" } else { "http" };
                let host = LOOPBACK_HOSTS.get(host as usize)?;
                Some(if port == 0 { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{port}") })
            }
            Origin::Listed(index) => allowed.get(index as usize).filter(|o| *o != "*").cloned(),
        }
    }

    fn encode(self) -> (u8, u16) {
        match self {
            Origin::Own => (0, 0),
            Origin::Loopback { https, host, port } => (1 + host + if https { 3 } else { 0 }, port),
            Origin::Listed(index) => (7, index),
        }
    }

    fn decode(kind: u8, value: u16) -> Option<Origin> {
        match kind {
            0 => Some(Origin::Own),
            1..=6 => Some(Origin::Loopback { https: kind > 3, host: (kind - 1) % 3, port: value }),
            7 => Some(Origin::Listed(value)),
            _ => None,
        }
    }
}

fn mac(key: &Key, label: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut all: Vec<&[u8]> = vec![label.as_bytes(), b"\0"];
    all.extend_from_slice(parts);
    key.mac(&all)
}

/// A new instance sign-in: its state and everything derived from it, with the
/// secret hash and return origin it was started with. Nothing is stored.
#[allow(clippy::too_many_arguments)]
pub fn issue(
    key: &Key,
    provider: &Provider,
    return_origin: &str,
    secret_hash: &str,
    test: bool,
    now_ms: i64,
    public_url: &str,
    allowed: &[String],
) -> Result<SignIn> {
    let secret = hex_bytes(secret_hash).ok_or_else(|| Error::invalid("secret_hash must be a SHA-256 in hex"))?;
    let (kind, value) = Origin::of(return_origin, public_url, allowed)?.encode();
    let expires_at = now_ms + TTL_MS;
    let mut payload = Vec::with_capacity(PAYLOAD);
    let mut random = [0u8; RANDOM];
    getrandom::fill(&mut random).expect("the OS random number generator failed");
    payload.extend_from_slice(&random);
    payload.extend_from_slice(&((expires_at / 1000) as u64).to_be_bytes()[3..]);
    payload.push(test as u8);
    payload.push(kind);
    payload.extend_from_slice(&value.to_be_bytes());
    payload.extend_from_slice(&secret[..SECRET]);
    let tag = mac(key, "fuwa sso state", &[provider.trust_key().as_bytes(), b"\0", &payload]);
    payload.extend_from_slice(&tag[..MAC]);
    let state = URL_SAFE_NO_PAD.encode(&payload);
    Ok(derive(key, provider, &state, expires_at, test, hex(&secret[..SECRET]), return_origin.to_string()))
}

/// Reads back a state this instance issued for `provider`, if it's genuine,
/// still for the same provider and hasn't run out.
pub fn read(
    key: &Key,
    provider: &Provider,
    state: &str,
    now_ms: i64,
    public_url: &str,
    allowed: &[String],
) -> Result<SignIn> {
    let ran_out = || Error::FailedPrecondition("this sign-in ran out or single sign-on changed; start again".into());
    let bytes = URL_SAFE_NO_PAD.decode(state.trim()).map_err(|_| ran_out())?;
    if bytes.len() != PAYLOAD + MAC {
        return Err(ran_out());
    }
    let (payload, tag) = bytes.split_at(PAYLOAD);
    let expected = mac(key, "fuwa sso state", &[provider.trust_key().as_bytes(), b"\0", payload]);
    if !crate::auth::constant_time_eq(tag, &expected[..MAC]) {
        return Err(ran_out());
    }
    let mut at = RANDOM;
    let mut seconds = [0u8; 8];
    seconds[3..].copy_from_slice(&payload[at..at + 5]);
    at += 5;
    let expires_at = u64::from_be_bytes(seconds) as i64 * 1000;
    if expires_at < now_ms {
        return Err(ran_out());
    }
    let test = payload[at] == 1;
    let kind = payload[at + 1];
    let value = u16::from_be_bytes([payload[at + 2], payload[at + 3]]);
    at += 4;
    let secret = hex(&payload[at..at + SECRET]);
    let origin =
        Origin::decode(kind, value).and_then(|origin| origin.origin(public_url, allowed)).ok_or_else(ran_out)?;
    Ok(derive(key, provider, state, expires_at, test, secret, origin))
}

fn derive(
    key: &Key,
    provider: &Provider,
    state: &str,
    expires_at: i64,
    test: bool,
    secret_hash: String,
    return_origin: String,
) -> SignIn {
    let verifier = URL_SAFE_NO_PAD.encode(mac(key, "fuwa sso verifier", &[state.as_bytes()]));
    let nonce = URL_SAFE_NO_PAD.encode(mac(key, "fuwa sso nonce", &[state.as_bytes()]));
    let request_id = format!("_{}", hex(&mac(key, "fuwa sso request", &[state.as_bytes()])[..20]));
    SignIn {
        state: state.to_string(),
        secret_hash,
        return_origin,
        account_id: String::new(),
        test,
        provider_key: provider.trust_key(),
        verifier,
        nonce,
        request_id,
        code_hash: None,
        identity: None,
        expires_at,
    }
}

/// The PKCE challenge for a verifier (S256).
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    (0..64).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: &str = "https://chat.example.com";

    fn key() -> Key {
        Key::from_cluster_key("a cluster key of at least thirty-two characters")
    }

    fn provider(entity: &str) -> Provider {
        Provider {
            protocol: super::super::Protocol::Saml,
            name: "Acme".into(),
            saml_entity_id: entity.into(),
            saml_sso_url: "https://idp.acme.com/sso".into(),
            ..Default::default()
        }
    }

    #[test]
    fn tickets_come_back_whole_and_fit_in_a_relay_state() {
        let allowed = vec!["https://app.example.com".to_string()];
        let secret = "ab".repeat(32);
        for origin in [PUBLIC, "http://127.0.0.1:53124", "https://[::1]", "https://app.example.com"] {
            let issued = issue(&key(), &provider("acme"), origin, &secret, true, 1_000, PUBLIC, &allowed).unwrap();
            assert!(issued.state.len() <= 80, "{}", issued.state.len());
            let read = read(&key(), &provider("acme"), &issued.state, 2_000, PUBLIC, &allowed).unwrap();
            assert_eq!(read.return_origin, origin);
            assert!(read.test);
            assert_eq!(read.secret_hash, "ab".repeat(16));
            assert_eq!((read.verifier.len(), &read.nonce, &read.request_id), (43, &issued.nonce, &issued.request_id));
        }
    }

    #[test]
    fn tickets_are_refused_when_changed_late_or_for_another_provider() {
        let secret = "cd".repeat(32);
        let issued = issue(&key(), &provider("acme"), PUBLIC, &secret, false, 1_000, PUBLIC, &[]).unwrap();
        assert!(read(&key(), &provider("other"), &issued.state, 2_000, PUBLIC, &[]).is_err(), "another provider");
        assert!(read(&key(), &provider("acme"), &issued.state, 1_000 + TTL_MS + 1_000, PUBLIC, &[]).is_err());
        let mut bytes = URL_SAFE_NO_PAD.decode(&issued.state).unwrap();
        bytes[RANDOM + 5] ^= 1; // the test flag
        assert!(read(&key(), &provider("acme"), &URL_SAFE_NO_PAD.encode(&bytes), 2_000, PUBLIC, &[]).is_err());
        let other = Key::from_cluster_key("another cluster key of at least thirty-two characters");
        assert!(read(&other, &provider("acme"), &issued.state, 2_000, PUBLIC, &[]).is_err(), "another key");
        assert!(issue(&key(), &provider("acme"), "https://evil.example", &secret, false, 1_000, PUBLIC, &[]).is_err());
    }
}
