//! GIF search and the GIFs people send (`GifService`, `api/gifs.rs`).
//!
//! The instance's admins pick a provider (GIPHY or Klipy, `giphy.rs` and
//! `klipy.rs`) and give its key. The instance makes every call to it itself,
//! with nothing about who asked: no address, no account, none of their
//! headers, and the search words never reach a log. The pictures in an
//! answer are rewritten to come through the instance (`App::picture_link`),
//! so apps never load anything from the provider either.
//!
//! A result's id is a signed token holding what the provider said about it
//! (its title and files), so sending one needs no second question to the
//! provider. Sending stores the GIF once for the whole instance (`store.rs`),
//! without its metadata, so it outlives the provider dropping it; the
//! message holds a [`pb::MessageGif`] whose seal says this instance made it.

mod giphy;
mod klipy;
pub mod store;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::app::App;
use crate::error::{Error, Result};
use crate::id::now_ms;
use crate::pb;

/// How long a provider has to answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(8);

/// The biggest answer read from a provider.
const MAX_ANSWER_BYTES: usize = 2 * 1024 * 1024;

/// Questions to the provider at once; more wait their turn.
const MAX_ASKS: usize = 8;

/// How long an answer is kept and given again.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_ENTRIES: usize = 512;

/// How long the categories' pictures are kept.
const CATEGORIES_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// The longest search.
pub const MAX_QUERY: usize = 100;

/// The longest title kept.
const MAX_TITLE: usize = 200;

/// Moods to browse, as GIF pickers usually offer them.
pub const CATEGORIES: [(&str, &str); 16] = [
    ("Happy", "happy"),
    ("Laugh", "laughing"),
    ("Love", "love"),
    ("Sad", "sad"),
    ("Hug", "hug"),
    ("Dance", "dance"),
    ("Wow", "wow"),
    ("Yes", "yes"),
    ("No", "no"),
    ("Thank you", "thank you"),
    ("Facepalm", "facepalm"),
    ("Hello", "hello"),
    ("Good night", "good night"),
    ("Excited", "excited"),
    ("Cat", "cat"),
    ("Anime", "anime"),
];

/// Ratings a provider is asked to stay within, mildest first.
const RATINGS: [&str; 4] = ["g", "pg", "pg-13", "r"];

/// Which provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Off,
    Giphy,
    Klipy,
}

impl Kind {
    pub fn to_pb(self) -> pb::GifProvider {
        match self {
            Kind::Off => pb::GifProvider::Unspecified,
            Kind::Giphy => pb::GifProvider::Giphy,
            Kind::Klipy => pb::GifProvider::Klipy,
        }
    }

    fn from_pb(value: i32) -> Self {
        match pb::GifProvider::try_from(value) {
            Ok(pb::GifProvider::Giphy) => Kind::Giphy,
            Ok(pb::GifProvider::Klipy) => Kind::Klipy,
            _ => Kind::Off,
        }
    }

    /// Its name in the anonymous report.
    fn report_id(self) -> &'static str {
        match self {
            Kind::Off => "off",
            Kind::Giphy => "giphy",
            Kind::Klipy => "klipy",
        }
    }
}

/// The instance's GIF settings (`InstanceSettings.gifs`), key and all.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Setup {
    pub provider: Kind,
    pub api_key: String,
    /// Empty for "pg-13".
    pub rating: String,
    pub gif_bytes: Option<i64>,
    pub searches_per_minute: Option<i64>,
    pub provider_calls_per_day: Option<i64>,
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("provider", &self.provider)
            .field("api_key", &if self.api_key.is_empty() { "" } else { "***" })
            .field("rating", &self.rating)
            .field("gif_bytes", &self.gif_bytes)
            .field("searches_per_minute", &self.searches_per_minute)
            .field("provider_calls_per_day", &self.provider_calls_per_day)
            .finish()
    }
}

impl Setup {
    /// A provider and its key are set: search works.
    pub fn usable(&self) -> bool {
        self.provider != Kind::Off && !self.api_key.is_empty()
    }

    fn rating(&self) -> &str {
        if self.rating.is_empty() { "pg-13" } else { &self.rating }
    }

