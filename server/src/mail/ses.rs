//! Sending through Amazon SES's v2 API (SendEmail,
//! https://docs.aws.amazon.com/ses/latest/APIReference-V2/API_SendEmail.html):
//! one POST to `/v2/email/outbound-emails`, signed with AWS Signature
//! Version 4 by hand (the same signing as `replica/s3.rs`), so fuwa needs no
//! AWS SDK.

use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use super::{Mail, Sender, provider_said};
use crate::error::{Error, Result};
use crate::replica::s3::{amz_date, hex, hmac};

const PATH: &str = "/v2/email/outbound-emails";

#[derive(Clone, PartialEq)]
pub struct Config {
    /// FUWA_SES_REGION, else AWS_REGION or AWS_DEFAULT_REGION.
    pub region: String,
    /// FUWA_SES_ACCESS_KEY_ID and FUWA_SES_SECRET_ACCESS_KEY, else
    /// AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY.
    pub access_key_id: String,
    pub secret_access_key: String,
    /// FUWA_SES_SESSION_TOKEN, else AWS_SESSION_TOKEN: for temporary credentials.
    pub session_token: Option<String>,
    /// FUWA_SES_ENDPOINT: where the API is, default https://email.<region>.amazonaws.com.
    pub endpoint: String,
    /// FUWA_SES_CONFIGURATION_SET: the configuration set mail goes out under.
    pub configuration_set: Option<String>,
}

impl Config {
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> std::result::Result<Self, String> {
        let first = |keys: &[&str]| {
            keys.iter().find_map(|key| get(key)).map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
        };
        let region = first(&["FUWA_SES_REGION", "AWS_REGION", "AWS_DEFAULT_REGION"])
            .ok_or("FUWA_MAIL_PROVIDER=ses needs FUWA_SES_REGION (or AWS_REGION)")?;
        if !region.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("FUWA_SES_REGION must be a region like us-east-1, got {region:?}"));
        }
        let access_key_id = first(&["FUWA_SES_ACCESS_KEY_ID", "AWS_ACCESS_KEY_ID"]);
        let secret_access_key = first(&["FUWA_SES_SECRET_ACCESS_KEY", "AWS_SECRET_ACCESS_KEY"]);
        let (Some(access_key_id), Some(secret_access_key)) = (access_key_id, secret_access_key) else {
            return Err("FUWA_MAIL_PROVIDER=ses needs FUWA_SES_ACCESS_KEY_ID and FUWA_SES_SECRET_ACCESS_KEY \
                        (or AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY)"
                .into());
        };
        let endpoint = first(&["FUWA_SES_ENDPOINT"])
            .map(|url| url.trim_end_matches('/').to_string())
            .unwrap_or_else(|| format!("https://email.{region}.amazonaws.com"));
        if reqwest::Url::parse(&endpoint).ok().and_then(|url| url.host_str().map(str::to_string)).is_none() {
            return Err(format!(
                "FUWA_SES_ENDPOINT must be an address like https://email.us-east-1.amazonaws.com, got {endpoint:?}"
            ));
        }
        Ok(Self {
            region,
            access_key_id,
            secret_access_key,
            session_token: first(&["FUWA_SES_SESSION_TOKEN", "AWS_SESSION_TOKEN"]),
            endpoint,
            configuration_set: first(&["FUWA_SES_CONFIGURATION_SET"]),
        })
    }
}

pub struct Ses {
    config: Config,
    /// The Host header: the endpoint's host, and its port if it names one.
    host: String,
    http: reqwest::Client,
}

impl Ses {
    pub fn new(config: Config) -> Result<Self> {
        let url = reqwest::Url::parse(&config.endpoint).map_err(|err| Error::internal(format!("mail: {err}")))?;
        let host = match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            (None, _) => return Err(Error::internal("mail: FUWA_SES_ENDPOINT has no host")),
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| Error::internal(format!("mail: {err}")))?;
        Ok(Self { config, host, http })
    }
}

