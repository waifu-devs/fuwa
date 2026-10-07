//! Single sign-on through an organization's identity provider (OpenID
//! Connect or SAML), for an instance's own sign-in or one community
//! server's, the desktop's side of `web/src/lib/sso.ts`.
//!
//! It runs like signing in with waifu.dev ([`linked`]): the app keeps a
//! secret, opens the provider's page in the browser and listens on this
//! computer; the instance checks who signed in and sends the browser to its
//! `/auth/sso/done` page, which hands the one-time code on to the app's
//! loopback address, and the app finishes with the secret.

use std::sync::Arc;

use crate::core::api::{Api, Problem};
use crate::core::store::sort_members;
use crate::core::{Core, linked, vault};
use crate::pb;
use crate::rpc;

/// What the browser hands back, once the provider and the instance are done.
pub(crate) struct Answer {
    pub(crate) state: String,
    pub(crate) code: String,
    pub(crate) secret: String,
}

/// A secret to keep and the hash the instance is told.
pub(crate) fn secret() -> Result<(String, String), Problem> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Problem::new(tonic::Code::Internal, e.to_string()))?;
    let secret = vault::sha256_hex(&bytes);
    let hash = vault::sha256_hex(secret.as_bytes());
    Ok((secret, hash))
}

/// Opens the provider's page (only a real web page) and waits for the browser.
pub(crate) async fn round_trip(
    callback: linked::Callback,
    authorize_url: &str,
    state: &str,
    secret: String,
    open_page: impl FnOnce(&str) + Send,
) -> Result<Answer, Problem> {
    if !linked::safe_sign_in_page(authorize_url) {
        return Err(Problem::new(
            tonic::Code::PermissionDenied,
            "The instance gave a sign-in page that isn't https, so fuwa won't open it.",
        ));
    }
    open_page(authorize_url);
    match callback.wait(state).await {
        linked::Returned::Code { code, state } => Ok(Answer { state, code, secret }),
        linked::Returned::Failed(message) => Err(Problem::new(tonic::Code::Cancelled, message)),
    }
}

pub(crate) async fn listen() -> Result<linked::Callback, Problem> {
    linked::Callback::listen().await.map_err(|e| Problem::new(tonic::Code::Internal, e.to_string()))
}

impl Core {
    /// Signs in to an instance through its identity provider, in the
    /// browser. Returns the instance's key, and whether the account is new.
    pub async fn sso_sign_in(
        self: &Arc<Self>,
        url: &str,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<(String, bool), Problem> {
        let api = Api::new(url, None).map_err(|e| Problem::new(tonic::Code::InvalidArgument, e.to_string()))?;
        let callback = listen().await?;
        let (secret, secret_hash) = secret()?;
        let started = rpc!(
            api.auth(),
            start_sso_sign_in(pb::StartSsoSignInRequest {
                return_origin: callback.origin.clone(),
                secret_hash,
                test: false,
            })
        )
        .await?;
        let answer = round_trip(callback, &started.authorize_url, &started.state, secret, open_page).await?;
        let res = rpc!(
            api.auth(),
            finish_sso_sign_in(pb::FinishSsoSignInRequest {
                state: answer.state,
                code: answer.code,
                secret: answer.secret,
            })
        )
        .await?;
        Ok((self.add_instance(url, Some(res.token)), res.created))
    }

    /// An instance admin's check of the saved provider: signs in through it in
    /// the browser and says who it signed in, changing nothing.
    pub async fn test_instance_sso(
        self: &Arc<Self>,
        key: &str,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<Option<pb::SsoIdentity>, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let callback = listen().await?;
        let (secret, secret_hash) = secret()?;
        let started = rpc!(
            api.auth(),
            start_sso_sign_in(pb::StartSsoSignInRequest {
                return_origin: callback.origin.clone(),
                secret_hash,
                test: true,
            })
        )
        .await?;
        let answer = round_trip(callback, &started.authorize_url, &started.state, secret, open_page).await?;
        let res = rpc!(
            api.auth(),
            finish_sso_sign_in(pb::FinishSsoSignInRequest {
                state: answer.state,
                code: answer.code,
                secret: answer.secret,
            })
        )
        .await?;
        Ok(res.identity)
    }

    /// Signs in through a community server's identity provider, in the
    /// browser: to join it (with the invite, for one out of Browse) or to
    /// see its channels again once a sign-in ran out. Returns your
    /// membership, if you're a member.
    pub async fn server_sso(
        self: &Arc<Self>,
        key: &str,
        server_id: &str,
        invite_code: Option<String>,
        open_page: impl FnOnce(&str) + Send,
    ) -> Result<Option<pb::Member>, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let callback = listen().await?;
        let (secret, secret_hash) = secret()?;
        let started = rpc!(
            api.sso(),
            start_server_sso(pb::StartServerSsoRequest {
                server_id: server_id.into(),
                return_origin: callback.origin.clone(),
                secret_hash,
                invite_code: invite_code.unwrap_or_default(),
            })
        )
        .await?;
        let answer = round_trip(callback, &started.authorize_url, &started.state, secret, open_page).await?;
        let res = rpc!(
            api.sso(),
            finish_server_sso(pb::FinishServerSsoRequest {
                server_id: server_id.into(),
                state: answer.state,
                code: answer.code,
                secret: answer.secret,
            })
        )
        .await?;
        // The channels come back through the event stream; the membership now.
        if let Some(member) = &res.member {
            self.shared.instance(key, |i| {
                let members = i.members.entry(server_id.to_owned()).or_default();
                let id = member.user.as_ref().map(|u| u.id.as_str());
                match members.iter_mut().find(|m| m.user.as_ref().map(|u| u.id.as_str()) == id) {
                    Some(m) => *m = member.clone(),
                    None => members.push(member.clone()),
                }
                sort_members(members);
            });
        }
        Ok(res.member)
    }
}

