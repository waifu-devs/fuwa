//! Pictures from other sites, fetched by the instance so readers never load
//! them from the site themselves.
//!
//! A picture someone links to (an embed's image, a webhook post's avatar, a
//! waifu.dev profile picture) would otherwise be loaded by every reader's
//! browser or app straight from wherever it is, telling that site each
//! reader's IP address, their app and when they looked. So links are
//! rewritten when they're stored to `/media/outside/<signature>?url=<link>`
//! on this instance, which fetches the picture itself, without anything about
//! the reader, and hands it back.
//!
//! The signature is an HMAC of the link, so the instance only fetches links
//! it rewrote itself and can't be used to fetch anything else. A link to
//! another fuwa instance's picture ([`link_on_origin`], `&only=origin`) is
//! signed apart and never follows a redirect off that instance. Fetches go
//! only to public addresses (never loopback, private networks or cloud
//! metadata), follow at most a few redirects, and take only pictures of at
//! most [`MAX_BYTES`].

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path as UrlPath, RawQuery};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use http::{HeaderValue, StatusCode, header};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::app::App;

/// The biggest picture fetched.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Where rewritten links point, after the instance's address.
const PATH: &str = "/media/outside/";

/// How long one fetch may take, redirects and all.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Redirects followed before giving up.
const MAX_REDIRECTS: usize = 3;

/// Fetches at once; more wait their turn.
const MAX_FETCHES: usize = 16;

/// Pictures kept in memory, so a picture a whole server sees is fetched once.
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// The key links are signed with: 32 bytes, the same on every part of a
/// split instance (from the cluster key) or kept in node.db by a single process.
#[derive(Clone)]
pub struct Key([u8; 32]);

