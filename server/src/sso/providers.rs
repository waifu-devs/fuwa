//! Signing in with an account elsewhere: Google, X and Twitch. Unlike the
//! instance's single sign-on (one provider its admins describe), these are
//! fixed: every address below is a constant, so nothing an admin types is
//! ever fetched. Admins only turn a provider on and give it the client ID
//! and secret they made in its console (InstanceSettings.sign_in_providers).
//!
//! Each is OAuth 2.0's code flow, the instance as a confidential client: the
//! state names the sign-in, PKCE where the provider takes it. The access
//! token is used once, to read who signed in, and dropped. Only the
//! provider's stable id, a name and (when the person asks) a picture are
//! taken from the profile, never an email.
//!
//! Adding another provider is a row in [`ALL`] and a [`Profile`] reader.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::Identity;
use crate::error::{Error, Result};
use crate::pb;

/// How a provider's answer about who signed in reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// OpenID Connect's UserInfo: sub, name, picture.
    Google,
    /// `GET /2/users/me`: data.id, data.name, data.username, data.profile_image_url.
    X,
    /// Helix `GET /users`: data[0].id, display_name, login, profile_image_url.
    Twitch,
}

/// One provider, as fuwa knows it.
#[derive(Debug)]
pub struct Spec {
    /// What apps and InstanceSettings call it.
    pub id: &'static str,
    pub name: &'static str,
    pub authorize: &'static str,
    pub token: &'static str,
    pub profile_url: &'static str,
    pub scopes: &'static str,
    /// Sends a PKCE challenge (S256).
    pub pkce: bool,
    /// Authenticates to the token endpoint with HTTP Basic instead of the form.
    pub basic_auth: bool,
    /// Asked of the sign-in page, so a shared computer doesn't sign in
    /// whoever used it last without asking.
    pub authorize_extra: &'static [(&'static str, &'static str)],
    pub profile: Profile,
    /// The only hosts a profile picture is fetched from.
    pub picture_hosts: &'static [&'static str],
}

pub const ALL: [Spec; 3] = [
    Spec {
        id: "google",
        name: "Google",
        authorize: "https://accounts.google.com/o/oauth2/v2/auth",
        token: "https://oauth2.googleapis.com/token",
        profile_url: "https://openidconnect.googleapis.com/v1/userinfo",
        scopes: "openid profile",
        pkce: true,
        basic_auth: false,
        authorize_extra: &[("prompt", "select_account")],
        profile: Profile::Google,
        picture_hosts: &["lh3.googleusercontent.com"],
    },
    Spec {
        id: "x",
        name: "X",
        authorize: "https://x.com/i/oauth2/authorize",
        token: "https://api.x.com/2/oauth2/token",
        profile_url: "https://api.x.com/2/users/me?user.fields=profile_image_url",
        // X answers /users/me only with both.
        scopes: "users.read tweet.read",
        pkce: true,
        basic_auth: true,
        authorize_extra: &[],
        profile: Profile::X,
        picture_hosts: &["pbs.twimg.com"],
    },
    Spec {
        id: "twitch",
        name: "Twitch",
        authorize: "https://id.twitch.tv/oauth2/authorize",
        token: "https://id.twitch.tv/oauth2/token",
        profile_url: "https://api.twitch.tv/helix/users",
        scopes: "",
        pkce: false,
        basic_auth: false,
        authorize_extra: &[("force_verify", "true")],
        profile: Profile::Twitch,
        picture_hosts: &["static-cdn.jtvnw.net"],
    },
];

pub fn spec(id: &str) -> Option<&'static Spec> {
    ALL.iter().find(|spec| spec.id == id)
}

/// Where tests run their fake providers: every address of a provider moves
/// under this base. Only code can set it, never configuration.
fn overrides() -> &'static Mutex<HashMap<&'static str, String>> {
    static OVERRIDES: OnceLock<Mutex<HashMap<&'static str, String>>> = OnceLock::new();
    OVERRIDES.get_or_init(Default::default)
}