/// A copy of a provider with every part present, so a form can edit any field.
pub fn full_provider(p: Option<&pb::IdentityProvider>) -> pb::IdentityProvider {
    let mut next = p.cloned().unwrap_or_default();
    next.oidc.get_or_insert_with(Default::default);
    next.saml.get_or_insert_with(Default::default);
    next
}

/// Everything a save would send about a provider, to compare two of them
/// (the web's `providerFingerprint`): the parts of the other protocol and
/// blank edges don't count.
pub fn provider_print(p: Option<&pb::IdentityProvider>) -> String {
    let Some(p) = p.filter(|p| p.protocol != pb::SsoProtocol::Unspecified as i32) else { return String::new() };
    let mut parts = vec![p.protocol.to_string(), p.name.trim().to_owned(), p.email_domains.join(",")];
    if p.protocol == pb::SsoProtocol::Oidc as i32 {
        let o = p.oidc.clone().unwrap_or_default();
        parts.extend([
            o.issuer.trim().to_owned(),
            o.client_id.trim().to_owned(),
            o.client_secret,
            o.extra_scopes.trim().to_owned(),
        ]);
    } else {
        let s = p.saml.clone().unwrap_or_default();
        parts.extend([s.entity_id.trim().to_owned(), s.sso_url.trim().to_owned(), s.certificates.trim().to_owned()]);
    }
    parts.join("\n")
}

/// "acme.com" from whatever someone typed ("@Acme.com ", "https://acme.com").
pub fn clean_domain(text: &str) -> String {
    let t = text.trim().to_lowercase();
    let t = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(&t);
    let t = t.strip_prefix('@').unwrap_or(t);
    t.split('/').next().unwrap_or_default().to_owned()
}

/// The most email domains a provider takes.
pub const MAX_DOMAINS: usize = 20;

/// Adds the domains in `text` (split by spaces or commas) that aren't there yet.
pub fn add_domains(domains: &mut Vec<String>, text: &str) {
    for d in text.split(|c: char| c.is_whitespace() || c == ',').map(clean_domain) {
        if !d.is_empty() && !domains.contains(&d) && domains.len() < MAX_DOMAINS {
            domains.push(d);
        }
    }
}

/// What an identity provider's SAML metadata says: its entity ID, where
/// people sign in (HTTP-Redirect), and its signing certificates as PEM (up
/// to four). None when it isn't provider metadata, or is over [`MAX_METADATA`].
/// The web's `readSamlMetadata`.
#[derive(Debug, Default, PartialEq)]
pub struct SamlMetadata {
    pub entity_id: String,
    pub sso_url: String,
    pub certificates: String,
}

/// The most pasted metadata read: real metadata is a few kilobytes.
pub const MAX_METADATA: usize = 1_000_000;
/// The most signing certificates kept from metadata.
const MAX_CERTIFICATES: usize = 4;

