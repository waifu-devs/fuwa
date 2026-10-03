//! How the window fetches pictures (avatars, server icons, emoji): GPUI asks
//! its HTTP client, and this one runs the request on the core's runtime with
//! the same TLS setup as uploads. Without it, every picture falls back to
//! its letters.

use std::time::Duration;

use bytes::Bytes;
use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui_kit::http_client::{AsyncBody, HttpClient, Response, Url, http};
use http_body_util::{BodyExt as _, Empty, Limited};

/// Pictures bigger than this aren't worth drawing.
const MAX: usize = 10 * 1024 * 1024;
const REDIRECTS: usize = 3;

pub struct Client {
    runtime: tokio::runtime::Handle,
    agent: http::HeaderValue,
}

impl Client {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        let agent = format!("fuwa-desktop/{}", env!("CARGO_PKG_VERSION"));
        Self { runtime, agent: http::HeaderValue::from_str(&agent).expect("a plain header") }
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
        let (uri, agent) = (req.uri().clone(), self.agent.clone());
        let task = self.runtime.spawn(fetch(uri, agent));
        async move { task.await? }.boxed()
    }
}

/// A GET, following a few redirects, with the body read into memory.
async fn fetch(mut uri: http::Uri, agent: http::HeaderValue) -> anyhow::Result<Response<AsyncBody>> {
    let roots = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
        Ok(roots) => roots,
        Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
    };
    let connector = roots.https_or_http().enable_http1().build();
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build::<_, Empty<Bytes>>(connector);
    for _ in 0..=REDIRECTS {
        anyhow::ensure!(matches!(uri.scheme_str(), Some("http" | "https")), "not a web address");
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
}