impl Key {
    /// The key for a split instance, from the cluster key every part has.
    pub fn from_cluster_key(cluster_key: &str) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"fuwa picture links\0");
        hash.update(cluster_key.as_bytes());
        Self(hash.finalize().into())
    }

    /// A key kept as hex in node.db.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| hex.get(i..i + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()))
            .collect::<Option<Vec<u8>>>()?;
        Some(Self(bytes.try_into().ok()?))
    }

    /// A MAC of `parts` under this key, for other things the instance signs
    /// (each with its own label first, so one can't stand for another).
    pub(crate) fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC takes keys of any length");
        for part in parts {
            mac.update(part);
        }
        mac.finalize().into_bytes().into()
    }

    fn sign(&self, url: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC takes keys of any length");
        mac.update(url.as_bytes());
        mac.finalize().into_bytes()[..16].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The signature of a link fetched only from its own origin: of
    /// something no plain link can be (a link has no NUL in it).
    fn sign_on_origin(&self, url: &str) -> String {
        self.sign(&format!("on its origin\0{url}"))
    }

    #[cfg(test)]
    fn signed(&self, signature: &str, url: &str) -> bool {
        crate::auth::constant_time_eq(signature.as_bytes(), self.sign(url).as_bytes())
    }

    fn signed_for(&self, signature: &str, wanted: &Wanted) -> bool {
        let expected = if wanted.on_origin { self.sign_on_origin(&wanted.url) } else { self.sign(&wanted.url) };
        crate::auth::constant_time_eq(signature.as_bytes(), expected.as_bytes())
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

/// The link to store for a picture at `url`: as it is when it's already on
/// this instance (an upload, or a link this instance rewrote), otherwise
/// rewritten to be fetched through `public_url`. Empty stays empty.
pub fn link(key: &Key, public_url: &str, url: &str) -> String {
    if url.is_empty() || is_ours(key, public_url, url) {
        return url.to_string();
    }
    let signature = key.sign(url);
    format!("{}{PATH}{signature}?url={}", public_url.trim_end_matches('/'), utf8_percent_encode(url, NON_ALPHANUMERIC))
}

/// The link to show for a picture on another fuwa instance (`url`, which
/// the caller checked is on it): fetched through `public_url` as [`link`]
/// does, but only from that instance, never following a redirect elsewhere.
pub fn link_on_origin(key: &Key, public_url: &str, url: &str) -> String {
    if url.is_empty() || is_ours(key, public_url, url) {
        return url.to_string();
    }
    let signature = key.sign_on_origin(url);
    format!(
        "{}{PATH}{signature}?url={}&only=origin",
        public_url.trim_end_matches('/'),
        utf8_percent_encode(url, NON_ALPHANUMERIC)
    )
}

/// Whether a link already points at this instance: an upload at its public
/// address, or a picture it fetches (on any of its addresses, as uploads are).
fn is_ours(key: &Key, public_url: &str, url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else { return false };
    let own = Url::parse(public_url).is_ok_and(|own| own.origin() == parsed.origin());
    if own && crate::media::id_in_url(url).is_some() {
        return true;
    }
    let Some(signature) = parsed.path().strip_prefix(PATH) else { return false };
    key.signed_for(signature, &wanted(parsed.query().map(str::to_string)))
}

/// `GET /media/outside/<signature>?url=<link>`: a picture from elsewhere.
pub fn routes(app: Arc<App>) -> Router {
    Router::new().route(
        "/media/outside/{signature}",
        get(move |signature: UrlPath<String>, query: RawQuery| serve(app.clone(), signature.0, wanted(query.0))),
    )
}

/// What a rewritten link asks for: the picture's `url`, and whether it's
/// fetched only from that link's own origin.
struct Wanted {
    url: String,
    on_origin: bool,
}

/// What a query string asks for.
fn wanted(query: Option<String>) -> Wanted {
    let query = query.unwrap_or_default();
    let pairs = || url::form_urlencoded::parse(query.as_bytes());
    Wanted {
        url: pairs().find(|(name, _)| name == "url").map(|(_, url)| url.into_owned()).unwrap_or_default(),
        on_origin: pairs().any(|(name, value)| name == "only" && value == "origin"),
    }
}

async fn serve(app: Arc<App>, signature: String, wanted: Wanted) -> Response {
    if !app.picture_key().signed_for(&signature, &wanted) {
        return failed(StatusCode::NOT_FOUND, "not found");
    }
    match cached(&wanted).await {
        Ok(picture) => picture.respond(),
        Err(Missing::ShuttingDown) => failed(StatusCode::SERVICE_UNAVAILABLE, "shutting down"),
        Err(Missing::Failed(reason)) => {
            tracing::debug!(reason, "couldn't fetch a picture from another site");
            failed(StatusCode::NOT_FOUND, "couldn't fetch that picture")
        }
        Err(Missing::TimedOut) => failed(StatusCode::GATEWAY_TIMEOUT, "that picture took too long to fetch"),
    }
}

/// Why a picture from elsewhere couldn't be had.
enum Missing {
    ShuttingDown,
    Failed(&'static str),
    TimedOut,
}

/// The picture a link asks for, from the cache or fetched (and then cached).
async fn cached(wanted: &Wanted) -> Result<Picture, Missing> {
    // Kept apart: the same link fetched both ways may end up elsewhere.
    let key = if wanted.on_origin { format!("on its origin\0{}", wanted.url) } else { wanted.url.clone() };
    if let Some(picture) = CACHE.get(&key) {
        return Ok(picture);
    }
    let Ok(_turn) = FETCHES.acquire().await else { return Err(Missing::ShuttingDown) };
    if let Some(picture) = CACHE.get(&key) {
        return Ok(picture);
    }
    let client = if wanted.on_origin { &*ON_ORIGIN } else { &*CLIENT };
    match tokio::time::timeout(FETCH_TIMEOUT, fetch_with(client, &wanted.url, MAX_BYTES)).await {
        Ok(Ok(picture)) => {
            CACHE.put(&key, picture.clone());
            Ok(picture)
        }
        Ok(Err(reason)) => Err(Missing::Failed(reason)),
        Err(_) => Err(Missing::TimedOut),
    }
}

/// The bytes and type of the picture a message links to, for the server
/// itself to read (AutoMod providers that look at pictures): an upload on
/// this instance from its files (only ever one of the message's own
/// attachments, checked as its sender's: see `automod::picture_links`), a link it rewrote or any other link
/// fetched like readers' pictures are (public addresses only, at most
/// [`MAX_BYTES`], cached). `None` when it isn't a picture or can't be had.
pub async fn picture(app: &App, url: &str) -> Option<(&'static str, Bytes)> {
    let parsed = Url::parse(url).ok()?;
    let public_url = app.settings().public_url.clone();
    let own = Url::parse(&public_url).is_ok_and(|own| own.origin() == parsed.origin());
    if own && let Some(id) = crate::media::id_in_url(url) {
        // Uploads are kept where node.db is; elsewhere they're left out.
        return own_file(app.media().ok()?.path(&id)).await;
    }
    if own && let Some((server_id, id)) = crate::media::server_file_in_url(url) {
        // A server's files are kept on the shard holding it: read here, or not at all.
        if !app.servers.holds(&server_id) {
            return None;
        }
        return own_file(app.config.data_path.join(crate::cluster::pictures::name(&server_id, &id))).await;
    }
    let inner = match parsed.path().strip_prefix(PATH) {
        Some(signature) => {
            let inner = wanted(parsed.query().map(str::to_string));
            if !app.picture_key().signed_for(signature, &inner) {
                return None;
            }
            inner
        }
        None => Wanted { url: url.to_string(), on_origin: false },
    };
    cached(&inner).await.ok().map(|picture| (picture.content_type, picture.bytes))
}

/// A picture uploaded here, read from `path`: none when it isn't a picture,
/// or is bigger than one fetched from elsewhere may be (an attachment can
/// be any size).
async fn own_file(path: std::path::PathBuf) -> Option<(&'static str, Bytes)> {
    let size = tokio::fs::metadata(&path).await.ok()?.len();
    if size > MAX_BYTES as u64 {
        return None;
    }
    let bytes = tokio::fs::read(&path).await.ok()?;
    let kind = crate::media::sniff(&bytes[..bytes.len().min(16)])?;
    Some((kind, bytes.into()))
}

fn failed(status: StatusCode, message: &str) -> Response {
    let mut response = (status, format!("{message}\n")).into_response();
    // Try again later, not on every scroll. Browsers only, as with every picture.
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    response
}

/// A fetched picture: its bytes and the type they turned out to be.
#[derive(Clone)]
pub(crate) struct Picture {
    pub content_type: &'static str,
    pub bytes: Bytes,
}

impl Picture {
    fn respond(self) -> Response {
        let mut response = self.bytes.into_response();
        let h = response.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static(self.content_type));
        // Browsers only: a message taken down shouldn't leave its pictures in a shared cache.
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"));
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        h.insert("cross-origin-resource-policy", HeaderValue::from_static("cross-origin"));
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
        h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        response
    }
}

static FETCHES: Semaphore = Semaphore::const_new(MAX_FETCHES);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| client(false));

/// The fetcher for pictures on another fuwa instance: as [`CLIENT`], but a
/// redirect may only go elsewhere on the same instance.
static ON_ORIGIN: LazyLock<reqwest::Client> = LazyLock::new(|| client(true));

fn client(on_origin: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("fuwa/{} (picture fetcher; +https://github.com/waifu-devs/fuwa)", crate::VERSION))
        // Never through a proxy from the environment: it would resolve
        // names itself, past the check on where they point.
        .no_proxy()
        .dns_resolver(Arc::new(PublicOnly))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            let elsewhere =
                on_origin && attempt.previous().first().is_some_and(|first| first.origin() != attempt.url().origin());
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if elsewhere {
                attempt.error("a redirect off the instance")
            } else if let Err(reason) = fetchable(attempt.url()) {
                attempt.error(reason)
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(Duration::from_secs(5))
        .timeout(FETCH_TIMEOUT)
        .build()
        .expect("the picture fetcher's settings are valid")
}

/// A picture at `url` of at most `max` bytes, fetched as readers' pictures
/// are (public addresses only), without the cache: for the instance to keep
/// (a GIF someone sends).
pub(crate) async fn fetch_up_to(url: &str, max: usize) -> Result<Picture, &'static str> {
    fetch_with(&CLIENT, url, max).await
}

