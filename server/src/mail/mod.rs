//! Sending email: the codes people sign in with (`api/email.rs`,
//! docs/email-sign-in.md). Each way of sending is a [`Sender`]: SMTP
//! (`smtp.rs`), Amazon SES's v2 API (`ses.rs`), Resend's API (`resend.rs`)
//! and Cloudflare Email Service's API (`cloudflare.rs`); FUWA_MAIL_PROVIDER
//! picks one, and a new provider is one
//! more file and one more arm in [`Provider`]. Everything about it comes from
//! the environment alone, so the credentials never reach node.db, the API,
//! the logs or another part of a split instance.

pub mod cloudflare;
pub mod resend;
pub mod ses;
pub mod smtp;

use std::sync::Arc;

use crate::error::{Error, Result};

/// One email to one person: plain text, with an HTML version of the same.
#[derive(Debug, Clone, PartialEq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// A way of sending email.
#[tonic::async_trait]
pub trait Sender: Send + Sync {
    /// Hands `mail` to the provider, from `from` ("Name <address>" or an
    /// address). An error says what the provider answered, never the
    /// credentials or the recipient.
    async fn send(&self, from: &str, mail: &Mail) -> Result<()>;
}

/// The providers fuwa sends through, each with its settings.
#[derive(Clone, PartialEq)]
pub enum Provider {
    Smtp(smtp::Config),
    Ses(ses::Config),
    Resend(resend::Config),
    Cloudflare(cloudflare::Config),
}

/// How the instance sends email: FUWA_MAIL_PROVIDER, FUWA_MAIL_FROM and the
/// provider's own variables.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// Who mail comes from: `fuwa <no-reply@example.com>` or an address.
    pub from: String,
    pub provider: Provider,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("mail::Config").field("from", &self.from).field("provider", &self.name()).finish()
    }
}

impl Config {
    /// Reads FUWA_MAIL_PROVIDER (smtp, ses, resend or cloudflare; unset or `off` sends
    /// nothing) and what that provider needs. A provider without what it
    /// needs is an error, so a typo doesn't quietly leave sign-ins broken.
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> std::result::Result<Option<Self>, String> {
        let provider = match get("FUWA_MAIL_PROVIDER").as_deref().map(str::trim) {
            None | Some("off" | "") => return Ok(None),
            Some("smtp") => Provider::Smtp(smtp::Config::from_lookup(get)?),
            Some("ses") => Provider::Ses(ses::Config::from_lookup(get)?),
            Some("resend") => Provider::Resend(resend::Config::from_lookup(get)?),
            Some("cloudflare") => Provider::Cloudflare(cloudflare::Config::from_lookup(get)?),
            Some(other) => {
                return Err(format!("FUWA_MAIL_PROVIDER must be smtp, ses, resend, cloudflare or off, got {other:?}"));
            }
        };
        let from = get("FUWA_MAIL_FROM").map(|from| from.trim().to_string()).unwrap_or_default();
        if from.is_empty() {
            return Err("FUWA_MAIL_PROVIDER needs FUWA_MAIL_FROM, the address mail comes from".into());
        }
        if from_address(&from).is_none() {
            return Err(format!("FUWA_MAIL_FROM must be an address or \"Name <address>\", got {from:?}"));
        }
        Ok(Some(Self { from, provider }))
    }

    /// The provider's name, as `InstanceSettings.mail_provider` says it.
    pub fn name(&self) -> &'static str {
        match self.provider {
            Provider::Smtp(_) => "smtp",
            Provider::Ses(_) => "ses",
            Provider::Resend(_) => "resend",
            Provider::Cloudflare(_) => "cloudflare",
        }
    }
}

/// Sends the instance's mail through the provider it was set up with.
#[derive(Clone)]
pub struct Mailer {
    from: String,
    sender: Arc<dyn Sender>,
}

impl Mailer {
    pub fn new(config: &Config) -> Result<Self> {
        let sender: Arc<dyn Sender> = match &config.provider {
            Provider::Smtp(smtp) => Arc::new(smtp::Smtp::new(smtp)?),
            Provider::Ses(ses) => Arc::new(ses::Ses::new(ses.clone())?),
            Provider::Resend(resend) => Arc::new(resend::Resend::new(resend.clone())?),
            Provider::Cloudflare(cloudflare) => Arc::new(cloudflare::Cloudflare::new(cloudflare.clone())?),
        };
        Ok(Self { from: config.from.clone(), sender })
    }

    /// A mailer over any sender, for tests.
    pub fn with_sender(from: &str, sender: Arc<dyn Sender>) -> Self {
        Self { from: from.to_string(), sender }
    }

    pub async fn send(&self, mail: &Mail) -> Result<()> {
        self.sender.send(&self.from, mail).await
    }
}

