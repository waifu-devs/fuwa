//! Moderation providers: services outside the instance that read a message and
//! say how likely it is to be each kind fuwa asks about (hate, scams, ...).
//!
//! A provider is anything that implements [`Provider`]; [`KINDS`] lists the
//! ones fuwa knows by name, and [`build`] makes one from how the instance's
//! admins set it up ([`Setup`], the `automod_providers` setting). The rules
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
//! kind and provider id only.

use std::fmt;
use std::sync::LazyLock;
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
}

impl fmt::Debug for Setup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Setup")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .field("api_key", &if self.api_key.is_empty() { "" } else { "<redacted>" })
            .field("model", &self.model)
            .field("account_id", &self.account_id)
            .finish()
    }
}

impl Setup {
    pub fn kind(&self) -> Option<&'static Kind> {
        kind(&self.id)
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
        }
    }

    /// A setup from an admin's request, checked. An empty key keeps
    /// `previous`'s, since keys are never sent out.
    pub fn from_pb(from: &pb::AutoModProviderSettings, previous: Option<&Setup>) -> Result<Self> {
        let kind = kind(from.id.trim()).ok_or_else(|| Error::invalid("that isn't a moderation provider fuwa knows"))?;
        let key = from.api_key.trim();
        if key.len() > MAX_KEY || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(Error::invalid(format!("{}'s API key doesn't look right", kind.name)));
        }
        let api_key = match (key, previous) {
            ("", Some(previous)) => previous.api_key.clone(),
            (key, _) => key.to_string(),
        };
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
        let setup =
            Self { id: kind.id.to_string(), enabled: from.enabled, api_key, model: model.to_string(), account_id };
        if setup.enabled {
            setup.ready().map_err(|why| Error::invalid(format!("{why} before it can be turned on")))?;
        }
        Ok(setup)
    }

    /// What servers see of it.
    pub fn offer(&self) -> Option<pb::AutoModProvider> {
        let kind = self.kind()?;
        Some(pb::AutoModProvider {
            id: kind.id.into(),
            name: kind.name.into(),
            host: kind.host.into(),
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

/// One setup for each provider fuwa knows, in order: the saved ones, and a
/// blank one for each of the rest.
pub fn complete(saved: &[Setup]) -> Vec<Setup> {
    KINDS
        .iter()
        .map(|kind| {
            saved
                .iter()
                .find(|s| s.id == kind.id)
                .cloned()
                .unwrap_or_else(|| Setup { id: kind.id.to_string(), ..Default::default() })
        })
        .collect()
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
    let kind = setup.kind().expect("ready checked the kind");
    let url = match kind.id {
        "typesafe-jev" => format!("https://{}/v1/systemone", kind.host),
        _ => format!("https://{}/client/v4/accounts/{}/ai/run/{}", kind.host, setup.account_id, setup.model()),
    };
    Ok(Box::new(SystemOne { kind, url, key: setup.api_key.clone(), model: setup.model().to_string() }))
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
    let id = setup.kind().map_or("unknown", |k| k.id);
    reports::server_timing(&format!("automod:{id}"), took);
    if let Err(failure) = &answer {
        reports::server_error(failure.kind(), Some(id));
    }
    (answer, took)
}

/// Only fuwa's fixed addresses, https, no redirects (a redirect could send
/// the key or the text somewhere else).
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .user_agent(concat!("fuwa/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("the HTTP client builds")
});

/// The System One decision API, which Jev and Clef both answer: one yes-or-no
/// ("noul") question per label, all in one request.
struct SystemOne {
    kind: &'static Kind,
    url: String,
    key: String,
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
        json!({
            "model": self.model,
            "state": { "chat_message": text },
            "questions": questions,
        })
    }
}

impl Provider for SystemOne {
    fn id(&self) -> &'static str {
        self.kind.id
    }

    fn classify<'a>(&'a self, text: &'a str) -> BoxFuture<'a, std::result::Result<Scores, Failure>> {
        Box::pin(async move {
            let response =
                CLIENT.post(&self.url).bearer_auth(&self.key).json(&self.body(text)).send().await.map_err(|err| {
                    if err.is_timeout() { Failure::TimedOut } else { Failure::Unreachable(short(err)) }
                })?;
            let status = response.status();
            let body: Value = response.json().await.unwrap_or(Value::Null);
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
fn short(err: reqwest::Error) -> String {
    let err = err.without_url();
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
            kind: setup.kind().unwrap(),
            url: String::new(),
            key: setup.api_key.clone(),
            model: setup.model().into(),
        };
        let body = provider.body("hi");
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"], json!({ "chat_message": "hi" }));
        assert_eq!(body["questions"].as_object().unwrap().len(), LABELS.len());
        assert_eq!(body["questions"]["scam"]["type"], "noul");
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
