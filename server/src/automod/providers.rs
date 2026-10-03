//! Moderation providers: services outside the instance that read a message and
//! say how likely it is to be each kind fuwa asks about (hate, scams, ...).
//!
//! A provider is anything that implements [`Provider`]; [`KINDS`] lists the
//! ones fuwa knows by name, and [`build`] makes one from how the instance's
//! admins set it up ([`Setup`], the `automod_providers` setting). Admins can
//! also add their own ("custom-..." ids): any https address that answers the
//! same requests Jev and Clef do (docs/automod.md). The rules
//! a server writes itself (keywords, pings, links) are the built-in provider
//! and need no setup: they're in `automod/mod.rs`.
//!
//! TypeSafe's Jev and Cloudflare's Clef both answer System One requests (a
//! `state` and typed `questions`, a probability back for each), so they share
//! one adapter, [`SystemOne`], and differ only in address, key and model.
//!
//! What goes out: the message's text with mentions, channel links and custom
//! emoji ids taken out ([`outgoing`]), its pictures when the server's rule
//! asks for them and the provider reads pictures ([`Kind::pictures`]), and
//! nothing else: never who wrote it, where, or which server. Only the instance calls providers, never an app.
//! A provider that fails or is slow never stops a message: the caller lets it
//! through its rule, and the failure is counted in the anonymous report by
//! kind and provider id only ("custom" for the admins' own, never its name or
//! address).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::{Error, Result};
use crate::pb::{self, AutoModLevel as Level};
use crate::reports;

/// How long a message waits for its provider before going through unchecked.
pub const TIMEOUT: Duration = Duration::from_secs(3);
/// The same with pictures, which take longer to send and read.
pub const PICTURES_TIMEOUT: Duration = Duration::from_secs(6);
/// The most pictures one request carries, each at most [`MAX_PICTURE_BYTES`]
/// and [`MAX_PICTURE_PIXELS`], all of them at most [`MAX_PICTURES_BYTES`]
/// (Clef's limits).
pub const MAX_PICTURES: usize = 4;
pub const MAX_PICTURE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PICTURE_PIXELS: u64 = 16_000_000;
pub const MAX_PICTURES_BYTES: usize = 8 * 1024 * 1024;
/// The most characters of a message a provider reads.
const MAX_TEXT: usize = 4000;
const MAX_KEY: usize = 512;
/// How many providers of their own admins can add.
pub const MAX_CUSTOM: usize = 8;
/// What an admin's request calls a provider of their own it adds.
pub const NEW_CUSTOM: &str = "custom";
const CUSTOM_PREFIX: &str = "custom-";
/// Headers a key can't go in: the request needs them as they are.
const FIXED_HEADERS: &[&str] =
    &["host", "content-type", "content-length", "transfer-encoding", "connection", "user-agent", "accept"];

/// A kind of provider fuwa knows: where it is and what it needs.
#[derive(Debug)]
pub struct Kind {
    pub id: &'static str,
    pub name: &'static str,
    /// Where checked messages go.
    pub host: &'static str,
    pub models: &'static [&'static str],
    /// Cloudflare's addresses name the account.
    pub needs_account: bool,
    /// It reads pictures as well as text.
    pub pictures: bool,
}

pub const KINDS: &[Kind] = &[
    Kind {
        id: "typesafe-jev",
        name: "TypeSafe Jev",
        host: "api.typesafe.ai",
        models: &["jev-latest", "jev-preview"],
        needs_account: false,
        pictures: false,
    },
    Kind {
        id: "cloudflare-clef",
        name: "Cloudflare Clef",
        host: "api.cloudflare.com",
        models: &["@cf/cloudflare/clef", "@cf/cloudflare/clef-flash"],
        needs_account: true,
        pictures: true,
    },
];

pub fn kind(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

/// One kind of message fuwa asks providers about, as a yes-or-no question.
#[derive(Debug)]
pub struct Label {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    question: &'static str,
    yes: &'static str,
    no: &'static str,
    /// What a new rule does about it.
    pub default_level: Level,
}

pub const LABELS: &[Label] = &[
    Label {
        id: "hate",
        name: "Hate",
        description: "Attacks people for who they are: race, religion, gender, sexuality, disability.",
        question: "Does this chat message attack, demean or dehumanize people for who they are (race, ethnicity, nationality, religion, gender, sexuality, disability or similar)?",
        yes: "It attacks or demeans people for who they are, including slurs aimed at someone.",
        no: "It doesn't. Quoting, reporting on or discussing hate without endorsing it doesn't count.",
        default_level: Level::Block,
    },
    Label {
        id: "harassment",
        name: "Harassment",
        description: "Insults, bullying or pile-ons aimed at someone.",
        question: "Is this chat message insulting, bullying or harassing someone?",
        yes: "It insults, bullies, mocks or harasses a person.",
        no: "It doesn't. Friendly banter, swearing that targets nobody and disagreement don't count.",
        default_level: Level::Flag,
    },
    Label {
        id: "sexual",
        name: "Sexual",
        description: "Explicit sexual content.",
        question: "Is this chat message sexually explicit?",
        yes: "It describes sexual acts or is explicit sexual content.",
        no: "It isn't. Talking about relationships, health or sex education plainly doesn't count.",
        default_level: Level::Flag,
    },
    Label {
        id: "violence",
        name: "Violence and threats",
        description: "Threats to hurt someone, or praise for real violence.",
        question: "Does this chat message threaten to hurt someone, or encourage or praise real violence?",
        yes: "It threatens someone or encourages or glorifies real-world violence.",
        no: "It doesn't. Games, fiction, news and obvious jokes don't count.",
        default_level: Level::Block,
    },
    Label {
        id: "self_harm",
        name: "Self-harm",
        description: "Someone who may hurt themselves, or encouragement to.",
        question: "Does this chat message talk about wanting to hurt or kill oneself, or encourage someone to?",
        yes: "It says the writer may hurt themselves, or pushes someone toward self-harm.",
        no: "It doesn't. Recovery talk and support for others don't count.",
        default_level: Level::Flag,
    },
    Label {
        id: "scam",
        name: "Scams and phishing",
        description: "Fake giveaways, stolen-account bait, links asking for passwords or money.",
        question: "Is this chat message a scam or phishing attempt?",
        yes: "It tries to trick people: fake giveaways or free offers, requests for passwords, codes or money, suspicious links.",
        no: "It isn't.",
        default_level: Level::Block,
    },
    Label {
        id: "spam",
        name: "Spam",
        description: "Unwanted ads, repeated text or flooding.",
        question: "Is this chat message spam?",
        yes: "It's unsolicited advertising, self-promotion out of place, or text repeated to flood the chat.",
        no: "It isn't.",
        default_level: Level::Flag,
    },
];

pub fn label(id: &str) -> Option<&'static Label> {
    LABELS.iter().find(|l| l.id == id)
}

