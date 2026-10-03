//! How the window fetches pictures (avatars, server icons, emoji, cards'
//! images): GPUI asks its HTTP client, and this one runs the request on the
//! core's runtime with the same TLS setup as uploads. Without it, every
//! picture falls back to its letters.
//!
//! Pictures from the instances you added load from wherever they are (a
//! self-hosted instance may well be on your network). Anything else must be
//! https and on the public internet: each address a name resolves to is
//! checked before connecting, and so is every redirect, so a link someone
//! posts can't make the app reach into your network.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui_kit::http_client::{AsyncBody, HttpClient, Response, Url, http};
use http_body_util::{BodyExt as _, Empty, Limited};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::Name;

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

/// Whether an address is somewhere on the public internet, not this
/// computer, the local network, or a range kept for special uses.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_v4(v4),
            None => public_v6(v6),
        },
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        // Carrier-grade NAT, benchmarking, and the IETF's own block.
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 192 && b == 0 && c == 0)
        || a >= 240)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Unique local (fc00::/7) and link-local (fe80::/10).
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        // Documentation (2001:db8::/32).
        || (first == 0x2001 && ip.segments()[1] == 0x0db8))
}

/// Resolves names with the system, keeping only public addresses.
#[derive(Clone)]
struct PublicOnly;

impl tower::Service<Name> for PublicOnly {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = found.filter(|a| is_public(a.ip())).collect();
            if public.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "that picture isn't on the public internet",
                ));
            }
            Ok(public.into_iter())
        })
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
    if trusted_host(uri, trusted) {
        return match uri.scheme_str() {
            Some("http" | "https") => None,
            _ => Some("not a web address"),
        };
    }
    if uri.scheme_str() != Some("https") {
        return Some("pictures from elsewhere must be https");
    }
    // An address written out, rather than a name to resolve.
    let host = uri.host().unwrap_or_default().trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(ip) if !is_public(ip) => Some("that picture isn't on the public internet"),
        _ => None,
    }
}

/// A GET, following a few redirects (each checked again), with the body read into memory.
async fn fetch(mut uri: http::Uri, agent: http::HeaderValue, trusted: Trusted) -> anyhow::Result<Response<AsyncBody>> {
    let roots = || match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
        Ok(roots) => roots,
        Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
    };
    let builder = || hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new());
    // Your instances, wherever they are.
    let own = builder().build::<_, Empty<Bytes>>(roots().https_or_http().enable_http1().build());
    // Everything else: https only, public addresses only.
    let mut http = HttpConnector::new_with_resolver(PublicOnly);
    http.enforce_http(false);
    let public = builder().build::<_, Empty<Bytes>>(roots().https_only().enable_http1().wrap_connector(http));
    for _ in 0..=REDIRECTS {
        let trusted = trusted();
        if let Some(why) = refuse(&uri, &trusted) {
            anyhow::bail!("{why}");
        }
        let request =
            http::Request::get(uri.clone()).header(http::header::USER_AGENT, agent.clone()).body(Empty::new())?;
        let sent = if trusted_host(&uri, &trusted) { own.request(request) } else { public.request(request) };
        let response = tokio::time::timeout(Duration::from_secs(30), sent).await??;
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
    fn only_public_addresses_count() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "172.16.0.9",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in ["1.1.1.1", "140.82.112.3", "2606:4700::1111"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn instances_load_from_anywhere_and_the_rest_needs_https_on_the_internet() {
        let trusted = vec!["http://127.0.0.1:8787".to_owned(), "https://fuwa.chat".to_owned()];
        let uri = |s: &str| s.parse::<http::Uri>().unwrap();
        assert_eq!(refuse(&uri("http://127.0.0.1:8787/media/a.png"), &trusted), None);
        assert_eq!(refuse(&uri("https://fuwa.chat/media/a.png"), &trusted), None);
        assert!(refuse(&uri("http://127.0.0.1:9000/a.png"), &trusted).is_some());
        assert!(refuse(&uri("http://example.com/a.png"), &trusted).is_some());
        assert!(refuse(&uri("https://192.168.1.10/a.png"), &trusted).is_some());
        assert!(refuse(&uri("https://[::1]/a.png"), &trusted).is_some());
        assert_eq!(refuse(&uri("https://example.com/a.png"), &trusted), None);
    }
}
