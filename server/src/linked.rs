//! Signing in with waifu.dev (linked accounts). The instance is an OpenAuth
//! client of the issuer (https://api.waifu.dev unless changed) whose client ID
//! is its own public URL: it starts a code flow with PKCE, the issuer sends the
//! browser back to `<public URL>/auth/waifu/callback` with a code, and the
//! instance trades the code for a token and asks the issuer's `/userinfo` who
//! signed in. The token is made out to this instance, and waifu.dev takes it
//! for nothing but `/userinfo`.

use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Where the issuer sends people back to: this path on the instance, and on
/// any fuwa app a sign-in is handed on to.
pub const CALLBACK: &str = "/auth/waifu/callback";

/// How long a sign-in may take from start to finish.
pub const TTL_MS: i64 = 10 * 60 * 1000;

/// The client ID this instance signs in with for `public_url`: the URL as a
/// browser writes it (lowercase host, no default port, no trailing slash).
/// None when the issuer won't send people back there: it takes https, or
/// http on this machine while testing.
pub fn client_id(public_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(public_url).ok()?;
    let allowed = match url.scheme() {
        "https" => url.host().is_some(),
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    };
    if !allowed || !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
        return None;
    }
    if url.fragment().is_some() {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_string())
}

/// Whether the issuer can send people back to `url` (see [`client_id`]).
pub fn can_return_to(url: &str) -> bool {
    client_id(url).is_some()
}

/// The origin of an app a sign-in goes back to, like `https://app.example.com`:
/// https (or this machine), with nothing after the host. Only apps this
/// instance trusts get sign-ins back, or anyone could start one that returns
/// to their own site and send a victim the waifu.dev link to approve: the
/// instance's own address (`public_url`), apps on this machine (desktop
/// apps listen on loopback), and origins its admins listed in
/// `allowed_origins` (exactly; `*` lets browsers call the API, but doesn't
/// count here).
pub fn return_origin(value: &str, public_url: &str, allowed_origins: &[String]) -> Result<String> {
    let invalid = || Error::invalid("return_origin must be an https origin, like https://chat.example.com");
    let id = client_id(value.trim()).ok_or_else(invalid)?;
    let url = reqwest::Url::parse(&id).map_err(|_| invalid())?;
    if url.path() != "/" {
        return Err(invalid());
    }
    let origin = url.origin().ascii_serialization();
    let own = client_id(public_url)
        .and_then(|id| reqwest::Url::parse(&id).ok())
        .is_some_and(|public| public.origin().ascii_serialization() == origin);
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let listed = allowed_origins.iter().any(|allowed| allowed != "*" && *allowed == origin);
    if !(own || loopback || listed) {
        return Err(Error::denied(
            "this instance only sends sign-ins back to its own app, apps on this device, \
             or apps its admins list in its allowed origins",
        ));
    }
    Ok(origin)
}