    /// As the settings API shows it; the key only when `with_key`.
    pub fn to_pb(&self, with_key: bool) -> pb::GifSettings {
        pb::GifSettings {
            provider: self.provider.to_pb() as i32,
            api_key: if with_key { self.api_key.clone() } else { String::new() },
            api_key_set: !self.api_key.is_empty(),
            api_key_hint: crate::settings::hint(&self.api_key),
            rating: self.rating().to_string(),
            gif_bytes: self.gif_bytes,
            searches_per_minute: self.searches_per_minute,
            provider_calls_per_day: self.provider_calls_per_day,
        }
    }

    /// From a request: an empty key keeps `previous`'s.
    pub fn from_pb(given: &pb::GifSettings, previous: &Setup) -> Result<Setup> {
        let key = given.api_key.trim();
        let setup = Setup {
            provider: Kind::from_pb(given.provider),
            api_key: if key.is_empty() { previous.api_key.clone() } else { key.to_string() },
            rating: match given.rating.trim().to_ascii_lowercase().as_str() {
                "" | "pg-13" => String::new(),
                other => other.to_string(),
            },
            gif_bytes: given.gif_bytes,
            searches_per_minute: given.searches_per_minute,
            provider_calls_per_day: given.provider_calls_per_day,
        };
        setup.check()?;
        Ok(setup)
    }

    /// Whether it makes sense, as stored or sent.
    pub fn check(&self) -> Result<()> {
        if self.api_key.len() > 256 || self.api_key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(Error::invalid("the GIF provider's key is up to 256 characters, with no spaces"));
        }
        // Klipy takes its key in the address, so only what's safe there.
        if self.provider == Kind::Klipy && !self.api_key.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
        {
            return Err(Error::invalid("a Klipy key is letters, digits, - and _"));
        }
        if !self.rating.is_empty() && !RATINGS.contains(&self.rating.as_str()) {
            return Err(Error::invalid("the GIF rating is g, pg, pg-13 or r"));
        }
        for (name, cap) in [
            ("gif_bytes", self.gif_bytes),
            ("searches_per_minute", self.searches_per_minute),
            ("provider_calls_per_day", self.provider_calls_per_day),
        ] {
            if cap.is_some_and(|n| n < 0) {
                return Err(Error::invalid(format!("{name} must be 0 or more, or unset for no cap")));
            }
        }
        Ok(())
    }
}

/// One of a provider's files for a GIF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rendition {
    #[serde(rename = "u")]
    pub url: String,
    #[serde(rename = "w")]
    pub width: u32,
    #[serde(rename = "h")]
    pub height: u32,
    /// Bytes, 0 when the provider didn't say.
    #[serde(rename = "s")]
    pub size: u64,
}

/// A GIF in a provider's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    pub title: String,
    /// A small moving version.
    pub preview: Rendition,
    pub still: Option<Rendition>,
    /// What can be stored, biggest first.
    pub full: Vec<Rendition>,
}

/// A page of an answer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub found: Vec<Found>,
    /// Where the next page starts, if there is one.
    pub next: Option<String>,
}

/// What a question to the provider is.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    /// Empty for trending.
    pub query: &'a str,
    /// Where the page starts: a number, as `Page::next` gave it.
    pub cursor: u32,
    pub limit: u32,
}

/// Why the provider gave nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// It turned the key down.
    Key,
    /// Too many questions; it said to slow down.
    Busy,
    /// It didn't answer in time, or couldn't be reached.
    Unreachable,
    /// Its answer didn't read.
    BadAnswer,
}

impl Failure {
    fn kind(self) -> &'static str {
        match self {
            Failure::Key => "gif_provider_key",
            Failure::Busy => "gif_provider_busy",
            Failure::Unreachable => "gif_provider_unreachable",
            Failure::BadAnswer => "gif_provider_bad_answer",
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Failure::Key => "the provider turned the key down",
            Failure::Busy => "the provider says to slow down",
            Failure::Unreachable => "the provider didn't answer",
            Failure::BadAnswer => "the provider's answer didn't read",
        })
    }
}

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Busy => Error::ResourceExhausted("GIF search is busy; try again in a moment".into()),
            _ => Error::Unavailable("GIF search isn't working right now; try again later".into()),
        }
    }
}

static ASKS: Semaphore = Semaphore::const_new(MAX_ASKS);

/// The client for providers' APIs: public addresses only, never through a
/// proxy from the environment, sending nothing but what each call needs.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(format!("fuwa/{} (GIF search; +https://github.com/waifu-devs/fuwa)", crate::VERSION))
        .no_proxy()
        .dns_resolver(Arc::new(crate::outside::PublicOnly))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(ASK_TIMEOUT)
        .build()
        .expect("the GIF client's settings are valid")
});

