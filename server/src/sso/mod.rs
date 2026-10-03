//! Single sign-on: people sign in through an identity provider an
//! organization runs, over OpenID Connect (`oidc`) or SAML 2.0 (`saml`).
//! One provider model serves two places: the instance's own sign-in (set up
//! by its admins in InstanceSettings, kept in node.db, `AuthService`'s
//! Start/Get/FinishSsoSignIn) and joining one community server (set up by
//! its managers, kept in the server's file, `SsoService`). Both are off
//! until someone sets them up.
//!
//! A sign-in goes the same way at both: Start saves a row in `sso_sign_ins`
//! (node.db, or the server's file) and hands back the provider's page; the
//! provider sends the browser back here (`http.rs`: `/sso/instance/...` or
//! `/sso/servers/<id>/...`), which checks who signed in, saves that with a
//! one-time code and sends the browser to the app's `/auth/sso/done` page;
//! Finish takes the code and the secret only the starting app holds.

pub mod http;
pub mod oidc;
pub mod saml;
pub mod ticket;
pub mod xml;

use serde::{Deserialize, Serialize};
use turso::Connection;

use crate::db::query_one;
use crate::error::{Error, Result};
use crate::id::now_ms;
use crate::pb;

/// How long a sign-in may take from start to finish.
pub const TTL_MS: i64 = 10 * 60 * 1000;
/// The app page a finished sign-in lands on, at the instance's public URL
/// and at the app that started it.
pub const DONE: &str = "/auth/sso/done";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    None,
    Oidc,
    Saml,
}

/// An identity provider, as stored: the client secret included (but never in
/// its `Debug`).
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Provider {
    pub protocol: Protocol,
    pub name: String,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
    pub oidc_extra_scopes: String,
    pub saml_entity_id: String,
    pub saml_sso_url: String,
    pub saml_certificates: String,
    pub email_domains: Vec<String>,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("protocol", &self.protocol)
            .field("name", &self.name)
            .field("oidc_issuer", &self.oidc_issuer)
            .field("oidc_client_id", &self.oidc_client_id)
            .field("oidc_client_secret", &if self.oidc_client_secret.is_empty() { "" } else { "<redacted>" })
            .field("oidc_extra_scopes", &self.oidc_extra_scopes)
            .field("saml_entity_id", &self.saml_entity_id)
            .field("saml_sso_url", &self.saml_sso_url)
            .field("saml_certificates", &self.fingerprints())
            .field("email_domains", &self.email_domains)
            .finish()
    }
}

/// An https URL, or http on this machine while testing.
pub fn secure_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else { return false };
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

impl Provider {
    pub fn is_set(&self) -> bool {
        self.protocol != Protocol::None
    }

    /// Which provider this is: sign-ins made through one don't count for
    /// another. The issuer for OIDC, the entity ID for SAML.
    pub fn key(&self) -> String {
        match self.protocol {
            Protocol::None => String::new(),
            Protocol::Oidc => format!("oidc {}", self.oidc_issuer),
            Protocol::Saml => format!("saml {}", self.saml_entity_id),
        }
    }

    /// Everything that decides who the provider vouches for: the key, plus
    /// the client ID, the SAML sign-in URL and the signing certificates. A
    /// change to any of them is a new provider: earlier sign-ins through it
    /// don't count, and sign-ins already under way stop.
    pub fn trust_key(&self) -> String {
        match self.protocol {
            Protocol::None => String::new(),
            Protocol::Oidc => format!("{} client {}", self.key(), self.oidc_client_id),
            Protocol::Saml => {
                format!("{} at {} certs {}", self.key(), self.saml_sso_url, self.fingerprints().join(","))
            }
        }
    }

    /// The SHA-256 fingerprints of the SAML signing certificates, sorted.
    pub fn fingerprints(&self) -> Vec<String> {
        let mut found: Vec<String> = saml::certificates(&self.saml_certificates)
            .unwrap_or_default()
            .iter()
            .map(|der| {
                use sha2::Digest;
                sha2::Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
            })
            .collect();
        found.sort();
        found
    }

