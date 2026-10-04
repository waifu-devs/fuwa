//! How fuwa instances talk to each other (docs/federation.md): each instance
//! has an Ed25519 key, pins the keys of the instances it talks to, and signs
//! every call and every answer. Only the part keeping node.db (a single
//! process, or the directory) runs it; gateways pass it on.
//!
//! Nothing here sends or keeps anything about the people using either
//! instance: calls carry no forwarded addresses or user agents, and what's
//! counted for the anonymous reports never names another instance.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use prost::Message;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;
use tonic::{Request, Response, Status};

use crate::app::App;
use crate::error::{Error, Result};
use crate::{cpb, fpb};

/// What every signature starts with, so a federation signature can't be
/// taken for anything else signed with the same key.
const CONTEXT: &[u8] = b"fuwa-federation-v1";
/// How far an envelope's time may be from the receiver's clock.
const WINDOW_MS: i64 = 5 * 60 * 1000;
/// How long a nonce is remembered: past the window both ways.
const NONCE_KEEP_MS: i64 = 2 * WINDOW_MS;
/// The most nonces remembered for one instance at once; past it, its
/// signed calls wait (others' don't).
const MAX_NONCES_PER_PEER: usize = 50_000;
/// The largest payload an envelope carries.
pub const MAX_PAYLOAD: usize = 1 << 20;
/// How long a call to another instance may take.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How many instances this one fetches keys for a minute, when greeted by
/// ones it doesn't know yet: in all, and under one domain (so a wildcard
/// domain's endless names can't use up the rest).
const HELLOS_PER_MINUTE: usize = 60;
const HELLOS_PER_DOMAIN_PER_MINUTE: usize = 5;
/// The most instances this one pins a key for.
pub const MAX_PEERS: i64 = 1000;
/// Keys a share request pins under one registered domain, at most, so a
/// domain's endless names can't fill the pinned keys (an admin's check
/// isn't held to it).
const SHARE_PINS_PER_DOMAIN: usize = 3;
/// Share code lookups and asks a minute, at most, from one server here and
/// to one server here.
const SHARES_PER_SERVER_PER_MINUTE: usize = 20;
/// How often the time an instance was last heard from is written down.
const HEARD_EVERY_MS: i64 = 60 * 1000;

/// A new Ed25519 key, as PKCS#8.
pub fn new_key() -> Result<Vec<u8>> {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| Error::internal("couldn't make the instance's federation key"))?;
    Ok(document.as_ref().to_vec())
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

/// SHA-256 of a public key, as 8 groups of 4 hex digits, for people to compare.
pub fn fingerprint(public_key: &[u8]) -> String {
    let hash = to_hex(&Sha256::digest(public_key)[..16]);
    hash.as_bytes().chunks(4).map(|group| std::str::from_utf8(group).unwrap_or_default()).collect::<Vec<_>>().join(" ")
}

/// The host name in an address or a bare host name, lowercased, for the block
/// list. None for something that isn't one.
pub fn host_of(entry: &str) -> Option<String> {
    let entry = entry.trim();
    let with_scheme = if entry.contains("://") { entry.to_string() } else { format!("https://{entry}") };
    let url = url::Url::parse(&with_scheme).ok()?;
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty() && host.len() <= 253).then_some(host)
}

/// The origin (https://chat.example.com) an admin's address for another
/// instance stands for, or why it can't be one. Only https, and only public
/// addresses, unless `allow_private` (FUWA_FEDERATION_ALLOW_PRIVATE).
pub fn origin(address: &str, allow_private: bool) -> std::result::Result<String, String> {
    let bad = || "an instance's address is its host name or an https URL, like chat.example.com".to_string();
    let address = address.trim().trim_end_matches('/');
    if address.is_empty() || address.len() > 512 {
        return Err(bad());
    }
    let with_scheme = if address.contains("://") { address.to_string() } else { format!("https://{address}") };
    let url = url::Url::parse(&with_scheme).map_err(|_| bad())?;
    let scheme_ok = url.scheme() == "https" || (allow_private && url.scheme() == "http");
    if !scheme_ok
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(bad());
    }
    let internal = match url.host() {
        Some(url::Host::Ipv4(ip)) => !crate::outside::is_public(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => !crate::outside::is_public(IpAddr::V6(ip)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || [".localhost", ".internal", ".local"].iter().any(|end| name.ends_with(end))
        }
        None => true,
    };
    if internal && !allow_private {
        return Err(PRIVATE.into());
    }
    Ok(url.origin().ascii_serialization())
}

/// Why an instance at an internal address is refused.
const PRIVATE: &str = "other instances have to be on the internet, not a private or internal address \
     (whoever runs this instance can allow those with FUWA_FEDERATION_ALLOW_PRIVATE=1)";

/// Recent greetings from unknown instances: in all, and by domain.
type Hellos = (Vec<Instant>, HashMap<String, Vec<Instant>>);

