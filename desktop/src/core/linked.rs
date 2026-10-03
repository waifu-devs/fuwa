//! Signing in with waifu.dev from the desktop app (and, the same way,
//! through an identity provider: [`crate::core::sso`]).
//!
//! The instance runs the sign-in (`AuthService.StartLinkedSignIn`); the app
//! opens the sign-in page in the browser and listens on this machine
//! (`http://127.0.0.1:<port>`, which instances accept as a return origin) for
//! the browser to come back. waifu.dev sends the browser to the instance's
//! callback page, which asks the person to confirm and hands the code on to
//! `<return origin>/auth/waifu/callback#code=…&state=…`. A fragment never
//! reaches a server, so the page this listener serves there reads it and
//! passes it back to the app. The app then finishes the sign-in with the
//! secret it kept, so a sign-in link someone else made can't be finished
//! with it.

use std::convert::Infallible;
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// How long a sign-in may take: the instance forgets it after ten minutes.
pub const TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What the browser brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Returned {
    Code { code: String, state: String },
    Failed(String),
}

/// A listener waiting for the browser to come back.
pub struct Callback {
    pub origin: String,
    rx: mpsc::Receiver<Returned>,
    task: tokio::task::JoinHandle<()>,
}

impl Callback {
    /// Starts listening on a free port on this machine.
    pub async fn listen() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let (tx, rx) = mpsc::channel(4);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req| handle(req, tx.clone()));
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok(Self { origin, rx, task })
    }

    /// Waits for the browser to bring back the sign-in for `state`.
    pub async fn wait(mut self, state: &str) -> Returned {
        let deadline = tokio::time::sleep(TIMEOUT);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = &mut deadline => return Returned::Failed("The sign-in ran out. Start again.".into()),
                got = self.rx.recv() => match got {
                    Some(Returned::Code { code, state: s }) if s == state => return Returned::Code { code, state: s },
                    // Someone else's sign-in, or a stale tab: not this one.
                    Some(Returned::Code { .. }) => continue,
                    Some(failed) => return failed,
                    None => return Returned::Failed("Stopped waiting for the browser.".into()),
                },
            }
        }
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle(req: Request<Incoming>, tx: mpsc::Sender<Returned>) -> Result<Response<Full<Bytes>>, Infallible> {
    let query: Vec<(String, String)> =
        url::form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes()).into_owned().collect();
    let get = |name: &str| query.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default();
    let response = match req.uri().path() {
        // Where the instance hands a sign-in back: waifu.dev's, or single sign-on's.
        "/auth/waifu/callback" | "/auth/sso/done" => page(PAGE),
        "/returned" => {
            let (code, state) = (get("code"), get("state"));
            let problem = if get("error_description").is_empty() { get("error") } else { get("error_description") };
            let returned = if !code.is_empty() && !state.is_empty() {
                Returned::Code { code, state }
            } else {
                Returned::Failed(if problem.is_empty() { "The sign-in didn't come back.".into() } else { problem })
            };
            let _ = tx.send(returned).await;
            Response::builder().status(StatusCode::NO_CONTENT).body(Full::new(Bytes::new())).expect("a response")
        }
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not here")))
            .expect("a response"),
    };
    Ok(response)
}

fn page(html: &'static str) -> Response<Full<Bytes>> {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .header("referrer-policy", "no-referrer")
        .body(Full::new(Bytes::from_static(html.as_bytes())))
        .expect("a response")
}