#[doc(hidden)]
pub fn override_for_tests(id: &'static str, base: &str) {
    overrides().lock().unwrap_or_else(|p| p.into_inner()).insert(id, base.trim_end_matches('/').to_string());
}

impl Spec {
    fn overridden(&self) -> Option<String> {
        overrides().lock().unwrap_or_else(|p| p.into_inner()).get(self.id).cloned()
    }

    /// One of the provider's addresses, moved under a test base when set.
    fn at(&self, url: &str) -> String {
        match self.overridden() {
            Some(base) => {
                let parsed = reqwest::Url::parse(url).expect("provider addresses are valid");
                let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
                format!("{base}/{}{}{query}", self.id, parsed.path())
            }
            None => url.to_string(),
        }
    }

    /// The host people's browsers go to, which sees their address: apps
    /// name it next to the button.
    pub fn host(&self) -> String {
        reqwest::Url::parse(&self.at(self.authorize))
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// Fetches stay on the public internet, except a test's.
    fn public_only(&self) -> bool {
        self.overridden().is_none()
    }
}

/// A provider as an instance's admins set it up.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Setting {
    pub id: String,
    pub enabled: bool,
    pub client_id: String,
    pub client_secret: String,
}

impl std::fmt::Debug for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setting")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .field("client_id", &self.client_id)
            .field("client_secret", &if self.client_secret.is_empty() { "" } else { "<redacted>" })
            .finish()
    }
}

impl Setting {
    pub fn spec(&self) -> Option<&'static Spec> {
        spec(&self.id)
    }

    /// Whether people can sign in with it: on, with a client ID (and a
    /// secret where secrets are kept: [`check_ready`]).
    pub fn ready(&self) -> bool {
        self.enabled && !self.client_id.is_empty() && self.spec().is_some()
    }

    /// Changing the client makes it another provider to fuwa: sign-ins under
    /// way stop. Links stay, keyed by the provider's id for the person.
    pub fn trust_key(&self) -> String {
        format!("provider {} client {}", self.id, self.client_id)
    }

    /// The last four characters of the secret, so admins can tell which one is set.
    fn hint(&self) -> String {
        let chars: Vec<char> = self.client_secret.chars().collect();
        if chars.len() < 12 { String::new() } else { chars[chars.len() - 4..].iter().collect() }
    }

    pub fn to_pb(&self) -> pb::SignInProviderSetting {
        pb::SignInProviderSetting {
            id: self.id.clone(),
            enabled: self.enabled,
            client_id: self.client_id.clone(),
            client_secret: String::new(),
            client_secret_set: !self.client_secret.is_empty(),
            client_secret_hint: self.hint(),
        }
    }
}

/// The settings for every provider fuwa knows, in [`ALL`]'s order, from a
/// request: an empty secret keeps the stored one for the same client ID.
pub fn from_pb(from: &[pb::SignInProviderSetting], previous: &[Setting]) -> Result<Vec<Setting>> {
    let mut out = Vec::new();
    for given in from {
        let id = given.id.trim();
        if spec(id).is_none() {
            return Err(Error::invalid(format!("fuwa doesn't know a sign-in provider called {id:?}")));
        }
        if out.iter().any(|s: &Setting| s.id == id) {
            return Err(Error::invalid(format!("{id} is listed twice")));
        }
        let client_id = given.client_id.trim();
        if client_id.len() > 255 || !client_id.chars().all(|c| c.is_ascii_graphic()) {
            return Err(Error::invalid("a client ID is up to 255 characters, without spaces"));
        }
        let secret = given.client_secret.trim();
        if secret.len() > 1024 || !secret.chars().all(|c| c.is_ascii_graphic()) {
            return Err(Error::invalid("a client secret is up to 1024 characters, without spaces"));
        }
        let secret = match previous.iter().find(|p| p.id == id) {
            Some(previous) if secret.is_empty() && previous.client_id == client_id => previous.client_secret.clone(),
            _ => secret.to_string(),
        };
        out.push(Setting {
            id: id.to_string(),
            enabled: given.enabled,
            client_id: client_id.to_string(),
            client_secret: secret,
        });
    }
    out.sort_by_key(|s| ALL.iter().position(|spec| spec.id == s.id));
    Ok(out)
}

