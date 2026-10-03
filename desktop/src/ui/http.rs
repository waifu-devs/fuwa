//! How the window fetches pictures (avatars, server icons, emoji, cards'
//! images): GPUI asks its HTTP client, and this one runs the request on the
//! core's runtime with the same TLS setup as uploads. Without it, every
//! picture falls back to its letters.
//!
//! Pictures load only from fuwa instances: the ones you added, at the
//! address you gave or the one they say they have (a self-hosted instance
//! may well be on your network). A picture anywhere else would tell that
//! site your address and when you looked, so it isn't loaded at all;
//! instances fetch pictures from other sites for you and hand out links to
//! their own copy (`/media/outside/` on the instance), as the web app expects
//! too. Redirects are checked the same way, hop by hop.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui_kit::http_client::{AsyncBody, HttpClient, Response, Url, http};
use http_body_util::{BodyExt as _, Empty, Limited};

/// Pictures bigger than this aren't worth drawing.
const MAX: usize = 10 * 1024 * 1024;
const REDIRECTS: usize = 3;

/// The addresses of the instances you added, asked fresh for each picture.
pub type Trusted = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

pub struct Client {
    runtime: tokio::runtime::Handle,
    agent: http::HeaderValue,
    trusted: Trusted,
}

impl Client {
    pub fn new(runtime: tokio::runtime::Handle, trusted: Trusted) -> Self {
        let agent = format!("fuwa-desktop/{}", env!("CARGO_PKG_VERSION"));
        Self { runtime, agent: http::HeaderValue::from_str(&agent).expect("a plain header"), trusted }
    }
}

impl HttpClient for Client {
    fn user_agent(&self) -> Option<&http::HeaderValue> {
        Some(&self.agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(&self, req: http::Request<AsyncBody>) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let (uri, agent, trusted) = (req.uri().clone(), self.agent.clone(), self.trusted.clone());
        let task = self.runtime.spawn(fetch(uri, agent, trusted));
        async move { task.await? }.boxed()
    }
}

/// Whether a host is one of the instances you added.
fn trusted_host(uri: &http::Uri, trusted: &[String]) -> bool {
    let (Some(host), port) = (uri.host(), uri.port_u16()) else { return false };
    trusted.iter().filter_map(|t| t.parse::<http::Uri>().ok()).any(|t| {
        t.host().is_some_and(|h| h.eq_ignore_ascii_case(host))
            && t.port_u16().or(default_port(&t)) == port.or(default_port(uri))
    })
}

fn default_port(uri: &http::Uri) -> Option<u16> {
    match uri.scheme_str() {
        Some("https") => Some(443),
        Some("http") => Some(80),
        _ => None,
    }
}

/// Why a picture's address may not be fetched, if it may not.
fn refuse(uri: &http::Uri, trusted: &[String]) -> Option<&'static str> {
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return Some("not a web address");
    }
    if !trusted_host(uri, trusted) {
        return Some("pictures load only from fuwa instances");
    }
    None
}

/// A GET, following a few redirects (each checked again), with the body read into memory.
async fn fetch(mut uri: http::Uri, agent: http::HeaderValue, trusted: Trusted) -> anyhow::Result<Response<AsyncBody>> {
    let roots = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
        Ok(roots) => roots,
        Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
    };
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build::<_, Empty<Bytes>>(roots.https_or_http().enable_http1().build());
    for _ in 0..=REDIRECTS {
        let trusted = trusted();
        if let Some(why) = refuse(&uri, &trusted) {
            anyhow::bail!("{why}");
        }
        let request =
            http::Request::get(uri.clone()).header(http::header::USER_AGENT, agent.clone()).body(Empty::new())?;
        let response = tokio::time::timeout(Duration::from_secs(30), client.request(request)).await??;
        if response.status().is_redirection()
            && let Some(next) = response.headers().get(http::header::LOCATION).and_then(|l| l.to_str().ok())
        {
            uri = resolve(&uri, next)?;
            continue;
        }
        let (parts, body) = response.into_parts();
        let bytes = Limited::new(body, MAX).collect().await.map_err(|e| anyhow::anyhow!("{e}"))?.to_bytes();
        return Ok(Response::from_parts(parts, AsyncBody::from(bytes)));
    }
    anyhow::bail!("too many redirects")
}

/// Where a redirect points, which may be relative to where it came from.
fn resolve(from: &http::Uri, location: &str) -> anyhow::Result<http::Uri> {
    let base = Url::parse(&from.to_string())?;
    Ok(base.join(location)?.as_str().parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_resolve_against_where_they_came_from() {
        let from: http::Uri = "https://fuwa.chat/media/a/b.png".parse().unwrap();
        assert_eq!(resolve(&from, "/media/c.png").unwrap().to_string(), "https://fuwa.chat/media/c.png");
        assert_eq!(resolve(&from, "https://cdn.x/y").unwrap().to_string(), "https://cdn.x/y");
    }

    #[test]
    fn pictures_load_only_from_instances() {
        let trusted = vec!["http://127.0.0.1:8787".to_owned(), "https://fuwa.chat".to_owned()];
        let uri = |s: &str| s.parse::<http::Uri>().unwrap();
        assert_eq!(refuse(&uri("http://127.0.0.1:8787/media/a.png"), &trusted), None);
        assert_eq!(refuse(&uri("https://fuwa.chat/media/outside/sig?url=x"), &trusted), None);
        assert_eq!(refuse(&uri("https://FUWA.chat:443/media/a.png"), &trusted), None);
        assert!(refuse(&uri("http://fuwa.chat:8080/a.png"), &trusted).is_some());
        assert!(refuse(&uri("http://127.0.0.1:9000/a.png"), &trusted).is_some());
        assert!(refuse(&uri("https://example.com/a.png"), &trusted).is_some());
        assert!(refuse(&uri("https://192.168.1.10/a.png"), &trusted).is_some());
    }
}