/// How a provider is set up on this instance, as the `automod_providers`
/// setting keeps it: the key included (node.db is sealed under the
/// instance's key when it has one), but never in a client's answer.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setup {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_id: String,
    /// The admins' own providers: its name, address, and the header its key
    /// goes in (empty for `Authorization: Bearer`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header: String,
}

impl fmt::Debug for Setup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Setup")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .field("api_key", &if self.api_key.is_empty() { "" } else { "<redacted>" })
            .field("model", &self.model)
            .field("account_id", &self.account_id)
            .field("name", &self.name)
            .field("url", &self.url)
            .field("header", &self.header)
            .finish()
    }
}

impl Setup {
    /// Whether its provider reads pictures (the admins' own read only text).
    pub fn reads_pictures(&self) -> bool {
        self.kind().is_some_and(|kind| kind.pictures)
    }

    pub fn kind(&self) -> Option<&'static Kind> {
        kind(&self.id)
    }

    /// One of the admins' own.
    pub fn is_custom(&self) -> bool {
        is_custom_id(&self.id)
    }

    /// What servers see it called.
    pub fn name(&self) -> &str {
        self.kind().map_or(&self.name, |kind| kind.name)
    }

    /// Where checked messages go.
    pub fn host(&self) -> String {
        match self.kind() {
            Some(kind) => kind.host.to_string(),
            None => url::Url::parse(&self.url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default(),
        }
    }

    /// Its id in the anonymous report: the admins' own are all "custom".
    pub fn report_id(&self) -> &'static str {
        self.kind().map_or(NEW_CUSTOM, |kind| kind.id)
    }

    /// The model asked: the one chosen, or the provider's first.
    pub fn model(&self) -> &str {
        match (&self.model, self.kind()) {
            (model, _) if !model.is_empty() => model,
            (_, Some(kind)) => kind.models[0],
            _ => "",
        }
    }

    /// Whether it has what it needs to be asked.
    pub fn ready(&self) -> std::result::Result<(), String> {
        if self.is_custom() {
            if self.name.is_empty() {
                return Err("your provider needs a name".into());
            }
            check_url(&self.url)?;
            if !self.header.is_empty() && self.api_key.is_empty() {
                return Err(format!("{} needs the key it sends in {}", self.name, self.header));
            }
            return Ok(());
        }
        let kind = self.kind().ok_or_else(|| format!("fuwa doesn't know a provider called {}", self.id))?;
        if self.api_key.is_empty() {
            return Err(format!("{} needs an API key", kind.name));
        }
        if kind.needs_account && self.account_id.is_empty() {
            return Err(format!("{} needs the Cloudflare account id", kind.name));
        }
        Ok(())
    }

    /// Whether servers can use it now.
    pub fn usable(&self) -> bool {
        self.enabled && self.ready().is_ok()
    }

    /// As clients see it (`with_key` false) or as shards get it.
    pub fn to_pb(&self, with_key: bool) -> pb::AutoModProviderSettings {
        let hint: String = {
            let chars: Vec<char> = self.api_key.chars().collect();
            if chars.len() >= 12 { chars[chars.len() - 4..].iter().collect() } else { String::new() }
        };
        pb::AutoModProviderSettings {
            id: self.id.clone(),
            enabled: self.enabled,
            api_key: if with_key { self.api_key.clone() } else { String::new() },
            api_key_set: !self.api_key.is_empty(),
            api_key_hint: hint,
            model: self.model.clone(),
            account_id: self.account_id.clone(),
            name: self.name.clone(),
            url: self.url.clone(),
            header: self.header.clone(),
        }
    }

    /// As another part of the instance sent it, keys and all.
    pub fn from_cluster(from: &pb::AutoModProviderSettings) -> Self {
        Self {
            id: from.id.clone(),
            enabled: from.enabled,
            api_key: from.api_key.clone(),
            model: from.model.clone(),
            account_id: from.account_id.clone(),
            name: from.name.clone(),
            url: from.url.clone(),
            header: from.header.clone(),
        }
    }

    /// A setup from an admin's request, checked. An empty key keeps
    /// `previous`'s, since keys are never sent out.
    pub fn from_pb(from: &pb::AutoModProviderSettings, previous: Option<&Setup>) -> Result<Self> {
        let id = from.id.trim();
        if id == NEW_CUSTOM || is_custom_id(id) {
            return Self::custom_from_pb(from, previous);
        }
        let kind = kind(id).ok_or_else(|| Error::invalid("that isn't a moderation provider fuwa knows"))?;
        let api_key = checked_key(kind.name, &from.api_key, previous)?;
        if !from.name.trim().is_empty() || !from.url.trim().is_empty() || !from.header.trim().is_empty() {
            return Err(Error::invalid(format!("{}'s name and address are fuwa's", kind.name)));
        }
        let model = from.model.trim();
        if !model.is_empty() && !kind.models.contains(&model) {
            return Err(Error::invalid(format!("{} offers {}", kind.name, kind.models.join(" and "))));
        }
        let account_id = from.account_id.trim().to_ascii_lowercase();
        if !kind.needs_account && !account_id.is_empty() {
            return Err(Error::invalid(format!("{} doesn't take an account id", kind.name)));
        }
        if !account_id.is_empty() && (account_id.len() != 32 || !account_id.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(Error::invalid("a Cloudflare account id is 32 letters and digits (0-9, a-f)"));
        }
        let setup = Self {
            id: kind.id.to_string(),
            enabled: from.enabled,
            api_key,
            model: model.to_string(),
            account_id,
            ..Default::default()
        };
        if setup.enabled {
            setup.ready().map_err(|why| Error::invalid(format!("{why} before it can be turned on")))?;
        }
        Ok(setup)
    }

    /// One of the admins' own, checked; a new one ("custom") gets its id.
    fn custom_from_pb(from: &pb::AutoModProviderSettings, previous: Option<&Setup>) -> Result<Self> {
        let id = from.id.trim();
        let id = if id == NEW_CUSTOM {
            format!("{CUSTOM_PREFIX}{}", crate::id::new_id().to_ascii_lowercase())
        } else {
            id.into()
        };
        let name = from.name.trim();
        if !(1..=40).contains(&name.chars().count()) || name.chars().any(char::is_control) {
            return Err(Error::invalid("name your provider in 1 to 40 characters"));
        }
        let url = from.url.trim();
        check_url(url).map_err(Error::invalid)?;
        let header = from.header.trim().to_ascii_lowercase();
        if !header.is_empty()
            && (header.len() > 64
                || reqwest::header::HeaderName::from_bytes(header.as_bytes()).is_err()
                || FIXED_HEADERS.contains(&header.as_str()))
        {
            return Err(Error::invalid("the key's header is a name like x-api-key"));
        }
        let header = if header == "authorization" { String::new() } else { header };
        let model = from.model.trim();
        if model.chars().count() > 100 || model.chars().any(char::is_control) {
            return Err(Error::invalid("a model is at most 100 characters"));
        }
        if !from.account_id.trim().is_empty() {
            return Err(Error::invalid(format!("{name} doesn't take an account id")));
        }
        // A key saved for one address never goes to another unasked.
        let previous = previous.filter(|p| p.url == url);
        let setup = Self {
            id,
            enabled: from.enabled,
            api_key: checked_key(name, &from.api_key, previous)?,
            model: model.to_string(),
            account_id: String::new(),
            name: name.to_string(),
            url: url.to_string(),
            header,
        };
        if setup.enabled {
            setup.ready().map_err(|why| Error::invalid(format!("{why} before it can be turned on")))?;
        }
        Ok(setup)
    }

    /// What servers see of it.
    pub fn offer(&self) -> Option<pb::AutoModProvider> {
        if self.kind().is_none() && !self.is_custom() {
            return None;
        }
        Some(pb::AutoModProvider {
            id: self.id.clone(),
            name: self.name().into(),
            host: self.host(),
            pictures: self.reads_pictures(),
            labels: LABELS
                .iter()
                .map(|l| pb::AutoModLabel {
                    id: l.id.into(),
                    name: l.name.into(),
                    description: l.description.into(),
                    default_level: l.default_level as i32,
                })
                .collect(),
        })
    }
}

