//! Requests from scanners looking for software fuwa isn't (WordPress,
//! phpMyAdmin, Spring's actuator, a stray `.env` or `.git`). They're turned
//! away with a plain 404 before anything routes them, so they never reach the
//! API or the web app. Nothing about them is logged or kept, the sender's
//! address least of all. On fuwa.chat Railway's edge turns most of them away
//! first (`.railway/edge-rules.json`); this is the same list, for every
//! instance and anything the edge lets through.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};

/// Endings no fuwa address ever has. None is a top-level domain, so an
/// instance's address in a web app link (`/fuwa.example.pl/...`) never matches.
const ENDINGS: &[&str] = &[
    ".php", ".php3", ".php4", ".php5", ".php7", ".phtml", ".asp", ".aspx", ".ashx", ".asmx", ".axd", ".jsp", ".jspx",
    ".cgi", ".sql", ".bak", ".env", ".ini",
];

/// Folders other software keeps, wherever in a path they turn up.
const ANYWHERE: &[&str] = &["wp-admin", "wp-content", "wp-includes", "wp-json", "phpmyadmin"];

/// First folders of paths other software answers.
const FIRST: &[&str] = &[
    "actuator",
    "adminer",
    "autodiscover",
    "boaform",
    "cgi-bin",
    "hnap1",
    "myadmin",
    "pma",
    "server-status",
    "solr",
    "telescope",
    "vendor",
    "xmlrpc",
    "_ignition",
];

/// Whether `path` (without its query) is one only a scanner asks for.
/// `/.well-known/` stays open for whatever needs it.
pub fn is_probe(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or_default();
    let path = percent_encoding::percent_decode_str(path).decode_utf8_lossy().to_ascii_lowercase();
    let mut segments = path.split('/').filter(|s| !s.is_empty()).peekable();
    if segments.peek() == Some(&".well-known") {
        return false;
    }
    segments.enumerate().any(|(i, segment)| {
        // Hidden files and folders: .env, .git, .DS_Store, .aws, .htaccess...
        segment.starts_with('.')
            || ENDINGS.iter().any(|ending| segment.ends_with(ending))
            || ANYWHERE.contains(&segment)
            || (i == 0 && FIRST.contains(&segment))
    })
}

/// Middleware answering probes with a 404 that Railway's CDN may keep for a
/// day, so a scanner trying again doesn't reach the instance either.
pub async fn turn_away(request: Request, next: Next) -> Response {
    if !is_probe(request.uri().path()) {
        return next.run(request).await;
    }
    let mut response = (StatusCode::NOT_FOUND, "not found\n").into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    response
}

#[cfg(test)]
mod tests {
    use super::is_probe;

    #[test]
    fn scanners_are_caught() {
        for path in [
            "/.env",
            "/.env.production",
            "/api/.env",
            "/config.env",
            "/.git/config",
            "/.git/HEAD",
            "/.DS_Store",
            "/.aws/credentials",
            "/.htaccess",
            "/.vscode/sftp.json",
            "/wp-config.php",
            "/wp-config.php.bak",
            "/wp-login.php",
            "/wp-admin/",
            "/wp-admin/setup-config.php",
            "/blog/wp-admin/",
            "/wordpress/wp-includes/wlwmanifest.xml",
            "/wp-content/plugins/x/readme.txt",
            "/wp-json/wp/v2/users",
            "/xmlrpc.php",
            "/xmlrpc",
            "/phpMyAdmin/index.php",
            "/db/phpmyadmin/",
            "/pma/",
            "/adminer",
            "/actuator/env",
            "/actuator/health",
            "/cgi-bin/luci",
            "/vendor/phpunit/phpunit/src/Util/PHP/eval-stdin.php",
            "/index.php",
            "/info.php?x=1",
            "/default.aspx",
            "/login.jsp",
            "/backup.sql",
            "/site.bak",
            "/config.ini",
            "/HNAP1/",
            "/boaform/admin/formLogin",
            "/_ignition/execute-solution",
            "/telescope/requests",
            "/server-status",
            "/solr/admin/info/system",
            "/Autodiscover/Autodiscover.xml",
            "/%2eenv",
            "/%2Egit/config",
        ] {
            assert!(is_probe(path), "{path} should be turned away");
        }
    }

    #[test]
    fn fuwa_paths_go_through() {
        for path in [
            "/",
            "/healthz",
            "/fuwa.v1.EventService/Subscribe",
            "/fuwa.v1.NodeService/GetNode",
            "/grpc.health.v1.Health/Check",
            "/media/01J9ZK3Q8V6Y2N4M5P7R8S9T0A",
            "/media/upload/abc-_DEF123",
            "/media/outside/sig-_x?url=https%3A%2F%2Fexample.com%2Fa.png",
            "/media/servers/01J9ZK3Q8V6Y2N4M5P7R8S9T0A/pictures/01J9ZK3Q8V6Y2N4M5P7R8S9T0B",
            "/webhooks/01J9ZK3Q8V6Y2N4M5P7R8S9T0A/01J9ZK3Q8V6Y2N4M5P7R8S9T0B/_-token",
            "/sso/instance/oidc",
            "/sso/servers/01J9ZK3Q8V6Y2N4M5P7R8S9T0A/saml/metadata",
            "/assets/index-abc123.js",
            "/assets/m-plus-rounded-1c-latin-400-normal.woff2",
            "/fuwa.waifu.dev/01ABC/01DEF",
            "/fuwa.example.pl/01ABC/01DEF",
            "/fuwa.chat/invite/wp-fans",
            "/.well-known/openid-configuration",
            "/.well-known/acme-challenge/token",
        ] {
            assert!(!is_probe(path), "{path} should go through");
        }
    }
}