/// For FUWA_GIF_API_URL (tests): anywhere, still never through a proxy.
static TEST_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(ASK_TIMEOUT)
        .build()
        .expect("the GIF client's settings are valid")
});

/// Asks the provider, without the cache or the daily cap (an admin's test,
/// or a question the cache didn't have).
pub async fn fetch_page(app: &App, setup: &Setup, ask: Ask<'_>) -> std::result::Result<Page, Failure> {
    let base = app.config.gif_api_url.as_deref();
    let url = match setup.provider {
        Kind::Giphy => giphy::address(base, setup, ask),
        Kind::Klipy => klipy::address(base, setup, ask),
        Kind::Off => return Err(Failure::Key),
    };
    let client = if base.is_some() { &TEST_CLIENT } else { &CLIENT };
    let Ok(_turn) = ASKS.acquire().await else { return Err(Failure::Unreachable) };
    let started = Instant::now();
    let answer = tokio::time::timeout(ASK_TIMEOUT, async {
        let response = client
            .get(url)
            .header(http::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| Failure::Unreachable)?;
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(Failure::Key),
            429 => return Err(Failure::Busy),
            _ => return Err(Failure::BadAnswer),
        }
        if response.content_length().is_some_and(|n| n > MAX_ANSWER_BYTES as u64) {
            return Err(Failure::BadAnswer);
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Failure::Unreachable)?;
            if body.len() + chunk.len() > MAX_ANSWER_BYTES {
                return Err(Failure::BadAnswer);
            }
            body.extend_from_slice(&chunk);
        }
        let page = match setup.provider {
            Kind::Giphy => giphy::read(&body, ask),
            _ => klipy::read(&body, ask),
        };
        page.ok_or(Failure::BadAnswer)
    })
    .await
    .unwrap_or(Err(Failure::Unreachable));
    let took = started.elapsed();
    crate::reports::server_timing(&format!("gifs:{}", setup.provider.report_id()), took);
    if let Err(failure) = answer {
        crate::reports::server_error(failure.kind(), Some(setup.provider.report_id()));
    }
    answer
}

/// Answers kept a while, by a hash of the question (never the words).
struct Cache {
    pages: HashMap<[u8; 32], (Instant, Page)>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache { pages: HashMap::new() }));

fn cache_key(setup: &Setup, ask: Ask<'_>) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(setup.provider.report_id());
    hash.update([0]);
    hash.update(setup.rating());
    hash.update([0]);
    hash.update(ask.query.to_lowercase());
    hash.update([0]);
    hash.update(ask.cursor.to_le_bytes());
    hash.update(ask.limit.to_le_bytes());
    hash.finalize().into()
}

/// Asks the provider, or gives an answer kept from a recent ask. A new
/// question counts toward the instance's daily cap.
pub async fn ask(app: &App, setup: &Setup, ask: Ask<'_>) -> Result<Page> {
    let key = cache_key(setup, ask);
    {
        let cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, page)) = cache.pages.get(&key)
            && at.elapsed() < CACHE_TTL
        {
            return Ok(page.clone());
        }
    }
    if let Some(cap) = setup.provider_calls_per_day {
        store::count_call(app.node()?, now_ms(), cap).await?;
    }
    let page = fetch_page(app, setup, ask).await?;
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if cache.pages.len() >= CACHE_ENTRIES {
        cache.pages.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        if cache.pages.len() >= CACHE_ENTRIES {
            cache.pages.clear();
        }
    }
    cache.pages.insert(key, (Instant::now(), page.clone()));
    Ok(page)
}

/// Each account's searches this minute.
static MINUTES: LazyLock<Mutex<HashMap<String, (i64, i64)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Counts one search (or GIF sent from search) for `account_id`, refusing
/// past the instance's per-minute cap.
pub fn take_turn(account_id: &str, per_minute: Option<i64>) -> Result<()> {
    let Some(cap) = per_minute else { return Ok(()) };
    let minute = now_ms() / 60_000;
    let mut minutes = MINUTES.lock().unwrap_or_else(|p| p.into_inner());
    if minutes.len() > 10_000 {
        minutes.retain(|_, (at, _)| *at == minute);
    }
    let entry = minutes.entry(account_id.to_string()).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    if entry.1 >= cap {
        return Err(Error::ResourceExhausted("that's a lot of GIF searches; wait a minute".into()));
    }
    entry.1 += 1;
    Ok(())
}

