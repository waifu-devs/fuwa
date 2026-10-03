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
//! emoji ids taken out ([`outgoing`]), and nothing else: never who wrote it,
//! where, or which server. Only the instance calls providers, never an app.
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
}

pub const KINDS: &[Kind] = &[
    Kind {
        id: "typesafe-jev",
        name: "TypeSafe Jev",
        host: "api.typesafe.ai",
        models: &["jev-latest", "jev-preview"],
        needs_account: false,
    },
    Kind {
        id: "cloudflare-clef",
        name: "Cloudflare Clef",
        host: "api.cloudflare.com",
        models: &["@cf/cloudflare/clef", "@cf/cloudflare/clef-flash"],
        needs_account: true,
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
            || (a == 100 && b & 0xc0 == 64)
    }
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return v4(mapped);
            }
            // NAT64 (64:ff9b::/96) carries an IPv4 address too.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
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

/// A moderation provider: reads a message's text (already cleaned by
/// [`outgoing`]) and says how likely it is to be each of [`LABELS`].
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn classify<'a>(&'a self, text: &'a str) -> BoxFuture<'a, std::result::Result<Scores, Failure>>;
}

/// A provider for `setup`, when it has what it needs.
pub fn build(setup: &Setup) -> std::result::Result<Box<dyn Provider>, Failure> {
    setup.ready().map_err(Failure::NotSetUp)?;
    let url = match setup.kind().map(|kind| (kind.id, kind.host)) {
        Some(("typesafe-jev", host)) => format!("https://{host}/v1/systemone"),
        Some((_, host)) => format!("https://{host}/client/v4/accounts/{}/ai/run/{}", setup.account_id, setup.model()),
        None => setup.url.clone(),
    };
    Ok(Box::new(SystemOne {
        id: setup.report_id(),
        url,
        key: setup.api_key.clone(),
        header: setup.header.clone(),
        model: setup.model().to_string(),
    }))
}

/// Asks `setup`'s provider about `text`, giving up after [`TIMEOUT`]. Counts
/// how long it took, and any failure (by kind and provider id only), in the
/// anonymous report.
pub async fn check(setup: &Setup, text: &str) -> (std::result::Result<Scores, Failure>, Duration) {
    let started = Instant::now();
    let text = outgoing(text);
    let answer = match build(setup) {
        Ok(provider) => tokio::time::timeout(TIMEOUT, provider.classify(&text)).await.unwrap_or(Err(Failure::TimedOut)),
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
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
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
    fn body(&self, text: &str) -> Value {
        let questions: Map<String, Value> = LABELS
            .iter()
            .map(|l| {
                let question = json!({
                    "type": "noul",
                    "instructions": l.question,
                    "criteria": { "true": l.yes, "false": l.no },
                });
                (l.id.to_string(), question)
            })
            .collect();
        let mut body = json!({
            "state": { "chat_message": text },
            "questions": questions,
        });
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

    fn classify<'a>(&'a self, text: &'a str) -> BoxFuture<'a, std::result::Result<Scores, Failure>> {
        Box::pin(async move {
            let request = CLIENT.post(&self.url).json(&self.body(text));
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

/// An error without the address it was for.
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
        let body = provider.body("hi");
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"], json!({ "chat_message": "hi" }));
        assert_eq!(body["questions"].as_object().unwrap().len(), LABELS.len());
        assert_eq!(body["questions"]["scam"]["type"], "noul");
        let unnamed = SystemOne { model: String::new(), ..provider };
        assert!(unnamed.body("hi").get("model").is_none());
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