#[tonic::async_trait]
impl Sender for Ses {
    async fn send(&self, from: &str, mail: &Mail) -> Result<()> {
        let content = |data: &str| serde_json::json!({ "Data": data, "Charset": "UTF-8" });
        let mut body = serde_json::json!({
            "FromEmailAddress": from,
            "Destination": { "ToAddresses": [mail.to] },
            "Content": { "Simple": {
                "Subject": content(&mail.subject),
                "Body": { "Text": content(&mail.text), "Html": content(&mail.html) },
            } },
        });
        if let Some(set) = &self.config.configuration_set {
            body["ConfigurationSetName"] = serde_json::Value::from(set.as_str());
        }
        let body = serde_json::to_vec(&body).map_err(|err| Error::internal(err.to_string()))?;
        let date = amz_date(SystemTime::now());
        let mut extra = vec![("content-type", "application/json")];
        if let Some(token) = &self.config.session_token {
            extra.push(("x-amz-security-token", token.as_str()));
        }
        let authorization = sign(
            &Signing {
                method: "POST",
                host: &self.host,
                path: PATH,
                query: "",
                extra: &extra,
                payload: &body,
                date: &date,
                region: &self.config.region,
                service: "ses",
            },
            &self.config.access_key_id,
            &self.config.secret_access_key,
        );
        let mut request = self
            .http
            .post(format!("{}{PATH}", self.config.endpoint))
            .header("x-amz-date", &date)
            .header("authorization", authorization)
            .header("content-type", "application/json");
        if let Some(token) = &self.config.session_token {
            request = request.header("x-amz-security-token", token);
        }
        let response = request
            .body(body)
            .send()
            .await
            .map_err(|err| Error::Unavailable(format!("Amazon SES: {}", err.without_url())))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let kind = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|kind| kind.to_str().ok())
            .map(|kind| kind.split(':').next().unwrap_or(kind).to_string())
            .unwrap_or_default();
        let said = provider_said(&response.text().await.unwrap_or_default());
        let said = [kind, said].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ");
        Err(Error::Unavailable(format!("Amazon SES answered {status}: {said}")))
    }
}

/// What a request is signed over.
struct Signing<'a> {
    method: &'a str,
    host: &'a str,
    /// Already percent-encoded.
    path: &'a str,
    /// Already canonical.
    query: &'a str,
    /// Headers to sign besides host and x-amz-date, lowercase names.
    extra: &'a [(&'a str, &'a str)],
    payload: &'a [u8],
    /// `20150830T123600Z`.
    date: &'a str,
    region: &'a str,
    service: &'a str,
}

/// The Authorization header for a request, by AWS Signature Version 4.
fn sign(request: &Signing<'_>, access_key_id: &str, secret_access_key: &str) -> String {
    let day = &request.date[..8];
    let mut headers: Vec<(&str, &str)> = vec![("host", request.host), ("x-amz-date", request.date)];
    headers.extend_from_slice(request.extra);
    headers.sort();
    let signed_headers = headers.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(";");
    let canonical_headers: String = headers.iter().map(|(name, value)| format!("{name}:{}\n", value.trim())).collect();
    let canonical = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{}",
        request.method,
        request.path,
        request.query,
        hex(&Sha256::digest(request.payload))
    );
    let scope = format!("{day}/{}/{}/aws4_request", request.region, request.service);
    let to_sign =
        format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", request.date, hex(&Sha256::digest(canonical.as_bytes())));
    let key = [day, request.region, request.service, "aws4_request"]
        .iter()
        .fold(format!("AWS4{secret_access_key}").into_bytes(), |key, part| hmac(&key, part.as_bytes()));
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "get-vanilla" from AWS's Signature Version 4 test suite.
    #[test]
    fn signs_like_aws() {
        let authorization = sign(
            &Signing {
                method: "GET",
                host: "example.amazonaws.com",
                path: "/",
                query: "",
                extra: &[],
                payload: b"",
                date: "20150830T123600Z",
                region: "us-east-1",
                service: "service",
            },
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        );
        assert_eq!(
            authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn credentials_fall_back_to_aws_variables() {
        let vars = |key: &str| match key {
            "AWS_REGION" => Some("eu-west-1".to_string()),
            "AWS_ACCESS_KEY_ID" => Some("id".to_string()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret".to_string()),
            "FUWA_SES_CONFIGURATION_SET" => Some("sign-ins".to_string()),
            _ => None,
        };
        let config = Config::from_lookup(&vars).unwrap();
        assert_eq!(config.endpoint, "https://email.eu-west-1.amazonaws.com");
        assert_eq!((config.access_key_id.as_str(), config.session_token), ("id", None));
        assert_eq!(config.configuration_set.as_deref(), Some("sign-ins"));
        assert!(Config::from_lookup(&|key: &str| (key == "AWS_REGION").then(|| "us-east-1".into())).is_err());
    }
}