/// One setup for each provider fuwa knows, in order (the saved ones, and a
/// blank one for each of the rest), then the admins' own.
pub fn complete(saved: &[Setup]) -> Vec<Setup> {
    let known = KINDS.iter().map(|kind| {
        saved
            .iter()
            .find(|s| s.id == kind.id)
            .cloned()
            .unwrap_or_else(|| Setup { id: kind.id.to_string(), ..Default::default() })
    });
    known.chain(saved.iter().filter(|s| s.is_custom()).cloned()).collect()
}

/// An id of one of the admins' own providers.
pub fn is_custom_id(id: &str) -> bool {
    id.strip_prefix(CUSTOM_PREFIX).is_some_and(|rest| {
        (1..=40).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// A key from an admin's request: an empty one keeps `previous`'s.
fn checked_key(name: &str, key: &str, previous: Option<&Setup>) -> Result<String> {
    let key = key.trim();
    if key.len() > MAX_KEY || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::invalid(format!("{name}'s API key doesn't look right")));
    }
    Ok(match (key, previous) {
        ("", Some(previous)) => previous.api_key.clone(),
        (key, _) => key.to_string(),
    })
}

/// An admin's own provider's address: https, a host, nothing else odd.
fn check_url(url: &str) -> std::result::Result<(), String> {
    let bad = || "your provider's address is an https URL, like https://moderation.example.com/v1/check".to_string();
    if url.len() > 512 {
        return Err(bad());
    }
    let parsed = url::Url::parse(url).map_err(|_| bad())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(bad());
    }
    let internal = match parsed.host() {
        Some(url::Host::Ipv4(ip)) => private_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => private_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || [".localhost", ".internal", ".local"].iter().any(|end| name.ends_with(end))
        }
        None => true,
    };
    if internal && !*ALLOW_PRIVATE {
        return Err(PRIVATE.into());
    }
    Ok(())
}