    /// The host people's browsers are sent to when they sign in: the SAML
    /// sign-in URL's, or the OIDC issuer's.
    pub fn host(&self) -> String {
        let url = match self.protocol {
            Protocol::None => return String::new(),
            Protocol::Oidc => &self.oidc_issuer,
            Protocol::Saml => &self.saml_sso_url,
        };
        reqwest::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_default()
    }

    /// A provider from a request, checked. An empty client secret keeps
    /// `previous`'s, for the same issuer and client.
    pub fn from_pb(from: &pb::IdentityProvider, previous: &Provider) -> Result<Provider> {
        let protocol = match pb::SsoProtocol::try_from(from.protocol).unwrap_or_default() {
            pb::SsoProtocol::Unspecified => return Ok(Provider::default()),
            pb::SsoProtocol::Oidc => Protocol::Oidc,
            pb::SsoProtocol::Saml => Protocol::Saml,
        };
        let name = from.name.trim();
        if !(1..=40).contains(&name.chars().count()) {
            return Err(Error::invalid("the provider's name must be 1 to 40 characters"));
        }
        let mut provider = Provider { protocol, name: name.to_string(), ..Default::default() };
        match protocol {
            Protocol::Oidc => {
                let oidc = from.oidc.clone().unwrap_or_default();
                let issuer = oidc.issuer.trim().trim_end_matches('/');
                if issuer.len() > 1024 || !secure_url(issuer) || issuer.contains(['?', '#']) {
                    return Err(Error::invalid("the issuer must be an https URL, like https://login.example.com"));
                }
                let client_id = oidc.client_id.trim();
                if client_id.is_empty() || client_id.len() > 255 {
                    return Err(Error::invalid("the client ID must be 1 to 255 characters"));
                }
                let secret = oidc.client_secret.trim();
                if secret.len() > 1024 {
                    return Err(Error::invalid("the client secret can be at most 1024 characters"));
                }
                let secret = if secret.is_empty()
                    && previous.protocol == Protocol::Oidc
                    && previous.oidc_issuer == issuer
                    && previous.oidc_client_id == client_id
                {
                    previous.oidc_client_secret.clone()
                } else {
                    secret.to_string()
                };
                let scopes = oidc.extra_scopes.split_whitespace().collect::<Vec<_>>().join(" ");
                if scopes.len() > 255
                    || !scopes.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                    || scopes.contains(['"', '\\'])
                {
                    return Err(Error::invalid("extra scopes are up to 255 characters of plain scope names"));
                }
                provider.oidc_issuer = issuer.to_string();
                provider.oidc_client_id = client_id.to_string();
                provider.oidc_client_secret = secret;
                provider.oidc_extra_scopes = scopes;
            }
            Protocol::Saml => {
                let saml = from.saml.clone().unwrap_or_default();
                let entity_id = saml.entity_id.trim();
                if entity_id.is_empty() || entity_id.len() > 1024 {
                    return Err(Error::invalid("the provider's entity ID must be 1 to 1024 characters"));
                }
                let sso_url = saml.sso_url.trim();
                if sso_url.len() > 2048 || !secure_url(sso_url) {
                    return Err(Error::invalid("the SAML sign-in URL must be an https URL"));
                }
                saml::certificates(&saml.certificates)?;
                provider.saml_entity_id = entity_id.to_string();
                provider.saml_sso_url = sso_url.to_string();
                provider.saml_certificates = saml.certificates.trim().to_string();
            }
            Protocol::None => unreachable!(),
        }
        let mut domains = Vec::new();
        for domain in &from.email_domains {
            let domain = domain.trim().trim_start_matches('@').to_ascii_lowercase();
            if domain.is_empty() {
                continue;
            }
            let valid = domain.len() <= 253
                && domain.contains('.')
                && domain.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                });
            if !valid {
                return Err(Error::invalid(format!("{domain:?} isn't a domain, like example.com")));
            }
            if !domains.contains(&domain) {
                domains.push(domain);
            }
        }
        if domains.len() > 50 {
            return Err(Error::invalid("list up to 50 email domains"));
        }
        provider.email_domains = domains;
        Ok(provider)
    }

    /// What clients see: never the client secret.
    pub fn to_pb(&self) -> pb::IdentityProvider {
        if !self.is_set() {
            return pb::IdentityProvider::default();
        }
        pb::IdentityProvider {
            protocol: match self.protocol {
                Protocol::None => pb::SsoProtocol::Unspecified,
                Protocol::Oidc => pb::SsoProtocol::Oidc,
                Protocol::Saml => pb::SsoProtocol::Saml,
            } as i32,
            name: self.name.clone(),
            oidc: (self.protocol == Protocol::Oidc).then(|| pb::OidcProvider {
                issuer: self.oidc_issuer.clone(),
                client_id: self.oidc_client_id.clone(),
                client_secret: String::new(),
                client_secret_set: !self.oidc_client_secret.is_empty(),
                extra_scopes: self.oidc_extra_scopes.clone(),
            }),
            saml: (self.protocol == Protocol::Saml).then(|| pb::SamlProvider {
                entity_id: self.saml_entity_id.clone(),
                sso_url: self.saml_sso_url.clone(),
                certificates: self.saml_certificates.clone(),
            }),
            email_domains: self.email_domains.clone(),
        }
    }

    /// Whether people can sign in through it: set up, secret and all.
    pub fn ready(&self) -> Result<()> {
        match self.protocol {
            Protocol::None => Err(Error::FailedPrecondition("single sign-on isn't set up".into())),
            Protocol::Oidc if self.oidc_client_secret.is_empty() => {
                Err(Error::FailedPrecondition("the provider needs its client secret".into()))
            }
            _ => Ok(()),
        }
    }

    /// Reads a stored provider; empty or unreadable is none.
    pub fn parse(stored: &str) -> Provider {
        if stored.trim().is_empty() {
            return Provider::default();
        }
        serde_json::from_str(stored).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "ignoring a stored identity provider that doesn't read");
            Provider::default()
        })
    }

    pub fn stored(&self) -> String {
        if self.is_set() { serde_json::to_string(self).unwrap_or_default() } else { String::new() }
    }

    /// Lets in only people whose email is on one of its domains, if it lists any.
    fn admits(&self, identity: &Identity) -> Result<()> {
        if self.email_domains.is_empty() {
            return Ok(());
        }
        let domain = identity.email.rsplit_once('@').map(|(_, domain)| domain).unwrap_or_default();
        if identity.email_verified && self.email_domains.iter().any(|d| d == domain) {
            return Ok(());
        }
        Err(Error::denied(format!(
            "only people with a verified email at {} can sign in here",
            self.email_domains.join(", ")
        )))
    }
}