async fn fetch_with(client: &reqwest::Client, url: &str, max: usize) -> Result<Picture, &'static str> {
    let url = Url::parse(url).map_err(|_| "not a link")?;
    fetchable(&url)?;
    let response = client
        .get(url)
        .header(header::ACCEPT, "image/avif,image/webp,image/png,image/jpeg,image/gif")
        .send()
        .await
        .map_err(|_| "the site didn't answer")?;
    if !response.status().is_success() {
        return Err("the site said no");
    }
    if response.content_length().is_some_and(|length| length > max as u64) {
        return Err("too big");
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "cut off")?;
        if body.len() + chunk.len() > max {
            return Err("too big");
        }
        body.extend_from_slice(&chunk);
    }
    // Whatever the site says it is, only pictures fuwa takes get through.
    let content_type = crate::media::sniff(&body[..body.len().min(16)]).ok_or("not a picture")?;
    Ok(Picture { content_type, bytes: body.into() })
}

/// Whether a link may be fetched: http(s), on the usual ports, and not at
/// an address that's plainly internal. Names are checked as they're looked
/// up ([`PublicOnly`]).
fn fetchable(url: &Url) -> Result<(), &'static str> {
    if !matches!(url.scheme(), "https" | "http") {
        return Err("not http(s)");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("has a password in it");
    }
    if url.port().is_some_and(|port| !matches!(port, 80 | 443 | 8080 | 8443)) {
        return Err("an unusual port");
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) if !is_public(IpAddr::V4(ip)) => Err("an internal address"),
        Some(url::Host::Ipv6(ip)) if !is_public(IpAddr::V6(ip)) => Err("an internal address"),
        Some(url::Host::Domain(name)) if name.eq_ignore_ascii_case("localhost") || name.ends_with(".internal") => {
            Err("an internal address")
        }
        Some(_) => Ok(()),
        None => Err("no host"),
    }
}