/// Why a provider of the admins' own at an internal address is refused.
const PRIVATE: &str = "your provider has to be on the internet, not a private or internal address \
     (whoever runs the instance can allow those with FUWA_AUTOMOD_ALLOW_PRIVATE=1)";

/// Whoever runs the instance lets the admins' own providers be on its own
/// network (`FUWA_AUTOMOD_ALLOW_PRIVATE=1`). Off, the instance never calls a
/// private, loopback or link-local address for a provider, so a provider's
/// address can't be used to reach the instance's own network.
static ALLOW_PRIVATE: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("FUWA_AUTOMOD_ALLOW_PRIVATE").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "on" | "yes"))
});

/// An address that isn't on the public internet.
fn private_ip(ip: IpAddr) -> bool {
    fn v4(ip: Ipv4Addr) -> bool {
        let [a, b, ..] = ip.octets();
        ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_multicast()
            || a == 0
            || a >= 240
            || (a == 100 && b & 0xc0 == 64)
            || (a == 192 && b == 0 && ip.octets()[2] == 0)
            || (a == 198 && b & 0xfe == 18)
    }
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return v4(mapped);
            }
            // NAT64 (64:ff9b::/96) and the old IPv4-compatible ::a.b.c.d
            // carry an IPv4 address too.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] || (s[..6] == [0; 6] && !ip.is_loopback() && !ip.is_unspecified()) {
                let [_, _, _, _, _, _, _, _, _, _, _, _, a, b, c, d] = ip.octets();
                return v4(Ipv4Addr::new(a, b, c, d));
            }
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || s[0] & 0xfe00 == 0xfc00
                || s[0] & 0xffc0 == 0xfe80
        }
    }
}

/// Looks names up and drops internal addresses, so a public name that
/// points (or later re-points) inside the instance's network isn't called.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = found.filter(|a| *ALLOW_PRIVATE || !private_ip(a.ip())).collect();
            if public.is_empty() {
                return Err(PRIVATE.into());
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// How likely a message is to be each label, 0 to 1, in [`LABELS`] order.
pub type Scores = Vec<(&'static str, f32)>;

/// Why a provider didn't answer.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Failure {
    #[error("{0}")]
    NotSetUp(String),
    #[error("it didn't answer within {} seconds", TIMEOUT.as_secs())]
    TimedOut,
    #[error("it turned the key down ({0})")]
    Unauthorized(String),
    #[error("it's limiting how often this instance asks")]
    RateLimited,
    #[error("it couldn't be reached: {0}")]
    Unreachable(String),
    #[error("it answered {0}")]
    Status(String),
    #[error("its answer didn't read: {0}")]
    BadAnswer(String),
}

impl Failure {
    /// The kind, for the anonymous report.
    pub fn kind(&self) -> &'static str {
        match self {
            Failure::NotSetUp(_) => "automod_provider_not_set_up",
            Failure::TimedOut => "automod_provider_timeout",
            Failure::Unauthorized(_) => "automod_provider_unauthorized",
            Failure::RateLimited => "automod_provider_rate_limited",
            Failure::Unreachable(_) => "automod_provider_unreachable",
            Failure::Status(_) => "automod_provider_status",
            Failure::BadAnswer(_) => "automod_provider_bad_answer",
        }
    }
}

/// A picture in a message, as the server read it: PNG, JPEG or WebP within
/// Clef's limits (see [`Picture::read`]).
#[derive(Clone)]
pub struct Picture {
    pub content_type: &'static str,
    pub bytes: bytes::Bytes,
}

impl Picture {
    /// The picture in `bytes`, when it's a PNG, JPEG or WebP of at most
    /// [`MAX_PICTURE_BYTES`] and [`MAX_PICTURE_PIXELS`]; anything else is
    /// left out (a GIF, a huge photo).
    pub fn read(bytes: bytes::Bytes) -> Option<Self> {
        if bytes.len() > MAX_PICTURE_BYTES {
            return None;
        }
        let content_type = crate::media::sniff(&bytes[..bytes.len().min(16)])?;
        if !matches!(content_type, "image/png" | "image/jpeg" | "image/webp") {
            return None;
        }
        let (width, height) = dimensions(&bytes)?;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PICTURE_PIXELS {
            return None;
        }
        Some(Self { content_type, bytes })
    }

    fn data_uri(&self) -> String {
        use base64::Engine;
        format!("data:{};base64,{}", self.content_type, base64::engine::general_purpose::STANDARD.encode(&self.bytes))
    }
}

/// A PNG's, JPEG's or WebP's width and height, from its headers.
fn dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    let le16 = |i: usize| Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    let le24 = |i: usize| Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, 0]));
    let be32 = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?));
    if b.starts_with(b"\x89PNG") {
        // IHDR is always first.
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"\xff\xd8") {
        // Walk the segments to the frame header.
        let mut i = 2;
        while i + 4 <= b.len() {
            if b[i] != 0xff {
                return None;
            }
            let marker = b[i + 1];
            if marker == 0xff {
                i += 1;
                continue;
            }
            if matches!(marker, 0xd8 | 0x01 | 0xd0..=0xd7) {
                i += 2;
                continue;
            }
            let len = be16(i + 2)? as usize;
            if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + len;
        }
        return None;
    }
    if b.get(..4) == Some(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        };
    }
    None
}

/// What a provider reads about a message: its text, already cleaned by
/// [`outgoing`], and its pictures when the rule shows them (only to
/// providers that read pictures).
pub struct Message<'a> {
    pub text: &'a str,
    pub pictures: &'a [Picture],
}