/// Who signed in, as the provider said.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub subject: String,
    pub email: String,
    pub email_verified: bool,
    pub name: String,
    pub username: String,
    pub picture: Option<String>,
}

impl Identity {
    pub fn to_pb(&self, signed_in_at: i64) -> pb::SsoIdentity {
        pb::SsoIdentity {
            subject: self.subject.clone(),
            email: self.email.clone(),
            name: self.name.clone(),
            signed_in_at: Some(crate::id::timestamp(signed_in_at)),
        }
    }
}

/// Where a sign-in happens: the instance's own, or one server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Instance,
    Server(String),
}

/// The addresses this side has for one scope, under the public URL.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// A server's provider was set up by its managers, so fetches to it must
    /// stay on the public internet; the instance's admins may use their own network.
    pub public_only: bool,
    pub public_url: String,
    pub redirect_uri: String,
    pub entity_id: String,
    pub acs_url: String,
}

impl Endpoints {
    pub fn new(public_url: &str, scope: &Scope) -> Self {
        let public_url = public_url.trim_end_matches('/').to_string();
        let base = match scope {
            Scope::Instance => format!("{public_url}/sso/instance"),
            Scope::Server(id) => format!("{public_url}/sso/servers/{id}"),
        };
        Self {
            public_only: matches!(scope, Scope::Server(_)),
            redirect_uri: format!("{base}/oidc"),
            entity_id: format!("{base}/saml/metadata"),
            acs_url: format!("{base}/saml"),
            public_url,
        }
    }