/// Looks names up and keeps only public addresses, so a name pointing at
/// this machine or its network can't be fetched. Single sign-on uses it too,
/// for the providers server managers set up.
pub(crate) struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = found.filter(|address| is_public(address.ip())).collect();
            if public.is_empty() {
                return Err("that name doesn't point anywhere public".into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// Whether an address is on the public internet.
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let first = ip.segments()[0];
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00 // unique local, fc00::/7
                || (first & 0xffc0) == 0xfe80 // link-local, fe80::/10
                || (first & 0xffc0) == 0xfec0 // site-local, fec0::/10
                || first == 0x2001 && ip.segments()[1] == 0x0db8 // documentation
                || first == 0x0064 && ip.segments()[1] == 0xff9b // NAT64, which reaches IPv4 inside
                || first == 0x2002 // 6to4, likewise
                || ip.segments()[..6] == [0; 6]) // IPv4-compatible, ::/96
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // shared address space, 100.64.0.0/10
        || (a == 192 && b == 0 && ip.octets()[2] == 0) // IETF protocol assignments
        || (a == 198 && (18..20).contains(&b)) // benchmarking
        || a >= 240) // reserved
}

/// Recently fetched pictures, up to [`CACHE_BYTES`], oldest dropped first.
struct Cache {
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    pictures: HashMap<String, (Instant, Picture)>,
    order: VecDeque<String>,
    bytes: usize,
}

static CACHE: LazyLock<Cache> = LazyLock::new(|| Cache { inner: Mutex::new(CacheInner::default()) });

