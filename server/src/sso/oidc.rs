//! OpenID Connect, as a confidential client: the code flow with PKCE, a
//! nonce and the state (which names the sign-in). The ID token's signature
//! is checked against the provider's published keys (its jwks_uri; never
//! "none" or a shared-secret algorithm), then its issuer, audience, expiry
//! and nonce. Every fetch is https and follows no redirects; for a server's
//! provider (set up by its managers, not the instance's admins) it also
//! refuses to reach loopback, private or link-local addresses.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

use super::{Identity, Provider};
use crate::error::{Error, Result};

/// How long what an issuer says about itself is remembered.
const DISCOVERY_TTL_MS: i64 = 10 * 60 * 1000;
const SKEW_SECS: i64 = 3 * 60;

/// What was fetched, by address, and when.
type Cache<T> = Mutex<HashMap<String, (i64, T)>>;

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
}

/// Whether an address is out on the internet: not this machine, a private
/// network, link-local, shared, documentation or reserved space.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && v4.octets()[2] == 0)
                || (a == 198 && (18..20).contains(&b)))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
                || (first == 0x2001 && v6.segments()[1] == 0xdb8)
                || first == 0x0064)
        }
    }
}

/// Resolves names only to public addresses.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let found: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = found.into_iter().filter(|addr| is_public(addr.ip())).collect();
            if public.is_empty() {
                return Err(format!("{} has no public address", name.as_str()).into());
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The client for fetches to a provider; `public_only` for providers a
/// server's managers set up.
pub(super) fn client(public_only: bool) -> &'static reqwest::Client {
    static ANY: OnceLock<reqwest::Client> = OnceLock::new();
    static PUBLIC: OnceLock<reqwest::Client> = OnceLock::new();
    let build = |public_only: bool| {
        let mut builder = reqwest::Client::builder()
            .user_agent(format!("fuwa/{}", crate::VERSION))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none());
        if public_only {
            builder = builder.dns_resolver(Arc::new(PublicOnly));
        }
        builder.build().expect("building an HTTP client")
    };
    if public_only { PUBLIC.get_or_init(|| build(true)) } else { ANY.get_or_init(|| build(false)) }
}

/// Checks an address before fetching it: https (or http on this machine,
/// for the instance's own provider while testing), and for `public_only`
/// no address written out that isn't public.
fn fetchable(url: &str, public_only: bool) -> Result<reqwest::Url> {
    let refused = || Error::FailedPrecondition(format!("fuwa won't fetch {url} for single sign-on"));
    let parsed = reqwest::Url::parse(url).map_err(|_| refused())?;
    if public_only {
        if parsed.scheme() != "https" {
            return Err(refused());
        }
        match parsed.host() {
            Some(url::Host::Domain(name)) if name.eq_ignore_ascii_case("localhost") || name.ends_with(".localhost") => {
                return Err(refused());
            }
            Some(url::Host::Ipv4(ip)) if !is_public(IpAddr::V4(ip)) => return Err(refused()),
            Some(url::Host::Ipv6(ip)) if !is_public(IpAddr::V6(ip)) => return Err(refused()),
            None => return Err(refused()),
            _ => {}
        }
    } else if !super::secure_url(url) {
        return Err(refused());
    }
    Ok(parsed)
}

fn unreachable(err: reqwest::Error) -> Error {
    tracing::info!(error = %err, "couldn't reach an identity provider");
    Error::Unavailable("the identity provider can't be reached right now; try again soon".into())
}