/// A moderation provider: reads a message and says how likely it is to be
/// each of [`LABELS`].
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn classify<'a>(&'a self, message: &'a Message<'a>) -> BoxFuture<'a, std::result::Result<Scores, Failure>>;
}

/// A provider for `setup`, when it has what it needs.
pub fn build(setup: &Setup) -> std::result::Result<Box<dyn Provider>, Failure> {
    setup.ready().map_err(Failure::NotSetUp)?;
    Ok(Box::new(system_one(setup)))
}

/// Where `setup`'s requests go and the model they name.
fn system_one(setup: &Setup) -> SystemOne {
    let (url, model) = match setup.kind().map(|kind| (kind.id, kind.host)) {
        Some(("typesafe-jev", host)) => (format!("https://{host}/v1/systemone"), setup.model()),
        // Cloudflare runs the model the address names (`@cf/cloudflare/clef`),
        // and its body wants the bare name ("clef"): anything else is a 400.
        // The account id in the address is what lets account-owned tokens in,
        // the same as user tokens.
        Some((_, host)) => (
            format!("https://{host}/client/v4/accounts/{}/ai/run/{}", setup.account_id, setup.model()),
            setup.model().rsplit('/').next().unwrap_or_default(),
        ),
        None => (setup.url.clone(), setup.model()),
    };
    SystemOne {
        id: setup.report_id(),
        url,
        key: setup.api_key.clone(),
        header: setup.header.clone(),
        model: model.to_string(),
    }
}

/// Asks `setup`'s provider about `text` and `pictures` (left out unless it
/// reads pictures), giving up after [`TIMEOUT`] ([`PICTURES_TIMEOUT`] with
/// pictures). Counts how long it took, and any failure (by kind and provider
/// id only), in the anonymous report.
pub async fn check(
    setup: &Setup,
    text: &str,
    pictures: &[Picture],
) -> (std::result::Result<Scores, Failure>, Duration) {
    let started = Instant::now();
    let text = outgoing(text);
    let pictures = if setup.reads_pictures() { &pictures[..pictures.len().min(MAX_PICTURES)] } else { &[] };
    let message = Message { text: &text, pictures };
    let timeout = if pictures.is_empty() { TIMEOUT } else { PICTURES_TIMEOUT };
    let answer = match build(setup) {
        Ok(provider) => {
            tokio::time::timeout(timeout, provider.classify(&message)).await.unwrap_or(Err(Failure::TimedOut))
        }
        Err(failure) => Err(failure),
    };
    let took = started.elapsed();
    let id = setup.report_id();
    reports::server_timing(&format!("automod:{id}"), took);
    if let Err(failure) = &answer {
        reports::server_error(failure.kind(), Some(id));
    }
    (answer, took)
}

/// https only, no redirects (a redirect could send the key or the text
/// somewhere the admins didn't pick).
/// Names are looked up by [`PublicOnly`].
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(PublicOnly))
        // A proxy would look names up itself, past PublicOnly.
        .no_proxy()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PICTURES_TIMEOUT)
        .user_agent(concat!("fuwa/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("the HTTP client builds")
});

/// The System One decision API, which Jev and Clef both answer, and the
/// admins' own providers too: one yes-or-no ("noul") question per label, all
/// in one request.
struct SystemOne {
    id: &'static str,
    url: String,
    key: String,
    /// Where the key goes: empty for `Authorization: Bearer`.
    header: String,
    /// Left out when empty.
    model: String,
}

impl SystemOne {
    fn body(&self, message: &Message) -> Value {
        let questions: Map<String, Value> = LABELS
            .iter()
            .map(|l| {
                let instructions = if message.pictures.is_empty() {
                    l.question.to_string()
                } else {
                    format!("{} Count the pictures sent with it as part of the message.", l.question)
                };
                let question = json!({
                    "type": "noul",
                    "instructions": instructions,
                    "criteria": { "true": l.yes, "false": l.no },
                });
                (l.id.to_string(), question)
            })
            .collect();
        let mut body = json!({
            "state": { "chat_message": message.text },
            "questions": questions,
        });
        if !message.pictures.is_empty() {
            body["images"] = message.pictures.iter().map(Picture::data_uri).collect();
        }
        if !self.model.is_empty() {
            body["model"] = Value::from(self.model.as_str());
        }
        body
    }
}

impl Provider for SystemOne {
    fn id(&self) -> &'static str {
        self.id
    }

    fn classify<'a>(&'a self, message: &'a Message<'a>) -> BoxFuture<'a, std::result::Result<Scores, Failure>> {
        Box::pin(async move {
            let request = CLIENT.post(&self.url).json(&self.body(message));
            let request = match (self.key.is_empty(), self.header.is_empty()) {
                (true, _) => request,
                (false, true) => request.bearer_auth(&self.key),
                (false, false) => request.header(self.header.as_str(), &self.key),
            };
            let response = request
                .send()
                .await
                .map_err(|err| if err.is_timeout() { Failure::TimedOut } else { Failure::Unreachable(short(err)) })?;
            let status = response.status();
            let body = read_body(response).await;
            match status.as_u16() {
                200..=299 => read_answers(&body),
                401 | 403 => Err(Failure::Unauthorized(error_message(&body).unwrap_or_else(|| status.to_string()))),
                429 => Err(Failure::RateLimited),
                _ => Err(Failure::Status(match error_message(&body) {
                    Some(message) => format!("{status}: {message}"),
                    None => status.to_string(),
                })),
            }
        })
    }
}

/// The most of an answer fuwa reads.
const MAX_ANSWER: usize = 64 * 1024;