pub fn read_saml_metadata(xml: &str) -> Option<SamlMetadata> {
    if xml.len() > MAX_METADATA {
        return None;
    }
    let mut found = SamlMetadata::default();
    let (mut seen_entity, mut in_idp, mut seen_idp) = (false, false, false);
    // Inside a signing KeyDescriptor, and inside its X509Certificate.
    let (mut signing_key, mut in_cert) = (false, false);
    let mut certs: Vec<String> = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        let text = &rest[..open];
        if in_cert {
            let b64: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if !b64.is_empty() && certs.len() < MAX_CERTIFICATES && !certs.contains(&b64) {
                certs.push(b64);
            }
        }
        rest = &rest[open..];
        // Comments, declarations and CDATA are skipped whole.
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let close = rest.find('>')?;
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            match local(name.trim()) {
                "IDPSSODescriptor" => in_idp = false,
                "KeyDescriptor" => signing_key = false,
                "X509Certificate" => in_cert = false,
                _ => {}
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let name = local(tag.split_whitespace().next().unwrap_or_default());
        let attr = |key: &str| attribute(tag, key);
        match name {
            "EntityDescriptor" if !seen_entity => {
                seen_entity = true;
                found.entity_id = attr("entityID").unwrap_or_default();
            }
            "IDPSSODescriptor" if !seen_idp => {
                seen_idp = true;
                in_idp = !self_closing;
            }
            "SingleSignOnService" if in_idp && found.sso_url.is_empty() => {
                if attr("Binding").is_some_and(|b| b.ends_with(":HTTP-Redirect")) {
                    found.sso_url = attr("Location").unwrap_or_default();
                }
            }
            "KeyDescriptor" if in_idp => {
                signing_key = !self_closing && attr("use").is_none_or(|u| u == "signing");
            }
            "X509Certificate" if signing_key => in_cert = !self_closing,
            _ => {}
        }
    }
    if !seen_entity || !seen_idp {
        return None;
    }
    found.certificates = certs
        .iter()
        .map(|b64| {
            let lines: Vec<&str> =
                b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
            format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----", lines.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(found)
}

/// A name without its namespace prefix: "md:EntityDescriptor" is "EntityDescriptor".
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// One attribute's value in a start tag, quotes and the usual entities undone.
fn attribute(tag: &str, key: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(at) = rest.find(key) {
        let before_ok = rest[..at].ends_with(|c: char| c.is_whitespace());
        let after = rest[at + key.len()..].trim_start();
        if before_ok && let Some(after) = after.strip_prefix('=') {
            let after = after.trim_start();
            let quote = after.chars().next()?;
            if quote == '"' || quote == '\'' {
                let body = &after[1..];
                let end = body.find(quote)?;
                return Some(
                    body[..end]
                        .replace("&quot;", "\"")
                        .replace("&apos;", "'")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">")
                        .replace("&amp;", "&"),
                );
            }
        }
        rest = &rest[at + key.len()..];
    }
    None
}

/// Days in milliseconds, for sign-ins that run out.
const DAY_MS: i64 = 86_400_000;

/// Whether you're kept out of a server's channels until you sign in through
/// its provider: it requires one, you aren't its owner or an agent, and your
/// sign-in is missing or older than its recheck. The instance decides; this
/// only says why the channels went away (`ssoLocked` in `web/src/lib/sso.ts`).
pub fn locked(server: &pb::Server, me: Option<&pb::Member>, now_ms: i64) -> bool {
    let Some(member) = me else { return false };
    let Some(user) = member.user.as_ref() else { return false };
    if !server.sso_required || user.id == server.owner_id || user.kind == pb::AccountKind::Agent as i32 {
        return false;
    }
    let at = member.sso_signed_in_at.as_ref().map(|t| t.seconds * 1000).unwrap_or(0);
    if at == 0 {
        return true;
    }
    server.sso_recheck_days > 0 && now_ms - at >= i64::from(server.sso_recheck_days) * DAY_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saml_metadata_fills_in_the_provider() {
        let cert = "MIIB".to_owned() + &"A".repeat(70);
        let xml = format!(
            r#"<?xml version="1.0"?>
<!-- a comment <with> tags -->
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="https://idp.acme.com/meta?a=1&amp;b=2">
  <md:IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:KeyDescriptor use="encryption"><ds:KeyInfo><ds:X509Data><ds:X509Certificate>ENCRYPT</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>
    <md:KeyDescriptor use="signing"><ds:KeyInfo><ds:X509Data><ds:X509Certificate>
      {cert}
    </ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>
    <md:KeyDescriptor><ds:KeyInfo><ds:X509Data><ds:X509Certificate>{cert}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>
    <md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" Location="https://idp.acme.com/post"/>
    <md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="https://idp.acme.com/redirect"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#
        );
        let found = read_saml_metadata(&xml).unwrap();
        assert_eq!(found.entity_id, "https://idp.acme.com/meta?a=1&b=2");
        assert_eq!(found.sso_url, "https://idp.acme.com/redirect");
        assert_eq!(
            found.certificates.matches("BEGIN CERTIFICATE").count(),
            1,
            "same one twice, encryption key left out"
        );
        assert!(found.certificates.contains(&format!("{}\n{}", &cert[..64], &cert[64..])));
        assert_eq!(read_saml_metadata("<html><body>nope</body></html>"), None);
        assert_eq!(read_saml_metadata("not xml at all"), None);
        let huge = format!("{xml}{}", " ".repeat(MAX_METADATA));
        assert_eq!(read_saml_metadata(&huge), None, "too big to read");
        let many: String = (0..50)
            .map(|n| format!("<md:KeyDescriptor><ds:X509Certificate>C{n}</ds:X509Certificate></md:KeyDescriptor>"))
            .collect();
        let many = xml.replace("</md:IDPSSODescriptor>", &format!("{many}</md:IDPSSODescriptor>"));
        let found = read_saml_metadata(&many).unwrap();
        assert_eq!(found.certificates.matches("BEGIN CERTIFICATE").count(), 4, "four at most");
    }

    #[test]
    fn domains_are_cleaned_and_kept_once() {
        assert_eq!(clean_domain(" @Acme.com "), "acme.com");
        assert_eq!(clean_domain("https://acme.com/login"), "acme.com");
        let mut domains = vec!["acme.com".to_owned()];
        add_domains(&mut domains, "ACME.com, example.org  @b.io");
        assert_eq!(domains, ["acme.com", "example.org", "b.io"]);
    }

    #[test]
    fn prints_ignore_the_other_protocol() {
        let mut a = full_provider(None);
        a.protocol = pb::SsoProtocol::Oidc as i32;
        a.name = "Acme ".into();
        let mut b = a.clone();
        b.name = "Acme".into();
        b.saml.as_mut().unwrap().entity_id = "x".into();
        assert_eq!(provider_print(Some(&a)), provider_print(Some(&b)));
        b.oidc.as_mut().unwrap().issuer = "https://login.acme.com".into();
        assert_ne!(provider_print(Some(&a)), provider_print(Some(&b)));
        assert_eq!(provider_print(Some(&full_provider(None))), "");
    }

    #[test]
    fn locked_only_without_a_fresh_sign_in() {
        let server = |required, days| pb::Server {
            owner_id: "owner".into(),
            sso_required: required,
            sso_recheck_days: days,
            ..Default::default()
        };
        let member = |id: &str, kind: pb::AccountKind, at_days_ago: Option<i64>| pb::Member {
            user: Some(pb::User { id: id.into(), kind: kind as i32, ..Default::default() }),
            sso_signed_in_at: at_days_ago
                .map(|d| prost_types::Timestamp { seconds: (100 * DAY_MS - d * DAY_MS) / 1000, nanos: 0 }),
            ..Default::default()
        };
        let now = 100 * DAY_MS;
        let person = pb::AccountKind::Local;
        assert!(!locked(&server(false, 0), Some(&member("a", person, None)), now));
        assert!(locked(&server(true, 0), Some(&member("a", person, None)), now));
        assert!(!locked(&server(true, 0), Some(&member("a", person, Some(400))), now), "0 days is forever");
        assert!(!locked(&server(true, 30), Some(&member("a", person, Some(29))), now));
        assert!(locked(&server(true, 30), Some(&member("a", person, Some(30))), now));
        assert!(!locked(&server(true, 30), Some(&member("owner", person, None)), now));
        assert!(!locked(&server(true, 30), Some(&member("bot", pb::AccountKind::Agent, None)), now));
        assert!(!locked(&server(true, 30), None, now), "not a member: the join button asks instead");
    }
}