/// An address as fuwa keeps it: trimmed and lowercase, one `@` between a
/// local part and a domain with a dot, nothing that could break out of a
/// mail header, at most 254 characters.
pub fn normalize_address(address: &str) -> Result<String> {
    let address = address.trim().to_lowercase();
    let bad = || Error::invalid("that doesn't look like an email address");
    if address.len() > 254
        || address.chars().any(|c| c.is_whitespace() || c.is_control() || "<>()[],;:\"\\".contains(c))
    {
        return Err(bad());
    }
    let (local, domain) = address.split_once('@').ok_or_else(bad)?;
    let labels_ok = domain
        .split('.')
        .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-') && !label.contains('@'));
    if local.is_empty() || local.len() > 64 || !domain.contains('.') || !labels_ok {
        return Err(bad());
    }
    Ok(address)
}

/// The address in a From line: `a@b.c` alone, or inside `Name <a@b.c>`.
pub fn from_address(from: &str) -> Option<String> {
    let from = from.trim();
    let address = match (from.rfind('<'), from.ends_with('>')) {
        (Some(open), true) => &from[open + 1..from.len() - 1],
        (None, false) => from,
        _ => return None,
    };
    normalize_address(address).ok()
}

/// The name in a From line, if it has one: `fuwa` in `fuwa <a@b.c>`.
pub fn from_name(from: &str) -> Option<&str> {
    let open = from.rfind('<')?;
    let name = from[..open].trim().trim_matches('"').trim();
    (!name.is_empty()).then_some(name)
}

/// What a provider's error answer says, cut short: its `message` (or
/// `Message`) field when it's JSON, else the start of the body.
fn provider_said(body: &str) -> String {
    let said = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| {
            ["message", "Message"].iter().find_map(|key| json.get(key).and_then(|m| m.as_str()).map(str::to_string))
        })
        .unwrap_or_else(|| body.trim().to_string());
    said.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_kept_lowercase_and_plain() {
        assert_eq!(normalize_address("  Ana@Example.COM ").unwrap(), "ana@example.com");
        assert_eq!(normalize_address("ana+fuwa@mail.example.co").unwrap(), "ana+fuwa@mail.example.co");
        for bad in [
            "ana",
            "@example.com",
            "ana@localhost",
            "ana@example..com",
            "ana@-example.com",
            "ana@@example.com",
            "ana smith@example.com",
            "ana@example.com\r\nBcc: eve@example.com",
            "Ana <ana@example.com>",
            "\"ana\"@example.com",
        ] {
            assert!(normalize_address(bad).is_err(), "{bad:?}");
        }
        assert!(normalize_address(&format!("{}@example.com", "a".repeat(65))).is_err());
    }

    #[test]
    fn from_lines_read_with_or_without_a_name() {
        assert_eq!(from_address("no-reply@fuwa.chat").as_deref(), Some("no-reply@fuwa.chat"));
        assert_eq!(from_address("fuwa <no-reply@fuwa.chat>").as_deref(), Some("no-reply@fuwa.chat"));
        assert_eq!(from_name("\"fuwa chat\" <no-reply@fuwa.chat>"), Some("fuwa chat"));
        assert_eq!(from_name("no-reply@fuwa.chat"), None);
        assert_eq!(from_address("fuwa <no-reply@fuwa.chat"), None);
    }

    #[test]
    fn the_provider_comes_from_the_environment() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
        };
        assert_eq!(Config::from_lookup(&vars(&[])).unwrap(), None);
        assert_eq!(Config::from_lookup(&vars(&[("FUWA_MAIL_PROVIDER", "off")])).unwrap(), None);
        let resend = Config::from_lookup(&vars(&[
            ("FUWA_MAIL_PROVIDER", "resend"),
            ("FUWA_MAIL_FROM", "fuwa <no-reply@fuwa.chat>"),
            ("FUWA_RESEND_API_KEY", "re_123"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(resend.name(), "resend");
        assert!(!format!("{resend:?}").contains("re_123"), "no keys in logs");
        assert!(Config::from_lookup(&vars(&[("FUWA_MAIL_PROVIDER", "resend"), ("FUWA_RESEND_API_KEY", "k")])).is_err());
        assert!(Config::from_lookup(&vars(&[("FUWA_MAIL_PROVIDER", "resend"), ("FUWA_MAIL_FROM", "a@b.c")])).is_err());
        assert!(Config::from_lookup(&vars(&[("FUWA_MAIL_PROVIDER", "carrier-pigeon")])).is_err());
    }

    #[test]
    fn provider_errors_are_short() {
        assert_eq!(provider_said(r#"{"statusCode":422,"message":"Invalid `from` field."}"#), "Invalid `from` field.");
        assert_eq!(provider_said(r#"{"Message":"Email address is not verified."}"#), "Email address is not verified.");
        assert_eq!(provider_said(&"x".repeat(500)).len(), 200);
    }
}