/// Categories asked lately, by provider and rating, with when they were asked.
type KeptCategories = HashMap<String, (Instant, Vec<pb::GifCategory>)>;

/// The categories, each with the first GIF its search finds; kept a while.
pub async fn categories(app: &App, setup: &Setup) -> Result<Vec<pb::GifCategory>> {
    static KEPT: LazyLock<Mutex<KeptCategories>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = format!("{}:{}", setup.provider.report_id(), setup.rating());
    if let Some((at, kept)) = KEPT.lock().unwrap_or_else(|p| p.into_inner()).get(&key)
        && at.elapsed() < CATEGORIES_TTL
    {
        return Ok(kept.clone());
    }
    let asks = CATEGORIES.iter().map(|(_, query)| ask(app, setup, Ask { query, cursor: 0, limit: 1 }));
    let answers = futures::future::join_all(asks).await;
    let mut failed = None;
    let mut categories = Vec::new();
    for ((name, query), answer) in CATEGORIES.iter().zip(answers) {
        match answer {
            Ok(page) => categories.push(pb::GifCategory {
                name: name.to_string(),
                query: query.to_string(),
                preview: page.found.first().map(|found| result(app, setup.provider, found)),
            }),
            Err(err) => failed = Some(err),
        }
    }
    match failed {
        Some(err) if categories.is_empty() => Err(err),
        // Some didn't come: show what did, and ask again next time.
        Some(_) => Ok(categories),
        None => {
            KEPT.lock().unwrap_or_else(|p| p.into_inner()).insert(key, (Instant::now(), categories.clone()));
            Ok(categories)
        }
    }
}

/// What a result's id holds: what the provider said about the GIF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    #[serde(rename = "p")]
    pub provider: Kind,
    pub id: String,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "f")]
    pub full: Vec<Rendition>,
}

const TOKEN_LABEL: &[u8] = b"fuwa gif result\0";
const SEAL_LABEL: &[u8] = b"fuwa gif seal\0";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A result as apps see it: its id a signed [`Token`], its pictures through
/// this instance.
pub fn result(app: &App, provider: Kind, found: &Found) -> pb::GifResult {
    let token = Token { provider, id: found.id.clone(), title: clip(&found.title), full: found.full.clone() };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&token).expect("tokens always serialize"));
    let mac = app.picture_key().mac(&[TOKEN_LABEL, payload.as_bytes()]);
    pb::GifResult {
        id: format!("{payload}.{}", hex(&mac[..16])),
        title: token.title,
        preview_url: app.picture_link(&found.preview.url),
        still_url: found.still.as_ref().map(|s| app.picture_link(&s.url)).unwrap_or_default(),
        width: found.preview.width as i32,
        height: found.preview.height as i32,
    }
}

/// The token in a result's id, if this instance made it.
pub fn open_result(app: &App, id: &str) -> Result<Token> {
    let refused = || Error::invalid("that isn't a GIF from this instance's search");
    let (payload, mac) = id.rsplit_once('.').ok_or_else(refused)?;
    let expected = hex(&app.picture_key().mac(&[TOKEN_LABEL, payload.as_bytes()])[..16]);
    if id.len() > 8192 || !crate::auth::constant_time_eq(mac.as_bytes(), expected.as_bytes()) {
        return Err(refused());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).map_err(|_| refused())?;
    serde_json::from_slice(&bytes).map_err(|_| refused())
}

/// A title as kept: trimmed, one line, at most [`MAX_TITLE`] characters.
fn clip(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TITLE).collect()
}

/// The seal on a GIF stored as `media_id`: what SendMessage checks.
fn seal_of(app: &App, media_id: &str, gif: &pb::MessageGif) -> String {
    let mac = app.picture_key().mac(&[
        SEAL_LABEL,
        media_id.as_bytes(),
        b"\0",
        &gif.width.to_le_bytes(),
        &gif.height.to_le_bytes(),
        &gif.provider.to_le_bytes(),
        gif.title.as_bytes(),
    ]);
    hex(&mac[..16])
}