    pub fn to_pb(&self) -> pb::ServiceProvider {
        pb::ServiceProvider {
            oidc_redirect_uri: self.redirect_uri.clone(),
            saml_entity_id: self.entity_id.clone(),
            saml_acs_url: self.acs_url.clone(),
        }
    }
}

/// A sign-in on its way, as `sso_sign_ins` keeps it.
#[derive(Debug, Clone, Default)]
pub struct SignIn {
    pub state: String,
    /// SHA-256 (hex) of the starting app's secret.
    pub secret_hash: String,
    pub return_origin: String,
    /// For a server: the account signing in.
    pub account_id: String,
    /// For the instance: an admin checking the provider.
    pub test: bool,
    /// [`Provider::key`] when it started.
    pub provider_key: String,
    pub verifier: String,
    pub nonce: String,
    pub request_id: String,
    /// Once the provider answered: the code's hash and who signed in.
    pub code_hash: Option<String>,
    pub identity: Option<Identity>,
    pub expires_at: i64,
}

/// How many sign-ins may start in [`TTL_MS`] through one server's provider in
/// all, and by one account at one server. Each keeps a row until it runs out,
/// so this also bounds those. The instance's own sign-ins keep nothing until
/// the provider answers ([`ticket`]), so they need no such cap.
pub const MAX_STARTS_SERVER: usize = 1_000;
pub const MAX_STARTS_ACCOUNT: usize = 10;

/// Counts sign-ins started, per key, over the last [`TTL_MS`]. Instance
/// sign-ins all start on the directory and a server's on the shard holding it,
/// so one lock in one process sees every start for a key.
#[derive(Default)]
pub struct StartLimiter {
    starts: std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<i64>>>,
}

impl StartLimiter {
    /// Counts a start against every one of `limits`, or refuses it (counting
    /// nothing) when any is spent.
    pub fn start(&self, limits: &[(&str, usize)]) -> Result<()> {
        let mut starts = self.starts.lock().unwrap_or_else(|p| p.into_inner());
        let now = now_ms();
        starts.retain(|_, times| {
            while times.front().is_some_and(|t| now - t >= TTL_MS) {
                times.pop_front();
            }
            !times.is_empty()
        });
        if limits.iter().any(|(key, max)| starts.get(*key).is_some_and(|times| times.len() >= *max)) {
            return Err(Error::ResourceExhausted(
                "too many sign-ins started through this provider just now; try again in a few minutes".into(),
            ));
        }
        for (key, _) in limits {
            starts.entry(key.to_string()).or_default().push_back(now);
        }
        Ok(())
    }
}

/// The one limiter for this process.
pub fn starts() -> &'static StartLimiter {
    static STARTS: std::sync::OnceLock<StartLimiter> = std::sync::OnceLock::new();
    STARTS.get_or_init(StartLimiter::default)
}

/// Begins a server's sign-in: the provider's page, and the row to keep until
/// it's back. `return_origin` is already checked (`linked::return_origin`):
/// only apps the instance trusts get sign-ins back.
pub async fn start(
    provider: &Provider,
    endpoints: &Endpoints,
    return_origin: &str,
    secret_hash: &str,
    account_id: &str,
) -> Result<(String, SignIn)> {
    provider.ready()?;
    let secret_hash = secret_hash.trim().to_ascii_lowercase();
    if secret_hash.len() != 64 || !secret_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::invalid("secret_hash must be a SHA-256 in hex"));
    }
    let sign_in = SignIn {
        state: crate::auth::new_token(),
        secret_hash,
        return_origin: return_origin.to_string(),
        account_id: account_id.to_string(),
        provider_key: provider.trust_key(),
        verifier: crate::auth::new_token(),
        nonce: crate::auth::new_token(),
        request_id: format!("_{}", crate::auth::new_token()),
        expires_at: now_ms() + TTL_MS,
        ..Default::default()
    };
    let url = authorize(provider, endpoints, &sign_in).await?;
    Ok((url, sign_in))
}