/// What an issuer says about itself, from its discovery document.
async fn discover(issuer: &str, public_only: bool) -> Result<Discovery> {
    static CACHE: OnceLock<Cache<Discovery>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let now = crate::id::now_ms();
    if let Some((at, found)) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(issuer)
        && now - at < DISCOVERY_TTL_MS
    {
        return Ok(found.clone());
    }
    let url = fetchable(&format!("{issuer}/.well-known/openid-configuration"), public_only)?;
    let response = client(public_only).get(url).send().await.map_err(unreachable)?;
    if !response.status().is_success() {
        return Err(Error::FailedPrecondition(format!(
            "{issuer} has no OpenID configuration (it answered {}); check the issuer",
            response.status()
        )));
    }
    let found: Discovery = response.json().await.map_err(|_| {
        Error::FailedPrecondition(format!("{issuer}'s OpenID configuration couldn't be read; check the issuer"))
    })?;
    if found.issuer.trim_end_matches('/') != issuer {
        return Err(Error::FailedPrecondition(format!(
            "{issuer}'s OpenID configuration names another issuer ({}); use that one",
            found.issuer
        )));
    }
    for endpoint in [&found.authorization_endpoint, &found.token_endpoint, &found.jwks_uri] {
        fetchable(endpoint, public_only)?;
    }
    cache.lock().unwrap_or_else(|p| p.into_inner()).insert(issuer.to_string(), (now, found.clone()));
    Ok(found)
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kty: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default, rename = "use")]
    usage: Option<String>,
    #[serde(default)]
    n: String,
    #[serde(default)]
    e: String,
    #[serde(default)]
    crv: String,
    #[serde(default)]
    x: String,
    #[serde(default)]
    y: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