/// What one instance keeps for talking to others.
pub struct Federation {
    allow_private: bool,
    client: reqwest::Client,
    key: OnceCell<Arc<Ed25519KeyPair>>,
    /// Nonces seen from each instance, until they can't be replayed anyway.
    nonces: Mutex<HashMap<String, HashMap<Vec<u8>, i64>>>,
    /// When this process started: envelopes signed before it are refused, as
    /// their nonces were forgotten with the last process.
    started_ms: i64,
    /// When the last greetings from unknown instances came, in all and by
    /// domain, for the caps.
    hellos: Mutex<Hellos>,
    /// When each instance's last_heard was written down.
    heard: Mutex<HashMap<String, i64>>,
    /// Instances this process said Hello to, and that answered.
    introduced: Mutex<HashSet<String>>,
    /// When the last share code lookups and asks were, by server, for the cap.
    shares: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Federation {
    pub fn new(allow_private: bool) -> Self {
        let client = reqwest::Client::builder()
            .dns_resolver(Arc::new(Resolver { allow_private }))
            // A proxy would look names up itself, past the resolver.
            .no_proxy()
            .https_only(!allow_private)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("the HTTP client builds");
        Self {
            allow_private,
            client,
            key: OnceCell::new(),
            nonces: Mutex::default(),
            started_ms: crate::id::now_ms(),
            hellos: Mutex::default(),
            heard: Mutex::default(),
            introduced: Mutex::default(),
            shares: Mutex::default(),
        }
    }

    pub fn allows_private(&self) -> bool {
        self.allow_private
    }

    /// Remembers a nonce, or says it was seen.
    fn fresh_nonce(&self, from: &str, nonce: &[u8], now: i64) -> std::result::Result<(), Refusal> {
        let mut all = self.nonces.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if all.len() > 64 && !all.contains_key(from) {
            all.retain(|_, seen| {
                seen.retain(|_, at| now - *at < NONCE_KEEP_MS);
                !seen.is_empty()
            });
        }
        let nonces = all.entry(from.to_string()).or_default();
        if nonces.len() >= MAX_NONCES_PER_PEER / 2 {
            nonces.retain(|_, seen| now - *seen < NONCE_KEEP_MS);
        }
        if nonces.len() >= MAX_NONCES_PER_PEER {
            return Err(Refusal::Busy);
        }
        match nonces.entry(nonce.to_vec()) {
            std::collections::hash_map::Entry::Occupied(_) => Err(Refusal::Replayed),
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(now);
                Ok(())
            }
        }
    }

    /// Whether another greeting from an instance this one doesn't know may
    /// be looked into now.
    fn take_hello(&self, origin: &str) -> bool {
        let mut hellos = self.hellos.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (all, by_domain) = &mut *hellos;
        let now = Instant::now();
        let recent = |at: &Instant| now.duration_since(*at) < Duration::from_secs(60);
        all.retain(recent);
        by_domain.retain(|_, times| {
            times.retain(recent);
            !times.is_empty()
        });
        let domain = domain_of(origin);
        let under_domain = by_domain.get(&domain).map_or(0, Vec::len);
        if all.len() >= HELLOS_PER_MINUTE || under_domain >= HELLOS_PER_DOMAIN_PER_MINUTE {
            return false;
        }
        all.push(now);
        by_domain.entry(domain).or_default().push(now);
        true
    }

    /// Whether another share code lookup or ask for `key` (a server, coming
    /// or going) may go now.
    fn take_share(&self, key: &str) -> bool {
        let mut shares = self.shares.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        shares.retain(|_, times| {
            times.retain(|at| now.duration_since(*at) < Duration::from_secs(60));
            !times.is_empty()
        });
        if shares.len() >= 10_000 && !shares.contains_key(key) {
            return false;
        }
        let times = shares.entry(key.to_string()).or_default();
        if times.len() >= SHARES_PER_SERVER_PER_MINUTE {
            return false;
        }
        times.push(now);
        true
    }

    fn introduced(&self, origin: &str) -> bool {
        self.introduced.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains(origin)
    }

    fn set_introduced(&self, origin: &str, introduced: bool) {
        let mut set = self.introduced.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if introduced {
            set.insert(origin.to_string());
        } else {
            set.remove(origin);
        }
    }

    /// Whether it's time to write down when `origin` was last heard from.
    fn note_heard(&self, origin: &str, now: i64) -> bool {
        let mut heard = self.heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match heard.get(origin) {
            Some(at) if now - at < HEARD_EVERY_MS => false,
            _ => {
                heard.insert(origin.to_string(), now);
                true
            }
        }
    }
}

/// Looks names up and drops internal addresses, so another instance's name
/// that points (or later re-points) inside this instance's network isn't called.
struct Resolver {
    allow_private: bool,
}

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let allow_private = self.allow_private;
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let public: Vec<SocketAddr> =
                found.filter(|a| allow_private || crate::outside::is_public(a.ip())).collect();
            if public.is_empty() {
                return Err(PRIVATE.into());
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Why a signed envelope was turned down. What the other side is told is
/// fixed text; nothing about this instance's insides.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Refusal {
    Malformed,
    NotForUs,
    Clock,
    Signature,
    Replayed,
    NotAnAnswer,
    BeforeStart,
    Busy,
}

impl Refusal {
    fn report(self) -> &'static str {
        match self {
            Refusal::Malformed => "federation_malformed",
            Refusal::NotForUs => "federation_not_for_us",
            Refusal::Clock => "federation_clock",
            Refusal::Signature => "federation_signature",
            Refusal::Replayed => "federation_replayed",
            Refusal::NotAnAnswer => "federation_not_an_answer",
            Refusal::BeforeStart => "federation_before_start",
            Refusal::Busy => "federation_busy",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Refusal::Malformed => "that envelope doesn't read",
            Refusal::NotForUs => "that envelope is for another instance",
            Refusal::Clock => "that envelope's time is more than 5 minutes off this instance's clock",
            Refusal::Signature => "that envelope's signature doesn't check out",
            Refusal::Replayed => "that envelope was already sent",
            Refusal::NotAnAnswer => "that answer isn't for this call",
            Refusal::BeforeStart => "that envelope was signed before this instance last started; send a new one",
            Refusal::Busy => "this instance is busy; try again",
        }
    }

    fn status(self) -> Status {
        crate::reports::server_error(self.report(), Some("federation"));
        match self {
            Refusal::Busy => Status::unavailable(self.message()),
            Refusal::Signature => Status::unauthenticated(self.message()),
            _ => Status::invalid_argument(self.message()),
        }
    }

    fn error(self, origin: &str) -> Error {
        crate::reports::server_error(self.report(), Some("federation"));
        Error::FailedPrecondition(format!("{}'s answer was turned down: {}", display(origin), self.message()))
    }
}

