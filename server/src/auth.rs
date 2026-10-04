//! Passwords, session tokens, and working out who is calling.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use base64::Engine;
use sha2::{Digest, Sha256};
use tonic::metadata::MetadataMap;

use crate::error::{Error, Result};
use crate::id::now_ms;
use crate::node::{Account, NodeDb};

/// Who a request comes from.
#[derive(Debug, Clone)]
pub enum Viewer {
    Account {
        account: Account,
        token_hash: String,
    },
    /// The operator's FUWA_ADMIN_TOKEN: instance-admin rights, but no account.
    Operator,
}

/// A signed-in account and the session it called with.
#[derive(Debug, Clone)]
pub struct Caller {
    pub account: Account,
    pub token_hash: String,
}

impl Viewer {
    pub fn account(&self) -> Result<&Account> {
        match self {
            Self::Account { account, .. } => Ok(account),
            Self::Operator => Err(Error::denied("the admin token can't act as an account; sign in instead")),
        }
    }

    /// The account and session, for calls that act on the session itself.
    pub fn caller(self) -> Result<Caller> {
        match self {
            Self::Account { account, token_hash } => Ok(Caller { account, token_hash }),
            Self::Operator => Err(Error::denied("the admin token can't act as an account; sign in instead")),
        }
    }

    pub fn is_instance_admin(&self) -> bool {
        match self {
            Self::Account { account, .. } => account.admin,
            Self::Operator => true,
        }
    }
}

/// The device a request comes from, as its User-Agent says.
pub fn user_agent(metadata: &MetadataMap) -> String {
    metadata.get("user-agent").and_then(|v| v.to_str().ok()).map(crate::node::clip_user_agent).unwrap_or_default()
}

/// The bearer token on a request, if any.
pub fn bearer(metadata: &MetadataMap) -> Option<&str> {
    let value = metadata.get("authorization")?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim()).filter(|t| !t.is_empty())
}

/// The bearer token in plain HTTP headers, like [`bearer`].
pub fn bearer_header(headers: &http::HeaderMap) -> Option<&str> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim()).filter(|t| !t.is_empty())
}

pub async fn authenticate(node: &NodeDb, admin_token: Option<&str>, metadata: &MetadataMap) -> Result<Viewer> {
    authenticate_token(node, admin_token, bearer(metadata).ok_or(Error::Unauthenticated)?).await
}

/// Who a bearer token belongs to: the operator, or a signed-in account.
pub async fn authenticate_token(node: &NodeDb, admin_token: Option<&str>, token: &str) -> Result<Viewer> {
    if let Some(admin_token) = admin_token
        && constant_time_eq(token.as_bytes(), admin_token.as_bytes())
    {
        return Ok(Viewer::Operator);
    }
    let token_hash = hash_token(token);
    let account = node.session_account(&token_hash).await?.ok_or(Error::Unauthenticated)?;
    Ok(Viewer::Account { account, token_hash })
}

/// A new session token: 32 random bytes, URL-safe base64.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A password an admin hands someone to sign in with: four groups of four
/// letters and digits that can't be misread (no 0/O, 1/l/I).
pub fn temporary_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    let chars: Vec<char> = bytes.iter().map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char).collect();
    chars.chunks(4).map(|group| group.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

/// What's stored for a token: its SHA-256, so a leaked database leaks no sessions.
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Password work running and waiting at once. Each Argon2 hash holds 19 MB
/// and a core's worth of time for a moment: 2000 wrong sign-ins at once took
/// a 2-core instance to 2.9 GB and everyone else's messages to 14 s
/// (docs/capacity.md). So half the cores hash (at least one) and a few
/// hundred wait; anyone past that is told the instance is busy. This is a
/// protective limit, on by default.
struct Hashing {
    running: tokio::sync::Semaphore,
    waiting: AtomicUsize,
}

const HASH_WAITING: usize = 256;

const HASH_BUSY: &str = "this instance is busy signing people in; try again in a moment";

/// A turn at password work, given back on drop.
struct HashTurn {
    _running: tokio::sync::SemaphorePermit<'static>,
}

async fn hash_turn() -> Result<HashTurn> {
    static HASHING: OnceLock<Hashing> = OnceLock::new();
    let hashing = HASHING.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        Hashing { running: tokio::sync::Semaphore::new((cores / 2).max(1)), waiting: AtomicUsize::new(0) }
    });
    struct Waiting(&'static AtomicUsize);
    impl Drop for Waiting {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    if hashing.waiting.fetch_add(1, Ordering::AcqRel) >= HASH_WAITING {
        hashing.waiting.fetch_sub(1, Ordering::AcqRel);
        crate::reports::server_error("sign_in_busy", None);
        return Err(Error::ResourceExhausted(HASH_BUSY.into()));
    }
    let waiting = Waiting(&hashing.waiting);
    let running = hashing.running.acquire().await.map_err(|_| Error::ResourceExhausted(HASH_BUSY.into()))?;
    drop(waiting);
    Ok(HashTurn { _running: running })
}

/// Hashes a password with Argon2id, off the async runtime.
pub async fn hash_password(password: String) -> Result<String> {
    let turn = hash_turn().await?;
    tokio::task::spawn_blocking(move || {
        let _turn = turn;
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default().hash_password(password.as_bytes(), &salt).map(|hash| hash.to_string())
    })
    .await
    .map_err(|err| Error::internal(format!("password hashing task: {err}")))?
    .map_err(|err| Error::internal(format!("hashing password: {err}")))
}

/// Checks a password against a stored hash, off the async runtime. With no hash
/// (unknown username), still does the work so timing doesn't reveal which
/// usernames exist.
pub async fn verify_password(password: String, hash: Option<String>) -> Result<bool> {
    let turn = hash_turn().await?;
    tokio::task::spawn_blocking(move || {
        let _turn = turn;
        let known = hash.is_some();
        let hash = hash.unwrap_or_else(|| dummy_hash().to_string());
        let parsed = PasswordHash::new(&hash).map_err(|err| Error::internal(format!("stored password hash: {err}")))?;
        Ok(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok() && known)
    })
    .await
    .map_err(|err| Error::internal(format!("password check task: {err}")))?
}

fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default().hash_password(b"not a real password", &salt).map(|h| h.to_string()).unwrap_or_default()
    })
}

