//! Sending through Cloudflare Email Service's REST API
//! (https://developers.cloudflare.com/email-service/api/send-emails/rest-api/):
//! one POST to `/accounts/{account}/email/sending/send` with an API token
//! that can send email ("Email Sending: Edit") as a bearer token. The
//! sending domain has to be on Cloudflare DNS and onboarded to Email Sending.

use std::time::Duration;

use super::{Mail, Sender, from_address, from_name};
use crate::error::{Error, Result};

const DEFAULT_URL: &str = "https://api.cloudflare.com/client/v4";

#[derive(Clone, PartialEq)]
pub struct Config {
    /// FUWA_CLOUDFLARE_ACCOUNT_ID: the account the sending domain is on.
    pub account_id: String,
    /// FUWA_CLOUDFLARE_API_TOKEN.
    pub api_token: String,
    /// FUWA_CLOUDFLARE_URL: where the API is, default https://api.cloudflare.com/client/v4.
    pub url: String,
}

impl Config {
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> std::result::Result<Self, String> {
        let var = |name: &str| get(name).map(|value| value.trim().to_string()).unwrap_or_default();
        let (account_id, api_token) = (var("FUWA_CLOUDFLARE_ACCOUNT_ID"), var("FUWA_CLOUDFLARE_API_TOKEN"));
        if account_id.is_empty() || api_token.is_empty() {
            return Err(
                "FUWA_MAIL_PROVIDER=cloudflare needs FUWA_CLOUDFLARE_ACCOUNT_ID and FUWA_CLOUDFLARE_API_TOKEN".into()
            );
        }
        if !account_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("FUWA_CLOUDFLARE_ACCOUNT_ID must be the account's id, got {account_id:?}"));
        }
        let url = get("FUWA_CLOUDFLARE_URL")
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_URL.into());
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("FUWA_CLOUDFLARE_URL must start with https://, got {url:?}"));
        }
        Ok(Self { account_id, api_token, url })
    }
}

pub struct Cloudflare {
    config: Config,
    http: reqwest::Client,
}

impl Cloudflare {
    pub fn new(config: Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| Error::internal(format!("mail: {err}")))?;
        Ok(Self { config, http })
    }
}

/// The request body: `from` is the bare address, or `{address, name}` when
/// FUWA_MAIL_FROM has a name.
fn body(from: &str, mail: &Mail) -> serde_json::Value {
    let address = from_address(from).unwrap_or_else(|| from.to_string());
    let from = match from_name(from) {
        Some(name) => serde_json::json!({ "address": address, "name": name }),
        None => serde_json::json!(address),
    };
    serde_json::json!({
        "from": from,
        "to": mail.to,
        "subject": mail.subject,
        "text": mail.text,
        "html": mail.html,
    })
}

/// What an answer's `errors` say, cut short: `code: message`, joined.
fn errors_said(body: &serde_json::Value) -> String {
    let said: Vec<String> = body
        .get("errors")
        .and_then(|errors| errors.as_array())
        .into_iter()
        .flatten()
        .map(|error| {
            let message = error.get("message").and_then(|m| m.as_str()).unwrap_or_default();
            match error.get("code").and_then(|c| c.as_i64()) {
                Some(code) => format!("{code}: {message}"),
                None => message.to_string(),
            }
        })
        .collect();
    let said = said.join("; ");
    said.chars().take(200).collect()
}