/// The provider's signing keys; `fresh` skips the cache (a key it rotated in).
async fn keys(jwks_uri: &str, public_only: bool, fresh: bool) -> Result<Vec<Jwk>> {
    static CACHE: OnceLock<Cache<Vec<Jwk>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let now = crate::id::now_ms();
    if !fresh
        && let Some((at, found)) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(jwks_uri)
        && now - at < DISCOVERY_TTL_MS
    {
        return Ok(found.clone());
    }
    let url = fetchable(jwks_uri, public_only)?;
    let response = client(public_only).get(url).send().await.map_err(unreachable)?;
    let found: Jwks = response
        .error_for_status()
        .map_err(unreachable)?
        .json()
        .await
        .map_err(|_| Error::FailedPrecondition("the provider's signing keys couldn't be read".into()))?;
    cache.lock().unwrap_or_else(|p| p.into_inner()).insert(jwks_uri.to_string(), (now, found.keys.clone()));
    Ok(found.keys)
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

/// Checks a JWS's signature against one key, by its algorithm.
fn signed_by(alg: &str, key: &Jwk, message: &[u8], signature: &[u8]) -> bool {
    use ring::signature;
    let decode = |value: &str| URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')).ok();
    match (alg, key.kty.as_str()) {
        ("RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512", "RSA") => {
            let (Some(n), Some(e)) = (decode(&key.n), decode(&key.e)) else { return false };
            let params: &signature::RsaParameters = match alg {
                "RS256" => &signature::RSA_PKCS1_2048_8192_SHA256,
                "RS384" => &signature::RSA_PKCS1_2048_8192_SHA384,
                "RS512" => &signature::RSA_PKCS1_2048_8192_SHA512,
                "PS256" => &signature::RSA_PSS_2048_8192_SHA256,
                "PS384" => &signature::RSA_PSS_2048_8192_SHA384,
                _ => &signature::RSA_PSS_2048_8192_SHA512,
            };
            signature::RsaPublicKeyComponents { n, e }.verify(params, message, signature).is_ok()
        }
        ("ES256" | "ES384", "EC") => {
            let (Some(x), Some(y)) = (decode(&key.x), decode(&key.y)) else { return false };
            let (algorithm, curve, size): (&signature::EcdsaVerificationAlgorithm, _, _) = match alg {
                "ES256" => (&signature::ECDSA_P256_SHA256_FIXED, "P-256", 32),
                _ => (&signature::ECDSA_P384_SHA384_FIXED, "P-384", 48),
            };
            if key.crv != curve || x.len() != size || y.len() != size {
                return false;
            }
            let point = [&[4u8][..], &x, &y].concat();
            signature::UnparsedPublicKey::new(algorithm, point).verify(message, signature).is_ok()
        }
        ("EdDSA", "OKP") if key.crv == "Ed25519" => {
            let Some(x) = decode(&key.x) else { return false };
            signature::UnparsedPublicKey::new(&signature::ED25519, x).verify(message, signature).is_ok()
        }
        _ => false,
    }
}

/// Checks an ID token's signature with the provider's keys and returns its claims.
async fn verified_claims(id_token: &str, jwks_uri: &str, public_only: bool) -> Result<Vec<u8>> {
    let refused = |why: &str| Error::denied(format!("the identity provider's answer was refused: {why}"));
    let parts: Vec<&str> = id_token.split('.').collect();
    let [header, payload, signature] = parts[..] else { return Err(refused("its ID token isn't a signed JWT")) };
    let decode = |value: &str| {
        URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')).map_err(|_| refused("its ID token isn't a JWT"))
    };
    let parsed: Header = serde_json::from_slice(&decode(header)?).map_err(|_| refused("its ID token isn't a JWT"))?;
    if !matches!(
        parsed.alg.as_str(),
        "RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512" | "ES256" | "ES384" | "EdDSA"
    ) {
        return Err(refused("its ID token must be signed with a public key (RS, PS, ES or EdDSA)"));
    }
    let signature = decode(signature)?;
    let message = format!("{header}.{payload}");
    for fresh in [false, true] {
        let keys = keys(jwks_uri, public_only, fresh).await?;
        let candidates: Vec<&Jwk> = keys
            .iter()
            .filter(|k| k.usage.as_deref().is_none_or(|u| u == "sig"))
            .filter(|k| match &parsed.kid {
                Some(kid) => k.kid.as_deref() == Some(kid.as_str()),
                None => true,
            })
            .collect();
        if candidates.iter().any(|key| signed_by(&parsed.alg, key, message.as_bytes(), &signature)) {
            return decode(payload);
        }
        if !candidates.is_empty() {
            break;
        }
    }
    Err(refused("its ID token isn't signed by the provider's keys"))
}

/// The provider's sign-in page for one sign-in.
pub async fn authorize_url(
    provider: &Provider,
    public_only: bool,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> Result<String> {
    let found = discover(&provider.oidc_issuer, public_only).await?;
    let mut scope = String::from("openid email profile");
    for extra in provider.oidc_extra_scopes.split_whitespace() {
        if !scope.split(' ').any(|s| s == extra) {
            scope.push(' ');
            scope.push_str(extra);
        }
    }
    let mut url = reqwest::Url::parse(&found.authorization_endpoint)
        .map_err(|_| Error::FailedPrecondition("the provider's authorization endpoint isn't a URL".into()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &provider.oidc_client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &scope)
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.into())
}

#[derive(Deserialize)]
struct Tokens {
    id_token: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    exp: i64,
    #[serde(default)]
    iat: Option<i64>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    picture: Option<String>,
}

/// Trades the code the provider sent back for who signed in.
pub async fn identify(
    provider: &Provider,
    public_only: bool,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
    nonce: &str,
    now_secs: i64,
) -> Result<Identity> {
    let refused = |why: &str| Error::denied(format!("the identity provider's answer was refused: {why}"));
    let found = discover(&provider.oidc_issuer, public_only).await?;
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    let methods = &found.token_endpoint_auth_methods_supported;
    // Basic is the default when the provider doesn't say.
    let basic = methods.is_empty() || methods.iter().any(|m| m == "client_secret_basic");
    let mut request = client(public_only).post(fetchable(&found.token_endpoint, public_only)?);
    if basic {
        // RFC 6749 2.3.1: each form-encoded before they're joined.
        const FORM: &percent_encoding::AsciiSet =
            &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'*');
        let encode = |value: &str| percent_encoding::utf8_percent_encode(value, FORM).to_string();
        request = request.basic_auth(encode(&provider.oidc_client_id), Some(encode(&provider.oidc_client_secret)));
    } else {
        form.push(("client_id", &provider.oidc_client_id));
        form.push(("client_secret", &provider.oidc_client_secret));
    }
    let response = request.form(&form).send().await.map_err(unreachable)?;
    if !response.status().is_success() {
        let answer = response.text().await.unwrap_or_default();
        tracing::info!(answer = %answer.chars().take(300).collect::<String>(), "an identity provider refused a code");
        return Err(Error::FailedPrecondition(
            "the identity provider didn't take this sign-in; check the client ID and secret, then start again".into(),
        ));
    }
    let tokens: Tokens = response.json().await.map_err(|_| refused("it sent no ID token"))?;
    let payload = verified_claims(&tokens.id_token, &found.jwks_uri, public_only).await?;
    let claims: Claims = serde_json::from_slice(&payload).map_err(|_| refused("its ID token is missing claims"))?;

    if claims.iss.trim_end_matches('/') != provider.oidc_issuer {
        return Err(refused("its ID token comes from another issuer"));
    }
    let audiences = match &claims.aud {
        Audience::One(one) => vec![one.as_str()],
        Audience::Many(many) => many.iter().map(String::as_str).collect(),
    };
    if !audiences.contains(&provider.oidc_client_id.as_str()) {
        return Err(refused("its ID token is meant for another client"));
    }
    if audiences.len() > 1 && claims.azp.as_deref() != Some(provider.oidc_client_id.as_str()) {
        return Err(refused("its ID token is meant for another client"));
    }
    if claims.exp + SKEW_SECS <= now_secs || claims.iat.is_some_and(|iat| iat > now_secs + SKEW_SECS) {
        return Err(refused("its ID token ran out (is a clock wrong?)"));
    }
    if claims.nonce.as_deref() != Some(nonce) {
        return Err(refused("its ID token answers another sign-in"));
    }
    if claims.sub.is_empty() || claims.sub.len() > 255 {
        return Err(refused("its ID token names nobody"));
    }
    let email_verified = match claims.email_verified {
        Some(serde_json::Value::Bool(verified)) => verified,
        Some(serde_json::Value::String(text)) => text == "true",
        // Providers that don't say only hand out addresses they manage.
        _ => true,
    };
    Ok(Identity {
        subject: claims.sub,
        email: claims.email.unwrap_or_default().trim().to_lowercase(),
        email_verified,
        name: claims.name.unwrap_or_default(),
        username: claims.preferred_username.unwrap_or_default(),
        picture: claims.picture,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses_count_as_public() {
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn server_providers_are_fetched_only_over_https_on_the_internet() {
        assert!(fetchable("https://login.example.com/.well-known/openid-configuration", true).is_ok());
        for url in [
            "http://login.example.com/x",
            "https://localhost/x",
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://169.254.169.254/latest",
            "https://10.0.0.5/x",
        ] {
            assert!(fetchable(url, true).is_err(), "{url}");
        }
        // The instance's own provider may be on the admins' network, or this
        // machine while testing, but never plain http elsewhere.
        assert!(fetchable("https://10.0.0.5/x", false).is_ok());
        assert!(fetchable("http://127.0.0.1:8080/x", false).is_ok());
        assert!(fetchable("http://login.example.com/x", false).is_err());
    }

    #[test]
    fn id_tokens_need_a_public_key_signature() {
        let jwks: Jwks = serde_json::from_str(include_str!("../../tests/fixtures/sso-idp.jwks.json")).unwrap();
        let key = &jwks.keys[0];
        assert!(!signed_by("HS256", key, b"m", b"s"));
        assert!(!signed_by("none", key, b"m", b""));
        assert!(!signed_by("RS256", key, b"m", &[0; 256]));
    }
}