/// The provider's sign-in page for `sign_in`.
pub async fn authorize(provider: &Provider, endpoints: &Endpoints, sign_in: &SignIn) -> Result<String> {
    provider.ready()?;
    match provider.protocol {
        Protocol::Oidc => {
            oidc::authorize_url(
                provider,
                endpoints.public_only,
                &endpoints.redirect_uri,
                &sign_in.state,
                &sign_in.nonce,
                &ticket::challenge(&sign_in.verifier),
            )
            .await
        }
        Protocol::Saml => saml::authorize_url(
            provider,
            &endpoints.entity_id,
            &endpoints.acs_url,
            &sign_in.state,
            &sign_in.request_id,
            now_ms(),
        ),
        Protocol::None => unreachable!("ready() refuses it"),
    }
}

/// What the provider sent back.
pub enum Answer {
    Oidc { code: String, error: String },
    Saml { response: String },
}

/// Works out who signed in from the provider's answer, and checks they may.
pub async fn identify(
    provider: &Provider,
    endpoints: &Endpoints,
    sign_in: &SignIn,
    answer: Answer,
) -> Result<Identity> {
    if provider.trust_key() != sign_in.provider_key {
        return Err(Error::FailedPrecondition("single sign-on changed while you were signing in; start again".into()));
    }
    provider.ready()?;
    let identity = match answer {
        Answer::Oidc { error, .. } if !error.is_empty() => {
            return Err(Error::denied(format!("the identity provider didn't sign you in ({error})")));
        }
        Answer::Oidc { code, .. } if provider.protocol == Protocol::Oidc && !code.is_empty() => {
            oidc::identify(
                provider,
                endpoints.public_only,
                &endpoints.redirect_uri,
                &code,
                &sign_in.verifier,
                &sign_in.nonce,
                now_ms() / 1000,
            )
            .await?
        }
        Answer::Saml { response } if provider.protocol == Protocol::Saml => saml::identify(
            &response,
            &saml::Expect {
                provider,
                entity_id: &endpoints.entity_id,
                acs_url: &endpoints.acs_url,
                request_id: &sign_in.request_id,
                now: now_ms(),
            },
        )?,
        _ => return Err(Error::invalid("that isn't how this identity provider answers")),
    };
    provider.admits(&identity)?;
    Ok(identity)
}

/// Checks a finish against its sign-in: the app's secret and the code the
/// browser came back with. Returns who signed in.
pub fn check_finish(sign_in: &SignIn, code: &str, secret: &str) -> Result<Identity> {
    // An instance ticket keeps the first half of the hash; a server's row all of it.
    let hash = crate::linked::secret_hash(secret);
    let expected = &sign_in.secret_hash;
    if !matches!(expected.len(), 32 | 64)
        || !crate::auth::constant_time_eq(&hash.as_bytes()[..expected.len().min(hash.len())], expected.as_bytes())
    {
        return Err(Error::denied("another app started this sign-in"));
    }
    let (Some(code_hash), Some(identity)) = (&sign_in.code_hash, &sign_in.identity) else {
        return Err(Error::FailedPrecondition("this sign-in hasn't come back from the identity provider yet".into()));
    };
    if !crate::auth::constant_time_eq(crate::auth::hash_token(code.trim()).as_bytes(), code_hash.as_bytes()) {
        return Err(Error::denied("that sign-in code doesn't match"));
    }
    Ok(identity.clone())
}

// ─────────────────── sign-ins on their way (inside a write) ───────────────────