/// An answer's JSON, or null when it's longer than [`MAX_ANSWER`] or isn't JSON.
async fn read_body(mut response: reqwest::Response) -> Value {
    let mut bytes = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if bytes.len() + chunk.len() > MAX_ANSWER {
            return Value::Null;
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// An error without the address it was for.
fn short(err: reqwest::Error) -> String {
    let err = err.without_url();
    let mut source: Option<&dyn std::error::Error> = Some(&err);
    while let Some(err) = source {
        if err.to_string() == PRIVATE {
            return PRIVATE.into();
        }
        source = err.source();
    }
    let text = if err.is_connect() { "couldn't connect".to_string() } else { err.to_string() };
    text.chars().take(160).collect()
}

/// The message in an error answer: System One's `error.message`, or
/// Cloudflare's `errors[0].message`, cut short.
fn error_message(body: &Value) -> Option<String> {
    let message =
        body.pointer("/error/message").or_else(|| body.pointer("/errors/0/message")).and_then(Value::as_str)?;
    Some(message.chars().filter(|c| !c.is_control()).take(160).collect())
}

/// Each label's probability from a System One answer (Cloudflare wraps it in
/// `result`).
fn read_answers(body: &Value) -> std::result::Result<Scores, Failure> {
    let body = body.get("result").filter(|r| r.is_object()).unwrap_or(body);
    let answers =
        body.get("answers").and_then(Value::as_object).ok_or_else(|| Failure::BadAnswer("no answers".into()))?;
    LABELS
        .iter()
        .map(|l| {
            let answer = answers.get(l.id).ok_or_else(|| Failure::BadAnswer(format!("no answer for {}", l.id)))?;
            let p = answer
                .get("noul")
                .or_else(|| answer.get("probability"))
                .or_else(|| answer.pointer("/probabilities/true"))
                .and_then(Value::as_f64)
                .filter(|p| (0.0..=1.0).contains(p))
                .ok_or_else(|| Failure::BadAnswer(format!("no probability for {}", l.id)))?;
            Ok((l.id, p as f32))
        })
        .collect()
}

/// A message's text as it goes to a provider: people, roles, channels and
/// custom emoji written by id or name become placeholders, so no ids or names
/// leave the instance, and only the first [`MAX_TEXT`] characters go.
pub fn outgoing(content: &str) -> String {
    let mut out = String::with_capacity(content.len().min(MAX_TEXT));
    let mut rest = content;
    while let Some(c) = rest.chars().next() {
        if let Some((replacement, len)) = placeholder(rest, out.chars().next_back()) {
            out.push_str(replacement);
            rest = &rest[len..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out.chars().take(MAX_TEXT).collect()
}

/// The placeholder for what `text` starts with, and how many bytes it takes.
fn placeholder(text: &str, before: Option<char>) -> Option<(&'static str, usize)> {
    let name_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'.';
    if let Some(inner) = text.strip_prefix('<') {
        let end = inner.find('>').filter(|&end| end <= 100)?;
        let tag = &inner[..end];
        let id_like = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric());
        let replacement = if let Some(id) = tag.strip_prefix("@&") {
            id_like(id).then_some("@role")
        } else if let Some(id) = tag.strip_prefix('@') {
            id_like(id.trim_start_matches('!')).then_some("@someone")
        } else if let Some(id) = tag.strip_prefix('#') {
            id_like(id).then_some("#channel")
        } else if tag.starts_with(':') || tag.starts_with("a:") {
            let mut parts = tag.trim_start_matches('a').splitn(3, ':');
            (parts.next() == Some("") && parts.next().is_some() && parts.next().is_some_and(id_like))
                .then_some(":emoji:")
        } else {
            None
        }?;
        return Some((replacement, end + 2));
    }
    if let Some(name) = text.strip_prefix('@') {
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            return None;
        }
        let len = name.bytes().take_while(|&b| name_byte(b)).count();
        let word = name[..len].trim_end_matches('.');
        if word.is_empty() || word.eq_ignore_ascii_case("everyone") || word.eq_ignore_ascii_case("here") {
            return None;
        }
        return Some(("@someone", 1 + word.len()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_that_names_anyone_goes_out() {
        assert_eq!(
            outgoing("hey @mika and <@01J0ABC> <@!01J0ABC> in <#01J0CHAN>, <@&01J0ROLE> <:pog:01J0EMO> <a:spin:01J0X>"),
            "hey @someone and @someone @someone in #channel, @role :emoji: :emoji:"
        );
        assert_eq!(outgoing("@everyone mail me at a@b.com. @mika."), "@everyone mail me at a@b.com. @someone.");
        assert_eq!(outgoing("1 < 2 > 0 <not a tag>"), "1 < 2 > 0 <not a tag>");
        assert_eq!(outgoing(&"é".repeat(5000)).chars().count(), MAX_TEXT);
    }

    #[test]
    fn clef_gets_its_model_in_the_address_and_bare_in_the_body() {
        let account = "0123456789abcdef0123456789abcdef";
        for (chosen, path, bare) in [
            ("", "@cf/cloudflare/clef", "clef"),
            ("@cf/cloudflare/clef", "@cf/cloudflare/clef", "clef"),
            ("@cf/cloudflare/clef-flash", "@cf/cloudflare/clef-flash", "clef-flash"),
        ] {
            let setup = Setup {
                id: "cloudflare-clef".into(),
                enabled: true,
                api_key: "k".repeat(40),
                account_id: account.into(),
                model: chosen.into(),
                ..Default::default()
            };
            let provider = system_one(&setup);
            assert_eq!(provider.url, format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/{path}"));
            assert_eq!(provider.header, "", "the token goes as Authorization: Bearer");
            let body = provider.body(&Message { text: "hi", pictures: &[] });
            assert_eq!(body["model"], bare);
        }
        let jev = system_one(&Setup { id: "typesafe-jev".into(), api_key: "k".repeat(20), ..Default::default() });
        assert_eq!(jev.url, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(jev.model, "jev-latest");
    }

    #[test]
    fn reads_system_one_answers_and_cloudflare_s_wrapper() {
        let answers: Map<String, Value> = LABELS
            .iter()
            .enumerate()
            .map(|(i, l)| (l.id.to_string(), json!({ "type": "noul", "noul": i as f64 / 10.0 })))
            .collect();
        let plain = json!({ "model": "jev-1.13", "answers": answers });
        let scores = read_answers(&plain).unwrap();
        assert_eq!(scores.len(), LABELS.len());
        assert_eq!(scores[1], ("harassment", 0.1));
        let wrapped = json!({ "success": true, "result": plain });
        assert_eq!(read_answers(&wrapped).unwrap(), scores);
        let mut missing = plain.clone();
        missing["answers"].as_object_mut().unwrap().remove("spam");
        assert!(matches!(read_answers(&missing), Err(Failure::BadAnswer(_))));
        let mut wild = plain.clone();
        wild["answers"]["hate"] = json!({ "noul": 3.0 });
        assert!(read_answers(&wild).is_err());
    }

    #[test]
    fn asks_one_question_per_label_about_the_text_alone() {
        let setup = Setup { id: "typesafe-jev".into(), enabled: true, api_key: "k".repeat(20), ..Default::default() };
        let provider = SystemOne {
            id: setup.report_id(),
            url: String::new(),
            key: setup.api_key.clone(),
            header: String::new(),
            model: setup.model().into(),
        };
        let body = provider.body(&Message { text: "hi", pictures: &[] });
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"], json!({ "chat_message": "hi" }));
        assert_eq!(body["questions"].as_object().unwrap().len(), LABELS.len());
        assert_eq!(body["questions"]["scam"]["type"], "noul");
        let unnamed = SystemOne { model: String::new(), ..provider };
        assert!(unnamed.body(&Message { text: "hi", pictures: &[] }).get("model").is_none());
    }

    #[test]
    fn admins_add_their_own_at_https_addresses() {
        let from = |url: &str| pb::AutoModProviderSettings {
            id: NEW_CUSTOM.into(),
            enabled: true,
            name: "Our classifier".into(),
            url: url.into(),
            api_key: "secret-key-123456".into(),
            header: "X-Api-Key".into(),
            ..Default::default()
        };
        let made = Setup::from_pb(&from("https://mod.example.com/v1/check"), None).unwrap();
        assert!(made.is_custom() && is_custom_id(&made.id), "{}", made.id);
        assert_eq!(
            (made.name(), made.host().as_str(), made.header.as_str()),
            ("Our classifier", "mod.example.com", "x-api-key")
        );
        assert_eq!(made.report_id(), "custom");
        assert!(build(&made).is_ok());
        let offered = made.offer().unwrap();
        assert_eq!((offered.id.as_str(), offered.host.as_str()), (made.id.as_str(), "mod.example.com"));
        assert_eq!(offered.labels.len(), LABELS.len());
        // Saving it again keeps its id and key, unless the address moves.
        let shown = made.to_pb(false);
        let again = Setup::from_pb(&shown, Some(&made)).unwrap();
        assert_eq!((again.id.as_str(), again.api_key.as_str()), (made.id.as_str(), made.api_key.as_str()));
        let moved = pb::AutoModProviderSettings { url: "https://elsewhere.example.com/".into(), ..shown.clone() };
        assert!(Setup::from_pb(&moved, Some(&made)).is_err(), "a key never follows a new address unasked");
        let moved = pb::AutoModProviderSettings { enabled: false, ..moved };
        assert!(Setup::from_pb(&moved, Some(&made)).unwrap().api_key.is_empty());
        for url in [
            "http://mod.example.com/",
            "https://user:pw@mod.example.com/",
            "https://mod.example.com/#x",
            "nope",
            "file:///etc/passwd",
        ] {
            assert!(Setup::from_pb(&from(url), None).is_err(), "{url}");
        }
        for header in ["Host", "content-type", "bad header"] {
            let given = pb::AutoModProviderSettings { header: header.into(), ..from("https://a.example/") };
            assert!(Setup::from_pb(&given, None).is_err(), "{header}");
        }
        let bearer = pb::AutoModProviderSettings { header: "Authorization".into(), ..from("https://a.example/") };
        assert!(Setup::from_pb(&bearer, None).unwrap().header.is_empty());
        let keyless =
            pb::AutoModProviderSettings { api_key: String::new(), header: String::new(), ..from("https://a.example/") };
        assert!(Setup::from_pb(&keyless, None).unwrap().usable(), "a provider of your own may need no key");
        let nameless = pb::AutoModProviderSettings { name: " ".into(), ..from("https://a.example/") };
        assert!(Setup::from_pb(&nameless, None).is_err());
        let ids: Vec<String> = complete(std::slice::from_ref(&made)).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["typesafe-jev", "cloudflare-clef", made.id.as_str()]);
        assert!(!is_custom_id("custom-") && !is_custom_id("custom-../x") && !is_custom_id("typesafe-jev"));
    }

    /// A PNG's first bytes for a `width` by `height` picture.
    fn png(width: u32, height: u32) -> bytes::Bytes {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b.into()
    }

    #[test]
    fn pictures_are_read_within_clef_s_limits() {
        assert_eq!(Picture::read(png(1024, 768)).unwrap().content_type, "image/png");
        // 16 megapixels at most, and nothing empty.
        assert!(Picture::read(png(4000, 4000)).is_some());
        assert!(Picture::read(png(6000, 4000)).is_none());
        assert!(Picture::read(png(0, 10)).is_none());
        // JPEG: an APP0 segment, then the frame header.
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 17, 8];
        jpeg.extend_from_slice(&480u16.to_be_bytes());
        jpeg.extend_from_slice(&640u16.to_be_bytes());
        assert_eq!(dimensions(&jpeg), Some((640, 480)));
        assert_eq!(Picture::read(jpeg.into()).unwrap().content_type, "image/jpeg");
        // WebP, extended: sizes minus one, 24 bits each.
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0xff, 0x07, 0, 0x37, 0x04, 0]);
        assert_eq!(dimensions(&webp), Some((2048, 1080)));
        // Clef doesn't read GIFs, and nothing goes over 4 MiB.
        assert!(Picture::read(bytes::Bytes::from_static(b"GIF89a\x10\0\x10\0")).is_none());
        let mut big = png(100, 100).to_vec();
        big.resize(MAX_PICTURE_BYTES + 1, 0);
        assert!(Picture::read(big.into()).is_none());
        assert!(Picture::read(bytes::Bytes::from_static(b"\x89PNG\r\n")).is_none());
    }

    #[test]
    fn pictures_go_as_data_uris_with_the_text() {
        let provider = SystemOne {
            id: "cloudflare-clef",
            url: String::new(),
            key: String::new(),
            header: String::new(),
            model: String::new(),
        };
        let pictures = [Picture::read(png(2, 2)).unwrap()];
        let body = provider.body(&Message { text: "look", pictures: &pictures });
        let images = body["images"].as_array().unwrap();
        assert_eq!(images.len(), 1);
        assert!(images[0].as_str().unwrap().starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert!(body["questions"]["scam"]["instructions"].as_str().unwrap().contains("pictures"));
        let text_only = provider.body(&Message { text: "look", pictures: &[] });
        assert!(text_only.get("images").is_none());
        assert!(!text_only["questions"]["scam"]["instructions"].as_str().unwrap().contains("pictures"));
        assert!(kind("cloudflare-clef").unwrap().pictures && !kind("typesafe-jev").unwrap().pictures);
    }

    #[test]
    fn own_providers_stay_off_the_instance_s_network() {
        for url in [
            "https://127.0.0.1:8443/",
            "https://10.0.0.5/",
            "https://192.168.1.2/",
            "https://169.254.169.254/latest",
            "https://100.64.0.1/",
            "https://0.0.0.0/",
            "https://2130706433/",
            "https://[::1]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://[::ffff:10.0.0.1]/",
            "https://[64:ff9b::a00:1]/",
            "https://[::7f00:1]/",
            "https://192.0.0.8/",
            "https://198.18.0.1/",
            "https://240.0.0.1/",
            "https://localhost/",
            "https://LOCALHOST./",
            "https://api.localhost/",
            "https://metadata.google.internal/",
            "https://printer.local/",
        ] {
            assert_eq!(check_url(url), Err(PRIVATE.to_string()), "{url}");
        }
        for url in ["https://moderation.example.com/v1", "https://1.1.1.1/", "https://[2606:4700::1111]/"] {
            assert!(check_url(url).is_ok(), "{url}");
        }
    }

    #[tokio::test]
    async fn names_that_point_inside_are_dropped_when_called() {
        use reqwest::dns::Resolve;
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        let refused = PublicOnly.resolve(name).await.err().expect("localhost resolves to loopback only");
        assert_eq!(refused.to_string(), PRIVATE);
    }

    #[test]
    fn setups_keep_keys_to_themselves() {
        let from = |key: &str, enabled: bool| pb::AutoModProviderSettings {
            id: "cloudflare-clef".into(),
            enabled,
            api_key: key.into(),
            account_id: "0123456789ABCDEF0123456789abcdef".into(),
            ..Default::default()
        };
        assert!(Setup::from_pb(&from("", true), None).is_err());
        let first = Setup::from_pb(&from("cf-token-1234567890", true), None).unwrap();
        assert_eq!(first.account_id, "0123456789abcdef0123456789abcdef");
        let shown = first.to_pb(false);
        assert!(shown.api_key.is_empty() && shown.api_key_set);
        assert_eq!(shown.api_key_hint, "7890");
        assert!(!format!("{first:?}").contains("cf-token"));
        let again = Setup::from_pb(&shown, Some(&first)).unwrap();
        assert_eq!(again.api_key, first.api_key);
        assert!(
            Setup::from_pb(&pb::AutoModProviderSettings { model: "gpt".into(), ..from("x", false) }, None).is_err()
        );
        assert!(
            Setup::from_pb(&pb::AutoModProviderSettings { account_id: "nope".into(), ..from("x", false) }, None)
                .is_err()
        );
        assert!(Setup::from_pb(&pb::AutoModProviderSettings { id: "other".into(), ..from("x", false) }, None).is_err());
        let ids: Vec<String> = complete(&[first]).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["typesafe-jev", "cloudflare-clef"]);
    }

    #[test]
    fn addresses_are_fixed_per_provider() {
        let cf = Setup {
            id: "cloudflare-clef".into(),
            api_key: "t".into(),
            account_id: "a".repeat(32),
            model: "@cf/cloudflare/clef-flash".into(),
            ..Default::default()
        };
        assert!(build(&cf).is_ok());
        let jev = Setup { id: "typesafe-jev".into(), api_key: "t".into(), ..Default::default() };
        assert!(build(&jev).is_ok());
        assert!(matches!(build(&Setup { api_key: String::new(), ..jev }), Err(Failure::NotSetUp(_))));
    }
}