#[tonic::async_trait]
impl Sender for Cloudflare {
    async fn send(&self, from: &str, mail: &Mail) -> Result<()> {
        let response = self
            .http
            .post(format!("{}/accounts/{}/email/sending/send", self.config.url, self.config.account_id))
            .bearer_auth(&self.config.api_token)
            .json(&body(from, mail))
            .send()
            .await
            .map_err(|err| Error::Unavailable(format!("Cloudflare: {}", err.without_url())))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let answer = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
        let success = answer.get("success").and_then(|s| s.as_bool()).unwrap_or(status.is_success());
        if !status.is_success() || !success {
            let said = errors_said(&answer);
            let said = if said.is_empty() { text.chars().take(200).collect() } else { said };
            return Err(Error::Unavailable(format!("Cloudflare answered {status}: {said}")));
        }
        // A 200 can still say the address bounced for good.
        let bounced = answer
            .pointer("/result/permanent_bounces")
            .and_then(|b| b.as_array())
            .is_some_and(|bounces| !bounces.is_empty());
        if bounced {
            return Err(Error::invalid("that address can't receive email"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;

    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    fn mail() -> Mail {
        Mail { to: "kai@example.com".into(), subject: "s".into(), text: "t".into(), html: "<p>t</p>".into() }
    }

    #[test]
    fn config() {
        let config = Config::from_lookup(&vars(&[
            ("FUWA_CLOUDFLARE_ACCOUNT_ID", " 0123abcd "),
            ("FUWA_CLOUDFLARE_API_TOKEN", "tok"),
        ]))
        .unwrap();
        assert_eq!(config.account_id, "0123abcd");
        assert_eq!(config.url, DEFAULT_URL);
        assert!(Config::from_lookup(&vars(&[("FUWA_CLOUDFLARE_API_TOKEN", "tok")])).is_err());
        assert!(
            Config::from_lookup(&vars(&[
                ("FUWA_CLOUDFLARE_ACCOUNT_ID", "a/../b"),
                ("FUWA_CLOUDFLARE_API_TOKEN", "tok"),
            ]))
            .is_err()
        );
    }

    #[test]
    fn from_with_and_without_a_name() {
        let named = body("Fuwa <No-Reply@example.com>", &mail());
        assert_eq!(named["from"], serde_json::json!({ "address": "no-reply@example.com", "name": "Fuwa" }));
        assert_eq!(named["to"], "kai@example.com");
        assert_eq!(body("no-reply@example.com", &mail())["from"], "no-reply@example.com");
    }

    type Seen = Arc<Mutex<Vec<(String, String, serde_json::Value)>>>;

    async fn fake(answer: (StatusCode, &'static str)) -> (String, Seen) {
        #[derive(Clone)]
        struct Fake(Seen, (StatusCode, &'static str));
        async fn send(
            State(Fake(seen, answer)): State<Fake>,
            uri: axum::http::Uri,
            headers: HeaderMap,
            body: axum::body::Bytes,
        ) -> (StatusCode, String) {
            let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            seen.lock().unwrap().push((uri.path().to_string(), auth, serde_json::from_slice(&body).unwrap()));
            (answer.0, answer.1.to_string())
        }
        let seen = Seen::default();
        let router = Router::new()
            .route("/accounts/{account}/email/sending/send", post(send))
            .with_state(Fake(seen.clone(), answer));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, seen)
    }

    fn sender(url: String) -> Cloudflare {
        Cloudflare::new(Config { account_id: "acct".into(), api_token: "tok".into(), url }).unwrap()
    }

    #[tokio::test]
    async fn sends() {
        let ok = r#"{"success":true,"errors":[],"messages":[],"result":{"delivered":["kai@example.com"],"permanent_bounces":[],"queued":[]}}"#;
        let (url, seen) = fake((StatusCode::OK, ok)).await;
        sender(url).send("Fuwa <no-reply@example.com>", &mail()).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].0, "/accounts/acct/email/sending/send");
        assert_eq!(seen[0].1, "Bearer tok");
        assert_eq!(seen[0].2["subject"], "s");
    }

    #[tokio::test]
    async fn says_what_went_wrong() {
        let bad = r#"{"success":false,"errors":[{"code":10001,"message":"email.sending.error.invalid_request_schema"}],"messages":[],"result":null}"#;
        let (url, _) = fake((StatusCode::BAD_REQUEST, bad)).await;
        let err = sender(url).send("no-reply@example.com", &mail()).await.unwrap_err();
        assert!(format!("{err:?}").contains("10001: email.sending.error.invalid_request_schema"), "{err:?}");

        let bounced = r#"{"success":true,"errors":[],"messages":[],"result":{"delivered":[],"permanent_bounces":["kai@example.com"],"queued":[]}}"#;
        let (url, _) = fake((StatusCode::OK, bounced)).await;
        assert!(sender(url).send("no-reply@example.com", &mail()).await.is_err());
    }
}