/// A stored GIF, sealed, at this instance's address.
pub fn sealed(app: &App, media_id: &str, width: i32, height: i32, title: &str, provider: i32) -> pb::MessageGif {
    let mut gif = pb::MessageGif {
        url: format!("{}/media/{media_id}", app.settings().public_url),
        width,
        height,
        title: title.to_string(),
        provider,
        seal: String::new(),
    };
    gif.seal = seal_of(app, media_id, &gif);
    gif
}

/// Checks the GIF a message is sent with: sealed by this instance (on any
/// part of it), and gives it back as stored (at this instance's address,
/// without the seal).
pub fn open_seal(app: &App, gif: &pb::MessageGif) -> Result<pb::MessageGif> {
    let refused = || Error::invalid("that GIF didn't come from this instance; pick it again");
    let media_id = crate::media::id_in_url(&gif.url).ok_or_else(refused)?;
    if !crate::auth::constant_time_eq(seal_of(app, &media_id, gif).as_bytes(), gif.seal.as_bytes()) {
        return Err(refused());
    }
    Ok(pb::MessageGif {
        url: format!("{}/media/{media_id}", app.settings().public_url),
        seal: String::new(),
        ..gif.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_check_what_they_hold() {
        let given = |provider: pb::GifProvider, key: &str| pb::GifSettings {
            provider: provider as i32,
            api_key: key.into(),
            ..Default::default()
        };
        let setup = Setup::from_pb(&given(pb::GifProvider::Giphy, " abc123 "), &Setup::default()).unwrap();
        assert_eq!(setup.api_key, "abc123");
        assert!(setup.usable());
        // An empty key keeps the saved one.
        let kept = Setup::from_pb(&given(pb::GifProvider::Giphy, ""), &setup).unwrap();
        assert_eq!(kept.api_key, "abc123");
        assert!(Setup::from_pb(&given(pb::GifProvider::Giphy, "a b"), &Setup::default()).is_err());
        assert!(Setup::from_pb(&given(pb::GifProvider::Klipy, "a/b"), &Setup::default()).is_err());
        let bad_rating = pb::GifSettings { rating: "nc-17".into(), ..given(pb::GifProvider::Giphy, "k") };
        assert!(Setup::from_pb(&bad_rating, &Setup::default()).is_err());
        let negative = pb::GifSettings { gif_bytes: Some(-1), ..given(pb::GifProvider::Giphy, "k") };
        assert!(Setup::from_pb(&negative, &Setup::default()).is_err());
        assert!(!Setup::default().usable());
        // Shown to admins: whether a key is set and its end, never the key.
        let long = Setup { api_key: "0123456789abcdef".into(), ..setup.clone() };
        let shown = long.to_pb(false);
        assert!(shown.api_key.is_empty() && shown.api_key_set);
        assert_eq!(shown.api_key_hint, "cdef");
        assert_eq!(shown.rating, "pg-13");
        assert!(!format!("{long:?}").contains("0123456789abcdef"));
    }

    #[test]
    fn turns_run_out_per_minute() {
        let account = crate::id::new_id();
        for _ in 0..3 {
            take_turn(&account, Some(3)).unwrap();
        }
        assert!(matches!(take_turn(&account, Some(3)), Err(Error::ResourceExhausted(_))));
        // Someone else has their own.
        take_turn(&crate::id::new_id(), Some(3)).unwrap();
        // No cap, no count.
        for _ in 0..100 {
            take_turn(&account, None).unwrap();
        }
    }

    #[test]
    fn cache_keys_tell_questions_apart() {
        let setup = Setup { provider: Kind::Giphy, api_key: "k".into(), ..Default::default() };
        let ask = |query, cursor| Ask { query, cursor, limit: 24 };
        assert_eq!(cache_key(&setup, ask("Cats", 0)), cache_key(&setup, ask("cats", 0)));
        assert_ne!(cache_key(&setup, ask("cats", 0)), cache_key(&setup, ask("cats", 24)));
        assert_ne!(cache_key(&setup, ask("cats", 0)), cache_key(&setup, ask("dogs", 0)));
        let klipy = Setup { provider: Kind::Klipy, ..setup.clone() };
        assert_ne!(cache_key(&setup, ask("cats", 0)), cache_key(&klipy, ask("cats", 0)));
    }

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(clip("  a\n cat\tdancing "), "a cat dancing");
        assert_eq!(clip(&"x".repeat(500)).chars().count(), MAX_TITLE);
    }
}