/// Normalizes and checks a username: 2 to 32 of a-z, 0-9, `_` and `.`, starting
/// with a letter or digit.
pub fn validate_username(username: &str) -> Result<String> {
    let username = username.trim().to_lowercase();
    let valid_chars = username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    let valid_start = username.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if !(2..=32).contains(&username.len()) || !valid_chars || !valid_start {
        return Err(Error::invalid(
            "usernames are 2 to 32 characters of a-z, 0-9, _ and ., starting with a letter or digit",
        ));
    }
    Ok(username)
}

pub fn validate_password(password: &str) -> Result<()> {
    let length = password.chars().count();
    if !(8..=256).contains(&length) {
        return Err(Error::invalid("passwords are 8 to 256 characters"));
    }
    Ok(())
}

/// Slows down password guessing: after too many attempts for one key in a
/// window, further attempts are refused until the window passes.
///
/// Each attempt is counted before it's checked, under one lock, so guesses
/// sent all at once can't slip past the count while the first ones are still
/// being checked. A right answer clears the count.
#[derive(Default)]
pub struct SignInLimiter {
    attempts: Mutex<HashMap<String, (u32, i64)>>,
}

const MAX_ATTEMPTS: u32 = 10;
const WINDOW_MS: i64 = 15 * 60 * 1000;

impl SignInLimiter {
    /// Counts an attempt for `key`, or refuses it when there have been too
    /// many in the window.
    pub fn attempt(&self, key: &str) -> Result<()> {
        let mut attempts = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        let now = now_ms();
        attempts.retain(|_, (_, since)| now - *since < WINDOW_MS);
        let entry = attempts.entry(key.to_string()).or_insert((0, now));
        if entry.0 >= MAX_ATTEMPTS {
            return Err(Error::ResourceExhausted(
                "too many failed sign-ins for this account; try again in a few minutes".into(),
            ));
        }
        entry.0 += 1;
        Ok(())
    }

    /// Clears the count after a right answer.
    pub fn succeeded(&self, key: &str) {
        self.attempts.lock().unwrap_or_else(|p| p.into_inner()).remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        assert_eq!(validate_username("  Juan.A ").unwrap(), "juan.a");
        assert!(validate_username("a").is_err());
        assert!(validate_username("_juan").is_err());
        assert!(validate_username("juan alvarez").is_err());
        assert!(validate_username("ユーザー").is_err());
    }

    #[tokio::test]
    async fn passwords_round_trip() {
        let hash = hash_password("correct horse".into()).await.unwrap();
        assert!(verify_password("correct horse".into(), Some(hash.clone())).await.unwrap());
        assert!(!verify_password("wrong horse".into(), Some(hash)).await.unwrap());
        assert!(!verify_password("anything".into(), None).await.unwrap());
    }

    #[test]
    fn limiter_blocks_after_repeated_attempts() {
        let limiter = SignInLimiter::default();
        for _ in 0..MAX_ATTEMPTS {
            limiter.attempt("juan").unwrap();
        }
        assert!(limiter.attempt("juan").is_err());
        assert!(limiter.attempt("someone").is_ok());
        limiter.succeeded("juan");
        assert!(limiter.attempt("juan").is_ok());
    }

    #[test]
    fn limiter_counts_attempts_made_at_once() {
        let limiter = std::sync::Arc::new(SignInLimiter::default());
        let threads: Vec<_> = (0..64)
            .map(|_| {
                let limiter = limiter.clone();
                std::thread::spawn(move || limiter.attempt("juan").is_ok())
            })
            .collect();
        let let_through = threads.into_iter().map(|t| t.join().unwrap()).filter(|ok| *ok).count();
        assert_eq!(let_through, MAX_ATTEMPTS as usize);
    }
}