pub async fn save(conn: &Connection, sign_in: &SignIn) -> Result<()> {
    conn.execute("DELETE FROM sso_sign_ins WHERE expires_at < ?1", [now_ms()]).await?;
    conn.execute(
        "INSERT INTO sso_sign_ins (state, secret_hash, return_origin, account_id, test, provider_key, verifier, nonce, request_id, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        (
            sign_in.state.as_str(),
            sign_in.secret_hash.as_str(),
            sign_in.return_origin.as_str(),
            sign_in.account_id.as_str(),
            sign_in.test,
            sign_in.provider_key.as_str(),
            sign_in.verifier.as_str(),
            sign_in.nonce.as_str(),
            sign_in.request_id.as_str(),
            sign_in.expires_at,
        ),
    )
    .await?;
    Ok(())
}

/// A sign-in that hasn't run out.
pub async fn load(conn: &Connection, state: &str) -> Result<Option<SignIn>> {
    query_one(
        conn,
        "SELECT state, secret_hash, return_origin, account_id, test, provider_key, verifier, nonce, request_id, code_hash, identity, expires_at
         FROM sso_sign_ins WHERE state = ?1 AND expires_at >= ?2 AND coalesce(code_hash, 'x') <> ''",
        (state, now_ms()),
        |r| {
            Ok(SignIn {
                state: r.get(0)?,
                secret_hash: r.get(1)?,
                return_origin: r.get(2)?,
                account_id: r.get(3)?,
                test: r.get(4)?,
                provider_key: r.get(5)?,
                verifier: r.get(6)?,
                nonce: r.get(7)?,
                request_id: r.get(8)?,
                code_hash: r.get(9)?,
                identity: r.get::<Option<String>>(10)?.and_then(|json| serde_json::from_str(&json).ok()),
                expires_at: r.get(11)?,
            })
        },
    )
    .await
}

/// Records the provider's answer, once: false if it was already answered.
pub async fn answered(conn: &Connection, state: &str, code_hash: &str, identity: &Identity) -> Result<bool> {
    let json = serde_json::to_string(identity).map_err(|err| Error::internal(err.to_string()))?;
    Ok(conn
        .execute(
            "UPDATE sso_sign_ins SET code_hash = ?2, identity = ?3, verifier = '', nonce = ''
             WHERE state = ?1 AND code_hash IS NULL",
            (state, code_hash, json.as_str()),
        )
        .await?
        == 1)
}

/// Uses a sign-in up: false if something else already did. The row stays,
/// emptied, until it runs out, so the provider's answer can't be used again.
pub async fn take(conn: &Connection, state: &str) -> Result<bool> {
    Ok(conn
        .execute(
            "UPDATE sso_sign_ins SET code_hash = '', identity = NULL WHERE state = ?1 AND code_hash <> ''",
            [state],
        )
        .await?
        == 1)
}