/// Refuses turning a provider on without its client ID and secret. Only
/// where secrets are kept (an admin's change), never on settings a gateway
/// is sent, which carry no secrets.
pub fn check_ready(settings: &[Setting]) -> Result<()> {
    match settings.iter().find(|s| s.enabled && (s.client_id.is_empty() || s.client_secret.is_empty())) {
        Some(setting) => Err(Error::FailedPrecondition(format!(
            "{} needs its client ID and secret to be turned on",
            setting.spec().map(|spec| spec.name).unwrap_or("that provider")
        ))),
        None => Ok(()),
    }
}

/// Where a provider sends people back, under the instance's public URL.
pub fn redirect_uri(public_url: &str, id: &str) -> String {
    format!("{}/sso/instance/providers/{id}", public_url.trim_end_matches('/'))
}

/// The provider's sign-in page.
pub fn authorize_url(setting: &Setting, redirect_uri: &str, state: &str, verifier: &str) -> Result<String> {
    let spec = setting.spec().ok_or_else(|| Error::FailedPrecondition("that sign-in provider isn't known".into()))?;
    let mut url = reqwest::Url::parse(&spec.at(spec.authorize)).map_err(|_| Error::internal("a provider address"))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &setting.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state);
        if !spec.scopes.is_empty() {
            query.append_pair("scope", spec.scopes);
        }
        if spec.pkce {
            query
                .append_pair("code_challenge", &super::ticket::challenge(verifier))
                .append_pair("code_challenge_method", "S256");
        }
        for (key, value) in spec.authorize_extra {
            query.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
}

/// Trades the code for an access token, reads who signed in with it, and
/// forgets the token.
pub async fn identify(setting: &Setting, redirect_uri: &str, code: &str, verifier: &str) -> Result<Identity> {
    let spec = setting.spec().ok_or_else(|| Error::FailedPrecondition("that sign-in provider isn't known".into()))?;
    if setting.client_secret.is_empty() {
        return Err(Error::FailedPrecondition(format!("{} needs its client secret; an admin can add it", spec.name)));
    }
    let client = super::oidc::client(spec.public_only());
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", setting.client_id.as_str()),
    ];
    if spec.pkce {
        form.push(("code_verifier", verifier));
    }
    if !spec.basic_auth {
        form.push(("client_secret", setting.client_secret.as_str()));
    }
    let mut request = client.post(spec.at(spec.token)).header("accept", "application/json").form(&form);
    if spec.basic_auth {
        request = request.basic_auth(&setting.client_id, Some(&setting.client_secret));
    }
    let response = request.send().await.map_err(super::oidc::unreachable)?;
    if !response.status().is_success() {
        tracing::info!("a sign-in provider refused a code");
        return Err(Error::denied(format!("{} didn't sign you in; start again", spec.name)));
    }
    let body = super::oidc::read_capped(response).await?;
    let token: TokenAnswer = serde_json::from_slice(&body)
        .map_err(|_| Error::FailedPrecondition(format!("{} answered in a way fuwa doesn't read", spec.name)))?;
    let mut request = client.get(spec.at(spec.profile_url)).bearer_auth(&token.access_token);
    if spec.profile == Profile::Twitch {
        request = request.header("client-id", &setting.client_id);
    }
    let response = request.header("accept", "application/json").send().await.map_err(super::oidc::unreachable)?;
    if !response.status().is_success() {
        tracing::info!("a sign-in provider didn't say who signed in");
        return Err(Error::Unavailable(format!("{} didn't say who you are; try again soon", spec.name)));
    }
    let body = super::oidc::read_capped(response).await?;
    read_profile(spec, &body)
}