/// The page the browser lands on: hands the code to the app and says so.
const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>fuwa</title>
<style>
  :root { color-scheme: light dark; --bg:#fff5f8; --fg:#3b2330; --muted:#8a6577; --card:#fff; --primary:#f06292; }
  @media (prefers-color-scheme: dark) { :root { --bg:#14111f; --fg:#ece6ff; --muted:#9a90b8; --card:#1f1a2e; --primary:#b388ff; } }
  html,body { height:100%; margin:0; background:var(--bg); color:var(--fg); font-family:"M PLUS Rounded 1c",ui-rounded,system-ui,sans-serif; }
  body { display:grid; place-items:center; }
  .card { background:var(--card); border-radius:28px; padding:36px 40px; text-align:center; max-width:360px;
          box-shadow:0 30px 80px -40px var(--primary); animation:rise .6s cubic-bezier(.22,1,.36,1) both; }
  .mark { width:64px; height:64px; margin:0 auto 16px; border-radius:20px; background:var(--primary); display:grid; place-items:center;
          color:#fff; font-size:34px; animation:pop .7s .15s cubic-bezier(.34,1.56,.64,1) both; }
  h1 { font-size:22px; margin:0 0 8px; } p { color:var(--muted); margin:0; line-height:1.5; }
  @keyframes rise { from { opacity:0; transform:translateY(24px) scale(.97); } }
  @keyframes pop { from { opacity:0; transform:scale(.4) rotate(-20deg); } }
  @media (prefers-reduced-motion: reduce) { .card,.mark { animation:none; } }
</style></head>
<body><div class="card"><div class="mark" id="mark">&#10047;</div><h1 id="title">Signing you in…</h1><p id="text">Handing your sign-in to the fuwa app.</p></div>
<script>
  const params = new URLSearchParams(location.hash.slice(1) || location.search.slice(1));
  history.replaceState(null, "", location.pathname);
  fetch("/returned?" + params.toString()).then(() => {
    const failed = !params.get("code");
    document.getElementById("mark").textContent = failed ? "!" : "✓";
    document.getElementById("title").textContent = failed ? "That didn't work" : "You're signed in";
    document.getElementById("text").textContent = failed ? "Go back to fuwa and try again." : "You can close this tab and go back to fuwa.";
  }, () => {
    document.getElementById("title").textContent = "fuwa isn't listening anymore";
    document.getElementById("text").textContent = "Go back to fuwa and start again.";
  });
</script></body></html>"#;

/// Whether a sign-in page an instance hands over may open in the browser:
/// https, or http only on this computer (a local test instance).
pub fn safe_sign_in_page(url: &str) -> bool {
    let Ok(uri) = url.parse::<http::Uri>() else { return false };
    match (uri.scheme_str(), uri.host()) {
        (Some("https"), Some(_)) => true,
        (Some("http"), Some(host)) => {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_browser_hands_the_code_back() {
        let callback = Callback::listen().await.unwrap();
        let origin = callback.origin.clone();
        assert!(origin.starts_with("http://127.0.0.1:"));
        let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build_http::<Full<Bytes>>();
        let get = |path: String| {
            let client = client.clone();
            async move {
                let res = client.get(path.parse().unwrap()).await.unwrap();
                res.status()
            }
        };
        assert_eq!(get(format!("{origin}/auth/waifu/callback")).await, StatusCode::OK);
        assert_eq!(get(format!("{origin}/auth/sso/done")).await, StatusCode::OK);
        // Someone else's sign-in is ignored; ours comes through.
        assert_eq!(get(format!("{origin}/returned?code=x&state=other")).await, StatusCode::NO_CONTENT);
        assert_eq!(get(format!("{origin}/returned?code=abc&state=mine")).await, StatusCode::NO_CONTENT);
        assert_eq!(callback.wait("mine").await, Returned::Code { code: "abc".into(), state: "mine".into() });
    }

    #[test]
    fn sign_in_pages_must_be_https_or_this_computer() {
        assert!(safe_sign_in_page("https://www.waifu.dev/authorize?x=1"));
        assert!(safe_sign_in_page("http://127.0.0.1:3000/authorize"));
        assert!(safe_sign_in_page("http://localhost/authorize"));
        assert!(!safe_sign_in_page("http://evil.example/authorize"));
        assert!(!safe_sign_in_page("file:///C:/Windows/System32/calc.exe"));
        assert!(!safe_sign_in_page("ms-settings:"));
    }
}