/// Records the provider's answer to an instance ticket: its first row, so
/// false if the state was used already.
pub async fn answered_ticket(
    conn: &Connection,
    sign_in: &SignIn,
    code_hash: &str,
    identity: &Identity,
) -> Result<bool> {
    let json = serde_json::to_string(identity).map_err(|err| Error::internal(err.to_string()))?;
    conn.execute("DELETE FROM sso_sign_ins WHERE expires_at < ?1", [now_ms()]).await?;
    let inserted = conn
        .execute(
            "INSERT INTO sso_sign_ins (state, secret_hash, return_origin, test, provider_key, code_hash, identity, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                sign_in.state.as_str(),
                sign_in.secret_hash.as_str(),
                sign_in.return_origin.as_str(),
                sign_in.test,
                sign_in.provider_key.as_str(),
                code_hash,
                json.as_str(),
                sign_in.expires_at,
            ),
        )
        .await;
    match inserted {
        Ok(_) => Ok(true),
        Err(err) => {
            let err = Error::from(err);
            if crate::db::is_unique_violation(&err) { Ok(false) } else { Err(err) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oidc(issuer: &str, secret: &str) -> pb::IdentityProvider {
        pb::IdentityProvider {
            protocol: pb::SsoProtocol::Oidc as i32,
            name: "Acme".into(),
            oidc: Some(pb::OidcProvider {
                issuer: issuer.into(),
                client_id: "fuwa".into(),
                client_secret: secret.into(),
                ..Default::default()
            }),
            email_domains: vec![" @Acme.com ".into(), "acme.com".into()],
            ..Default::default()
        }
    }

    #[test]
    fn providers_are_checked_and_keep_their_secret() {
        let first = Provider::from_pb(&oidc("https://login.acme.com/", "s3cret"), &Provider::default()).unwrap();
        assert_eq!(first.oidc_issuer, "https://login.acme.com");
        assert_eq!(first.email_domains, ["acme.com"]);
        assert_eq!(first.key(), "oidc https://login.acme.com");
        let shown = first.to_pb();
        assert!(shown.oidc.as_ref().unwrap().client_secret.is_empty() && shown.oidc.unwrap().client_secret_set);
        // Saving what was shown keeps the secret...
        let again = Provider::from_pb(&first.to_pb(), &first).unwrap();
        assert_eq!(again, first);
        // ...but not for another issuer.
        let moved = Provider::from_pb(&oidc("https://other.acme.com", ""), &first).unwrap();
        assert!(moved.ready().is_err());
        assert!(Provider::from_pb(&oidc("http://login.acme.com", "x"), &first).is_err());
        assert!(Provider::from_pb(&oidc("https://login.acme.com?x", "x"), &first).is_err());
        assert_eq!(Provider::parse(&first.stored()), first);
        assert!(!Provider::parse("").is_set());
    }

    #[test]
    fn a_new_client_or_certificate_is_a_new_provider_and_never_debugs_its_secret() {
        let first = Provider::from_pb(&oidc("https://login.acme.com", "s3cret"), &Provider::default()).unwrap();
        let mut other_client = first.clone();
        other_client.oidc_client_id = "another".into();
        assert_eq!(first.key(), other_client.key(), "the same accounts");
        assert_ne!(first.trust_key(), other_client.trust_key(), "but not the same trust");
        assert_eq!(first.host(), "login.acme.com");
        let shown = format!("{first:?}");
        assert!(!shown.contains("s3cret") && shown.contains("<redacted>"), "{shown}");
    }

    #[test]
    fn starting_sign_ins_is_limited_per_key() {
        let limiter = StartLimiter::default();
        for _ in 0..3 {
            limiter.start(&[("server", 5), ("ana", 3)]).unwrap();
        }
        assert!(limiter.start(&[("server", 5), ("ana", 3)]).is_err(), "ana's are spent");
        limiter.start(&[("server", 5), ("bea", 3)]).unwrap();
        limiter.start(&[("server", 5), ("bea", 3)]).unwrap();
        assert!(limiter.start(&[("server", 5), ("cai", 3)]).is_err(), "the server's are spent");
    }

    #[test]
    fn email_domains_decide_who_gets_in() {
        let provider = Provider::from_pb(&oidc("https://login.acme.com", "s"), &Provider::default()).unwrap();
        let person =
            |email: &str, verified| Identity { email: email.into(), email_verified: verified, ..Default::default() };
        assert!(provider.admits(&person("ana@acme.com", true)).is_ok());
        assert!(provider.admits(&person("ana@acme.com", false)).is_err());
        assert!(provider.admits(&person("ana@evil-acme.com", true)).is_err());
        assert!(provider.admits(&person("ana@acme.com.evil", true)).is_err());
        assert!(provider.admits(&person("", true)).is_err());
    }

    #[test]
    fn endpoints_sit_under_the_public_url() {
        let server = Endpoints::new("https://chat.example.com/", &Scope::Server("01ABC".into()));
        assert_eq!(server.redirect_uri, "https://chat.example.com/sso/servers/01ABC/oidc");
        assert_eq!(server.acs_url, "https://chat.example.com/sso/servers/01ABC/saml");
        assert_eq!(server.entity_id, "https://chat.example.com/sso/servers/01ABC/saml/metadata");
        assert_eq!(
            Endpoints::new("https://chat.example.com", &Scope::Instance).redirect_uri,
            "https://chat.example.com/sso/instance/oidc"
        );
    }
}
