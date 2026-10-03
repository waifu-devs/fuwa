//! The embedded web client. Runs only when the binary is built with it:
//! `cd web && pnpm build`, then `cargo test --features web`.
#![cfg(feature = "web")]

use std::net::SocketAddr;
use std::path::Path;

use fuwa_server::app::App;
use fuwa_server::config::Config;

async fn start(dir: &Path, web: &str) -> (std::sync::Arc<App>, SocketAddr) {
    let dir = dir.to_str().unwrap().to_string();
    let web = web.to_string();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_WEB" => Some(web.clone()),
        _ => None,
    })
    .unwrap();
    let app = App::open(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    let shutdown = app.shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    });
    (app, addr)
}

#[tokio::test]
async fn the_app_opens_on_any_address() {
    let dir = tempfile::tempdir().unwrap();
    let (app, addr) = start(dir.path(), "on").await;
    let base = format!("http://{addr}");
    let http = reqwest::Client::new();

    let index = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(index.status(), 200);
    assert_eq!(index.headers()["content-type"], "text/html");
    assert_eq!(index.headers()["cache-control"], "no-cache");
    let csp = index.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(csp.contains("script-src 'self'") && csp.contains("form-action 'self'"), "{csp}");
    let etag = index.headers()["etag"].clone();
    let html = index.text().await.unwrap();
    assert!(html.contains(r#"<div id="root">"#));

    // A deep link to a channel on some instance gets the app, which routes it.
    let deep = http.get(format!("{base}/fuwa.waifu.dev/01ABC/01DEF")).send().await.unwrap();
    assert_eq!(deep.status(), 200);
    assert_eq!(deep.text().await.unwrap(), html);
    // A scanner's guess doesn't, even though any other path would.
    for probe in ["/.env", "/wp-config.php", "/.DS_Store", "/phpmyadmin/"] {
        assert_eq!(http.get(format!("{base}{probe}")).send().await.unwrap().status(), 404, "{probe}");
    }

    // Built assets are cached for good; a missing one is a real 404.
    let script = html.split(r#"src=""#).nth(1).unwrap().split('"').next().unwrap();
    let asset = http.get(format!("{base}{script}")).send().await.unwrap();
    assert_eq!(asset.status(), 200);
    assert_eq!(asset.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert!(asset.headers()["content-type"].to_str().unwrap().contains("javascript"));
    assert_eq!(http.get(format!("{base}/assets/missing.js")).send().await.unwrap().status(), 404);

    // Scripts go out gzipped to browsers that ask; fonts, compressed already, don't.
    let gzipped = http.get(format!("{base}{script}")).header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(gzipped.headers()["content-encoding"], "gzip");
    let css = html.split(r#"href=""#).find(|s| s.contains(".css")).unwrap().split('"').next().unwrap();
    let styles = http.get(format!("{base}{css}")).send().await.unwrap().text().await.unwrap();
    let font = styles.split("url(").find(|s| s.contains(".woff2")).unwrap().split(')').next().unwrap();
    let font = http.get(format!("{base}{font}")).header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(font.status(), 200);
    assert_eq!(font.headers()["content-type"], "font/woff2");
    assert!(font.headers().get("content-encoding").is_none());

    // Revalidation answers 304 with no body.
    let again = http.get(format!("{base}/")).header("if-none-match", etag).send().await.unwrap();
    assert_eq!(again.status(), 304);

    // The API still answers on the same port.
    assert_eq!(http.get(format!("{base}/healthz")).send().await.unwrap().text().await.unwrap(), "ok");
    let grpc_web = http
        .post(format!("{base}/fuwa.v1.NodeService/GetNode"))
        .header("content-type", "application/grpc-web+proto")
        .body(vec![0u8, 0, 0, 0, 0])
        .send()
        .await
        .unwrap();
    assert_eq!(grpc_web.status(), 200);
    assert_eq!(grpc_web.headers()["content-type"], "application/grpc-web+proto");
    app.shutdown.cancel();
}

#[tokio::test]
async fn operators_can_turn_the_app_off() {
    let dir = tempfile::tempdir().unwrap();
    let (app, addr) = start(dir.path(), "off").await;
    let http = reqwest::Client::new();
    let root = http.get(format!("http://{addr}/")).send().await.unwrap().text().await.unwrap();
    assert!(root.contains("is a fuwa instance"));
    assert_eq!(http.get(format!("http://{addr}/somewhere")).send().await.unwrap().status(), 404);
    // Signing in with waifu.dev still comes back here, for apps on other addresses.
    let callback = http.get(format!("http://{addr}/auth/waifu/callback?code=c&state=s")).send().await.unwrap();
    assert!(callback.text().await.unwrap().contains(r#"<div id="root">"#));

    // Admins can switch it back on (and off) while the instance runs.
    let mut settings = (*app.settings()).clone();
    settings.web = true;
    app.replace_settings(settings);
    let root = http.get(format!("http://{addr}/")).send().await.unwrap().text().await.unwrap();
    assert!(root.contains(r#"<div id="root">"#));
    app.shutdown.cancel();
}
