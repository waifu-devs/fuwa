//! Sending through Resend's API (https://resend.com/docs/api-reference/emails/send-email):
//! one POST to `/emails` with the API key as a bearer token.

use std::time::Duration;

use super::{Mail, Sender, provider_said};
use crate::error::{Error, Result};

const DEFAULT_URL: &str = "https://api.resend.com";

#[derive(Clone, PartialEq)]
pub struct Config {
    /// FUWA_RESEND_API_KEY.
    pub api_key: String,
    /// FUWA_RESEND_URL: where the API is, default https://api.resend.com.
    pub url: String,
}

impl Config {
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> std::result::Result<Self, String> {
        let api_key = get("FUWA_RESEND_API_KEY").map(|key| key.trim().to_string()).unwrap_or_default();
        if api_key.is_empty() {
            return Err("FUWA_MAIL_PROVIDER=resend needs FUWA_RESEND_API_KEY".into());
        }
        let url = get("FUWA_RESEND_URL")
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_URL.into());
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("FUWA_RESEND_URL must start with https://, got {url:?}"));
        }
        Ok(Self { api_key, url })
    }
}

pub struct Resend {
    config: Config,
    http: reqwest::Client,
}

impl Resend {
    pub fn new(config: Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| Error::internal(format!("mail: {err}")))?;
        Ok(Self { config, http })
    }
}

#[tonic::async_trait]
impl Sender for Resend {
    async fn send(&self, from: &str, mail: &Mail) -> Result<()> {
        let body = serde_json::json!({
            "from": from,
            "to": [mail.to],
            "subject": mail.subject,
            "text": mail.text,
            "html": mail.html,
        });
        let response = self
            .http
            .post(format!("{}/emails", self.config.url))
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| Error::Unavailable(format!("Resend: {}", err.without_url())))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let said = provider_said(&response.text().await.unwrap_or_default());
        Err(Error::Unavailable(format!("Resend answered {status}: {said}")))
    }
}