/// The domain an origin's host is under, roughly: its last two labels, or
/// three under a two-letter country's well-known second level (example.co.uk),
/// an IPv4 address itself, or an IPv6 address's /48. Only for capping
/// greetings, so rough is enough, but never finer than a registered domain.
fn domain_of(origin: &str) -> String {
    let host = host_of(origin).unwrap_or_default();
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return ip.to_string();
    }
    if let Ok(ip) = host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::Ipv6Addr>() {
        let s = ip.segments();
        return format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2]);
    }
    const SECOND_LEVELS: [&str; 10] = ["co", "com", "net", "org", "ac", "gov", "edu", "ne", "or", "go"];
    let labels: Vec<&str> = host.split('.').collect();
    let keep = match labels.as_slice() {
        [.., _, second, tld] if tld.len() == 2 && SECOND_LEVELS.contains(second) => 3,
        _ => 2,
    };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

/// An origin as people read it: the host, and the port when there is one.
pub fn display(origin: &str) -> &str {
    origin.split_once("://").map_or(origin, |(_, rest)| rest)
}

/// An origin as share codes and other instances' ids carry it: the host
/// (and port) for https, the whole origin otherwise (only ever with
/// FUWA_FEDERATION_ALLOW_PRIVATE), so [`origin`] reads it back the same.
pub fn address(origin: &str) -> &str {
    origin.strip_prefix("https://").unwrap_or(origin)
}