/// Who signed in, from the provider's profile answer: only its id for the
/// person, a name, a handle and a picture's address.
pub fn read_profile(spec: &Spec, body: &[u8]) -> Result<Identity> {
    let unreadable = || Error::FailedPrecondition(format!("{} answered in a way fuwa doesn't read", spec.name));
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|_| unreadable())?;
    let user = match spec.profile {
        Profile::Google => &json,
        Profile::X => &json["data"],
        Profile::Twitch => &json["data"][0],
    };
    let field = |name: &str| user[name].as_str().unwrap_or_default().trim().to_string();
    let (subject, name, username, picture) = match spec.profile {
        Profile::Google => (field("sub"), field("name"), String::new(), field("picture")),
        Profile::X => (field("id"), field("name"), field("username"), field("profile_image_url")),
        Profile::Twitch => (field("id"), field("display_name"), field("login"), field("profile_image_url")),
    };
    if subject.is_empty() || subject.len() > 255 || !subject.chars().all(|c| c.is_ascii_graphic()) {
        return Err(unreadable());
    }
    Ok(Identity {
        subject,
        name: name.chars().take(64).collect(),
        username: username.chars().take(64).collect(),
        picture: Some(picture).filter(|p| !p.is_empty()),
        ..Default::default()
    })
}

/// The most read for a profile picture.
pub const MAX_PICTURE: usize = 8 * 1024 * 1024;

/// Whether a picture's address is one of the provider's own picture hosts,
/// over https. A test's provider stands in for the hosts with its own host
/// and port, so the check still runs.
pub fn picture_allowed(spec: &Spec, picture: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(picture) else { return false };
    match spec.overridden().and_then(|base| reqwest::Url::parse(&base).ok()) {
        Some(base) => url.scheme() == base.scheme() && url.host_str() == base.host_str() && url.port() == base.port(),
        None => {
            url.scheme() == "https"
                && url.port().is_none()
                && url
                    .host_str()
                    .is_some_and(|host| spec.picture_hosts.iter().any(|allowed| host.eq_ignore_ascii_case(allowed)))
        }
    }
}