/// A PKCE verifier and its S256 challenge.
pub fn pkce() -> (String, String) {
    let verifier = crate::auth::new_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// The issuer's sign-in page for one sign-in.
pub fn authorize_url(issuer: &str, client_id: &str, state: &str, challenge: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{issuer}/authorize"))
        .map_err(|err| Error::internal(format!("linked_issuer {issuer:?} isn't a URL: {err}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &format!("{client_id}{CALLBACK}"))
        .append_pair("response_type", "code")
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.into())
}

/// Who signed in, as the issuer's `/userinfo` tells it.
#[derive(Debug, Clone, Deserialize)]
pub struct Identity {
    /// Never changes, unlike the username.
    pub sub: String,
    #[serde(default)]
    pub preferred_username: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub picture: Option<String>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(format!("fuwa/{}", crate::VERSION))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("building an HTTP client")
    })
}

fn unreachable(_: reqwest::Error) -> Error {
    tracing::warn!("couldn't reach the linked accounts issuer");
    Error::Unavailable("waifu.dev can't be reached right now; try again soon".into())
}

/// Trades a sign-in's code for who signed in.
pub async fn identify(issuer: &str, client_id: &str, code: &str, verifier: &str) -> Result<Identity> {
    let redirect_uri = format!("{client_id}{CALLBACK}");
    let response = client()
        .post(format!("{issuer}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(unreachable)?;
    if response.status().is_client_error() {
        let _ = response.text().await;
        tracing::info!("the issuer refused a sign-in code");
        return Err(Error::FailedPrecondition("waifu.dev didn't take this sign-in; start again".into()));
    }
    let tokens: Tokens = response.error_for_status().map_err(unreachable)?.json().await.map_err(unreachable)?;

    // The instance needs nothing more from the issuer, so it hands back the
    // refresh token (waifu.dev's sign-out endpoint; elsewhere it's a 404).
    if !tokens.refresh_token.is_empty() {
        let revoke = client()
            .post(format!("{issuer}/session/revoke"))
            .json(&serde_json::json!({ "refreshToken": tokens.refresh_token }));
        tokio::spawn(async move {
            let _ = revoke.send().await;
        });
    }

    let identity: Identity = client()
        .get(format!("{issuer}/userinfo"))
        .bearer_auth(&tokens.access_token)
        .send()
        .await
        .map_err(unreachable)?
        .error_for_status()
        .map_err(unreachable)?
        .json()
        .await
        .map_err(unreachable)?;
    if identity.sub.is_empty() || identity.sub.len() > 255 {
        return Err(Error::internal("the issuer's /userinfo had no usable sub"));
    }
    Ok(identity)
}

/// A username to start from for someone new: theirs on waifu.dev, made to fit
/// fuwa's rules (2 to 32 of a-z, 0-9, _ and ., starting with a letter or digit).
pub fn username_base(preferred: &str) -> String {
    let mapped: String = preferred
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' { c } else { '_' })
        .collect();
    // 28 leaves room for a suffix like _99.
    let name: String = mapped.trim_start_matches(['_', '.']).chars().take(28).collect();
    match name.len() {
        0 => "member".into(),
        1 => format!("{name}_"),
        _ => name,
    }
}

/// The name to try after `base` was taken: `base_2`, `base_3`, …
pub fn username_candidate(base: &str, attempt: u32) -> String {
    if attempt <= 1 { base.to_string() } else { format!("{base}_{attempt}") }
}

/// SHA-256 of an app's secret, hex, as StartLinkedSignIn takes it.
pub fn secret_hash(secret: &str) -> String {
    crate::auth::hash_token(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_are_https_addresses_as_browsers_write_them() {
        assert_eq!(client_id("https://Chat.Example.com/").as_deref(), Some("https://chat.example.com"));
        assert_eq!(client_id("https://chat.example.com:443").as_deref(), Some("https://chat.example.com"));
        assert_eq!(client_id("http://localhost:8080").as_deref(), Some("http://localhost:8080"));
        assert_eq!(client_id("http://[::1]:8080").as_deref(), Some("http://[::1]:8080"));
        assert_eq!(client_id("http://192.168.1.5:8080"), None);
        assert_eq!(client_id("https://me:pw@chat.example.com"), None);
        assert_eq!(client_id("https://chat.example.com/?x=1"), None);
        assert_eq!(client_id("chat.example.com"), None);
    }

    #[test]
    fn return_origins_are_origins() {
        let listed = ["https://app.example.com".to_string()];
        let origin = |value: &str| return_origin(value, "https://chat.example.com", &listed);
        assert_eq!(origin("https://App.example.com").unwrap(), "https://app.example.com");
        assert_eq!(origin("http://localhost:5173/").unwrap(), "http://localhost:5173");
        assert!(origin("https://app.example.com/path").is_err());
        assert!(origin("http://app.example.com").is_err());
        assert!(origin("javascript:alert(1)").is_err());
    }

    #[test]
    fn sign_ins_go_back_only_to_apps_the_instance_trusts() {
        let public = "https://chat.example.com";
        let any = ["*".to_string()];
        assert_eq!(return_origin("https://chat.example.com", public, &any).unwrap(), "https://chat.example.com");
        assert!(return_origin("http://127.0.0.1:43123", public, &any).is_ok(), "desktop apps listen on loopback");
        assert!(return_origin("https://localhost:8443", public, &any).is_ok());
        let refused = return_origin("https://evil.example", public, &any).unwrap_err();
        assert!(matches!(refused, Error::PermissionDenied(_)), "{refused:?}");
        let listed = ["https://app.example.com".to_string()];
        assert!(return_origin("https://app.example.com", public, &listed).is_ok());
        assert!(return_origin("https://other.example.com", public, &listed).is_err());
        assert!(return_origin("https://chat.example.com.evil.example", public, &listed).is_err());
    }

    #[test]
    fn usernames_fit_the_rules() {
        for (preferred, expected) in [
            ("shixzie", "shixzie"),
            ("Juan-Dev", "juan_dev"),
            ("-dash", "dash"),
            ("x", "x_"),
            ("", "member"),
            ("日本", "member"),
            ("a.very-long-github-login-that-goes-on", "a.very_long_github_login_tha"),
        ] {
            let base = username_base(preferred);
            assert_eq!(base, expected, "{preferred}");
            for attempt in [1, 2, 99] {
                crate::auth::validate_username(&username_candidate(&base, attempt)).unwrap();
            }
        }
    }

    #[test]
    fn authorize_urls_carry_the_whole_request() {
        let url = authorize_url("https://api.waifu.dev", "https://chat.example.com", "st", "ch").unwrap();
        let url = reqwest::Url::parse(&url).unwrap();
        assert_eq!(url.path(), "/authorize");
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["redirect_uri"], "https://chat.example.com/auth/waifu/callback");
        assert_eq!(query["client_id"], "https://chat.example.com");
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!((query["state"].as_str(), query["code_challenge"].as_str()), ("st", "ch"));
    }
}