/// The bytes an envelope's signature covers.
fn signed_bytes(envelope: &fpb::Envelope) -> Vec<u8> {
    let mut out = Vec::with_capacity(CONTEXT.len() + envelope.payload.len() + 128);
    out.extend_from_slice(CONTEXT);
    for part in [envelope.from.as_bytes(), envelope.to.as_bytes()] {
        out.extend_from_slice(&(part.len() as u64).to_be_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&envelope.sent_at_ms.to_be_bytes());
    for part in [envelope.nonce.as_slice(), envelope.reply_to.as_slice(), envelope.payload.as_slice()] {
        out.extend_from_slice(&(part.len() as u64).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// What can be checked about an envelope before its sender's key is known.
fn precheck(own: &str, envelope: &fpb::Envelope) -> std::result::Result<(), Refusal> {
    if envelope.nonce.len() != 16 || envelope.payload.len() > MAX_PAYLOAD || envelope.signature.len() != 64 {
        return Err(Refusal::Malformed);
    }
    if envelope.to != own {
        return Err(Refusal::NotForUs);
    }
    if (crate::id::now_ms() - envelope.sent_at_ms).abs() > WINDOW_MS {
        return Err(Refusal::Clock);
    }
    Ok(())
}

/// Envelopes signed before this process started can't be checked for
/// replays (their nonces went with the last process), so they're refused.
fn after_start(federation: &Federation, envelope: &fpb::Envelope) -> std::result::Result<(), Refusal> {
    if envelope.sent_at_ms < federation.started_ms { Err(Refusal::BeforeStart) } else { Ok(()) }
}

/// Checks an envelope that came in against the key pinned for its sender:
/// addressed here, on time, signed, not seen before, and (for an answer)
/// answering `call`.
fn check(
    federation: &Federation,
    own: &str,
    envelope: &fpb::Envelope,
    public_key: &[u8],
    call: Option<&[u8]>,
) -> std::result::Result<(), Refusal> {
    precheck(own, envelope)?;
    after_start(federation, envelope)?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&signed_bytes(envelope), &envelope.signature)
        .map_err(|_| Refusal::Signature)?;
    match call {
        Some(call) if envelope.reply_to != call => return Err(Refusal::NotAnAnswer),
        None if !envelope.reply_to.is_empty() => return Err(Refusal::Malformed),
        _ => {}
    }
    federation.fresh_nonce(&envelope.from, &envelope.nonce, crate::id::now_ms())
}

/// This instance's origin, or why it can't talk to others yet.
pub fn own_origin(app: &App) -> Result<String> {
    let public_url = app.settings().public_url.clone();
    if public_url.is_empty() {
        return Err(Error::FailedPrecondition(
            "set this instance's public URL first: other instances know it by that address".into(),
        ));
    }
    origin(&public_url, app.federation.allows_private()).map_err(|why| {
        Error::FailedPrecondition(format!("this instance's public URL can't be used with other instances: {why}"))
    })
}

/// This instance's origin as its public URL names it, for naming it (in
/// share codes, say), not for calling: any part can say it, whether or not
/// it has FUWA_FEDERATION_ALLOW_PRIVATE, since the part that keeps the key
/// checks again before anything is sent.
pub fn named_origin(app: &App) -> Option<String> {
    origin(&app.settings().public_url, true).ok()
}

/// Whether an instance is on the block list.
pub fn blocked(app: &App, origin: &str) -> bool {
    host_of(origin).is_some_and(|host| app.settings().federation_blocked_hosts.contains(&host))
}

/// This instance's key pair, read (or made) once.
async fn key_pair(app: &App) -> Result<Arc<Ed25519KeyPair>> {
    app.federation
        .key
        .get_or_try_init(|| async {
            let pkcs8 = app.node()?.federation_key().await?;
            Ed25519KeyPair::from_pkcs8(&pkcs8)
                .map(Arc::new)
                .map_err(|_| Error::internal("the instance's federation key doesn't read"))
        })
        .await
        .cloned()
}

/// This instance's public key.
pub async fn public_key(app: &App) -> Result<Vec<u8>> {
    Ok(key_pair(app).await?.public_key().as_ref().to_vec())
}

/// A signed envelope from this instance to `to`.
async fn seal(app: &App, own: &str, to: &str, payload: Vec<u8>, reply_to: Vec<u8>) -> Result<fpb::Envelope> {
    let mut nonce = vec![0u8; 16];
    SystemRandom::new().fill(&mut nonce).map_err(|_| Error::internal("the OS random number generator failed"))?;
    let mut envelope = fpb::Envelope {
        from: own.to_string(),
        to: to.to_string(),
        sent_at_ms: crate::id::now_ms(),
        nonce,
        reply_to,
        payload,
        signature: Vec::new(),
    };
    envelope.signature = key_pair(app).await?.sign(&signed_bytes(&envelope)).as_ref().to_vec();
    Ok(envelope)
}

/// Writes down that `origin` was heard from, now and then.
async fn heard(app: &App, origin: &str) {
    if app.federation.note_heard(origin, crate::id::now_ms())
        && let Ok(node) = app.node()
        && node.heard_from_peer(origin).await.is_err()
    {
        crate::reports::server_error("federation_heard", Some("federation"));
    }
}

// --- Calling other instances ---

/// One gRPC-Web call to another instance, as a browser would make it, so it
/// goes through that instance's gateway like any other call.
async fn unary<Req: Message, Res: Message + Default>(
    app: &App,
    origin: &str,
    method: &str,
    request: &Req,
) -> Result<Res> {
    let shown = display(origin);
    let body = request.encode_to_vec();
    let mut framed = Vec::with_capacity(body.len() + 5);
    framed.push(0);
    framed.extend_from_slice(&(body.len() as u32).to_be_bytes());
    framed.extend_from_slice(&body);
    let started = Instant::now();
    let sent = app
        .federation
        .client
        .post(format!("{origin}/fuwa.federation.v1.FederationService/{method}"))
        .header("content-type", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(framed)
        .send()
        .await;
    let mut response = match sent {
        Ok(response) => response,
        Err(err) => {
            crate::reports::server_error("federation_unreachable", Some("federation"));
            let why = if err.is_timeout() {
                "it didn't answer in time"
            } else if err.is_connect() {
                "couldn't connect to it"
            } else {
                "the connection failed"
            };
            return Err(Error::Unavailable(format!("couldn't reach {shown}: {why}")));
        }
    };
    if !response.status().is_success() {
        crate::reports::server_error("federation_http_status", Some("federation"));
        return Err(Error::Unavailable(format!(
            "{shown} answered HTTP {}: is fuwa running there, with federation on?",
            response.status().as_u16()
        )));
    }
    if let Some(status) = grpc_status(response.headers().get("grpc-status"), response.headers().get("grpc-message")) {
        return Err(remote_error(shown, status));
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_PAYLOAD + 64 * 1024 {
                    return Err(Error::Unavailable(format!("{shown}'s answer is too big")));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return Err(Error::Unavailable(format!("{shown}'s answer was cut off"))),
        }
    }
    crate::reports::server_timing(&format!("federation:{method}"), started.elapsed());
    let mut message = None;
    let mut rest = bytes.as_slice();
    while rest.len() >= 5 {
        let flag = rest[0];
        let len = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        let Some(frame) = rest.get(5..5 + len) else { break };
        if flag & 0x80 == 0 {
            message.get_or_insert(frame);
        } else if let Some(status) = trailer_status(frame) {
            return Err(remote_error(shown, status));
        }
        rest = &rest[5 + len..];
    }
    let frame = message.ok_or_else(|| Error::Unavailable(format!("{shown}'s answer didn't read")))?;
    Res::decode(frame).map_err(|_| Error::Unavailable(format!("{shown}'s answer didn't read")))
}

/// A non-OK status from headers, as (code, message).
fn grpc_status(
    code: Option<&reqwest::header::HeaderValue>,
    message: Option<&reqwest::header::HeaderValue>,
) -> Option<(i32, String)> {
    let code: i32 = code?.to_str().ok()?.trim().parse().ok()?;
    (code != 0).then(|| (code, message.and_then(|m| m.to_str().ok()).map(percent_decode).unwrap_or_default()))
}

/// A non-OK status from a gRPC-Web trailers frame.
fn trailer_status(frame: &[u8]) -> Option<(i32, String)> {
    let text = std::str::from_utf8(frame).ok()?;
    let mut code = None;
    let mut message = String::new();
    for line in text.split("\r\n") {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "grpc-status" => code = value.trim().parse::<i32>().ok(),
                "grpc-message" => message = percent_decode(value.trim()),
                _ => {}
            }
        }
    }
    code.filter(|code| *code != 0).map(|code| (code, message))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What another instance said went wrong, for this instance's admins: its own
/// words, clipped and on one line.
fn remote_error(shown: &str, (code, message): (i32, String)) -> Error {
    crate::reports::server_error("federation_refused", Some("federation"));
    let message: String = message.chars().filter(|c| !c.is_control()).take(200).collect::<String>().trim().to_string();
    // What a shared channel's other end says when the share is gone, so
    // this end lets go of it too.
    if tonic::Code::from(code) == tonic::Code::NotFound {
        match message.as_str() {
            "shared channel not found" => return Error::NotFound("shared channel"),
            "share code not found" => return Error::NotFound("share code"),
            "server not found" => return Error::NotFound("server"),
            _ => {}
        }
    }
    let message = if message.is_empty() { format!("error {code}") } else { message };
    let text = format!("{shown} said: {message}");
    match tonic::Code::from(code) {
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => Error::Unavailable(text),
        _ => Error::FailedPrecondition(text),
    }
}

/// Another instance's key, fetched from it and checked to be for the
/// address asked.
async fn fetch_key(app: &App, origin: &str) -> Result<Vec<u8>> {
    let shown = display(origin);
    let key: fpb::GetKeyResponse = unary(app, origin, "GetKey", &fpb::GetKeyRequest {}).await?;
    if key.origin != origin {
        return Err(Error::FailedPrecondition(format!(
            "{shown} says it's {}, so its public URL doesn't match the address it was reached on",
            display(&key.origin)
        )));
    }
    if key.public_key.len() != 32 {
        return Err(Error::FailedPrecondition(format!("{shown}'s key doesn't read")));
    }
    Ok(key.public_key)
}

/// Another instance's key, fetched from it and pinned (or checked against
/// the one already pinned).
async fn pin(app: &App, origin: &str) -> Result<crate::node::FederationPeer> {
    let public_key = fetch_key(app, origin).await?;
    app.node()?.pin_federation_peer(origin, &public_key).await
}

/// Pins another instance's key for a share request: as [`pin`] does, but
/// no more than a few under one registered domain.
async fn pin_for_share(app: &App, origin: &str, public_key: &[u8]) -> Result<crate::node::FederationPeer> {
    let node = app.node()?;
    if node.federation_peer(origin).await?.is_none() {
        let domain = domain_of(origin);
        let under = node.federation_peers().await?.iter().filter(|peer| domain_of(&peer.origin) == domain).count();
        if under >= SHARE_PINS_PER_DOMAIN {
            crate::reports::server_error("federation_domain_pins", Some("federation"));
            return Err(Error::ResourceExhausted(format!(
                "this instance already knows {SHARE_PINS_PER_DOMAIN} instances under {domain}; an admin here can check another one"
            )));
        }
    }
    node.pin_federation_peer(origin, public_key).await
}

fn shares_capped() -> Error {
    crate::reports::server_error("federation_shares_capped", Some("federation"));
    Error::Limited("too many share codes looked up at once; try again in a minute".into(), 60_000)
}

/// Sends a signed envelope to another instance and checks its signed answer.
/// The signed calls.
#[derive(Clone, Copy)]
enum Method {
    Hello,
    Call,
}

async fn exchange(
    app: &App,
    own: &str,
    peer: &crate::node::FederationPeer,
    method: Method,
    payload: Vec<u8>,
) -> Result<Vec<u8>> {
    let envelope = seal(app, own, &peer.origin, payload, Vec::new()).await?;
    let answer = match method {
        Method::Hello => {
            let request = fpb::HelloRequest { envelope: Some(envelope.clone()) };
            unary::<_, fpb::HelloResponse>(app, &peer.origin, "Hello", &request).await?.envelope
        }
        Method::Call => {
            let request = fpb::CallRequest { envelope: Some(envelope.clone()) };
            unary::<_, fpb::CallResponse>(app, &peer.origin, "Call", &request).await?.envelope
        }
    };
    let answer = answer.ok_or_else(|| Refusal::Malformed.error(&peer.origin))?;
    if answer.from != peer.origin {
        return Err(Refusal::NotAnAnswer.error(&peer.origin));
    }
    check(&app.federation, own, &answer, &peer.public_key, Some(&envelope.nonce))
        .map_err(|refusal| refusal.error(&peer.origin))?;
    heard(app, &peer.origin).await;
    Ok(answer.payload)
}

/// Makes sure another instance can be talked to: federation is on, it isn't
/// blocked, and its key is pinned here (fetched and pinned now only when
/// `pin_new`: an admin's check, never on another instance's say-so). Says
/// Hello once per process, or again when `hello`.
async fn reach(app: &App, address: &str, pin_new: bool, hello: bool) -> Result<(String, crate::node::FederationPeer)> {
    let (own, origin) = allowed(app, address)?;
    let peer = match app.node()?.federation_peer(&origin).await? {
        Some(peer) => peer,
        None if pin_new => pin(app, &origin).await?,
        None => return Err(unknown(&origin)),
    };
    if hello || !app.federation.introduced(&origin) {
        exchange(app, &own, &peer, Method::Hello, fpb::Hello {}.encode_to_vec()).await?;
        app.federation.set_introduced(&origin, true);
    }
    Ok((own, peer))
}

/// This instance's origin and another's, if this one may talk to it:
/// federation is on, and it's someone else, not on the block list.
fn allowed(app: &App, address: &str) -> Result<(String, String)> {
    if !app.settings().federation {
        return Err(Error::FailedPrecondition("federation is off on this instance".into()));
    }
    let own = own_origin(app)?;
    let origin = origin(address, app.federation.allows_private()).map_err(Error::invalid)?;
    if origin == own {
        return Err(Error::invalid("that's this instance's own address"));
    }
    if blocked(app, &origin) {
        return Err(Error::FailedPrecondition(format!("{} is on this instance's block list", display(&origin))));
    }
    Ok((own, origin))
}

fn unknown(origin: &str) -> Error {
    Error::FailedPrecondition(format!(
        "this instance doesn't know {} yet: an admin here has to check it first",
        display(origin)
    ))
}

/// A call to the other end of a channel shared with another instance: a
/// server there, known here as "<id>@<instance>". A share code's lookup
/// may go to an instance whose key isn't pinned (checked with its key as
/// fetched now); asking for the share pins it; everything after needs it.
pub async fn shared(app: &App, mut call: cpb::SharedCall) -> Result<cpb::SharedReply> {
    let (server_id, address) = call
        .server_id
        .split_once('@')
        .ok_or(Error::NotFound("server"))
        .map(|(id, at)| (id.to_string(), at.to_string()))?;
    let (own, origin) = allowed(app, &address)?;
    let asker = match &call.call {
        Some(cpb::shared_call::Call::Lookup(lookup)) => Some(lookup.guest_server_id.as_str()),
        Some(cpb::shared_call::Call::Ask(ask)) => ask.guest.as_ref().map(|guest| guest.id.as_str()),
        _ => None,
    };
    if let Some(asker) = asker
        && !app.federation.take_share(&format!("out:{asker}"))
    {
        return Err(shares_capped());
    }
    let pinned = app.node()?.federation_peer(&origin).await?;
    let known = pinned.is_some();
    let peer = match pinned {
        Some(peer) => peer,
        // Checked with its key as fetched now; an ask pins it once the
        // home's signed answer checks out.
        None if asker.is_some() => crate::node::FederationPeer {
            public_key: fetch_key(app, &origin).await?,
            origin: origin.clone(),
            first_seen: 0,
            last_heard: 0,
        },
        None => return Err(unknown(&origin)),
    };
    call.server_id = server_id;
    call.from_instance.clear();
    call.from_fingerprint.clear();
    let started = Instant::now();
    let request = fpb::Request { call: Some(fpb::request::Call::Shared(Box::new(call.clone()))) };
    let answer = call_peer(app, &own, &peer, request).await?;
    crate::reports::server_timing("federation:shared", started.elapsed());
    let reply = match answer.answer {
        Some(fpb::response::Answer::Shared(reply)) => {
            crate::api::shared_returned(&call, *reply, &origin, &fingerprint(&peer.public_key))?
        }
        _ => return Err(Error::Unavailable(format!("{}'s answer didn't read", display(&origin)))),
    };
    if !known
        && let Some(undo) = crate::api::shared_undo(&call)
        && let Err(err) = pin_for_share(app, &origin, &peer.public_key).await
    {
        // Take the request back: what comes after couldn't be checked.
        let request = fpb::Request { call: Some(fpb::request::Call::Shared(Box::new(undo))) };
        let _ = call_peer(app, &own, &peer, request).await;
        return Err(err);
    }
    Ok(reply)
}

/// A signed call to another instance, and its answer.
pub async fn call(app: &App, address: &str, request: fpb::Request) -> Result<fpb::Response> {
    let (own, peer) = reach(app, address, false, false).await?;
    call_peer(app, &own, &peer, request).await
}

async fn call_peer(
    app: &App,
    own: &str,
    peer: &crate::node::FederationPeer,
    request: fpb::Request,
) -> Result<fpb::Response> {
    let answer = match exchange(app, own, peer, Method::Call, request.encode_to_vec()).await {
        Ok(answer) => answer,
        Err(err) => {
            // Say Hello again next time, in case the other side forgot this one.
            app.federation.set_introduced(&peer.origin, false);
            return Err(err);
        }
    };
    fpb::Response::decode(answer.as_slice())
        .map_err(|_| Error::Unavailable(format!("{}'s answer didn't read", display(&peer.origin))))
}

/// What an admin's check of another instance found.
pub struct Checked {
    pub peer: crate::node::FederationPeer,
    /// How long the signed Hello took there and back.
    pub took: Duration,
    /// The other instance has this one's key pinned too (its admins checked
    /// this one), so signed calls go through both ways.
    pub known_there: bool,
}

/// An admin's check that another instance can be talked to: pins its key
/// here, and times a signed Hello there and back, which it checks with this
/// instance's key fetched from this instance's address (without pinning it).
/// Then a signed ping says whether it pinned this instance too.
pub async fn check_instance(app: &App, address: &str) -> Result<Checked> {
    let started = Instant::now();
    let (own, peer) = reach(app, address, true, true).await?;
    let took = started.elapsed();
    let ping = fpb::Request { call: Some(fpb::request::Call::Ping(fpb::Ping {})) };
    let known_there = matches!(
        call_peer(app, &own, &peer, ping).await,
        Ok(fpb::Response { answer: Some(fpb::response::Answer::Pong(_)) })
    );
    let peer = app.node()?.federation_peer(&peer.origin).await?.unwrap_or(peer);
    Ok(Checked { peer, took, known_there })
}

/// A peer for the admin API.
pub fn peer_pb(app: &App, peer: &crate::node::FederationPeer) -> crate::pb::FederationPeer {
    crate::pb::FederationPeer {
        origin: peer.origin.clone(),
        fingerprint: fingerprint(&peer.public_key),
        first_seen: Some(crate::id::timestamp(peer.first_seen)),
        last_heard: Some(crate::id::timestamp(peer.last_heard)),
        blocked: blocked(app, &peer.origin),
    }
}

// --- Answering other instances ---

/// FederationService, on the instance's public port.
pub struct Service(pub Arc<App>);

impl Service {
    /// Federation is on and this instance has an address to be known by.
    fn ready(&self) -> std::result::Result<String, Status> {
        if !self.0.settings().federation {
            return Err(Status::failed_precondition("federation is off on this instance"));
        }
        own_origin(&self.0).map_err(|_| Status::failed_precondition("this instance isn't set up for federation"))
    }

    /// An envelope's sender, if it's an address this instance would talk to.
    fn sender(&self, envelope: &fpb::Envelope) -> std::result::Result<String, Status> {
        let from = origin(&envelope.from, self.0.federation.allows_private())
            .ok()
            .filter(|from| *from == envelope.from)
            .ok_or_else(|| Refusal::Malformed.status())?;
        if blocked(&self.0, &from) {
            return Err(Status::permission_denied("this instance doesn't talk to yours"));
        }
        Ok(from)
    }

    async fn answer(
        &self,
        own: &str,
        to: &str,
        call: &[u8],
        payload: Vec<u8>,
    ) -> std::result::Result<fpb::Envelope, Status> {
        seal(&self.0, own, to, payload, call.to_vec()).await.map_err(|_| {
            crate::reports::server_error("federation_seal", Some("federation"));
            Status::internal("this instance couldn't sign its answer")
        })
    }

    fn node(&self) -> std::result::Result<&crate::node::NodeDb, Status> {
        self.0.node().map_err(|_| Status::internal("this part keeps no data"))
    }

    /// The key of an instance this one hasn't pinned, fetched from its own
    /// address, which only whoever runs that address can answer, to check
    /// what it signed. Not pinned here. No more often than the caps, so
    /// strangers can't make this instance fetch endlessly.
    async fn stranger_key(&self, from: &str) -> std::result::Result<Vec<u8>, Status> {
        if !self.0.federation.take_hello(from) {
            crate::reports::server_error("federation_hellos_capped", Some("federation"));
            return Err(Status::resource_exhausted(
                "this instance is meeting too many instances; try again in a minute",
            ));
        }
        let key: fpb::GetKeyResponse = unary(&self.0, from, "GetKey", &fpb::GetKeyRequest {})
            .await
            .map_err(|_| Status::failed_precondition("this instance couldn't fetch your key from your address"))?;
        if key.origin != from || key.public_key.len() != 32 {
            return Err(Status::failed_precondition("the key at your address isn't for that address"));
        }
        Ok(key.public_key)
    }
}

#[tonic::async_trait]
impl fpb::federation_service_server::FederationService for Service {
    async fn get_key(
        &self,
        _request: Request<fpb::GetKeyRequest>,
    ) -> std::result::Result<Response<fpb::GetKeyResponse>, Status> {
        let own = self.ready()?;
        let public_key =
            public_key(&self.0).await.map_err(|_| Status::internal("this instance's key isn't readable"))?;
        Ok(Response::new(fpb::GetKeyResponse { origin: own, public_key }))
    }

    async fn hello(
        &self,
        request: Request<fpb::HelloRequest>,
    ) -> std::result::Result<Response<fpb::HelloResponse>, Status> {
        let own = self.ready()?;
        let envelope = request.into_inner().envelope.ok_or_else(|| Refusal::Malformed.status())?;
        let from = self.sender(&envelope)?;
        precheck(&own, &envelope).map_err(Refusal::status)?;
        let pinned =
            self.node()?.federation_peer(&from).await.map_err(|_| Status::internal("couldn't read the pinned keys"))?;
        let known = pinned.is_some();
        // Someone new isn't pinned: only an admin's check here, or a share
        // request, does that.
        let public_key = match pinned {
            Some(peer) => peer.public_key,
            None => self.stranger_key(&from).await?,
        };
        check(&self.0.federation, &own, &envelope, &public_key, None).map_err(Refusal::status)?;
        fpb::Hello::decode(envelope.payload.as_slice()).map_err(|_| Refusal::Malformed.status())?;
        if known {
            heard(&self.0, &from).await;
        }
        let answer = self.answer(&own, &from, &envelope.nonce, fpb::Hello {}.encode_to_vec()).await?;
        Ok(Response::new(fpb::HelloResponse { envelope: Some(answer) }))
    }

    async fn call(
        &self,
        request: Request<fpb::CallRequest>,
    ) -> std::result::Result<Response<fpb::CallResponse>, Status> {
        let own = self.ready()?;
        let envelope = request.into_inner().envelope.ok_or_else(|| Refusal::Malformed.status())?;
        let from = self.sender(&envelope)?;
        let pinned =
            self.node()?.federation_peer(&from).await.map_err(|_| Status::internal("couldn't read the pinned keys"))?;
        let known = pinned.is_some();
        // A share code's lookup or ask may come from an instance this one
        // doesn't know yet, checked with its key as fetched now.
        let public_key = match pinned {
            Some(peer) => peer.public_key,
            None => self.stranger_key(&from).await?,
        };
        check(&self.0.federation, &own, &envelope, &public_key, None).map_err(Refusal::status)?;
        let call = fpb::Request::decode(envelope.payload.as_slice()).map_err(|_| Refusal::Malformed.status())?;
        let first_contact = match &call.call {
            Some(fpb::request::Call::Shared(shared)) => {
                matches!(shared.call, Some(cpb::shared_call::Call::Lookup(_) | cpb::shared_call::Call::Ask(_)))
            }
            _ => false,
        };
        if !known && !first_contact {
            return Err(Status::unauthenticated("this instance doesn't know your key yet: say Hello first"));
        }
        if known {
            heard(&self.0, &from).await;
        }
        let started = Instant::now();
        let answer = match call.call {
            Some(fpb::request::Call::Ping(_)) => {
                fpb::Response { answer: Some(fpb::response::Answer::Pong(fpb::Pong {})) }
            }
            Some(fpb::request::Call::Shared(call)) => {
                let call = crate::api::shared_arrived(*call, &from, &fingerprint(&public_key)).map_err(Status::from)?;
                if first_contact && !self.0.federation.take_share(&format!("in:{}", call.server_id)) {
                    return Err(Status::from(shares_capped()));
                }
                // A share asked with a working code pins the asking
                // instance's key, so what comes after can be checked; one
                // this instance can't keep takes the request back.
                let undo = if known { None } else { crate::api::shared_undo(&call) };
                let reply = self.0.shared(call).await.map_err(Status::from)?;
                if let Some(undo) = undo
                    && let Err(err) = pin_for_share(&self.0, &from, &public_key).await
                {
                    let _ = self.0.shared(undo).await;
                    return Err(Status::from(err));
                }
                fpb::Response { answer: Some(fpb::response::Answer::Shared(Box::new(reply))) }
            }
            None => {
                return Err(Status::unimplemented("this instance doesn't know that call; it may run an older fuwa"));
            }
        };
        crate::reports::server_timing("federation:answer", started.elapsed());
        let answer = self.answer(&own, &from, &envelope.nonce, answer.encode_to_vec()).await?;
        Ok(Response::new(fpb::CallResponse { envelope: Some(answer) }))
    }
}

/// The service, with incoming messages capped.
pub fn server(app: Arc<App>) -> fpb::federation_service_server::FederationServiceServer<Service> {
    fpb::federation_service_server::FederationServiceServer::new(Service(app))
        .max_decoding_message_size(MAX_PAYLOAD + 64 * 1024)
        .max_encoding_message_size(MAX_PAYLOAD + 64 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_https_and_public_unless_allowed() {
        assert_eq!(origin("chat.example.com", false).unwrap(), "https://chat.example.com");
        assert_eq!(origin("https://Chat.Example.com/", false).unwrap(), "https://chat.example.com");
        assert_eq!(origin("https://chat.example.com:8443", false).unwrap(), "https://chat.example.com:8443");
        assert!(origin("http://chat.example.com", false).is_err());
        assert!(origin("https://chat.example.com/path", false).is_err());
        assert!(origin("https://user@chat.example.com", false).is_err());
        assert!(origin("https://chat.example.com?x=1", false).is_err());
        for internal in
            ["localhost", "127.0.0.1", "10.0.0.2", "[::1]", "db.railway.internal", "printer.local", "169.254.169.254"]
        {
            assert_eq!(origin(internal, false), Err(PRIVATE.to_string()), "{internal}");
        }
        assert_eq!(origin("http://127.0.0.1:4000", true).unwrap(), "http://127.0.0.1:4000");
    }

    #[test]
    fn hosts_for_the_block_list() {
        assert_eq!(host_of("Chat.Example.com").as_deref(), Some("chat.example.com"));
        assert_eq!(host_of("https://chat.example.com:8443/").as_deref(), Some("chat.example.com"));
        assert_eq!(host_of("not a host"), None);
    }

    #[test]
    fn greetings_are_capped_by_domain() {
        assert_eq!(domain_of("https://a.b.example.com"), "example.com");
        assert_eq!(domain_of("https://chat.example.co.uk"), "example.co.uk");
        assert_eq!(domain_of("http://127.0.0.1:4000"), "127.0.0.1");
        assert_eq!(domain_of("https://n1.abc.xyz"), "abc.xyz", "a short name isn't a public suffix");
        assert_eq!(domain_of("https://a.b.abc.io"), "abc.io");
        assert_eq!(domain_of("https://x.example.com.au"), "example.com.au");
        assert_eq!(domain_of("https://[2001:db8:1:2::5]"), "2001:db8:1::/48");
        assert_eq!(domain_of("https://[2001:db8:1:ffff::9]:8443"), "2001:db8:1::/48");
        let federation = Federation::new(false);
        for n in 0..HELLOS_PER_DOMAIN_PER_MINUTE {
            assert!(federation.take_hello(&format!("https://x{n}.wild.example")));
        }
        assert!(!federation.take_hello("https://another.wild.example"), "one domain's names share a cap");
        assert!(federation.take_hello("https://chat.example.org"), "other domains still get through");
    }

    #[test]
    fn share_lookups_are_capped_per_server() {
        let federation = Federation::new(false);
        for _ in 0..SHARES_PER_SERVER_PER_MINUTE {
            assert!(federation.take_share("in:a"));
        }
        assert!(!federation.take_share("in:a"));
        assert!(federation.take_share("in:b"), "other servers aren't held up");
    }

    #[test]
    fn nonces_are_kept_per_instance() {
        let federation = Federation::new(false);
        let now = crate::id::now_ms();
        assert_eq!(federation.fresh_nonce("https://a.example", &[1; 16], now), Ok(()));
        assert_eq!(federation.fresh_nonce("https://b.example", &[1; 16], now), Ok(()));
        assert_eq!(federation.fresh_nonce("https://a.example", &[1; 16], now), Err(Refusal::Replayed));
    }

    #[test]
    fn fingerprints_are_eight_groups_of_four() {
        let print = fingerprint(&[7u8; 32]);
        assert_eq!(print.split(' ').count(), 8);
        assert!(print.split(' ').all(|group| group.len() == 4));
    }

    #[test]
    fn signatures_cover_every_field() {
        let pkcs8 = new_key().unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(&pkcs8).unwrap();
        let federation = Federation::new(true);
        let mut envelope = fpb::Envelope {
            from: "https://a.example".into(),
            to: "https://b.example".into(),
            sent_at_ms: crate::id::now_ms(),
            nonce: vec![1; 16],
            reply_to: Vec::new(),
            payload: b"hi".to_vec(),
            signature: Vec::new(),
        };
        envelope.signature = pair.sign(&signed_bytes(&envelope)).as_ref().to_vec();
        let key = pair.public_key().as_ref().to_vec();
        let changed: [fn(&mut fpb::Envelope); 5] = [
            |e| e.from = "https://c.example".into(),
            |e| e.sent_at_ms += 1,
            |e| e.nonce = vec![2; 16],
            |e| e.reply_to = vec![3; 16],
            |e| e.payload = b"ho".to_vec(),
        ];
        for change in changed {
            let mut tampered = envelope.clone();
            change(&mut tampered);
            let checked = check(&federation, "https://b.example", &tampered, &key, None);
            assert!(matches!(checked, Err(Refusal::Signature | Refusal::Malformed)), "{checked:?}");
        }
        assert_eq!(check(&federation, "https://c.example", &envelope, &key, None), Err(Refusal::NotForUs));
        assert_eq!(check(&federation, "https://b.example", &envelope, &key, None), Ok(()));
        assert_eq!(check(&federation, "https://b.example", &envelope, &key, None), Err(Refusal::Replayed));
        // Signed before this process started: its nonce may have been seen
        // by the last one.
        let mut stale = envelope.clone();
        stale.nonce = vec![9; 16];
        stale.sent_at_ms = federation.started_ms - 1;
        stale.signature = pair.sign(&signed_bytes(&stale)).as_ref().to_vec();
        assert_eq!(check(&federation, "https://b.example", &stale, &key, None), Err(Refusal::BeforeStart));
        let mut old = envelope.clone();
        old.sent_at_ms -= WINDOW_MS + 1000;
        old.signature = pair.sign(&signed_bytes(&old)).as_ref().to_vec();
        assert_eq!(check(&federation, "https://b.example", &old, &key, None), Err(Refusal::Clock));
    }
}