impl Cache {
    fn get(&self, url: &str) -> Option<Picture> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.pictures.get(url).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, picture)| picture.clone())
    }

    fn put(&self, url: &str, picture: Picture) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, old)) = inner.pictures.remove(url) {
            inner.bytes -= old.bytes.len();
            inner.order.retain(|kept| kept != url);
        }
        while inner.bytes + picture.bytes.len() > CACHE_BYTES {
            let Some(oldest) = inner.order.pop_front() else { break };
            if let Some((_, dropped)) = inner.pictures.remove(&oldest) {
                inner.bytes -= dropped.bytes.len();
            }
        }
        inner.bytes += picture.bytes.len();
        inner.order.push_back(url.to_string());
        inner.pictures.insert(url.to_string(), (Instant::now(), picture));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: &str = "https://chat.example.com";

    fn key() -> Key {
        Key::from_cluster_key("a cluster key of at least thirty-two characters")
    }

    #[test]
    fn links_elsewhere_are_rewritten_to_come_through_the_instance() {
        let url = "https://pics.example.org/cat.png?size=2&x=a b";
        let rewritten = link(&key(), PUBLIC, url);
        assert!(rewritten.starts_with("https://chat.example.com/media/outside/"), "{rewritten}");
        let parsed = Url::parse(&rewritten).unwrap();
        let signature = parsed.path().strip_prefix(PATH).unwrap();
        let inner = parsed.query_pairs().find(|(name, _)| name == "url").unwrap().1;
        assert_eq!(inner, url);
        assert!(key().signed(signature, &inner));
        // Stored again (a profile saved unchanged), it stays as it is.
        assert_eq!(link(&key(), PUBLIC, &rewritten), rewritten);
        assert_eq!(link(&key(), PUBLIC, ""), "");
    }

    #[test]
    fn own_uploads_stay_but_lookalikes_elsewhere_dont() {
        let id = crate::media::new_id();
        let own = format!("{PUBLIC}/media/{id}");
        assert_eq!(link(&key(), PUBLIC, &own), own);
        let lookalike = format!("https://evil.example/media/{id}");
        assert!(link(&key(), PUBLIC, &lookalike).starts_with("https://chat.example.com/media/outside/"));
    }

    #[test]
    fn signatures_only_fit_their_link_and_key() {
        let signature = key().sign("https://a.example/1.png");
        assert!(key().signed(&signature, "https://a.example/1.png"));
        assert!(!key().signed(&signature, "https://a.example/2.png"));
        assert!(
            !Key::from_cluster_key("another cluster key, also long enough!!")
                .signed(&signature, "https://a.example/1.png")
        );
        // A link pretending to be rewritten, with a made-up signature, is rewritten itself.
        let forged = format!("{PUBLIC}{PATH}{}?url=http%3A%2F%2F169.254.169.254%2F", "0".repeat(32));
        assert_ne!(link(&key(), PUBLIC, &forged), forged);
    }

    #[test]
    fn links_fetched_only_from_their_origin_keep_to_it() {
        let url = "https://b.example/media/abc";
        let rewritten = link_on_origin(&key(), PUBLIC, url);
        assert!(rewritten.ends_with("&only=origin"), "{rewritten}");
        let parsed = Url::parse(&rewritten).unwrap();
        let signature = parsed.path().strip_prefix(PATH).unwrap();
        let on_origin = wanted(parsed.query().map(str::to_string));
        assert!(on_origin.on_origin && on_origin.url == url);
        assert!(key().signed_for(signature, &on_origin));
        // Its signature doesn't serve the same link without the origin rule,
        // nor a plain link's with it.
        assert!(!key().signed_for(signature, &Wanted { url: url.into(), on_origin: false }));
        let plain = key().sign(url);
        assert!(!key().signed_for(&plain, &on_origin));
        assert_eq!(link_on_origin(&key(), PUBLIC, &rewritten), rewritten);
    }

    #[test]
    fn keys_read_back_from_hex() {
        let hex = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let key = Key::from_hex(hex).unwrap();
        assert_eq!(key.0[1], 0x11);
        assert!(Key::from_hex("abc").is_none());
        assert!(Key::from_hex("zz").is_none());
    }

    #[test]
    fn internal_addresses_are_never_fetched() {
        for url in [
            "http://127.0.0.1/a.png",
            "http://localhost/a.png",
            "http://10.0.0.5/a.png",
            "http://192.168.1.1/a.png",
            "http://172.16.0.1/a.png",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/a.png",
            "http://0.0.0.0/a.png",
            "http://[::1]/a.png",
            "http://[fd12:3456::1]/a.png",
            "http://[fe80::1]/a.png",
            "http://[::ffff:127.0.0.1]/a.png",
            "http://[64:ff9b::7f00:1]/a.png",
            "http://fuwa.railway.internal/a.png",
            "https://pics.example.org:22/a.png",
            "https://user:pass@pics.example.org/a.png",
            "file:///etc/passwd",
            "ftp://pics.example.org/a.png",
        ] {
            assert!(fetchable(&Url::parse(url).unwrap()).is_err(), "{url} should be refused");
        }
        for url in ["https://pics.example.org/a.png", "http://93.184.215.14/a.png", "https://[2606:4700::1111]/a.png"] {
            assert!(fetchable(&Url::parse(url).unwrap()).is_ok(), "{url} should be allowed");
        }
    }

    #[test]
    fn the_cache_keeps_to_its_size() {
        let cache = Cache { inner: Mutex::new(CacheInner::default()) };
        let picture = |n: usize| Picture { content_type: "image/png", bytes: Bytes::from(vec![0u8; n]) };
        cache.put("a", picture(CACHE_BYTES / 2));
        cache.put("b", picture(CACHE_BYTES / 2));
        cache.put("c", picture(10));
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_some());
        assert!(cache.get("c").is_some());
        assert!(cache.inner.lock().unwrap().bytes <= CACHE_BYTES);
    }
}