/// Fetches a profile picture from the provider's picture host, capped.
/// What it is gets checked when it's stored, like any upload.
pub async fn fetch_picture(spec: &Spec, picture: &str) -> Result<Vec<u8>> {
    if !picture_allowed(spec, picture) {
        return Err(Error::FailedPrecondition(format!("fuwa won't fetch that picture from {}", spec.name)));
    }
    let response =
        super::oidc::client(spec.public_only()).get(picture).send().await.map_err(super::oidc::unreachable)?;
    let too_big = || Error::FailedPrecondition("that picture is too big".into());
    if !response.status().is_success() {
        return Err(Error::Unavailable(format!("{} didn't send the picture", spec.name)));
    }
    if response.content_length().is_some_and(|n| n > MAX_PICTURE as u64) {
        return Err(too_big());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(super::oidc::unreachable)? {
        if body.len() + chunk.len() > MAX_PICTURE {
            return Err(too_big());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(id: &str) -> Setting {
        Setting {
            id: id.into(),
            enabled: true,
            client_id: "client".into(),
            client_secret: "a long secret value".into(),
        }
    }

    fn query(url: &str) -> HashMap<String, String> {
        reqwest::Url::parse(url).unwrap().query_pairs().into_owned().collect()
    }

    #[test]
    fn sign_in_pages_ask_for_a_code_with_pkce_where_taken() {
        let back = "https://chat.example.com/sso/instance/providers/x";
        let x = authorize_url(&setting("x"), back, "the-state", "the-verifier").unwrap();
        assert!(x.starts_with("https://x.com/i/oauth2/authorize?"), "{x}");
        let q = query(&x);
        assert_eq!(q["state"], "the-state");
        assert_eq!(q["redirect_uri"], back);
        assert_eq!(q["scope"], "users.read tweet.read");
        assert_eq!(q["code_challenge"], super::super::ticket::challenge("the-verifier"));
        assert_eq!(q["code_challenge_method"], "S256");
        let google = query(&authorize_url(&setting("google"), back, "s", "v").unwrap());
        assert_eq!((google["scope"].as_str(), google["prompt"].as_str()), ("openid profile", "select_account"));
        let twitch = query(&authorize_url(&setting("twitch"), back, "s", "v").unwrap());
        assert!(!twitch.contains_key("code_challenge") && !twitch.contains_key("scope"));
        assert_eq!(twitch["force_verify"], "true");
    }

    #[test]
    fn profiles_read_only_the_id_name_handle_and_picture() {
        let google = read_profile(
            spec("google").unwrap(),
            br#"{"sub":"1087","name":"Ana","picture":"https://lh3.googleusercontent.com/a/x","email":"ana@example.com"}"#,
        )
        .unwrap();
        assert_eq!((google.subject.as_str(), google.name.as_str(), google.email.as_str()), ("1087", "Ana", ""));
        let x = read_profile(
            spec("x").unwrap(),
            br#"{"data":{"id":"44","name":"Bea","username":"bea_x","profile_image_url":"https://pbs.twimg.com/p.jpg"}}"#,
        )
        .unwrap();
        assert_eq!((x.subject.as_str(), x.username.as_str()), ("44", "bea_x"));
        assert_eq!(x.picture.as_deref(), Some("https://pbs.twimg.com/p.jpg"));
        let twitch = read_profile(
            spec("twitch").unwrap(),
            br#"{"data":[{"id":"9","login":"cai","display_name":"Cai","profile_image_url":""}]}"#,
        )
        .unwrap();
        assert_eq!((twitch.subject.as_str(), twitch.username.as_str(), twitch.picture), ("9", "cai", None));
        for missing in [&br#"{"data":[]}"#[..], br#"{"data":[{"id":""}]}"#, b"not json", br#"{"data":[{"id":"a b"}]}"#]
        {
            assert!(read_profile(spec("twitch").unwrap(), missing).is_err(), "{}", String::from_utf8_lossy(missing));
        }
    }

    #[test]
    fn pictures_come_only_from_the_providers_hosts() {
        let x = spec("x").unwrap();
        assert!(picture_allowed(x, "https://pbs.twimg.com/profile_images/1/a.jpg"));
        assert!(!picture_allowed(x, "http://pbs.twimg.com/profile_images/1/a.jpg"));
        assert!(!picture_allowed(x, "https://pbs.twimg.com:8443/a.jpg"));
        assert!(!picture_allowed(x, "https://pbs.twimg.com.evil.example/a.jpg"));
        assert!(!picture_allowed(x, "https://lh3.googleusercontent.com/a.jpg"), "another provider's host");
        assert!(!picture_allowed(x, "https://169.254.169.254/latest"));
    }

    #[test]
    fn settings_keep_their_secret_and_never_show_it() {
        let saved = from_pb(
            &[pb::SignInProviderSetting {
                id: "twitch".into(),
                enabled: true,
                client_id: "abc".into(),
                client_secret: "secret-ending-wxyz".into(),
                ..Default::default()
            }],
            &[],
        )
        .unwrap();
        let shown = saved[0].to_pb();
        assert_eq!(
            (shown.client_secret.as_str(), shown.client_secret_set, shown.client_secret_hint.as_str()),
            ("", true, "wxyz")
        );
        assert!(!format!("{:?}", saved[0]).contains("secret-ending"));
        // Saving what was shown keeps the secret; another client needs its own.
        assert_eq!(from_pb(std::slice::from_ref(&shown), &saved).unwrap(), saved);
        let moved = pb::SignInProviderSetting { client_id: "other".into(), ..shown };
        let moved = from_pb(&[moved], &saved).unwrap();
        assert!(moved[0].client_secret.is_empty(), "another client doesn't get the old secret");
        assert!(check_ready(&moved).is_err(), "and can't be on without its own");
        assert!(check_ready(&saved).is_ok());
        let unknown = pb::SignInProviderSetting { id: "myspace".into(), ..Default::default() };
        assert!(from_pb(&[unknown], &[]).is_err());
    }
}
